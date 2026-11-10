//! Saves of `remote://` text files through the Host's `write_text_file`.
//!
//! A Host with the capability `files.write.hash` compares `expectedSha256` with the file it
//! is about to replace and writes in the same call, so a save never overwrites an edit made
//! by the agent in the meantime. Without the capability the file is read again and compared
//! here, then written (today's behaviour, with a window of two round trips).
//!
//! The Host hashes the RAW bytes, a BOM is not stripped. So the client hashes the bytes it
//! read ([`super::document::TextDocument::disk`]), never the decoded text. A missing target
//! with `expectedSha256` counts as a mismatch, and a retry whose content is already on the
//! Host succeeds, so an ambiguous failure (a timeout) is retried with the same hash.
use super::document::{TextDocument, REMOTE_TEXT_BYTES};
use super::remote_read::{read_bytes, Cache, Fetched, Transport};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::path::Path;
use std::sync::atomic::AtomicBool;

/// Capability string of a Host that checks `expectedSha256` itself.
pub const HASH_CAPABILITY: &str = "files.write.hash";
/// The text of the Host's refusal when the file is no longer what the editor loaded.
pub const HOST_CHANGED: &str = "File changed on the host";
const STALE_COPY: &str = "The file changed on disk. Reload it before saving your edits.";

/// True when `error` is the Host refusing a conditional write because the file changed.
pub fn is_host_conflict(error: &str) -> bool {
    error.contains(HOST_CHANGED)
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// What a save checks against.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// The file must still be what the document loaded.
    Guarded,
    /// The user chose to replace what is there now: take its hash first.
    Overwrite,
}

fn missing(error: &str) -> bool {
    let error = error.to_ascii_lowercase();
    ["enoent", "no such file", "not found"]
        .iter()
        .any(|needle| error.contains(needle))
}

fn read_current(transport: &dyn Transport, cache: &Cache, path: &Path) -> Result<Fetched, String> {
    read_bytes(
        transport,
        cache,
        path,
        &mut |_, _| {},
        &AtomicBool::new(false),
        &|_, _| None,
    )
}

/// Saves `text` over the host file behind `path` and returns the bytes written.
pub(crate) fn save_remote(
    transport: &dyn Transport,
    cache: &Cache,
    path: &Path,
    document: &TextDocument,
    text: &str,
    mode: Mode,
) -> Result<Vec<u8>, String> {
    if let Some(reason) = &document.read_only {
        return Err(reason.clone());
    }
    let bytes = document.encode(text);
    if bytes.len() > REMOTE_TEXT_BYTES {
        return Err("Too large to save remotely (maximum 1 MB).".into());
    }
    let content = std::str::from_utf8(&bytes).map_err(|_| "File is not valid UTF-8.")?;
    let remote = crate::remote::parse_remote_path(&path.to_string_lossy())
        .ok_or("Invalid remote file path")?;
    let machine = transport.machine_for_environment(&remote.environment_id)?;
    let hashed = transport.host_info(&machine)?.supports(HASH_CAPABILITY);
    let mut args = json!({"path": remote.host_path, "content": content});
    match (hashed, mode) {
        (true, Mode::Guarded) => args["expectedSha256"] = json!(document.disk_sha256()),
        (true, Mode::Overwrite) => match read_current(transport, cache, path) {
            Ok(Fetched::Bytes(current)) => args["expectedSha256"] = json!(sha256_hex(&current)),
            Ok(Fetched::Refused(reason)) => return Err(reason),
            // The file was deleted meanwhile: Keep mine creates it, without a hash.
            Err(error) if missing(&error) => {}
            Err(error) => return Err(error),
        },
        (false, Mode::Guarded) => {
            if read_current(transport, cache, path)? != Fetched::Bytes(document.disk.clone()) {
                return Err(STALE_COPY.into());
            }
        }
        (false, Mode::Overwrite) => {}
    }
    transport
        .request(
            &machine,
            "workspace.run",
            json!({"command": "write_text_file", "args": args}),
        )
        .map_err(super::remote::workspace_error)?;
    Ok(bytes)
}

/// [`save_remote`] over the app's shared connections.
pub(crate) fn save_default(
    path: &Path,
    document: &TextDocument,
    text: &str,
    mode: Mode,
) -> Result<Vec<u8>, String> {
    let connections = crate::remote::default_connections()?;
    save_remote(
        &*connections,
        &super::remote_read::CACHE,
        path,
        document,
        text,
        mode,
    )
}

#[cfg(test)]
mod tests {
    use super::super::document::{document_from_bytes, Document};
    use super::*;
    use base64::Engine;
    use serde_json::Value;
    use std::cell::RefCell;

    const PATH: &str = "remote://env/work/repo/a.txt";

