//! Client for the Host's generic chunked uploads (`uploads.*`). Offsets are
//! idempotent on the Host, so a lost response is simply re-sent, and an
//! interrupted transfer resumes from the offset the Host reports.
use base64::{engine::general_purpose::STANDARD, Engine};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

/// The Host's chunk size; a smaller `chunkSize` from the Host wins.
pub const CHUNK_BYTES: usize = 512 * 1024;
pub const MAX_UPLOAD_BYTES: u64 = 500 * 1024 * 1024;
/// Re-sends of one chunk before the transfer gives up.
const CHUNK_ATTEMPTS: usize = 3;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UploadedFile {
    pub upload_id: String,
    pub name: String,
    /// Absolute path of the committed file on the Host.
    pub path: String,
    pub size: u64,
    pub sha256: String,
}

/// A failed transfer. `upload_id` lets the caller resume it later.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UploadError {
    pub message: String,
    pub upload_id: Option<String>,
    pub cancelled: bool,
}
impl std::fmt::Display for UploadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for UploadError {}

pub struct UploadOptions<'a> {
    /// An earlier `UploadError::upload_id` to continue from.
    pub resume_id: Option<String>,
    /// Called with `(bytes_done, bytes_total)` after every accepted chunk.
    pub progress: Option<&'a mut dyn FnMut(u64, u64)>,
    /// Checked between chunks; set it to stop the transfer.
    pub cancel: Option<&'a AtomicBool>,
}
impl Default for UploadOptions<'_> {
    fn default() -> Self {
        Self {
            resume_id: None,
            progress: None,
            cancel: None,
        }
    }
}

