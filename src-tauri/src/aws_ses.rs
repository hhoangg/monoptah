//! SES v2: which identities are verified, whether the account is still in the
//! sandbox, and sending a raw message.

use std::io::Read;
use std::time::Duration;

use serde::Serialize;

use crate::aws_sigv4::{
    amz_timestamp, authorization, sha256_hex, uri_encode, Credentials, SigningRequest,
};
use crate::mail_store::MailIdentity;

const HTTP_TIMEOUT: Duration = Duration::from_secs(30);
// A send uploads up to the 40 MB raw limit (about 53 MB once base64 wrapped in
// JSON), which does not fit in the read timeout's budget on a slow uplink.
const SEND_TIMEOUT: Duration = Duration::from_secs(180);
const MAX_BODY: u64 = 4 * 1024 * 1024;
const PAGE_SIZE: &str = "1000";
// A runaway token loop must end somewhere; 50 pages is 50,000 identities.
const MAX_PAGES: usize = 50;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SesErrorKind {
    AccessDenied,
    InvalidCredentials,
    ClockSkew,
    /// Could not reach SES at all: nothing was sent.
    Network,
    /// The request went out but no answer came back. For a send this means
    /// the message may have been delivered.
    Unconfirmed,
    /// SES understood the request and refused it; `message` carries its reason.
    Rejected,
    Other,
}

/// `message` is a sentence the settings page can show as is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SesError {
    pub kind: SesErrorKind,
    pub message: String,
}

impl std::fmt::Display for SesError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SesAccount {
    /// In the sandbox SES only delivers to verified addresses.
    pub sandbox: bool,
    pub sending_enabled: bool,
}

/// Envelope recipients of a raw send. SES delivers to these and ignores the
/// MIME headers, which is what lets `Bcc` stay out of the message itself.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Destination {
    pub to: Vec<String>,
    pub cc: Vec<String>,
    pub bcc: Vec<String>,
}

/// The seam the command layer mocks, so its tests never touch the network.
pub(crate) trait SesApi {
    // Identity `status` values: `verified`, `pending`, `failed`, and
    // `sending_disabled`. The last means AWS verified the identity but has
    // turned sending off for it, so it is not sendable and the fix is in the
    // AWS console, not in DNS. Settings copy must say that, not "failed".
    fn list_identities(&self) -> Result<Vec<MailIdentity>, SesError>;
    fn get_account(&self) -> Result<SesAccount, SesError>;
    /// Returns SES's own message id. Never retried here: a half-delivered
    /// email sent again silently is worse than a visible failure.
    fn send_raw(
        &self,
        from: &str,
        destination: &Destination,
        raw: &[u8],
    ) -> Result<String, SesError>;
}

pub(crate) struct SesClient {
    creds: Credentials,
    region: String,
}

impl SesClient {
    pub(crate) fn new(creds: Credentials, region: &str) -> Self {
        Self {
            creds,
            region: region.to_string(),
        }
    }