    /// A Host with one project and one file, mirroring the Host's conditional write.
    struct Fake {
        hash_capable: bool,
        file: RefCell<Option<Vec<u8>>>,
        writes: RefCell<Vec<Value>>,
        reads: RefCell<usize>,
        /// The next write is applied, then its reply is lost (a timeout).
        lose_next_reply: RefCell<bool>,
        info: crate::remote::host_info::HostInfoCache,
    }
    impl Fake {
        fn new(hash_capable: bool, file: Option<&[u8]>) -> Self {
            Self {
                hash_capable,
                file: RefCell::new(file.map(<[u8]>::to_vec)),
                writes: RefCell::new(Vec::new()),
                reads: RefCell::new(0),
                lose_next_reply: RefCell::new(false),
                info: Default::default(),
            }
        }
    }
    impl Transport for Fake {
        fn host_info(&self, machine: &str) -> Result<crate::remote::host_info::HostInfo, String> {
            let described = self.request(machine, "environment.describe", json!({}))?;
            Ok(self.info.record(machine, &described))
        }
        fn machine_for_environment(&self, _: &str) -> Result<String, String> {
            Ok("machine".into())
        }
        fn request(&self, _: &str, method: &str, params: Value) -> Result<Value, String> {
            match method {
                "environment.describe" => Ok(json!({"capabilities": if self.hash_capable {
                    json!(["files.readBytes", HASH_CAPABILITY])
                } else {
                    json!(["files.readBytes"])
                }})),
                "projects.list" => Ok(json!([{"id": "p1", "cwd": "/work/repo"}])),
                "git.worktrees" => Ok(json!({"worktrees": []})),
                "files.readBytes" => {
                    *self.reads.borrow_mut() += 1;
                    let file = self.file.borrow();
                    let data = file.as_ref().ok_or("ENOENT: no such file")?;
                    Ok(json!({
                        "data": base64::engine::general_purpose::STANDARD.encode(data),
                        "offset": data.len(),
                        "size": data.len(),
                    }))
                }
                "workspace.run" => {
                    assert_eq!(params["command"], "write_text_file");
                    let args = &params["args"];
                    self.writes.borrow_mut().push(args.clone());
                    let content = args["content"].as_str().unwrap().as_bytes().to_vec();
                    let mut file = self.file.borrow_mut();
                    if let Some(expected) = args["expectedSha256"].as_str() {
                        let matches = file.as_deref().map(sha256_hex).as_deref() == Some(expected);
                        // An identical-content retry is idempotent.
                        if !matches && file.as_deref() != Some(content.as_slice()) {
                            return Err(HOST_CHANGED.into());
                        }
                    }
                    *file = Some(content);
                    if self.lose_next_reply.replace(false) {
                        return Err("Request timed out".into());
                    }
                    Ok(Value::Null)
                }
                other => panic!("unexpected {other}"),
            }
        }
    }

    fn document(bytes: &[u8]) -> TextDocument {
        let Document::Text(doc) =
            document_from_bytes(Path::new(PATH), bytes.to_vec(), 2 * 1024 * 1024).unwrap()
        else {
            panic!("expected text")
        };
        doc
    }
    fn save(host: &Fake, doc: &TextDocument, text: &str, mode: Mode) -> Result<Vec<u8>, String> {
        save_remote(host, &Cache::default(), Path::new(PATH), doc, text, mode)
    }

    #[test]
    fn a_matching_hash_saves_and_sends_the_hash_of_the_raw_bytes() {
        let host = Fake::new(true, Some(b"one\n"));
        let doc = document(b"one\n");
        let written = save(&host, &doc, "two\n", Mode::Guarded).unwrap();
        assert_eq!(written, b"two\n");
        assert_eq!(host.file.borrow().as_deref(), Some(&b"two\n"[..]));
        let writes = host.writes.borrow();
        assert_eq!(writes[0]["expectedSha256"], sha256_hex(b"one\n"));
        assert_eq!(writes[0]["path"], "/work/repo/a.txt");
        // The compare is the Host's: no read-back by the client.
        assert_eq!(*host.reads.borrow(), 0);
    }

    #[test]
    fn a_mismatch_is_reported_and_nothing_is_written() {
        let host = Fake::new(true, Some(b"one\n"));
        let doc = document(b"one\n");
        *host.file.borrow_mut() = Some(b"agent edit\n".to_vec());
        let error = save(&host, &doc, "mine\n", Mode::Guarded).unwrap_err();
        assert!(is_host_conflict(&error), "{error}");
        assert_eq!(host.file.borrow().as_deref(), Some(&b"agent edit\n"[..]));
    }

