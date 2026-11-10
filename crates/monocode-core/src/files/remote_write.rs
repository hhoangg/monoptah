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
    /// The file must still be what `document.disk` says it was.
    Guarded,
    /// The file did not exist when the user chose to keep their text: create it, with no
    /// hash. (Keep mine over an existing version always uses [`Mode::Guarded`] with the
    /// bytes the user was shown.)
    Create,
}

/// True when a read failed because the file does not exist.
pub fn is_missing(error: &str) -> bool {
    let error = error.to_ascii_lowercase();
    ["enoent", "no such file", "not found"]
        .iter()
        .any(|needle| error.contains(needle))
}

/// The oldest Host that checks `expectedSha256`. An older Host ignores the argument and
/// would write unconditionally, so a stale capability list must not be trusted below it.
const MIN_HASH_HOST: (u64, u64, u64) = (1, 1, 0);

fn parse_version(text: &str) -> Option<(u64, u64, u64)> {
    let core = text.split(['+', '-']).next()?;
    let mut parts = core.split('.').map(|part| part.parse::<u64>().ok());
    Some((parts.next()??, parts.next()??, parts.next()??))
}

/// The Host verifiably checks the hash: it says so and it is a fork Host new enough.
fn checks_hash(info: &crate::remote::host_info::HostInfo) -> bool {
    info.supports(HASH_CAPABILITY)
        && info.is_monoptah()
        && info
            .version
            .as_deref()
            .and_then(parse_version)
            .is_some_and(|version| version >= MIN_HASH_HOST)
}

/// The Host does not know the command or argument (it was replaced by an older one).
fn is_unsupported(error: &str) -> bool {
    let error = error.to_ascii_lowercase();
    [
        "unsupported host method",
        "unsupported workspace command",
        "unsupported remote operation",
        "unknown argument",
        "unknown parameter",
        "unrecognized",
    ]
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
    let write = |args: serde_json::Value| {
        transport.request(
            &machine,
            "workspace.run",
            json!({"command": "write_text_file", "args": args}),
        )
    };
    let mut args = json!({"path": remote.host_path, "content": content});
    if mode == Mode::Guarded && checks_hash(&transport.host_info(&machine)?) {
        args["expectedSha256"] = json!(document.disk_sha256());
        match write(args.clone()) {
            Ok(_) => return Ok(bytes),
            Err(error) if is_unsupported(&error) => {
                // The Host is not the one described: forget it and compare here instead.
                transport.forget_host_info(&machine);
            }
            Err(error) => return Err(super::remote::workspace_error(error)),
        }
        args.as_object_mut()
            .map(|args| args.remove("expectedSha256"));
    }
    if mode == Mode::Guarded
        && read_current(transport, cache, path)? != Fetched::Bytes(document.disk.clone())
    {
        return Err(STALE_COPY.into());
    }
    write(args).map_err(super::remote::workspace_error)?;
    Ok(bytes)
}

/// Keep mine: saves over the version the user was shown (`shown` bytes, `None` when the
/// file did not exist), never over a newer one.
pub(crate) fn overwrite_remote(
    transport: &dyn Transport,
    cache: &Cache,
    path: &Path,
    document: &TextDocument,
    text: &str,
    shown: Option<&[u8]>,
) -> Result<Vec<u8>, String> {
    match shown {
        Some(shown) => {
            let mut baseline = document.clone();
            baseline.disk = shown.to_vec();
            save_remote(transport, cache, path, &baseline, text, Mode::Guarded)
        }
        None => save_remote(transport, cache, path, document, text, Mode::Create),
    }
}

