//! Decides what to do when a Claude transcript arrives at a machine that may already hold
//! the same session id (design 4.9, rule D10).
//!
//! The same id can exist as two files on two machines. Bytes cannot be compared, because the
//! cwd rewrite changes lines in both directions, so the transcripts are compared by the chain
//! of entry `uuid`s in file order:
//!
//! - the existing chain is a strict prefix of the incoming one: the incoming file extends it
//!   and replaces it (the old file is kept as a backup);
//! - the chains are equal: nothing to write;
//! - anything else: the two diverged. Neither is overwritten, the incoming copy is forked
//!   to a new session id.
//!
//! Files are read line by line and never into one JSON value. Only `uuid` and `sessionId`
//! are interpreted; every other field, including unknown future shapes, passes through.
//! The Host mirrors this rule in TypeScript and uses the same fixtures
//! (`tests/fixtures/transcript_sync/`).

use serde::Deserialize;
use serde_json::Value;

/// What to do with an incoming transcript.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    /// Nothing exists under that id: write the incoming file.
    Install,
    /// The incoming file extends the existing one: replace it, keeping a backup.
    Replace,
    /// Both hold the same entries: write nothing.
    AlreadyCurrent,
    /// Neither is a prefix of the other: fork the incoming copy to a new id.
    Diverged,
}

/// The only two fields of an entry that are interpreted.
#[derive(Deserialize)]
struct Probe {
    uuid: Option<Value>,
    #[serde(rename = "sessionId")]
    session_id: Option<Value>,
}

fn lines(jsonl: &[u8]) -> impl Iterator<Item = &[u8]> {
    jsonl
        .split(|b| *b == b'\n')
        .map(|line| line.strip_suffix(b"\r").unwrap_or(line))
        .filter(|line| !line.iter().all(u8::is_ascii_whitespace))
}

fn probe(line: &[u8]) -> Option<Probe> {
    serde_json::from_slice(line).ok()
}

/// The entry `uuid`s in file order. Lines without a string `uuid` (snapshots, unknown
/// shapes, a truncated last line) are not part of the chain; callers keep such lines when
/// they copy or rewrite a file. The ids are kept as strings so that a future non-UUID id
/// still compares exactly.
pub fn parse_chain(jsonl: &[u8]) -> Vec<String> {
    lines(jsonl)
        .filter_map(probe)
        .filter_map(|p| match p.uuid {
            Some(Value::String(uuid)) => Some(uuid),
            _ => None,
        })
        .collect()
}

/// Compares an incoming transcript with the one already installed under the same id.
pub fn decide(existing: Option<&[u8]>, incoming: &[u8]) -> Decision {
    let Some(existing) = existing else {
        return Decision::Install;
    };
    let old = parse_chain(existing);
    let new = parse_chain(incoming);
    if old == new {
        Decision::AlreadyCurrent
    } else if new.len() > old.len() && new[..old.len()] == old[..] {
        Decision::Replace
    } else {
        Decision::Diverged
    }
}

/// Rewrites every `sessionId` field of the entries to `new_id`. Every other byte of a line
/// is kept, so unknown fields survive; a line is only re-serialised when its `sessionId`
/// is not written in the compact form Claude uses.
pub fn fork(jsonl: &str, new_id: &str) -> String {
    let mut out = String::with_capacity(jsonl.len());
    for line in jsonl.split_inclusive('\n') {
        let (body, end) = match line.strip_suffix('\n') {
            Some(body) => (body, "\n"),
            None => (line, ""),
        };
        out.push_str(&fork_line(body, new_id));
        out.push_str(end);
    }
    out
}

fn fork_line(line: &str, new_id: &str) -> String {
    if !line.contains("\"sessionId\"") {
        return line.to_string();
    }
    let Some(Probe {
        session_id: Some(Value::String(old)),
        ..
    }) = probe(line.as_bytes())
    else {
        return line.to_string();
    };
    if old == new_id {
        return line.to_string();
    }
    let quote = |s: &str| serde_json::to_string(s).unwrap_or_default();
    let from = format!("\"sessionId\":{}", quote(&old));
    if line.contains(&from) {
        return line.replace(&from, &format!("\"sessionId\":{}", quote(new_id)));
    }
    // Spaced or reordered JSON: set the field and write the entry again.
    match serde_json::from_str::<Value>(line) {
        Ok(Value::Object(mut map)) => {
            map.insert("sessionId".into(), Value::String(new_id.into()));
            serde_json::to_string(&map).unwrap_or_else(|_| line.to_string())
        }
        _ => line.to_string(),
    }
}