    fn request(
        &self,
        method: &str,
        path: &str,
        query: &[(&str, &str)],
        body: &[u8],
        action: &str,
    ) -> Result<Vec<u8>, SesError> {
        let host = format!("email.{}.amazonaws.com", self.region);
        let amz_date = amz_timestamp(time::OffsetDateTime::now_utc().unix_timestamp());
        let (wire_path, canonical_path) = ses_paths(path);
        let mut headers = vec![("host", host.as_str()), ("x-amz-date", amz_date.as_str())];
        if !body.is_empty() {
            headers.push(("content-type", "application/json"));
        }
        let auth = authorization(
            &self.creds,
            &SigningRequest {
                method,
                canonical_uri: &canonical_path,
                query,
                headers: &headers,
                payload_hash: &sha256_hex(body),
            },
            &self.region,
            "ses",
            &amz_date,
        );
        let agent = ureq::AgentBuilder::new()
            .timeout(if body.is_empty() {
                HTTP_TIMEOUT
            } else {
                SEND_TIMEOUT
            })
            .redirects(0)
            .build();
        // Encoded by the signer's own routine, so the URL and the signature agree.
        let mut pairs: Vec<String> = query
            .iter()
            .map(|(k, v)| format!("{}={}", uri_encode(k, false), uri_encode(v, false)))
            .collect();
        pairs.sort();
        let mut url = format!("https://{host}{wire_path}");
        if !pairs.is_empty() {
            url.push('?');
            url.push_str(&pairs.join("&"));
        }
        let mut request = agent
            .request(method, &url)
            .set("x-amz-date", &amz_date)
            .set("Authorization", &auth);
        if !body.is_empty() {
            request = request.set("content-type", "application/json");
        }
        let result = if body.is_empty() {
            request.call()
        } else {
            request.send_bytes(body)
        };
        match result {
            Ok(response) if response.status() < 300 => {
                let mut bytes = Vec::new();
                response
                    .into_reader()
                    .take(MAX_BODY)
                    .read_to_end(&mut bytes)
                    .map_err(|_| {
                        if body.is_empty() {
                            network("The connection to SES dropped.")
                        } else {
                            unconfirmed()
                        }
                    })?;
                Ok(bytes)
            }
            Ok(response) => Err(classify(response.status(), None, "", action)),
            Err(ureq::Error::Status(status, response)) => {
                let error_type = response.header("x-amzn-errortype").map(str::to_string);
                let mut body = String::new();
                let _ = response
                    .into_reader()
                    .take(64 * 1024)
                    .read_to_string(&mut body);
                Err(classify(status, error_type.as_deref(), &body, action))
            }
            Err(ureq::Error::Transport(t)) => Err(transport_error(t.kind(), !body.is_empty())),
        }
    }

    fn get(&self, path: &str, query: &[(&str, &str)], action: &str) -> Result<Vec<u8>, SesError> {
        self.request("GET", path, query, &[], action)
    }
}

/// `(path as sent, path as signed)`. SigV4 encodes the path of every service
/// except S3 twice in the canonical request; the URL itself carries it once.
/// Getting this wrong only shows on a path with a reserved character, which no
/// call here has had until an identity name appears in one.
fn ses_paths(path: &str) -> (String, String) {
    let wire = uri_encode(path, true);
    let canonical = uri_encode(&wire, true);
    (wire, canonical)
}

/// Only a failure to connect proves nothing left this machine. Any other
/// transport error on a send happened with the request possibly on the wire.
fn transport_error(kind: ureq::ErrorKind, is_send: bool) -> SesError {
    let never_left = matches!(
        kind,
        ureq::ErrorKind::Dns
            | ureq::ErrorKind::ConnectionFailed
            | ureq::ErrorKind::InvalidUrl
            | ureq::ErrorKind::UnknownScheme
    );
    if is_send && !never_left {
        unconfirmed()
    } else {
        network("Could not reach SES. Check the network connection and the SES region.")
    }
}

fn unconfirmed() -> SesError {
    SesError {
        kind: SesErrorKind::Unconfirmed,
        message: "The connection dropped before SES confirmed the message. It may have been delivered, so check the recipient before sending it again.".to_string(),
    }
}

fn network(message: &str) -> SesError {
    SesError {
        kind: SesErrorKind::Network,
        message: message.to_string(),
    }
}

impl SesApi for SesClient {
    fn list_identities(&self) -> Result<Vec<MailIdentity>, SesError> {
        let mut all = Vec::new();
        let mut token: Option<String> = None;
        for _ in 0..MAX_PAGES {
            let mut query = vec![("PageSize", PAGE_SIZE)];
            if let Some(token) = token.as_deref() {
                query.push(("NextToken", token));
            }
            let body = self.get("/v2/email/identities", &query, "ses:ListEmailIdentities")?;
            let (page, next) = parse_identities(&body)?;
            all.extend(page);
            match next {
                Some(next) => token = Some(next),
                None => break,
            }
        }
        Ok(all)
    }

    fn get_account(&self) -> Result<SesAccount, SesError> {
        let body = self.get("/v2/email/account", &[], "ses:GetAccount")?;
        parse_account(&body)
    }