pub(crate) fn overwrite_default(
    path: &Path,
    document: &TextDocument,
    text: &str,
    shown: Option<&[u8]>,
) -> Result<Vec<u8>, String> {
    let connections = crate::remote::default_connections()?;
    overwrite_remote(
        &*connections,
        &super::remote_read::CACHE,
        path,
        document,
        text,
        shown,
    )
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
        /// Version the Host reports; `None` reports no `host` field.
        version: Option<&'static str>,
        /// A write that carries `expectedSha256` fails with this error.
        reject_hash: Option<&'static str>,
        forgotten: RefCell<usize>,
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
                version: Some("1.1.0"),
                reject_hash: None,
                forgotten: RefCell::new(0),
                info: Default::default(),
            }
        }
    }
    impl Transport for Fake {
        fn host_info(&self, machine: &str) -> Result<crate::remote::host_info::HostInfo, String> {
            let described = self.request(machine, "environment.describe", json!({}))?;
            Ok(self.info.record(machine, &described))
        }
        fn forget_host_info(&self, machine: &str) {
            *self.forgotten.borrow_mut() += 1;
            self.info.drop_machine(machine);
        }
        fn machine_for_environment(&self, _: &str) -> Result<String, String> {
            Ok("machine".into())
        }
        fn request(&self, _: &str, method: &str, params: Value) -> Result<Value, String> {
            match method {
                "environment.describe" => {
                    let mut described = json!({"capabilities": if self.hash_capable {
                        json!(["files.readBytes", HASH_CAPABILITY])
                    } else {
                        json!(["files.readBytes"])
                    }});
                    if let Some(version) = self.version {
                        described["host"] = json!({"flavor": "monoptah", "version": version});
                    }
                    Ok(described)
                }
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
                    if let (Some(error), Some(_)) =
                        (self.reject_hash, args["expectedSha256"].as_str())
                    {
                        return Err(error.into());
                    }
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

    fn keep_mine(
        host: &Fake,
        doc: &TextDocument,
        text: &str,
        shown: Option<&[u8]>,
    ) -> Result<Vec<u8>, String> {
        overwrite_remote(host, &Cache::default(), Path::new(PATH), doc, text, shown)
    }

    #[test]
    fn keep_mine_overwrites_the_version_that_was_shown_using_its_hash() {
        let host = Fake::new(true, Some(b"one\n"));
        let doc = document(b"one\n");
        *host.file.borrow_mut() = Some(b"agent edit\n".to_vec());
        assert!(save(&host, &doc, "mine\n", Mode::Guarded).is_err());
        keep_mine(&host, &doc, "mine\n", Some(b"agent edit\n")).unwrap();
        assert_eq!(host.file.borrow().as_deref(), Some(&b"mine\n"[..]));
        let writes = host.writes.borrow();
        assert_eq!(writes[1]["expectedSha256"], sha256_hex(b"agent edit\n"));
        // No read-back: the hash is the one of the shown version, not of "now".
        assert_eq!(*host.reads.borrow(), 0);
    }

    #[test]
    fn keep_mine_does_not_overwrite_a_version_the_user_never_saw() {
        let host = Fake::new(true, Some(b"one\n"));
        let doc = document(b"one\n");
        // The dialog showed A; the agent wrote B before the click.
        *host.file.borrow_mut() = Some(b"B\n".to_vec());
        let error = keep_mine(&host, &doc, "mine\n", Some(b"A\n")).unwrap_err();
        assert!(is_host_conflict(&error), "{error}");
        assert_eq!(host.file.borrow().as_deref(), Some(&b"B\n"[..]));
    }

    #[test]
    fn keep_mine_recreates_a_file_that_was_missing_when_shown_without_a_hash() {
        let host = Fake::new(true, Some(b"one\n"));
        let doc = document(b"one\n");
        *host.file.borrow_mut() = None;
        let error = save(&host, &doc, "mine\n", Mode::Guarded).unwrap_err();
        assert!(is_host_conflict(&error), "{error}");
        keep_mine(&host, &doc, "mine\n", None).unwrap();
        assert_eq!(host.file.borrow().as_deref(), Some(&b"mine\n"[..]));
        assert!(host.writes.borrow()[1].get("expectedSha256").is_none());
    }

    #[test]
    fn a_host_older_than_the_pin_is_compared_by_a_read_even_if_it_lists_the_capability() {
        for version in [Some("1.0.0"), None] {
            let mut host = Fake::new(true, Some(b"one\n"));
            host.version = version;
            let doc = document(b"one\n");
            save(&host, &doc, "two\n", Mode::Guarded).unwrap();
            assert_eq!(*host.reads.borrow(), 1, "{version:?}");
            assert!(host.writes.borrow()[0].get("expectedSha256").is_none());
            *host.file.borrow_mut() = Some(b"agent edit\n".to_vec());
            assert!(save(&host, &doc, "mine\n", Mode::Guarded).is_err());
            assert_eq!(host.writes.borrow().len(), 1);
        }
    }

    #[test]
    fn a_host_that_rejects_the_hash_is_forgotten_and_compared_by_a_read() {
        let mut host = Fake::new(true, Some(b"one\n"));
        host.reject_hash = Some("Unsupported host method: write_text_file");
        let doc = document(b"one\n");
        save(&host, &doc, "two\n", Mode::Guarded).unwrap();
        assert_eq!(*host.forgotten.borrow(), 1);
        assert_eq!(*host.reads.borrow(), 1);
        assert_eq!(host.file.borrow().as_deref(), Some(&b"two\n"[..]));
        // The fallback still refuses a changed file.
        *host.file.borrow_mut() = Some(b"agent edit\n".to_vec());
        let error = save(&host, &doc, "mine\n", Mode::Guarded).unwrap_err();
        assert!(error.contains("changed on disk"), "{error}");
        assert_eq!(host.file.borrow().as_deref(), Some(&b"agent edit\n"[..]));
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