    #[test]
    fn keep_mine_overwrites_with_the_new_hash() {
        let host = Fake::new(true, Some(b"one\n"));
        let doc = document(b"one\n");
        *host.file.borrow_mut() = Some(b"agent edit\n".to_vec());
        assert!(save(&host, &doc, "mine\n", Mode::Guarded).is_err());
        save(&host, &doc, "mine\n", Mode::Overwrite).unwrap();
        assert_eq!(host.file.borrow().as_deref(), Some(&b"mine\n"[..]));
        let writes = host.writes.borrow();
        assert_eq!(writes[1]["expectedSha256"], sha256_hex(b"agent edit\n"));
        assert_ne!(writes[1]["expectedSha256"], writes[0]["expectedSha256"]);
    }

    #[test]
    fn keep_mine_recreates_a_deleted_file_without_a_hash() {
        let host = Fake::new(true, Some(b"one\n"));
        let doc = document(b"one\n");
        *host.file.borrow_mut() = None;
        let error = save(&host, &doc, "mine\n", Mode::Guarded).unwrap_err();
        assert!(is_host_conflict(&error), "{error}");
        save(&host, &doc, "mine\n", Mode::Overwrite).unwrap();
        assert_eq!(host.file.borrow().as_deref(), Some(&b"mine\n"[..]));
        assert!(host.writes.borrow()[1].get("expectedSha256").is_none());
    }

    #[test]
    fn without_the_capability_the_file_is_read_again_and_no_hash_is_sent() {
        let host = Fake::new(false, Some(b"one\n"));
        let doc = document(b"one\n");
        save(&host, &doc, "two\n", Mode::Guarded).unwrap();
        assert_eq!(*host.reads.borrow(), 1);
        assert!(host.writes.borrow()[0].get("expectedSha256").is_none());
        // A change between the read and the save is caught by the read-back.
        *host.file.borrow_mut() = Some(b"agent edit\n".to_vec());
        let error = save(&host, &doc, "mine\n", Mode::Guarded).unwrap_err();
        assert!(error.contains("changed on disk"), "{error}");
        assert_eq!(host.writes.borrow().len(), 1);
    }

    #[test]
    fn a_bom_file_hashes_the_raw_bytes_and_saves() {
        let raw = b"\xef\xbb\xbfa\r\n";
        let host = Fake::new(true, Some(raw));
        let doc = document(raw);
        assert!(doc.bom && doc.crlf);
        assert_eq!(doc.disk_sha256(), sha256_hex(raw));
        // Hashing the decoded text instead would not match the Host.
        assert_ne!(doc.disk_sha256(), sha256_hex(doc.text.as_bytes()));
        let written = save(&host, &doc, "changed\n", Mode::Guarded).unwrap();
        assert_eq!(written, b"\xef\xbb\xbfchanged\r\n");
        assert_eq!(host.file.borrow().as_deref(), Some(&written[..]));
        assert_eq!(host.writes.borrow()[0]["expectedSha256"], sha256_hex(raw));
    }

    #[test]
    fn a_retry_after_a_lost_reply_resends_the_same_hash_and_succeeds() {
        let host = Fake::new(true, Some(b"one\n"));
        let doc = document(b"one\n");
        *host.lose_next_reply.borrow_mut() = true;
        let error = save(&host, &doc, "two\n", Mode::Guarded).unwrap_err();
        assert!(error.contains("timed out"));
        // The write did land, so the hash no longer matches; the Host sees the same
        // content and answers success.
        save(&host, &doc, "two\n", Mode::Guarded).unwrap();
        let writes = host.writes.borrow();
        assert_eq!(writes.len(), 2);
        assert_eq!(writes[0]["expectedSha256"], writes[1]["expectedSha256"]);
        assert_eq!(host.file.borrow().as_deref(), Some(&b"two\n"[..]));
    }

    #[test]
    fn a_file_over_one_megabyte_is_read_only_and_never_sent() {
        let big = "aaaaaaaaa\n".repeat(150_000).into_bytes();
        let doc = document(&big);
        assert_eq!(
            doc.read_only.as_deref(),
            Some(super::super::document::REMOTE_READ_ONLY)
        );
        assert!(document(&big[..1024 * 1024]).read_only.is_none());
        let host = Fake::new(true, Some(&big));
        let error = save(
            &host,
            &doc,
            &String::from_utf8(big.clone()).unwrap(),
            Mode::Guarded,
        )
        .unwrap_err();
        assert!(error.contains("1 MB"), "{error}");
        assert!(host.writes.borrow().is_empty());
        // A file the editor could edit that grows past the limit is refused as well.
        let small = document(b"a");
        assert!(save(&host, &small, &"aaaaaaaaa\n".repeat(110_000), Mode::Guarded).is_err());
        assert!(host.writes.borrow().is_empty());
    }

    #[test]
    fn the_hash_argument_survives_argument_mapping() {
        let (_, args) = super::super::remote::host_arguments_for_tests(
            "write_text_file",
            json!({"path": PATH, "content": "x", "expectedSha256": "abc"}),
        )
        .unwrap();
        assert_eq!(args["expectedSha256"], "abc");
        assert_eq!(args["path"], "/work/repo/a.txt");
    }
}
