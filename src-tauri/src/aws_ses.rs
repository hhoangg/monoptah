//! The read side of SES v2: which identities are verified, and whether the
//! account is still in the sandbox. Sending joins this file with the compose
//! work.

use std::io::Read;
use std::time::Duration;

use serde::Serialize;

use crate::aws_sigv4::{
    amz_timestamp, authorization, sha256_hex, uri_encode, Credentials, SigningRequest,
};
use crate::mail_store::MailIdentity;

const HTTP_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_BODY: u64 = 4 * 1024 * 1024;
const PAGE_SIZE: &str = "1000";
// A runaway token loop must end somewhere; 50 pages is 50,000 identities.
const MAX_PAGES: usize = 50;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SesErrorKind {
    AccessDenied,
    InvalidCredentials,
    ClockSkew,
    Network,
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

/// The seam the command layer mocks, so its tests never touch the network.
pub(crate) trait SesApi {
    fn list_identities(&self) -> Result<Vec<MailIdentity>, SesError>;
    fn get_account(&self) -> Result<SesAccount, SesError>;
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

    fn get(&self, path: &str, query: &[(&str, &str)], action: &str) -> Result<Vec<u8>, SesError> {
        let host = format!("email.{}.amazonaws.com", self.region);
        let amz_date = amz_timestamp(time::OffsetDateTime::now_utc().unix_timestamp());
        let headers = [("host", host.as_str()), ("x-amz-date", amz_date.as_str())];
        let auth = authorization(
            &self.creds,
            &SigningRequest {
                method: "GET",
                canonical_uri: path,
                query,
                headers: &headers,
                payload_hash: &sha256_hex(b""),
            },
            &self.region,
            "ses",
            &amz_date,
        );
        let agent = ureq::AgentBuilder::new()
            .timeout(HTTP_TIMEOUT)
            .redirects(0)
            .build();
        // Encoded by the signer's own routine, so the URL and the signature agree.
        let mut pairs: Vec<String> = query
            .iter()
            .map(|(k, v)| format!("{}={}", uri_encode(k, false), uri_encode(v, false)))
            .collect();
        pairs.sort();
        let mut url = format!("https://{host}{path}");
        if !pairs.is_empty() {
            url.push('?');
            url.push_str(&pairs.join("&"));
        }
        let request = agent
            .get(&url)
            .set("x-amz-date", &amz_date)
            .set("Authorization", &auth);
        match request.call() {
            Ok(response) if response.status() < 300 => {
                let mut bytes = Vec::new();
                response
                    .into_reader()
                    .take(MAX_BODY)
                    .read_to_end(&mut bytes)
                    .map_err(|_| network("The connection to SES dropped."))?;
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
            Err(_) => Err(network(
                "Could not reach SES. Check the network connection and the SES region.",
            )),
        }
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
    let message_text = serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| {
            v.get("message")
                .or_else(|| v.get("Message"))?
                .as_str()
                .map(str::to_lowercase)
        })
        .unwrap_or_default();
    let (kind, message) = if message_text.contains("expired") || code == "RequestExpired" {
        (
            SesErrorKind::ClockSkew,
            "This computer's clock is more than 5 minutes off AWS time. Fix the system clock and try again."
                .to_string(),
        )
    } else {
        match code.as_str() {
            "AccessDeniedException" | "AccessDenied" => (
                SesErrorKind::AccessDenied,
                format!("Access denied. The key pair needs {action}."),
            ),
            "UnrecognizedClientException"
            | "InvalidSignatureException"
            | "SignatureDoesNotMatch"
            | "InvalidClientTokenId"
            | "IncompleteSignature"
            | "MissingAuthenticationToken" => (
                SesErrorKind::InvalidCredentials,
                "AWS rejected the access key id or secret.".to_string(),
            ),
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
}