pub fn sha256_file(path: &Path) -> Result<(String, u64), String> {
    let mut file = File::open(path).map_err(|e| format!("Cannot read {}: {e}", path.display()))?;
    let mut hash = Sha256::new();
    let mut buffer = vec![0u8; CHUNK_BYTES];
    let mut size = 0u64;
    loop {
        let read = file.read(&mut buffer).map_err(|e| e.to_string())?;
        if read == 0 {
            break;
        }
        hash.update(&buffer[..read]);
        size += read as u64;
    }
    Ok((hex(&hash.finalize()), size))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Uploads `path` as `name` through `request(method, params)`, which sends
/// one Host RPC. Returns the committed file on the Host.
pub fn upload_file(
    request: &mut dyn FnMut(&str, Value) -> Result<Value, String>,
    path: &Path,
    name: &str,
    mut options: UploadOptions<'_>,
) -> Result<UploadedFile, UploadError> {
    let fail = |message: String, id: Option<&str>| UploadError {
        message,
        upload_id: id.map(str::to_string),
        cancelled: false,
    };
    let (sha256, size) = sha256_file(path).map_err(|m| fail(m, None))?;
    if size > MAX_UPLOAD_BYTES {
        return Err(fail("File is larger than 500 MB.".into(), None));
    }
    let mut begin = json!({"name": name, "size": size, "sha256": sha256});
    if let Some(id) = &options.resume_id {
        begin["uploadId"] = json!(id);
    }
    let started =
        request("uploads.begin", begin).map_err(|m| fail(m, options.resume_id.as_deref()))?;
    let id = started["uploadId"]
        .as_str()
        .filter(|id| !id.is_empty())
        .ok_or_else(|| fail("Invalid upload response".into(), None))?
        .to_string();
    let mut offset = started["offset"].as_u64().unwrap_or(0);
    let chunk = started["chunkSize"]
        .as_u64()
        .and_then(|n| usize::try_from(n).ok())
        .filter(|n| *n > 0)
        .map_or(CHUNK_BYTES, |n| n.min(CHUNK_BYTES));
    if offset > size {
        return Err(fail("Invalid upload offset".into(), Some(&id)));
    }
    let mut file = File::open(path).map_err(|e| fail(e.to_string(), Some(&id)))?;
    let mut buffer = vec![0u8; chunk];
    while offset < size {
        if options.cancel.is_some_and(|c| c.load(Ordering::Relaxed)) {
            let _ = request("uploads.discard", json!({"uploadId": id}));
            return Err(UploadError {
                message: "Upload cancelled".into(),
                upload_id: None,
                cancelled: true,
            });
        }
        let want = chunk.min((size - offset) as usize);
        file.seek(SeekFrom::Start(offset))
            .and_then(|_| file.read_exact(&mut buffer[..want]))
            .map_err(|e| fail(format!("Cannot read {}: {e}", path.display()), Some(&id)))?;
        let params = json!({
            "uploadId": id,
            "offset": offset,
            "data": STANDARD.encode(&buffer[..want]),
        });
        let mut last = String::new();
        let mut accepted = None;
        for _ in 0..CHUNK_ATTEMPTS {
            match request("uploads.chunk", params.clone()) {
                Ok(reply) => {
                    accepted = Some(reply);
                    break;
                }
                Err(e) => last = e,
            }
        }
        let reply = accepted.ok_or_else(|| fail(last, Some(&id)))?;
        let next = reply["offset"].as_u64().unwrap_or(0);
        if next != offset + want as u64 {
            return Err(fail("Invalid upload chunk response".into(), Some(&id)));
        }
        offset = next;
        if let Some(progress) = options.progress.as_mut() {
            progress(offset, size);
        }
    }
    let done =
        request("uploads.commit", json!({"uploadId": id})).map_err(|m| fail(m, Some(&id)))?;
    let path = done["path"].as_str().filter(|p| !p.is_empty());
    match path {
        Some(path) if done["sha256"].as_str() == Some(sha256.as_str()) => Ok(UploadedFile {
            upload_id: id,
            name: name.to_string(),
            path: path.to_string(),
            size,
            sha256,
        }),
        _ => Err(fail("Invalid upload commit response".into(), Some(&id))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// An in-memory Host with the same upload semantics as `host/uploads.ts`.
    #[derive(Default)]
    struct FakeHost {
        uploads: HashMap<String, (String, u64, String, Vec<u8>)>,
        calls: Vec<String>,
        /// Fail this many chunk calls after applying them (a lost response).
        lose_responses: usize,
        /// Fail every chunk call from this call index on (a dropped tunnel).
        break_after_chunks: Option<usize>,
        chunks: usize,
        committed_path: Option<String>,
    }
    impl FakeHost {
        fn call(&mut self, method: &str, p: Value) -> Result<Value, String> {
            self.calls.push(method.into());
            match method {
                "uploads.begin" => {
                    let size = p["size"].as_u64().ok_or("size")?;
                    let (name, sha) = (p["name"].as_str().unwrap(), p["sha256"].as_str().unwrap());
                    let id = match p["uploadId"].as_str() {
                        Some(id) => {
                            let u = self.uploads.get(id).ok_or("Upload not found")?;
                            if (&u.0, u.1, &u.2) != (&name.to_string(), size, &sha.to_string()) {
                                return Err("Upload does not match".into());
                            }
                            id.to_string()
                        }
                        None => {
                            let id = format!("{:032x}", self.uploads.len() + 1);
                            self.uploads
                                .insert(id.clone(), (name.into(), size, sha.into(), vec![]));
                            id
                        }
                    };
                    let offset = self.uploads[&id].3.len();
                    Ok(json!({"uploadId": id, "offset": offset, "chunkSize": CHUNK_BYTES}))
                }
                "uploads.chunk" => {
                    self.chunks += 1;
                    if self.break_after_chunks.is_some_and(|n| self.chunks > n) {
                        return Err("tunnel closed".into());
                    }
                    let u = self
                        .uploads
                        .get_mut(p["uploadId"].as_str().unwrap())
                        .ok_or("nf")?;
                    let off = p["offset"].as_u64().unwrap() as usize;
                    let data = STANDARD.decode(p["data"].as_str().unwrap()).unwrap();
                    if off > u.3.len() {
                        return Err("Upload chunks are out of order".into());
                    }
                    let overlap = data.len().min(u.3.len() - off);
                    if u.3[off..off + overlap] != data[..overlap] {
                        return Err("Upload retry does not match uploaded bytes".into());
                    }
                    u.3.extend_from_slice(&data[overlap..]);
                    if self.lose_responses > 0 {
                        self.lose_responses -= 1;
                        return Err("response lost".into());
                    }
                    Ok(json!({"offset": u.3.len()}))
                }
                "uploads.commit" => {
                    let id = p["uploadId"].as_str().unwrap();
                    let u = self.uploads.get(id).ok_or("nf")?;
                    if u.3.len() as u64 != u.1 {
                        return Err("Upload is incomplete".into());
                    }
                    let got = hex(&Sha256::digest(&u.3));
                    if got != u.2 {
                        return Err("Upload sha256 does not match".into());
                    }
                    let path = format!("/home/u/.monocode-host/uploads/{id}/{}", u.0);
                    self.committed_path = Some(path.clone());
                    Ok(
                        json!({"uploadId": id, "name": u.0, "path": path, "size": u.1, "sha256": u.2}),
                    )
                }
                "uploads.discard" => {
                    let gone = self
                        .uploads
                        .remove(p["uploadId"].as_str().unwrap())
                        .is_some();
                    Ok(json!({"discarded": gone}))
                }
                _ => Err("Unsupported remote operation".into()),
            }
        }
    }

    fn file(bytes: &[u8]) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("kit.tar");
        std::fs::write(&path, bytes).unwrap();
        (dir, path)
    }
    fn data(len: usize) -> Vec<u8> {
        (0..len).map(|i| (i % 251) as u8).collect()
    }

    #[test]
    fn uploads_in_512_kb_chunks_with_progress_and_a_verified_commit() {
        let bytes = data(CHUNK_BYTES * 2 + 17);
        let (_dir, path) = file(&bytes);
        let mut host = FakeHost::default();
        let mut seen = vec![];
        let mut progress = |done: u64, total: u64| seen.push((done, total));
        let done = upload_file(
            &mut |m, p| host.call(m, p),
            &path,
            "kit.tar",
            UploadOptions {
                progress: Some(&mut progress),
                ..Default::default()
            },
        )
        .unwrap();
        let total = bytes.len() as u64;
        assert_eq!(
            seen,
            vec![
                (CHUNK_BYTES as u64, total),
                (CHUNK_BYTES as u64 * 2, total),
                (total, total)
            ]
        );
        assert_eq!(done.size, total);
        assert_eq!(done.sha256, hex(&Sha256::digest(&bytes)));
        assert_eq!(Some(done.path), host.committed_path);
        assert_eq!(
            host.calls.first().map(String::as_str),
            Some("uploads.begin")
        );
        assert_eq!(
            host.calls.last().map(String::as_str),
            Some("uploads.commit")
        );
    }

    #[test]
    fn resends_a_chunk_whose_response_was_lost() {
        let bytes = data(CHUNK_BYTES + 5);
        let (_dir, path) = file(&bytes);
        let mut host = FakeHost {
            lose_responses: 2,
            ..Default::default()
        };
        let done = upload_file(
            &mut |m, p| host.call(m, p),
            &path,
            "kit.tar",
            Default::default(),
        )
        .unwrap();
        assert_eq!(done.size, bytes.len() as u64);
        assert_eq!(host.uploads.values().next().unwrap().3, bytes);
    }

    #[test]
    fn resumes_from_the_offset_the_host_reports() {
        let bytes = data(CHUNK_BYTES * 3);
        let (_dir, path) = file(&bytes);
        let mut host = FakeHost {
            break_after_chunks: Some(1),
            ..Default::default()
        };
        let err = upload_file(
            &mut |m, p| host.call(m, p),
            &path,
            "kit.tar",
            Default::default(),
        )
        .unwrap_err();
        let id = err.upload_id.clone().expect("the id is kept for a resume");
        assert!(!err.cancelled);
        host.break_after_chunks = None;
        host.chunks = 0;
        host.calls.clear();
        let done = upload_file(
            &mut |m, p| host.call(m, p),
            &path,
            "kit.tar",
            UploadOptions {
                resume_id: Some(id.clone()),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(done.upload_id, id);
        // Only the two missing chunks are sent.
        assert_eq!(host.chunks, 2);
        assert_eq!(host.uploads[&id].3, bytes);
    }

    #[test]
    fn a_hash_mismatch_on_the_host_fails_the_commit() {
        let bytes = data(10);
        let (_dir, path) = file(&bytes);
        let mut host = FakeHost::default();
        let err = upload_file(
            &mut |m, p| {
                let mut p = p;
                if m == "uploads.chunk" {
                    p["data"] = json!(STANDARD.encode(b"corrupted!"));
                }
                host.call(m, p)
            },
            &path,
            "kit.tar",
            Default::default(),
        )
        .unwrap_err();
        assert!(err.message.contains("sha256"), "{err}");
    }

    #[test]
    fn cancel_stops_between_chunks_and_discards_the_upload() {
        let bytes = data(CHUNK_BYTES * 3);
        let (_dir, path) = file(&bytes);
        let mut host = FakeHost::default();
        let cancel = AtomicBool::new(false);
        let mut progress = |_: u64, _: u64| cancel.store(true, Ordering::Relaxed);
        let err = upload_file(
            &mut |m, p| host.call(m, p),
            &path,
            "kit.tar",
            UploadOptions {
                progress: Some(&mut progress),
                cancel: Some(&cancel),
                ..Default::default()
            },
        )
        .unwrap_err();
        assert!(err.cancelled);
        assert_eq!(host.chunks, 1);
        assert!(host.uploads.is_empty());
        assert_eq!(
            host.calls.last().map(String::as_str),
            Some("uploads.discard")
        );
    }

    #[test]
    fn an_empty_file_commits_without_chunks() {
        let (_dir, path) = file(b"");
        let mut host = FakeHost::default();
        let done =
            upload_file(&mut |m, p| host.call(m, p), &path, "e", Default::default()).unwrap();
        assert_eq!(done.size, 0);
        assert_eq!(host.chunks, 0);
    }

    #[test]
    fn host_errors_surface_with_the_upload_id() {
        let (_dir, path) = file(b"abc");
        let err = upload_file(
            &mut |m, _| match m {
                "uploads.begin" => Ok(json!({"uploadId":"id1","offset":0,"chunkSize":CHUNK_BYTES})),
                _ => Err("Upload staging quota exceeded".into()),
            },
            &path,
            "a",
            Default::default(),
        )
        .unwrap_err();
        assert_eq!(err.upload_id.as_deref(), Some("id1"));
        assert!(err.message.contains("quota"));
        let missing = upload_file(
            &mut |_, _| unreachable!(),
            Path::new("/nonexistent/file"),
            "a",
            Default::default(),
        )
        .unwrap_err();
        assert!(missing.upload_id.is_none());
    }
}