    fn send_raw(
        &self,
        from: &str,
        destination: &Destination,
        raw: &[u8],
    ) -> Result<String, SesError> {
        let body = send_request_body(from, destination, raw);
        let response = self.request("POST", SEND_PATH, &[], &body, "ses:SendEmail")?;
        // The send is already accepted by now, so an odd body is not a failure.
        Ok(serde_json::from_slice::<serde_json::Value>(&response)
            .ok()
            .and_then(|v| v.get("MessageId")?.as_str().map(str::to_string))
            .unwrap_or_default())
    }
}

const SEND_PATH: &str = "/v2/email/outbound-emails";

fn send_request_body(from: &str, destination: &Destination, raw: &[u8]) -> Vec<u8> {
    use base64::Engine;
    let mut dest = serde_json::Map::new();
    for (key, list) in [
        ("ToAddresses", &destination.to),
        ("CcAddresses", &destination.cc),
        ("BccAddresses", &destination.bcc),
    ] {
        if !list.is_empty() {
            dest.insert(key.into(), serde_json::json!(list));
        }
    }
    serde_json::to_vec(&serde_json::json!({
        "FromEmailAddress": from,
        "Destination": dest,
        "Content": {
            "Raw": { "Data": base64::engine::general_purpose::STANDARD.encode(raw) }
        },
    }))
    // A map of strings always serialises.
    .unwrap_or_default()
}

fn unreadable() -> SesError {
    SesError {
        kind: SesErrorKind::Other,
        message: "SES returned a response that could not be read.".to_string(),
    }
}

fn parse_identities(body: &[u8]) -> Result<(Vec<MailIdentity>, Option<String>), SesError> {
    let json: serde_json::Value = serde_json::from_slice(body).map_err(|_| unreadable())?;
    let identities = json
        .get("EmailIdentities")
        .and_then(|v| v.as_array())
        .ok_or_else(unreadable)?
        .iter()
        .filter_map(|item| {
            let name = item.get("IdentityName")?.as_str()?;
            let kind = match item.get("IdentityType").and_then(|v| v.as_str()) {
                Some("EMAIL_ADDRESS") => "email",
                _ => "domain",
            };
            let sending_off = item.get("SendingEnabled").and_then(|v| v.as_bool()) == Some(false);
            let status = match item.get("VerificationStatus").and_then(|v| v.as_str()) {
                // Not sendable either way, but the fix is in AWS, not in DNS.
                Some("SUCCESS") if sending_off => "sending_disabled",
                Some("SUCCESS") => "verified",
                Some("PENDING" | "NOT_STARTED" | "TEMPORARY_FAILURE") => "pending",
                _ => "failed",
            };
            Some(MailIdentity {
                identity: name.to_string(),
                kind: kind.to_string(),
                status: status.to_string(),
            })
        })
        .collect();
    let next = json
        .get("NextToken")
        .and_then(|v| v.as_str())
        .filter(|t| !t.is_empty())
        .map(str::to_string);
    Ok((identities, next))
}

fn parse_account(body: &[u8]) -> Result<SesAccount, SesError> {
    let json: serde_json::Value = serde_json::from_slice(body).map_err(|_| unreadable())?;
    Ok(SesAccount {
        sandbox: !json
            .get("ProductionAccessEnabled")
            .and_then(|v| v.as_bool())
            .ok_or_else(unreadable)?,
        sending_enabled: json
            .get("SendingEnabled")
            .and_then(|v| v.as_bool())
            .unwrap_or(true),
    })
}

