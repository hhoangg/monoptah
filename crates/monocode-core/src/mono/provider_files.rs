//! The provider session files a Mono reset can also delete, and the ids that
//! context rotation retires (monoProviderFiles.ts, monoRotation.ts). A retired
//! provider session keeps its files on disk, so a reset offers to remove them.
use crate::{
    provider::Kind,
    provider_cleanup::{
        cleanup_request, is_uuid, plan_reset_cleanup, CleanupRequest, ProviderSessionRef,
    },
    store::Store,
};
use serde_json::Value;

/// Retired provider sessions remembered per conversation, newest kept.
pub const MAX_RETIRED: usize = 200;
/// Where the desktop app kept the same lists, keyed by its session id.
const DESKTOP_ROTATIONS: &str = "monocode:mono-rotations";

fn deletable(harness: &str) -> bool {
    matches!(harness, "claude" | "codex")
}

/// The reference for a harness and id, or `None` when either is not safe to delete.
pub fn provider_session_ref(harness: &str, id: &str) -> Option<ProviderSessionRef> {
    (deletable(harness) && is_uuid(id)).then(|| ProviderSessionRef {
        harness: harness.into(),
        id: id.into(),
    })
}

fn same(a: &ProviderSessionRef, b: &ProviderSessionRef) -> bool {
    a.harness == b.harness && a.id.eq_ignore_ascii_case(&b.id)
}

/// Drops repeats, keeping the first occurrence of each session.
pub fn dedupe(sessions: Vec<ProviderSessionRef>) -> Vec<ProviderSessionRef> {
    let mut out: Vec<ProviderSessionRef> = Vec::new();
    for session in sessions {
        if !out.iter().any(|known| same(known, &session)) {
            out.push(session);
        }
    }
    out
}

/// The valid, deduped sessions in a stored value. Anything that is not a list,
/// or any malformed entry, is dropped.
pub fn sanitize(value: &Value) -> Vec<ProviderSessionRef> {
    let Some(list) = value.as_array() else {
        return Vec::new();
    };
    dedupe(
        list.iter()
            .filter_map(|entry| {
                provider_session_ref(entry["harness"].as_str()?, entry["id"].as_str()?)
            })
            .collect(),
    )
}

/// The list after a rotation replaced `replaced` (harness and provider id):
/// earlier entries first, the new one last, re-retiring an id moves it to the
/// end, and only the newest [`MAX_RETIRED`] stay.
pub fn retire(previous: &Value, replaced: Option<(&str, &str)>) -> Vec<ProviderSessionRef> {
    let replaced = replaced.and_then(|(harness, id)| provider_session_ref(harness, id));
    let mut list: Vec<ProviderSessionRef> = sanitize(previous)
        .into_iter()
        .filter(|entry| replaced.as_ref().is_none_or(|new| !same(entry, new)))
        .collect();
    list.extend(replaced);
    let mut list = dedupe(list);
    list.drain(..list.len().saturating_sub(MAX_RETIRED));
    list
}

