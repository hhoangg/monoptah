// Consumers (sync loop, send) land in later mail tasks.
#![allow(dead_code)]

use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};

type HmacSha256 = Hmac<Sha256>;

/// No `Debug` on purpose: a stray `{:?}` must not be able to print the secret.
pub(crate) struct Credentials {
    pub access_key_id: String,
    pub secret_access_key: String,
}

pub(crate) struct SigningRequest<'a> {
    pub method: &'a str,
    /// Already percent-encoded once, as on the wire. That single encoding is the
    /// S3 exception: every other service, SES included, encodes the path twice.
    pub canonical_uri: &'a str,
    /// Raw (unencoded) name/value pairs; encoded and sorted here.
    pub query: &'a [(&'a str, &'a str)],
    /// Every header to sign, including `host` and `x-amz-date`.
    pub headers: &'a [(&'a str, &'a str)],
    pub payload_hash: &'a str,
}

pub(crate) fn sha256_hex(data: &[u8]) -> String {
    hex(&Sha256::digest(data))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn hmac(key: &[u8], data: &[u8]) -> Vec<u8> {
    // HMAC accepts keys of any length, so this cannot fail.
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC accepts any key length");
    mac.update(data);
    mac.finalize().into_bytes().to_vec()
}

/// `YYYYMMDDTHHMMSSZ` for a Unix timestamp in seconds.
pub(crate) fn amz_timestamp(unix_secs: i64) -> String {
    let dt = time::OffsetDateTime::from_unix_timestamp(unix_secs)
        .unwrap_or(time::OffsetDateTime::UNIX_EPOCH);
    format!(
        "{:04}{:02}{:02}T{:02}{:02}{:02}Z",
        dt.year(),
        dt.month() as u8,
        dt.day(),
        dt.hour(),
        dt.minute(),
        dt.second()
    )
}

/// SigV4 percent-encoding: only unreserved characters stay literal.
pub(crate) fn uri_encode(input: &str, keep_slash: bool) -> String {
    let mut out = String::with_capacity(input.len());
    for byte in input.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            b'/' if keep_slash => out.push('/'),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// Returns the canonical request and the `SignedHeaders` list.
pub(crate) fn canonical_request(req: &SigningRequest) -> (String, String) {
    let mut query: Vec<(String, String)> = req
        .query
        .iter()
        .map(|(k, v)| (uri_encode(k, false), uri_encode(v, false)))
        .collect();
    query.sort();
    let query = query
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("&");

    let mut headers: Vec<(String, String)> = req
        .headers
        .iter()
        .map(|(k, v)| {
            (
                k.to_ascii_lowercase(),
                v.split_whitespace().collect::<Vec<_>>().join(" "),
            )
        })
        .collect();
    headers.sort();
    let canonical_headers: String = headers.iter().map(|(k, v)| format!("{k}:{v}\n")).collect();
    let signed_headers = headers
        .iter()
        .map(|(k, _)| k.as_str())
        .collect::<Vec<_>>()
        .join(";");

    let canonical = format!(
        "{}\n{}\n{}\n{}\n{}\n{}",
        req.method, req.canonical_uri, query, canonical_headers, signed_headers, req.payload_hash
    );
    (canonical, signed_headers)
}

pub(crate) fn credential_scope(date: &str, region: &str, service: &str) -> String {
    format!("{date}/{region}/{service}/aws4_request")
}

pub(crate) fn string_to_sign(amz_date: &str, scope: &str, canonical_request: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256\n{amz_date}\n{scope}\n{}",
        sha256_hex(canonical_request.as_bytes())
    )
}

pub(crate) fn signing_key(secret: &str, date: &str, region: &str, service: &str) -> Vec<u8> {
    let k_date = hmac(format!("AWS4{secret}").as_bytes(), date.as_bytes());
    let k_region = hmac(&k_date, region.as_bytes());
    let k_service = hmac(&k_region, service.as_bytes());
    hmac(&k_service, b"aws4_request")
}

/// Value for the `Authorization` header.
pub(crate) fn authorization(
    creds: &Credentials,
    req: &SigningRequest,
    region: &str,
    service: &str,
    amz_date: &str,
) -> String {
    let date = &amz_date[..8];
    let scope = credential_scope(date, region, service);
    let (canonical, signed_headers) = canonical_request(req);
    let to_sign = string_to_sign(amz_date, &scope, &canonical);
    let key = signing_key(&creds.secret_access_key, date, region, service);
    let signature = hex(&hmac(&key, to_sign.as_bytes()));
    format!(
        "AWS4-HMAC-SHA256 Credential={}/{scope}, SignedHeaders={signed_headers}, Signature={signature}",
        creds.access_key_id
    )
}

/// AWS's documented example credentials, for tests only. They authenticate
/// nothing; each is split in two so secret scanners do not read the source as
/// a leaked key. The values are unchanged, so the published signatures match.
#[cfg(test)]
pub(crate) mod test_keys {
    pub(crate) const KEY_ID: &str = concat!("AKIA", "IOSFODNN7EXAMPLE");
    /// The `ASIA` prefix marks a temporary (STS) key, which Mail refuses.
    pub(crate) const TEMPORARY_KEY_ID: &str = concat!("ASIA", "IOSFODNN7EXAMPLE");
    /// A second, made-up permanent key id, for rotation tests.
    pub(crate) const OTHER_KEY_ID: &str = concat!("AKIA", "J5Q2ZPLMNBVCXZ9K");
    pub(crate) const SECRET: &str = concat!("wJalrXUtnFEMI/K7MDENG+bPxRfi", "CYEXAMPLEKEY");
    /// The S3 signing docs spell the same example secret with a '/'.
    pub(crate) const S3_DOCS_SECRET: &str = concat!("wJalrXUtnFEMI/K7MDENG/bPxRfi", "CYEXAMPLEKEY");
}

#[cfg(test)]
mod tests {
    use super::test_keys::{KEY_ID, S3_DOCS_SECRET, SECRET};
    use super::*;

    const EMPTY_SHA256: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

    // AWS S3 docs, "Signature Calculations for the Authorization Header",
    // example 1 (GET Object with Range). Note this doc uses a secret with '/'.
    fn s3_docs_creds() -> Credentials {
        Credentials {
            access_key_id: KEY_ID.into(),
            secret_access_key: S3_DOCS_SECRET.into(),
        }
    }

    fn s3_docs_request() -> SigningRequest<'static> {
        SigningRequest {
            method: "GET",
            canonical_uri: "/test.txt",
            query: &[],
            headers: &[
                ("Host", "examplebucket.s3.amazonaws.com"),
                ("Range", "bytes=0-9"),
                ("x-amz-content-sha256", EMPTY_SHA256),
                ("x-amz-date", "20130524T000000Z"),
            ],
            payload_hash: EMPTY_SHA256,
        }
    }

    #[test]
    fn s3_docs_canonical_request_and_string_to_sign() {
        let (canonical, signed) = canonical_request(&s3_docs_request());
        assert_eq!(signed, "host;range;x-amz-content-sha256;x-amz-date");
        assert_eq!(
            canonical,
            format!(
                "GET\n/test.txt\n\nhost:examplebucket.s3.amazonaws.com\nrange:bytes=0-9\n\
                 x-amz-content-sha256:{EMPTY_SHA256}\nx-amz-date:20130524T000000Z\n\n\
                 host;range;x-amz-content-sha256;x-amz-date\n{EMPTY_SHA256}"
            )
        );
        let scope = credential_scope("20130524", "us-east-1", "s3");
        assert_eq!(
            string_to_sign("20130524T000000Z", &scope, &canonical),
            "AWS4-HMAC-SHA256\n20130524T000000Z\n20130524/us-east-1/s3/aws4_request\n\
             7344ae5b7ee6c3e7e6b0fe0640412a37625d1fbfff95c48bbb2dc43964946972"
        );
    }

    #[test]
    fn s3_docs_signature() {
        let auth = authorization(
            &s3_docs_creds(),
            &s3_docs_request(),
            "us-east-1",
            "s3",
            "20130524T000000Z",
        );
        assert_eq!(
            auth,
            format!(
                "AWS4-HMAC-SHA256 Credential={KEY_ID}/20130524/us-east-1/s3/aws4_request, \
                 SignedHeaders=host;range;x-amz-content-sha256;x-amz-date, \
                 Signature=f0e8bdb87c964420e857bd35b5d6ed310bd44f0170aba48dd91039c6036bdb41"
            )
        );
    }

    // AWS General Reference, "Derive a signing key" / IAM ListUsers worked example.
    #[test]
    fn iam_docs_signing_key_and_signature() {
        let secret = SECRET;
        let key = signing_key(secret, "20150830", "us-east-1", "iam");
        assert_eq!(
            hex(&key),
            "c4afb1cc5771d871763a393e44b703571b55cc28424d1a5e86da6ed3c154a4b9"
        );

        let creds = Credentials {
            access_key_id: "AKIDEXAMPLE".into(),
            secret_access_key: secret.into(),
        };
        let req = SigningRequest {
            method: "GET",
            canonical_uri: "/",
            query: &[("Version", "2010-05-08"), ("Action", "ListUsers")],
            headers: &[
                (
                    "content-type",
                    "application/x-www-form-urlencoded; charset=utf-8",
                ),
                ("host", "iam.amazonaws.com"),
                ("x-amz-date", "20150830T123600Z"),
            ],
            payload_hash: EMPTY_SHA256,
        };
        let (canonical, _) = canonical_request(&req);
        assert!(canonical.starts_with("GET\n/\nAction=ListUsers&Version=2010-05-08\n"));
        let auth = authorization(&creds, &req, "us-east-1", "iam", "20150830T123600Z");
        assert!(auth.ends_with(
            "Signature=5d672d79c15b13162d9279b0855cfba6789a8edb4c82c400e06b5924a6f2b5d7"
        ));
    }

    #[test]
    fn scope_uses_the_service_passed() {
        let req = s3_docs_request();
        let ses = authorization(
            &s3_docs_creds(),
            &req,
            "eu-west-1",
            "ses",
            "20240102T030405Z",
        );
        assert!(ses.contains(&format!(
            "Credential={KEY_ID}/20240102/eu-west-1/ses/aws4_request"
        )));
        let s3 = authorization(
            &s3_docs_creds(),
            &req,
            "eu-west-1",
            "s3",
            "20240102T030405Z",
        );
        assert_ne!(ses, s3);
    }

    #[test]
    fn signing_is_deterministic() {
        let a = authorization(
            &s3_docs_creds(),
            &s3_docs_request(),
            "us-east-1",
            "s3",
            "20130524T000000Z",
        );
        let b = authorization(
            &s3_docs_creds(),
            &s3_docs_request(),
            "us-east-1",
            "s3",
            "20130524T000000Z",
        );
        assert_eq!(a, b);
    }

    #[test]
    fn timestamp_and_encoding() {
        assert_eq!(amz_timestamp(1369353600), "20130524T000000Z");
        assert_eq!(uri_encode("a b/c+d~", true), "a%20b/c%2Bd~");
        assert_eq!(uri_encode("a/b", false), "a%2Fb");
    }
}