fn classify(status: u16, error_type: Option<&str>, body: &str, action: &str) -> SesError {
    // `AccessDeniedException:http://internal.amazon.com/...` carries a URL tail.
    let code = error_type
        .map(|t| t.split(':').next().unwrap_or(t).to_string())
        .or_else(|| {
            serde_json::from_str::<serde_json::Value>(body)
                .ok()?
                .get("__type")?
                .as_str()
                .map(str::to_string)
        })
        .unwrap_or_default();
    let aws_message = serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| {
            v.get("message")
                .or_else(|| v.get("Message"))?
                .as_str()
                .map(str::to_string)
        })
        .unwrap_or_default();
    let message_text = aws_message.to_lowercase();
    // A 5xx says nothing about the clock, and for a send it must stay
    // ambiguous (`Other`), not become a "safe to retry" ClockSkew.
    let (kind, message) = if status < 500
        && (message_text.contains("expired") || code == "RequestExpired")
    {
        (
            SesErrorKind::ClockSkew,
            "This computer's clock is more than 5 minutes off AWS time. Fix the system clock and try again."
                .to_string(),
        )
    } else {
        match code.as_str() {
            // The arms below that mean "SES did not take it" need a status under
            // 500: a server error proves nothing, whatever its error header says.
            "AccessDeniedException" | "AccessDenied" if status < 500 => (
                SesErrorKind::AccessDenied,
                format!("Access denied. The key pair needs {action}."),
            ),
            "UnrecognizedClientException"
            | "InvalidSignatureException"
            | "SignatureDoesNotMatch"
            | "InvalidClientTokenId"
            | "IncompleteSignature"
            | "MissingAuthenticationToken"
                if status < 500 =>
            {
                (
                    SesErrorKind::InvalidCredentials,
                    "AWS rejected the access key id or secret.".to_string(),
                )
            }
            // SES refused this message and says why in words meant for a person
            // ("Email address is not verified..."), so pass that through.
            "MessageRejected"
            | "AccountSuspendedException"
            | "SendingPausedException"
            | "MailFromDomainNotVerifiedException"
            | "NotFoundException"
            | "BadRequestException"
            | "TooManyRequestsException"
            | "LimitExceededException"
                if status < 500 =>
            {
                (SesErrorKind::Rejected, rejection_text(&code, &aws_message))
            }
            _ if status == 403 => (
                SesErrorKind::AccessDenied,
                format!("Access denied. The key pair needs {action}."),
            ),
            _ => (
                SesErrorKind::Other,
                format!("SES returned an unexpected response (HTTP {status})."),
            ),
        }
    };
    SesError { kind, message }
}