/// What a reset says when some provider session files may have stayed behind.
pub fn problem_message(label: &str, problems: usize) -> String {
    format!(
        "Conversation reset. Some {label} session files may not have been deleted ({problems} {}); you can remove them manually.",
        if problems == 1 { "problem" } else { "problems" }
    )
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MonoProviderFiles {
    /// What a reset may ask the cleanup to delete.
    pub sessions: Vec<ProviderSessionRef>,
    /// Whether there is anything to delete.
    pub supported: bool,
    /// The provider or providers the files belong to, for the checkbox.
    pub label: String,
    /// Why nothing, or not everything, can be deleted.
    pub note: Option<String>,
}

fn title(harness: &str) -> String {
    Kind::parse(harness).map_or_else(|| harness.to_string(), |kind| kind.label().to_string())
}

/// The provider session files a Mono reset can also delete: the conversation's
/// current provider session plus the ones context rotation retired earlier.
pub fn mono_provider_files(
    harness: &str,
    provider_id: Option<&str>,
    retired: &[ProviderSessionRef],
) -> MonoProviderFiles {
    let current = provider_id.and_then(|id| provider_session_ref(harness, id));
    let sessions = dedupe(
        current
            .into_iter()
            .chain(
                retired
                    .iter()
                    .filter_map(|entry| provider_session_ref(&entry.harness, &entry.id)),
            )
            .collect(),
    );
    let providers: Vec<String> = ["claude", "codex"]
        .into_iter()
        .filter(|harness| sessions.iter().any(|entry| entry.harness == *harness))
        .map(title)
        .collect();
    let current_supported = deletable(harness);
    let label = if providers.is_empty() {
        title(harness)
    } else {
        providers.join(" and ")
    };
    if sessions.is_empty() {
        let note = if current_supported {
            format!("There are no {label} session files to delete yet.")
        } else {
            format!("Monoptah can't delete {label} session files yet.")
        };
        return MonoProviderFiles {
            sessions,
            supported: false,
            label,
            note: Some(note),
        };
    }
    MonoProviderFiles {
        sessions,
        supported: true,
        label,
        note: (!current_supported).then(|| {
            format!(
                "Files from the current {} session are not covered.",
                title(harness)
            )
        }),
    }
}

fn key(id: i64) -> String {
    format!("mono-retired:{id}")
}

fn native_retired(store: &Store, id: i64) -> Result<Value, String> {
    Ok(store
        .setting(&key(id))?
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or(Value::Null))
}

/// The provider sessions rotation retired for this conversation, oldest first:
/// the desktop app's list when this is its data, then the native one. Nothing
/// malformed in either is ever passed on.
pub fn retired_sessions(store: &Store, id: i64) -> Result<Vec<ProviderSessionRef>, String> {
    let desktop = store
        .desktop_preferences()
        .and_then(|prefs| prefs.json(DESKTOP_ROTATIONS))
        .zip(store.persisted_session_id(id).ok())
        .map(|(rotations, original)| sanitize(&rotations[original.as_str()]["retired"]))
        .unwrap_or_default();
    let mut all = desktop;
    all.extend(sanitize(&native_retired(store, id)?));
    Ok(dedupe(all))
}

/// Remembers that rotation replaced the provider session `provider_id`. The
/// setting goes with the conversation when its row is deleted.
pub fn retire_session(
    store: &Store,
    id: i64,
    harness: &str,
    provider_id: &str,
) -> Result<(), String> {
    let list = retire(&native_retired(store, id)?, Some((harness, provider_id)));
    if list.is_empty() {
        return Ok(());
    }
    store.save_setting(
        &key(id),
        &serde_json::to_string(&list).map_err(|e| e.to_string())?,
    )
}

/// Context rotation: remembers the provider session being replaced, then drops
/// it from the conversation so the next turn starts a fresh one. A failure to
/// remember never blocks the turn; it comes back as `Ok(Some(problem))`.
pub fn rotate(
    store: &Store,
    id: i64,
    harness: &str,
    provider_id: &str,
) -> Result<Option<String>, String> {
    let recorded = retire_session(store, id, harness, provider_id).err();
    store.reset_provider_id(id)?;
    Ok(recorded)
}

/// What a reset reads from the conversation before its row is deleted.
pub struct ResetRead {
    /// For the dialog; `None` when the record is gone.
    pub files: Option<MonoProviderFiles>,
    /// The cleanup to run once the row is gone.
    pub requests: Vec<CleanupRequest>,
    /// Ids that were skipped, or the missing record.
    pub problems: Vec<String>,
}

pub fn read_reset(store: &Store, id: i64) -> Result<ResetRead, String> {
    let retired = retired_sessions(store, id)?;
    let Some(session) = store.mono_session(id)? else {
        let (requests, problems) = plan_reset_cleanup(None, &retired, false);
        return Ok(ResetRead {
            files: None,
            requests,
            problems,
        });
    };
    let side = store
        .codex_side_thread_ids(id, &session.provider)
        .unwrap_or_default();
    let (requests, problems) = plan_reset_cleanup(cleanup_request(&session, &side), &retired, true);
    Ok(ResetRead {
        files: Some(mono_provider_files(
            &session.provider,
            session.provider_id.as_deref(),
            &retired,
        )),
        requests,
        problems,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::desktop_preferences::DesktopPreferences;
    use serde_json::json;
    use std::sync::Arc;

    const A: &str = "11111111-1111-4111-8111-111111111111";
    const B: &str = "22222222-2222-4222-8222-222222222222";
    const C: &str = "33333333-3333-4333-8333-333333333333";

    fn claude(id: &str) -> ProviderSessionRef {
        ProviderSessionRef {
            harness: "claude".into(),
            id: id.into(),
        }
    }
    fn codex(id: &str) -> ProviderSessionRef {
        ProviderSessionRef {
            harness: "codex".into(),
            id: id.into(),
        }
    }

    #[test]
    fn accepts_only_canonical_uuid_shaped_ids() {
        assert!(provider_session_ref("claude", A).is_some());
        assert!(provider_session_ref("claude", &A.to_uppercase()).is_some());
        for bad in [
            "../x".to_string(),
            "memory".into(),
            String::new(),
            format!("{A}0"),
            A[..35].into(),
            "zzzzzzzz-zzzz-zzzz-zzzz-zzzzzzzzzzzz".into(),
            A.replace('-', "_"),
            format!("{}/", &A[..35]),
        ] {
            assert!(provider_session_ref("claude", &bad).is_none(), "{bad}");
        }
        assert!(provider_session_ref("cursor", A).is_none());
    }

    #[test]
    fn offers_the_current_claude_session_and_the_retired_ones_newest_last() {
        let files = mono_provider_files("claude", Some(C), &[claude(A), claude(B)]);
        assert_eq!(files.sessions, vec![claude(C), claude(A), claude(B)]);
        assert!(files.supported);
        assert_eq!(files.label, "Claude Code");
        assert_eq!(files.note, None);
    }

    #[test]
    fn dedupes_a_current_session_that_was_also_retired() {
        let files = mono_provider_files("codex", Some(A), &[codex(A), codex(B), codex(B)]);
        assert_eq!(files.sessions, vec![codex(A), codex(B)]);
        assert_eq!(files.label, "Codex");
        let upper = mono_provider_files("codex", Some(A), &[codex(&A.to_uppercase())]);
        assert_eq!(upper.sessions, vec![codex(A)]);
    }

    #[test]
    fn names_both_providers_when_both_have_files() {
        let files = mono_provider_files("codex", Some(A), &[claude(B)]);
        assert_eq!(files.sessions, vec![codex(A), claude(B)]);
        assert_eq!(files.label, "Claude Code and Codex");
    }

    #[test]
    fn cannot_delete_other_harness_files_and_says_so() {
        for harness in ["cursor", "opencode", "grok", "pi", "devin"] {
            let files = mono_provider_files(harness, Some(A), &[]);
            assert!(files.sessions.is_empty());
            assert!(!files.supported);
            let note = files.note.unwrap();
            assert!(note.starts_with("Monoptah can't delete "), "{note}");
            assert!(note.ends_with(" session files yet."), "{note}");
        }
    }

    #[test]
    fn uses_the_harness_display_name_in_the_unsupported_note() {
        assert_eq!(
            mono_provider_files("cursor", Some(A), &[]).note.as_deref(),
            Some("Monoptah can't delete Cursor session files yet.")
        );
        assert_eq!(
            mono_provider_files("opencode", Some(A), &[])
                .note
                .as_deref(),
            Some("Monoptah can't delete OpenCode session files yet.")
        );
    }

    #[test]
    fn still_offers_retired_sessions_when_the_current_harness_is_unsupported() {
        let files = mono_provider_files("cursor", Some(A), &[claude(B)]);
        assert_eq!(files.sessions, vec![claude(B)]);
        assert!(files.supported);
        assert_eq!(files.label, "Claude Code");
        let note = files.note.unwrap();
        assert!(
            note.contains("Cursor") && note.contains("not covered"),
            "{note}"
        );
    }

    #[test]
    fn says_there_is_nothing_to_delete_yet_when_a_supported_harness_has_no_ids() {
        let none = mono_provider_files("claude", None, &[]);
        assert!(none.sessions.is_empty());
        assert!(!none.supported);
        assert_eq!(none.label, "Claude Code");
        assert_eq!(
            none.note.as_deref(),
            Some("There are no Claude Code session files to delete yet.")
        );
        assert_eq!(
            mono_provider_files("codex", None, &[]).note.as_deref(),
            Some("There are no Codex session files to delete yet.")
        );
    }

    #[test]
    fn uses_retired_sessions_when_the_current_one_has_no_id_yet() {
        let files = mono_provider_files("codex", None, &[codex(A)]);
        assert_eq!(files.sessions, vec![codex(A)]);
        assert_eq!(files.note, None);
    }

    #[test]
    fn refuses_malformed_ids() {
        let wrong_harness = ProviderSessionRef {
            harness: "cursor".into(),
            id: A.into(),
        };
        for bad in [
            "../x".to_string(),
            "memory".into(),
            String::new(),
            format!("{A}0"),
            "zzzzzzzz-zzzz-zzzz-zzzz-zzzzzzzzzzzz".into(),
        ] {
            let files =
                mono_provider_files("claude", Some(&bad), &[claude(&bad), wrong_harness.clone()]);
            assert!(files.sessions.is_empty(), "{bad}");
            assert!(!files.supported);
        }
    }

    #[test]
    fn words_the_leftover_files_message_for_one_and_for_several_problems() {
        assert_eq!(
            problem_message("Claude Code", 1),
            "Conversation reset. Some Claude Code session files may not have been deleted (1 problem); you can remove them manually."
        );
        assert_eq!(
            problem_message("Codex", 3),
            "Conversation reset. Some Codex session files may not have been deleted (3 problems); you can remove them manually."
        );
    }

    fn plan(previous: &Value, retiring: Option<(&str, &str)>) -> Vec<ProviderSessionRef> {
        retire(previous, retiring)
    }

    #[test]
    fn records_the_session_being_retired() {
        assert_eq!(plan(&Value::Null, Some(("claude", A))), vec![claude(A)]);
    }

    #[test]
    fn carries_earlier_retired_sessions_forward_and_dedupes() {
        let first = json!(plan(&Value::Null, Some(("codex", A))));
        let second = plan(&first, Some(("codex", B)));
        assert_eq!(second, vec![codex(A), codex(B)]);
        // Re-retiring an id moves it to the newest end.
        assert_eq!(
            plan(&json!(second), Some(("codex", A))),
            vec![codex(B), codex(A)]
        );
    }

    #[test]
    fn carries_retired_sessions_forward_when_nothing_new_is_retired() {
        let first = json!(plan(&Value::Null, Some(("claude", A))));
        assert_eq!(plan(&first, None), vec![claude(A)]);
        assert_eq!(plan(&first, Some(("claude", "../x"))), vec![claude(A)]);
    }

    #[test]
    fn keeps_only_the_newest_200() {
        let many: Vec<ProviderSessionRef> = (0..200)
            .map(|i| claude(&format!("00000000-0000-4000-8000-{i:012}")))
            .collect();
        let list = plan(&json!(many), Some(("claude", A)));
        assert_eq!(list.len(), 200);
        assert_eq!(list[199], claude(A));
        assert_eq!(list[0], many[1]);
    }

    #[test]
    fn ignores_an_unsupported_harness_or_a_malformed_id() {
        assert!(plan(&Value::Null, Some(("cursor", A))).is_empty());
        assert!(plan(&Value::Null, Some(("claude", "../x"))).is_empty());
        assert!(plan(&Value::Null, Some(("claude", ""))).is_empty());
    }

    #[test]
    fn drops_malformed_entries_from_the_stored_list_instead_of_failing() {
        for bad in [
            Value::Null,
            json!({"harness":"claude"}),
            json!({"harness":"claude","id":42}),
            json!({"harness":"cursor","id":B}),
            json!({"harness":"codex","id":"../x"}),
            json!("nope"),
        ] {
            let stored = json!([bad, {"harness":"claude","id":B}]);
            assert_eq!(
                plan(&stored, Some(("claude", A))),
                vec![claude(B), claude(A)],
                "{stored}"
            );
        }
    }

    #[test]
    fn drops_a_stored_list_that_is_not_an_array() {
        let stored = json!("x");
        assert!(plan(&stored, None).is_empty());
        assert_eq!(plan(&stored, Some(("claude", A))), vec![claude(A)]);
    }

    fn store() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("native.sqlite")).unwrap();
        (dir, store)
    }

    #[test]
    fn the_store_keeps_retired_sessions_per_conversation() {
        let (_dir, store) = store();
        retire_session(&store, 7, "claude", A).unwrap();
        retire_session(&store, 7, "claude", B).unwrap();
        retire_session(&store, 7, "claude", A).unwrap();
        retire_session(&store, 8, "codex", C).unwrap();
        assert_eq!(
            retired_sessions(&store, 7).unwrap(),
            vec![claude(B), claude(A)]
        );
        assert_eq!(retired_sessions(&store, 8).unwrap(), vec![codex(C)]);
        assert!(retired_sessions(&store, 9).unwrap().is_empty());
        assert_eq!(
            store.setting("mono-retired:8").unwrap().as_deref(),
            Some(r#"[{"harness":"codex","id":"33333333-3333-4333-8333-333333333333"}]"#)
        );
    }

    #[test]
    fn rotation_retires_the_replaced_session_and_clears_it_from_the_conversation() {
        let (_dir, store) = store();
        let session = store.create("claude", "/work").unwrap();
        store.provider_id(session.id, A).unwrap();

        assert_eq!(rotate(&store, session.id, "claude", A).unwrap(), None);

        assert_eq!(store.sessions().unwrap()[0].provider_id, None);
        assert_eq!(
            retired_sessions(&store, session.id).unwrap(),
            vec![claude(A)]
        );
        // The next provider session is retired after the first, never instead of it.
        store.provider_id(session.id, B).unwrap();
        rotate(&store, session.id, "claude", B).unwrap();
        assert_eq!(
            retired_sessions(&store, session.id).unwrap(),
            vec![claude(A), claude(B)]
        );
    }

    #[test]
    fn rotation_still_starts_fresh_when_the_old_id_cannot_be_retired() {
        let (_dir, store) = store();
        let session = store.create("cursor", "/work").unwrap();
        store.provider_id(session.id, "not-a-uuid").unwrap();

        assert_eq!(
            rotate(&store, session.id, "cursor", "not-a-uuid").unwrap(),
            None
        );

        assert_eq!(store.sessions().unwrap()[0].provider_id, None);
        assert_eq!(store.setting("mono-retired:1").unwrap(), None);
    }

    #[test]
    fn nothing_is_stored_when_the_retired_id_is_unusable() {
        let (_dir, store) = store();
        retire_session(&store, 7, "claude", "../x").unwrap();
        retire_session(&store, 7, "cursor", A).unwrap();
        assert_eq!(store.setting("mono-retired:7").unwrap(), None);
    }

    #[test]
    fn broken_stored_text_reads_as_nothing_and_is_replaced_by_the_next_rotation() {
        let (_dir, store) = store();
        store.save_setting("mono-retired:7", "not json").unwrap();
        assert!(retired_sessions(&store, 7).unwrap().is_empty());
        retire_session(&store, 7, "claude", A).unwrap();
        assert_eq!(retired_sessions(&store, 7).unwrap(), vec![claude(A)]);
    }

    #[test]
    fn deleting_the_conversation_forgets_its_retired_sessions() {
        let (_dir, mut store) = store();
        let session = store.create("claude", "/work").unwrap();
        retire_session(&store, session.id, "claude", A).unwrap();
        retire_session(&store, session.id + 1, "claude", B).unwrap();
        store.delete(session.id).unwrap();
        assert!(retired_sessions(&store, session.id).unwrap().is_empty());
        assert_eq!(
            retired_sessions(&store, session.id + 1).unwrap(),
            vec![claude(B)]
        );
    }

    #[test]
    fn desktop_data_adds_the_retired_sessions_the_desktop_app_recorded() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("desktop.db");
        let store = Store::open_monocode_with_preferences_for_test(
            &path,
            Arc::new(DesktopPreferences::from_values(&[])),
        )
        .unwrap();
        let session = store.create("claude", "/work").unwrap();
        let original = store.persisted_session_id(session.id).unwrap();
        drop(store);
        let rotations = json!({
            original.clone(): {"at":1,"earlier":[],"retired":[
                {"harness":"claude","id":A},{"harness":"cursor","id":B},"nope",{"harness":"codex","id":"../x"}
            ]},
            "someone-else": {"retired":[{"harness":"claude","id":C}]}
        });
        let store = Store::open_monocode_with_preferences_for_test(
            &path,
            Arc::new(DesktopPreferences::from_values(&[(
                DESKTOP_ROTATIONS,
                rotations,
            )])),
        )
        .unwrap();
        let id = store.sessions().unwrap()[0].id;
        assert_eq!(retired_sessions(&store, id).unwrap(), vec![claude(A)]);
        // A native rotation lands after the desktop ones, repeats dropped.
        retire_session(&store, id, "claude", B).unwrap();
        retire_session(&store, id, "claude", A).unwrap();
        assert_eq!(
            retired_sessions(&store, id).unwrap(),
            vec![claude(A), claude(B)]
        );
    }

    #[test]
    fn a_reset_reads_the_record_and_its_retired_sessions_before_the_row_goes() {
        let (_dir, store) = store();
        let session = store.create("claude", "/work").unwrap();
        store.provider_id(session.id, A).unwrap();
        retire_session(&store, session.id, "claude", B).unwrap();
        retire_session(&store, session.id, "codex", C).unwrap();
        let read = read_reset(&store, session.id).unwrap();
        let files = read.files.unwrap();
        assert_eq!(files.sessions, vec![claude(A), claude(B), codex(C)]);
        assert_eq!(files.label, "Claude Code and Codex");
        assert!(read.problems.is_empty(), "{:?}", read.problems);
        let mut found: Vec<(Kind, Vec<String>)> = read
            .requests
            .iter()
            .map(|r| (r.provider(), r.ids().to_vec()))
            .collect();
        found.sort_by_key(|(kind, _)| kind.id());
        assert_eq!(
            found,
            vec![
                (Kind::Claude, vec![A.to_string(), B.to_string()]),
                (Kind::Codex, vec![C.to_string()])
            ]
        );
    }

    #[test]
    fn a_reset_deletes_nothing_for_a_conversation_whose_record_is_gone() {
        let (_dir, store) = store();
        retire_session(&store, 42, "claude", A).unwrap();
        let read = read_reset(&store, 42).unwrap();
        assert!(read.files.is_none());
        assert!(read.requests.is_empty());
        assert_eq!(
            read.problems,
            vec!["conversation record not found; provider session files were kept".to_string()]
        );
    }

    #[test]
    fn a_conversation_without_provider_files_reads_an_empty_plan() {
        let (_dir, store) = store();
        let session = store.create("cursor", "/work").unwrap();
        store.provider_id(session.id, A).unwrap();
        let read = read_reset(&store, session.id).unwrap();
        assert!(read.requests.is_empty());
        assert!(read.problems.is_empty());
        let files = read.files.unwrap();
        assert!(!files.supported);
        assert_eq!(
            files.note.as_deref(),
            Some("Monoptah can't delete Cursor session files yet.")
        );
    }
}