/// The text shown to the user when a transcript was forked.
pub fn fork_notice(old_id: &str, new_id: &str) -> String {
    format!(
        "The cloud and local conversations diverged. The cloud copy continues as a new Claude session ({new_id}); the local one ({old_id}) was left untouched."
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cloud_sessions::rewrite_cwd;
    use std::path::PathBuf;

    fn fixture(name: &str) -> String {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/transcript_sync")
            .join(name);
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
    }

    fn manifest() -> Value {
        serde_json::from_str(&fixture("cases.json")).unwrap()
    }

    #[test]
    fn chains_match_the_shared_fixtures() {
        for chain in manifest()["chains"].as_array().unwrap() {
            let file = fixture(chain["file"].as_str().unwrap());
            let want: Vec<String> = chain["uuids"]
                .as_array()
                .unwrap()
                .iter()
                .map(|u| u.as_str().unwrap().to_string())
                .collect();
            assert_eq!(parse_chain(file.as_bytes()), want, "{}", chain["file"]);
        }
    }

    #[test]
    fn decisions_match_the_shared_fixtures() {
        let m = manifest();
        for case in m["cases"].as_array().unwrap() {
            let existing = case["existing"].as_str().map(fixture);
            let incoming = fixture(case["incoming"].as_str().unwrap());
            let want = match case["decision"].as_str().unwrap() {
                "install" => Decision::Install,
                "replace" => Decision::Replace,
                "already_current" => Decision::AlreadyCurrent,
                "diverged" => Decision::Diverged,
                other => panic!("unknown decision {other}"),
            };
            assert_eq!(
                decide(existing.as_deref().map(str::as_bytes), incoming.as_bytes()),
                want,
                "{}",
                case["name"]
            );
        }
    }

    #[test]
    fn comparison_ignores_the_cwd_rewrite() {
        let cloud = fixture("extended.jsonl");
        let local_base = rewrite_cwd(
            &fixture("base.jsonl"),
            &["/workspaces/app"],
            "/Users/me/app",
        );
        assert_ne!(local_base, fixture("base.jsonl"));
        let local_ext = rewrite_cwd(&cloud, &["/workspaces/app"], "/Users/me/app");
        assert_eq!(
            decide(Some(local_base.as_bytes()), local_ext.as_bytes()),
            Decision::Replace
        );
        assert_eq!(
            decide(Some(local_base.as_bytes()), cloud.as_bytes()),
            Decision::Replace
        );
        assert_eq!(
            decide(Some(local_ext.as_bytes()), cloud.as_bytes()),
            Decision::AlreadyCurrent
        );
    }

    #[test]
    fn a_cwd_rewrite_round_trip_is_the_identity() {
        for name in ["base.jsonl", "extended.jsonl", "diverged.jsonl"] {
            let original = fixture(name);
            let local = rewrite_cwd(&original, &["/workspaces/app"], "/Users/me/my app");
            assert!(local.contains("/Users/me/my app"));
            assert!(!local.contains("/workspaces/app"));
            let back = rewrite_cwd(&local, &["/Users/me/my app"], "/workspaces/app");
            assert_eq!(back, original, "{name}");
        }
    }

    #[test]
    fn fork_rewrites_only_session_ids() {
        let original = fixture("extended.jsonl");
        let forked = fork(&original, "22222222-2222-4222-8222-222222222222");
        assert!(!forked.contains("11111111-1111-4111-8111-111111111111"));
        assert_eq!(original.lines().count(), forked.lines().count());
        // The chain and every other byte survive.
        assert_eq!(
            parse_chain(forked.as_bytes()),
            parse_chain(original.as_bytes())
        );
        assert_eq!(
            forked.replace(
                "22222222-2222-4222-8222-222222222222",
                "11111111-1111-4111-8111-111111111111"
            ),
            original
        );
        // A line with no sessionId and an unknown shape stays byte for byte.
        assert!(forked.contains("\"futureField\":{\"nested\":[1,2,3]}"));
        assert!(forked
            .lines()
            .next()
            .unwrap()
            .starts_with("{\"type\":\"file-history-snapshot\""));
    }

    #[test]
    fn fork_handles_spaced_json_and_a_missing_final_newline() {
        let text = "{ \"sessionId\": \"old\", \"uuid\": \"a\" }\n{\"sessionId\":\"old\"}";
        let out = fork(text, "new");
        let mut lines = out.split('\n');
        let first: Value = serde_json::from_str(lines.next().unwrap()).unwrap();
        assert_eq!(first["sessionId"], "new");
        assert_eq!(first["uuid"], "a");
        assert_eq!(lines.next().unwrap(), "{\"sessionId\":\"new\"}");
        assert!(!out.ends_with('\n'));
    }
}