fn rejection_text(code: &str, aws_message: &str) -> String {
    let reason: String = aws_message.trim().chars().take(400).collect();
    if reason.is_empty() {
        format!("SES refused the message ({code}).")
    } else {
        format!("SES refused the message: {reason}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_verification_states() {
        let body = br#"{"EmailIdentities":[
          {"IdentityName":"Byptah.com","IdentityType":"DOMAIN","SendingEnabled":true,"VerificationStatus":"SUCCESS"},
          {"IdentityName":"a@x.com","IdentityType":"EMAIL_ADDRESS","SendingEnabled":true,"VerificationStatus":"PENDING"},
          {"IdentityName":"old.com","IdentityType":"DOMAIN","SendingEnabled":true,"VerificationStatus":"FAILED"},
          {"IdentityName":"off.com","IdentityType":"DOMAIN","SendingEnabled":false,"VerificationStatus":"SUCCESS"}
        ],"NextToken":"t1"}"#;
        let (identities, next) = parse_identities(body).unwrap();
        let states: Vec<(&str, &str, &str)> = identities
            .iter()
            .map(|i| (i.identity.as_str(), i.kind.as_str(), i.status.as_str()))
            .collect();
        assert_eq!(
            states,
            vec![
                ("Byptah.com", "domain", "verified"),
                ("a@x.com", "email", "pending"),
                ("old.com", "domain", "failed"),
                ("off.com", "domain", "sending_disabled"),
            ]
        );
        assert_eq!(next.as_deref(), Some("t1"));
    }

    #[test]
    fn reads_sandbox_state() {
        let account =
            parse_account(br#"{"ProductionAccessEnabled":false,"SendingEnabled":true}"#).unwrap();
        assert!(account.sandbox);
        assert!(
            !parse_account(br#"{"ProductionAccessEnabled":true}"#)
                .unwrap()
                .sandbox
        );
        assert!(parse_account(b"{}").is_err());
    }

    #[test]
    fn names_the_action_on_access_denied() {
        let error = classify(
            403,
            Some("AccessDeniedException:http://internal.amazon.com/"),
            r#"{"message":"User is not authorized"}"#,
            "ses:GetAccount",
        );
        assert_eq!(error.kind, SesErrorKind::AccessDenied);
        assert!(error.message.contains("ses:GetAccount"));
        assert_eq!(
            classify(
                403,
                Some("UnrecognizedClientException"),
                "{}",
                "ses:GetAccount"
            )
            .kind,
            SesErrorKind::InvalidCredentials
        );
        assert_eq!(
            classify(
                403,
                None,
                r#"{"message":"Signature expired: 2024 is now earlier"}"#,
                "x"
            )
            .kind,
            SesErrorKind::ClockSkew
        );
    }

    #[test]
    fn bcc_travels_in_the_destination_and_the_raw_data_is_base64() {
        use base64::Engine;
        let dest = Destination {
            to: vec!["a@x.com".into()],
            cc: vec![],
            bcc: vec!["secret@x.com".into()],
        };
        let body = send_request_body("me@y.com", &dest, b"Subject: hi\r\n\r\nbody");
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["FromEmailAddress"], "me@y.com");
        assert_eq!(json["Destination"]["ToAddresses"][0], "a@x.com");
        assert_eq!(json["Destination"]["BccAddresses"][0], "secret@x.com");
        assert!(json["Destination"].get("CcAddresses").is_none());
        let data = json["Content"]["Raw"]["Data"].as_str().unwrap();
        assert_eq!(
            base64::engine::general_purpose::STANDARD
                .decode(data)
                .unwrap(),
            b"Subject: hi\r\n\r\nbody"
        );
    }

    #[test]
    fn the_signed_path_is_encoded_twice_and_the_url_path_once() {
        assert_eq!(
            ses_paths("/v2/email/identities/a@b.com"),
            (
                "/v2/email/identities/a%40b.com".to_string(),
                "/v2/email/identities/a%2540b.com".to_string()
            )
        );
        // The send path has no reserved character, so both forms agree.
        assert_eq!(
            ses_paths(SEND_PATH),
            (SEND_PATH.to_string(), SEND_PATH.to_string())
        );
    }

    #[test]
    fn a_rejection_passes_ses_s_reason_through() {
        let error = classify(
            400,
            Some("MessageRejected"),
            r#"{"message":"Email address is not verified. The following identities failed the check in region EU-WEST-1: a@x.com"}"#,
            "ses:SendEmail",
        );
        assert_eq!(error.kind, SesErrorKind::Rejected);
        assert!(error.message.contains("a@x.com"));
        assert_eq!(
            classify(400, Some("MessageRejected"), "{}", "ses:SendEmail").message,
            "SES refused the message (MessageRejected)."
        );
    }

    #[test]
    fn a_send_cut_off_mid_request_is_unconfirmed_but_a_failed_connect_is_not() {
        assert_eq!(
            transport_error(ureq::ErrorKind::Io, true).kind,
            SesErrorKind::Unconfirmed
        );
        assert_eq!(
            transport_error(ureq::ErrorKind::ConnectionFailed, true).kind,
            SesErrorKind::Network
        );
        assert_eq!(
            transport_error(ureq::ErrorKind::Dns, true).kind,
            SesErrorKind::Network
        );
        // A read is safe to repeat whatever happened.
        assert_eq!(
            transport_error(ureq::ErrorKind::Io, false).kind,
            SesErrorKind::Network
        );
    }

    #[test]
    fn a_server_error_is_never_read_as_a_refusal() {
        for (code, status) in [
            ("MessageRejected", 500),
            ("BadRequestException", 503),
            ("AccessDeniedException", 500),
            ("InvalidSignatureException", 502),
        ] {
            let error = classify(status, Some(code), "{}", "ses:SendEmail");
            assert_eq!(error.kind, SesErrorKind::Other, "{code} {status}");
        }
        // The same codes under 500 keep their meaning.
        assert_eq!(
            classify(400, Some("MessageRejected"), "{}", "ses:SendEmail").kind,
            SesErrorKind::Rejected
        );
    }

    #[test]
    fn a_server_error_is_never_read_as_clock_skew() {
        for status in [500, 503] {
            let error = classify(
                status,
                None,
                r#"{"message":"Your request expired while the service was restarting"}"#,
                "ses:SendEmail",
            );
            assert_eq!(error.kind, SesErrorKind::Other, "{status}");
        }
        assert_eq!(
            classify(400, Some("RequestExpired"), "{}", "ses:SendEmail").kind,
            SesErrorKind::ClockSkew
        );
    }
}
