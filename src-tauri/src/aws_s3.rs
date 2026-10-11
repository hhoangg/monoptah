// Consumers (sync loop, send) land in later mail tasks.
#![allow(dead_code)]

use std::io::Read;
use std::time::Duration;

use base64::Engine;
use sha2::Digest;

use crate::aws_sigv4::{
    amz_timestamp, authorization, sha256_hex, uri_encode, Credentials, SigningRequest,
};

const HTTP_TIMEOUT: Duration = Duration::from_secs(60);
const MAX_ERROR_BODY: u64 = 64 * 1024;
// SES inbound mail is capped at 40 MB; anything past this is not a message.
const MAX_BODY: u64 = 256 * 1024 * 1024;
const DELETE_BATCH: usize = 1000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ObjectInfo {
    pub key: String,
    pub size: u64,
    pub etag: String,
    pub last_modified: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ObjectPage {
    pub objects: Vec<ObjectInfo>,
    /// Only filled when a delimiter was requested.
    pub common_prefixes: Vec<String>,
    pub next_token: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DeleteOutcome {
    pub key: String,
    /// `None` means the object is gone.
    pub error: Option<S3Error>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum S3ErrorKind {
    AccessDenied,
    NoSuchBucket,
    NoSuchKey,
    /// Wrong key id or secret.
    InvalidCredentials,
    /// The bucket lives in another region than the one configured.
    WrongRegion,
    ClockSkew,
    Network,
    Other,
}

/// `message` is a sentence the settings page can show as is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct S3Error {
    pub kind: S3ErrorKind,
    pub code: Option<String>,
    pub message: String,
}

impl std::fmt::Display for S3Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for S3Error {}

/// The seam the sync loop mocks, so its tests never touch the network.
pub(crate) trait S3Api {
    fn list_objects_v2(
        &self,
        prefix: &str,
        delimiter: Option<&str>,
        continuation_token: Option<&str>,
    ) -> Result<ObjectPage, S3Error>;
    fn get_object(&self, key: &str) -> Result<Vec<u8>, S3Error>;
    fn put_object(&self, key: &str, body: &[u8], content_type: &str) -> Result<(), S3Error>;
    /// One outcome per key, in order. No `Result`: a dead network shows up as
    /// every key carrying an error, so a caller cannot read failure as success.
    fn delete_objects(&self, keys: &[String]) -> Vec<DeleteOutcome>;
}

pub(crate) struct S3Client {
    creds: Credentials,
    region: String,
    bucket: String,
}

struct Prepared {
    url: String,
    headers: Vec<(String, String)>,
}

impl S3Client {
    pub(crate) fn new(creds: Credentials, region: &str, bucket: &str) -> Self {
        Self {
            creds,
            region: region.to_string(),
            bucket: bucket.to_string(),
        }
    }

    // Path-style: virtual-hosted-style puts the bucket in the TLS hostname,
    // which fails certificate checks for bucket names containing dots, and
    // buckets named after a domain are the expected case here.
    fn host(&self) -> String {
        format!("s3.{}.amazonaws.com", self.region)
    }

    // Signs the real payload hash rather than UNSIGNED-PAYLOAD: bodies are
    // already in memory, so it costs one SHA-256 and also guards integrity.
    fn prepare(
        &self,
        method: &str,
        key: &str,
        query: &[(&str, &str)],
        body: &[u8],
        extra_headers: &[(&str, &str)],
        amz_date: &str,
    ) -> Prepared {
        let host = self.host();
        let path = if key.is_empty() {
            format!("/{}", uri_encode(&self.bucket, false))
        } else {
            format!(
                "/{}/{}",
                uri_encode(&self.bucket, false),
                uri_encode(key, true)
            )
        };
        let payload_hash = sha256_hex(body);

        let mut headers: Vec<(&str, &str)> = vec![
            ("host", &host),
            ("x-amz-content-sha256", &payload_hash),
            ("x-amz-date", amz_date),
        ];
        headers.extend_from_slice(extra_headers);

        let auth = authorization(
            &self.creds,
            &SigningRequest {
                method,
                canonical_uri: &path,
                query,
                headers: &headers,
                payload_hash: &payload_hash,
            },
            &self.region,
            "s3",
            amz_date,
        );

        let mut url = format!("https://{host}{path}");
        if !query.is_empty() {
            let mut pairs: Vec<String> = query
                .iter()
                .map(|(k, v)| format!("{}={}", uri_encode(k, false), uri_encode(v, false)))
                .collect();
            pairs.sort();
            url.push('?');
            url.push_str(&pairs.join("&"));
        }

        // ureq sets Host itself from the URL.
        let mut out: Vec<(String, String)> = headers
            .iter()
            .filter(|(k, _)| *k != "host")
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        out.push(("Authorization".into(), auth));
        Prepared { url, headers: out }
    }

    fn send(
        &self,
        method: &str,
        key: &str,
        query: &[(&str, &str)],
        body: &[u8],
        extra_headers: &[(&str, &str)],
        action: &str,
    ) -> Result<Vec<u8>, S3Error> {
        let amz_date = amz_timestamp(now_unix());
        let prepared = self.prepare(method, key, query, body, extra_headers, &amz_date);
        let agent = ureq::AgentBuilder::new()
            .timeout(HTTP_TIMEOUT)
            .redirects(0)
            .build();
        let mut request = agent.request(method, &prepared.url);
        for (name, value) in &prepared.headers {
            request = request.set(name, value);
        }
        let result = if body.is_empty() && method != "POST" && method != "PUT" {
            request.call()
        } else {
            request.send_bytes(body)
        };
        match result {
            Ok(response) => {
                let status = response.status();
                let region = response.header("x-amz-bucket-region").map(str::to_string);
                let mut bytes = Vec::new();
                response
                    .into_reader()
                    .take(MAX_BODY + 1)
                    .read_to_end(&mut bytes)
                    .map_err(|_| S3Error {
                        kind: S3ErrorKind::Network,
                        code: None,
                        message: "The connection to S3 dropped while reading the response.".into(),
                    })?;
                if bytes.len() as u64 > MAX_BODY {
                    return Err(S3Error {
                        kind: S3ErrorKind::Other,
                        code: None,
                        message: "S3 returned an object too large to read.".into(),
                    });
                }
                check_status(status, bytes, action, region.as_deref())
            }
            Err(ureq::Error::Status(status, response)) => {
                let region = response.header("x-amz-bucket-region").map(str::to_string);
                let mut body = String::new();
                let _ = response
                    .into_reader()
                    .take(MAX_ERROR_BODY)
                    .read_to_string(&mut body);
                Err(error_from_response(
                    status,
                    &body,
                    action,
                    region.as_deref(),
                ))
            }
            Err(_) => Err(S3Error {
                kind: S3ErrorKind::Network,
                code: None,
                message: "Could not reach S3. Check the network connection and the region.".into(),
            }),
        }
    }
}

// ureq with redirects(0) hands a 3xx back as Ok, not Err(Status). Without this a
// wrong-region 301 would read as an empty, healthy bucket.
fn check_status(
    status: u16,
    body: Vec<u8>,
    action: &str,
    bucket_region: Option<&str>,
) -> Result<Vec<u8>, S3Error> {
    if status >= 300 {
        return Err(error_from_response(
            status,
            &String::from_utf8_lossy(&body),
            action,
            bucket_region,
        ));
    }
    Ok(body)
}

fn now_unix() -> i64 {
    time::OffsetDateTime::now_utc().unix_timestamp()
}

impl S3Api for S3Client {
    fn list_objects_v2(
        &self,
        prefix: &str,
        delimiter: Option<&str>,
        continuation_token: Option<&str>,
    ) -> Result<ObjectPage, S3Error> {
        let mut query = vec![("list-type", "2")];
        if !prefix.is_empty() {
            query.push(("prefix", prefix));
        }
        if let Some(delimiter) = delimiter {
            query.push(("delimiter", delimiter));
        }
        if let Some(token) = continuation_token {
            query.push(("continuation-token", token));
        }
        let body = self.send("GET", "", &query, &[], &[], "s3:ListBucket")?;
        Ok(parse_list_objects(&String::from_utf8_lossy(&body)))
    }

    fn get_object(&self, key: &str) -> Result<Vec<u8>, S3Error> {
        self.send("GET", key, &[], &[], &[], "s3:GetObject")
    }

    fn put_object(&self, key: &str, body: &[u8], content_type: &str) -> Result<(), S3Error> {
        self.send(
            "PUT",
            key,
            &[],
            body,
            &[("content-type", content_type)],
            "s3:PutObject",
        )
        .map(|_| ())
    }

    fn delete_objects(&self, keys: &[String]) -> Vec<DeleteOutcome> {
        delete_in_batches(keys, DELETE_BATCH, |batch| {
            let body = delete_request_body(batch);
            // DeleteObjects requires an integrity header; there is no MD5 crate here,
            // so use the SHA-256 checksum header S3 accepts in its place.
            let checksum = base64::engine::general_purpose::STANDARD
                .encode(sha2::Sha256::digest(body.as_bytes()));
            self.send(
                "POST",
                "",
                &[("delete", "")],
                body.as_bytes(),
                &[
                    ("content-type", "application/xml"),
                    ("x-amz-checksum-sha256", &checksum),
                    ("x-amz-sdk-checksum-algorithm", "SHA256"),
                ],
                "s3:DeleteObject",
            )
        })
    }
}

/// Runs `send` per batch. A failed batch keeps what earlier batches really
/// deleted and fails every key from there on, each exactly once.
fn delete_in_batches(
    keys: &[String],
    batch_size: usize,
    mut send: impl FnMut(&[String]) -> Result<Vec<u8>, S3Error>,
) -> Vec<DeleteOutcome> {
    let mut outcomes = Vec::with_capacity(keys.len());
    for batch in keys.chunks(batch_size) {
        match send(batch) {
            Ok(response) => outcomes.extend(parse_delete_result(
                &String::from_utf8_lossy(&response),
                batch,
            )),
            Err(error) => {
                let done = outcomes.len();
                outcomes.extend(keys[done..].iter().map(|key| DeleteOutcome {
                    key: key.clone(),
                    error: Some(error.clone()),
                }));
                break;
            }
        }
    }
    outcomes
}

fn delete_request_body(keys: &[String]) -> String {
    let mut xml = String::from("<Delete><Quiet>false</Quiet>");
    for key in keys {
        xml.push_str("<Object><Key>");
        xml.push_str(&xml_escape(key));
        xml.push_str("</Key></Object>");
    }
    xml.push_str("</Delete>");
    xml
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn xml_unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(start) = rest.find('&') {
        out.push_str(&rest[..start]);
        rest = &rest[start..];
        let decoded = rest.find(';').and_then(|end| {
            let entity = &rest[1..end];
            let ch = match entity {
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "quot" => Some('"'),
                "apos" => Some('\''),
                _ => entity
                    .strip_prefix("#x")
                    .and_then(|h| u32::from_str_radix(h, 16).ok())
                    .or_else(|| entity.strip_prefix('#').and_then(|d| d.parse().ok()))
                    .and_then(char::from_u32),
            };
            ch.map(|c| (c, end + 1))
        });
        match decoded {
            Some((c, len)) => {
                out.push(c);
                rest = &rest[len..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// Inner text of every `<tag>...</tag>`. Enough for S3's flat, attribute-free XML.
fn blocks<'a>(xml: &'a str, tag: &str) -> Vec<&'a str> {
    let (open, close) = (format!("<{tag}>"), format!("</{tag}>"));
    let mut found = Vec::new();
    let mut rest = xml;
    while let Some(start) = rest.find(&open) {
        let after = &rest[start + open.len()..];
        let Some(end) = after.find(&close) else { break };
        found.push(&after[..end]);
        rest = &after[end + close.len()..];
    }
    found
}

fn first(xml: &str, tag: &str) -> Option<String> {
    blocks(xml, tag).first().map(|s| xml_unescape(s))
}

fn parse_list_objects(xml: &str) -> ObjectPage {
    let objects = blocks(xml, "Contents")
        .into_iter()
        .filter_map(|item| {
            Some(ObjectInfo {
                key: first(item, "Key")?,
                size: first(item, "Size")
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0),
                etag: first(item, "ETag")
                    .unwrap_or_default()
                    .trim_matches('"')
                    .to_string(),
                last_modified: first(item, "LastModified").unwrap_or_default(),
            })
        })
        .collect();
    let truncated = first(xml, "IsTruncated").as_deref() == Some("true");
    let next_token = if truncated {
        first(xml, "NextContinuationToken")
    } else {
        None
    };
    let common_prefixes = blocks(xml, "CommonPrefixes")
        .into_iter()
        .filter_map(|b| first(b, "Prefix"))
        .collect();
    ObjectPage {
        objects,
        common_prefixes,
        next_token,
    }
}

fn parse_delete_result(xml: &str, requested: &[String]) -> Vec<DeleteOutcome> {
    let deleted: Vec<String> = blocks(xml, "Deleted")
        .into_iter()
        .filter_map(|b| first(b, "Key"))
        .collect();
    let failed: Vec<(String, String)> = blocks(xml, "Error")
        .into_iter()
        .filter_map(|b| Some((first(b, "Key")?, first(b, "Code").unwrap_or_default())))
        .collect();
    requested
        .iter()
        .map(|key| {
            let error = if let Some((_, code)) = failed.iter().find(|(k, _)| k == key) {
                Some(classify(None, Some(code), "s3:DeleteObject", None))
            } else if deleted.contains(key) {
                None
            } else {
                // Not reported either way: keeping the local row is the safe reading.
                Some(S3Error {
                    kind: S3ErrorKind::Other,
                    code: None,
                    message: "S3 did not confirm this deletion.".into(),
                })
            };
            DeleteOutcome {
                key: key.clone(),
                error,
            }
        })
        .collect()
}

fn error_from_response(
    status: u16,
    body: &str,
    action: &str,
    bucket_region: Option<&str>,
) -> S3Error {
    let code = first(body, "Code");
    // The header comes first; AuthorizationHeaderMalformed carries it in the body.
    let region = bucket_region
        .map(str::to_string)
        .or_else(|| first(body, "Region"))
        .filter(|r| !r.is_empty() && r.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'));
    classify(Some(status), code.as_deref(), action, region.as_deref())
}

fn classify(
    status: Option<u16>,
    code: Option<&str>,
    action: &str,
    region: Option<&str>,
) -> S3Error {
    let code = code.filter(|c| !c.is_empty());
    let (kind, message) = match code {
        Some("AccessDenied") => (
            S3ErrorKind::AccessDenied,
            format!("Access denied. The key pair needs {action} on this bucket."),
        ),
        Some("NoSuchBucket") => (
            S3ErrorKind::NoSuchBucket,
            "The bucket does not exist. Check the bucket name and region.".to_string(),
        ),
        Some("NoSuchKey") => (S3ErrorKind::NoSuchKey, "The message object no longer exists.".to_string()),
        Some("InvalidAccessKeyId" | "SignatureDoesNotMatch") => (
            S3ErrorKind::InvalidCredentials,
            "AWS rejected the access key id or secret.".to_string(),
        ),
        Some("RequestTimeTooSkewed") => (
            S3ErrorKind::ClockSkew,
            "This computer's clock is more than 5 minutes off AWS time. Fix the system clock and try again."
                .to_string(),
        ),
        Some("PermanentRedirect" | "AuthorizationHeaderMalformed") if region.is_some() => (
            S3ErrorKind::WrongRegion,
            format!(
                "The bucket is in region {}. Change this source's region to match.",
                region.unwrap_or_default()
            ),
        ),
        Some("PermanentRedirect") => (
            S3ErrorKind::WrongRegion,
            "The bucket is in a different region. Check this source's region.".to_string(),
        ),
        Some(other) => (S3ErrorKind::Other, format!("S3 returned an error ({other}).")),
        // No parsable body: a bare 403 is still an access problem.
        None if status == Some(403) => (
            S3ErrorKind::AccessDenied,
            format!("Access denied. The key pair needs {action} on this bucket."),
        ),
        None => (
            S3ErrorKind::Other,
            match status {
                Some(s) => format!("S3 returned an unexpected response (HTTP {s})."),
                None => "S3 returned an unexpected response.".to_string(),
            },
        ),
    };
    S3Error {
        kind,
        code: code.map(str::to_string),
        message,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn client(bucket: &str) -> S3Client {
        S3Client::new(
            Credentials {
                access_key_id: "AKIAIOSFODNN7EXAMPLE".into(),
                secret_access_key: "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY".into(),
            },
            "eu-west-1",
            bucket,
        )
    }

    #[test]
    fn parses_access_denied_body() {
        let body = r#"<?xml version="1.0" encoding="UTF-8"?>
<Error><Code>AccessDenied</Code><Message>Access Denied</Message><RequestId>4442587FB7D0A2F9</RequestId></Error>"#;
        let err = error_from_response(403, body, "s3:ListBucket", None);
        assert_eq!(err.kind, S3ErrorKind::AccessDenied);
        assert_eq!(err.code.as_deref(), Some("AccessDenied"));
        assert!(err.message.contains("s3:ListBucket"));
    }

    #[test]
    fn distinguishes_other_codes_and_falls_back() {
        let no_bucket = error_from_response(
            404,
            "<Error><Code>NoSuchBucket</Code></Error>",
            "s3:ListBucket",
            None,
        );
        assert_eq!(no_bucket.kind, S3ErrorKind::NoSuchBucket);
        let skew = error_from_response(
            403,
            "<Error><Code>RequestTimeTooSkewed</Code></Error>",
            "s3:GetObject",
            None,
        );
        assert_eq!(skew.kind, S3ErrorKind::ClockSkew);
        let garbage = error_from_response(500, "<html>oops", "s3:GetObject", None);
        assert_eq!(garbage.kind, S3ErrorKind::Other);
        assert_eq!(garbage.code, None);
        assert_eq!(
            error_from_response(403, "", "s3:GetObject", None).kind,
            S3ErrorKind::AccessDenied
        );
    }

    #[test]
    fn parses_list_response() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<ListBucketResult xmlns="http://s3.amazonaws.com/doc/2006-03-01/">
<Name>mail</Name><Prefix>inbox/</Prefix><KeyCount>2</KeyCount><MaxKeys>1000</MaxKeys>
<IsTruncated>true</IsTruncated><NextContinuationToken>1ueGcxLPRx1Tr/XYExHnhbYLgveDs2J/wm36Hy4vbOwM=</NextContinuationToken>
<Contents><Key>inbox/abc123</Key><LastModified>2024-05-01T10:00:00.000Z</LastModified><ETag>&quot;d41d8cd98f00b204e9800998ecf8427e&quot;</ETag><Size>2048</Size><StorageClass>STANDARD</StorageClass></Contents>
<Contents><Key>inbox/a&amp;b&lt;c</Key><LastModified>2024-05-02T11:30:00.000Z</LastModified><ETag>&quot;ffff&quot;</ETag><Size>7</Size></Contents>
</ListBucketResult>"#;
        let page = parse_list_objects(xml);
        assert_eq!(page.objects.len(), 2);
        assert_eq!(page.objects[0].key, "inbox/abc123");
        assert_eq!(page.objects[0].size, 2048);
        assert_eq!(page.objects[0].etag, "d41d8cd98f00b204e9800998ecf8427e");
        assert_eq!(page.objects[0].last_modified, "2024-05-01T10:00:00.000Z");
        assert_eq!(page.objects[1].key, "inbox/a&b<c");
        assert_eq!(
            page.next_token.as_deref(),
            Some("1ueGcxLPRx1Tr/XYExHnhbYLgveDs2J/wm36Hy4vbOwM=")
        );
    }

    #[test]
    fn last_page_has_no_token() {
        let page = parse_list_objects(
            "<ListBucketResult><IsTruncated>false</IsTruncated></ListBucketResult>",
        );
        assert!(page.objects.is_empty());
        assert_eq!(page.next_token, None);
    }

    #[test]
    fn delete_result_reports_each_key() {
        let xml = "<DeleteResult><Deleted><Key>a</Key></Deleted>\
                   <Error><Key>b&amp;c</Key><Code>AccessDenied</Code><Message>Access Denied</Message></Error></DeleteResult>";
        let keys: Vec<String> = ["a", "b&c", "missing"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let out = parse_delete_result(xml, &keys);
        assert_eq!(out[0].error, None);
        assert_eq!(
            out[1].error.as_ref().unwrap().kind,
            S3ErrorKind::AccessDenied
        );
        assert_eq!(out[2].error.as_ref().unwrap().kind, S3ErrorKind::Other);
    }

    #[test]
    fn delete_body_escapes_keys() {
        assert_eq!(
            delete_request_body(&["a&b".to_string()]),
            "<Delete><Quiet>false</Quiet><Object><Key>a&amp;b</Key></Object></Delete>"
        );
    }

    #[test]
    fn request_is_path_style_and_signed_without_network() {
        let c = client("mail.example.com");
        let p = c.prepare(
            "GET",
            "inbox/a b",
            &[("list-type", "2")],
            &[],
            &[],
            "20240102T030405Z",
        );
        assert_eq!(
            p.url,
            "https://s3.eu-west-1.amazonaws.com/mail.example.com/inbox/a%20b?list-type=2"
        );
        let auth = &p
            .headers
            .iter()
            .find(|(k, _)| k == "Authorization")
            .unwrap()
            .1;
        assert!(auth.starts_with(
            "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20240102/eu-west-1/s3/aws4_request, SignedHeaders=host;x-amz-content-sha256;x-amz-date, Signature="
        ));
        assert!(!p.headers.iter().any(|(k, _)| k == "host"));
    }

    #[test]
    fn redirect_status_is_an_error_not_a_payload() {
        let body = b"<Error><Code>PermanentRedirect</Code></Error>".to_vec();
        let err = check_status(301, body, "s3:ListBucket", None).unwrap_err();
        assert_eq!(err.code.as_deref(), Some("PermanentRedirect"));
        assert_eq!(err.kind, S3ErrorKind::WrongRegion);
        assert_eq!(
            check_status(200, b"ok".to_vec(), "s3:GetObject", None).unwrap(),
            b"ok"
        );
    }

    #[test]
    fn secret_never_reaches_the_request() {
        let p = client("b").prepare("GET", "k", &[("prefix", "p")], &[], &[], "20240102T030405Z");
        assert!(!p.url.contains("EXAMPLEKEY"));
        assert!(p
            .headers
            .iter()
            .all(|(k, v)| !k.contains("EXAMPLEKEY") && !v.contains("EXAMPLEKEY")));
    }

    #[test]
    fn parses_common_prefixes() {
        let xml = "<ListBucketResult><IsTruncated>false</IsTruncated>\
            <Contents><Key>top.txt</Key><Size>1</Size></Contents>\
            <CommonPrefixes><Prefix>owner/</Prefix></CommonPrefixes>\
            <CommonPrefixes><Prefix>seller/</Prefix></CommonPrefixes></ListBucketResult>";
        let page = parse_list_objects(xml);
        assert_eq!(page.objects.len(), 1);
        assert_eq!(page.common_prefixes, ["owner/", "seller/"]);
    }

    #[test]
    fn delimiter_is_url_encoded_in_the_query() {
        let p = client("b").prepare(
            "GET",
            "",
            &[("list-type", "2"), ("delimiter", "/")],
            &[],
            &[],
            "20240102T030405Z",
        );
        assert!(p.url.ends_with("?delimiter=%2F&list-type=2"));
    }

    fn ok_xml(keys: &[String]) -> Result<Vec<u8>, S3Error> {
        let items: String = keys
            .iter()
            .map(|k| format!("<Deleted><Key>{}</Key></Deleted>", xml_escape(k)))
            .collect();
        Ok(format!("<DeleteResult>{items}</DeleteResult>").into_bytes())
    }

    fn net_err() -> S3Error {
        S3Error {
            kind: S3ErrorKind::Network,
            code: None,
            message: "down".into(),
        }
    }

    fn five() -> Vec<String> {
        (0..5).map(|i| format!("k{i}")).collect()
    }

    fn assert_each_once(out: &[DeleteOutcome], keys: &[String]) {
        let got: Vec<&String> = out.iter().map(|o| &o.key).collect();
        assert_eq!(got, keys.iter().collect::<Vec<_>>());
    }

    #[test]
    fn batches_clean_run() {
        let keys = five();
        let out = delete_in_batches(&keys, 2, ok_xml);
        assert_each_once(&out, &keys);
        assert!(out.iter().all(|o| o.error.is_none()));
    }

    #[test]
    fn batches_fail_on_first_batch() {
        let keys = five();
        let out = delete_in_batches(&keys, 2, |_| Err(net_err()));
        assert_each_once(&out, &keys);
        assert!(out.iter().all(|o| o.error.is_some()));
    }

    #[test]
    fn batches_fail_on_middle_batch_keep_earlier_results() {
        let keys = five();
        let mut calls = 0;
        let out = delete_in_batches(&keys, 2, |b| {
            calls += 1;
            if calls == 2 {
                Err(net_err())
            } else {
                ok_xml(b)
            }
        });
        assert_each_once(&out, &keys);
        assert_eq!(calls, 2);
        assert!(out[..2].iter().all(|o| o.error.is_none()));
        assert!(out[2..].iter().all(|o| o.error.is_some()));
    }

    #[test]
    fn delete_result_has_one_entry_per_requested_key() {
        let keys = five();
        assert_eq!(parse_delete_result("garbage", &keys).len(), keys.len());
    }

    #[test]
    fn redirect_names_the_region_and_ignores_junk() {
        let body =
            "<Error><Code>PermanentRedirect</Code><Endpoint>b.s3.amazonaws.com</Endpoint></Error>";
        let err = error_from_response(301, body, "s3:ListBucket", Some("eu-west-1"));
        assert_eq!(err.kind, S3ErrorKind::WrongRegion);
        assert!(err.message.contains("eu-west-1"));
        let bad = error_from_response(301, body, "s3:ListBucket", Some("x<y"));
        assert_eq!(bad.kind, S3ErrorKind::WrongRegion);
        assert!(!bad.message.contains('<'));
    }
}
