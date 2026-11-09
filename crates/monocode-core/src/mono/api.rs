use super::cloud_api::validate as cloud;
use serde_json::Value;
/// Refusal for `sessions.start` with both `account` and a cloud `target`.
pub const ACCOUNT_LOCAL_ONLY: &str = "account applies to local sessions only; a cloud session uses the cloud setup (the API key or setup token configured for Codespaces or Docker Sandbox)";
/// Reject misspelled fields and ambiguous types before any app mutation.
pub fn validate(action: &str, input: &Value) -> Result<(), String> {
    let cloud_fields;
    let fields=match action{
        "models.list"|"soul.read"|"habits.list"=>"",
        "accounts.list"=>"provider",
        "sessions.list"|"folders.list"|"worktrees.list"=>"project",
        "sessions.read"=>"sessionId project before limit maxChars",
        "sessions.start"=>"prompt project harness model modelSettings effort runtimeMode title draft reveal workspaceMode worktreeBase worktreeCwd placement besideSessionId notifyOnComplete completionDelivery parentSessionId folderName folderId target account",
        action if action.starts_with("cloud.") => {
            cloud_fields = cloud::fields(action).ok_or("Unknown app action. Run app help")?;
            cloud_fields.as_str()
        }
        "sessions.send"=>"sessionId prompt project notifyOnComplete completionDelivery",
        "sessions.draft"=>"sessionId prompt project",
        "sessions.pullEvidence"=>"sessionId project path taskId",
        "sessions.stop"=>"sessionId project",
        "sessions.archive"=>"sessionId project cloud",
        "sessions.delete"=>"sessionId project providerFiles removeWorktree",
        "worktrees.create"=>"branch base existing project",
        "folders.move"=>"sessionId folderId newFolderName project",
        "notes.list"=>"limit offset","notes.read"=>"id","notes.write"=>"id title body tags",
        "artifacts.list"=>"kind limit offset","artifacts.read"=>"id","artifacts.write"=>"id kind title body summary",
        "soul.update"=>"text expectedHash","memory.read"=>"topic","memory.search"=>"query since",
        "memory.add"=>"fact topic until","memory.replace"=>"find fact topic until","memory.remove"=>"find topic",
        "habits.add"=>"name instructions schedule","habits.update"=>"id name instructions schedule enabled","habits.run"|"habits.remove"|"habits.export"=>"id","habits.import"=>"yaml",
        "chat.card"=>"type repo number note sessionId options name instructions schedule",
        _=>crate::today::actions::fields(action)
            .or_else(|| super::workflows::cli::fields(action))
            .ok_or("Unknown app action. Run app help")?,
    };
    // today.* takes lists and flags (items, tags); its handler checks every field strictly.
    let today = action.starts_with("today.");
    for (key, value) in input.as_object().ok_or("Input must be an object")? {
        if !fields.split_whitespace().any(|f| f == key) {
            return Err(format!("{action} does not take {key}"));
        }
        if today {
            continue;
        }
        match key.as_str() {
            "completionDelivery" if !matches!(value.as_str(), Some("group" | "each")) => {
                return Err("completionDelivery must be \"group\" or \"each\"".into());
            }
            // Native session IDs are positive integers, sent as a number or numeric text.
            "parentSessionId"
                if !value
                    .as_i64()
                    .or_else(|| value.as_str()?.parse().ok())
                    .is_some_and(|n| n > 0) =>
            {
                return Err("parentSessionId must be a session ID (a positive number)".into());
            }
            _ => (),
        }
        if let "folderId" | "folderName" = key.as_str() {
            let max = if key == "folderId" { 256 } else { 100 };
            if value
                .as_str()
                .is_none_or(|s| s.trim().is_empty() || s.chars().count() > max)
            {
                return Err(format!(
                    "{key} must be a non-empty string of at most {max} characters"
                ));
            }
        }
        if [
            "draft",
            "reveal",
            "notifyOnComplete",
            "existing",
            "enabled",
            "providerFiles",
            "removeWorktree",
        ]
        .contains(&key.as_str())
            && !value.is_boolean()
        {
            return Err(format!("{key} must be true or false"));
        }
        if ["limit", "offset", "maxChars", "number"].contains(&key.as_str())
            && value.as_u64().is_none()
        {
            return Err(format!("{key} must be a non-negative integer"));
        }
        if key == "before" && !value.is_string() && value.as_u64().is_none() {
            return Err("before must be a turn ID".into());
        }
        // A cloud session lives in the remote store, whose IDs are negative numbers.
        let session = matches!(key.as_str(), "sessionId" | "besideSessionId");
        if matches!(key.as_str(), "id" | "sessionId" | "besideSessionId")
            && !value.is_string()
            && value.as_u64().is_none()
            && !(session && value.as_i64().is_some_and(|n| n < 0))
        {
            return Err(format!("{key} must be an ID"));
        }
        if ![
            "draft",
            "reveal",
            "notifyOnComplete",
            "existing",
            "enabled",
            "providerFiles",
            "removeWorktree",
            "before",
            "limit",
            "offset",
            "maxChars",
            "number",
            "id",
            "sessionId",
            "besideSessionId",
            "modelSettings",
            "schedule",
            "tags",
            "options",
            "parentSessionId",
            // sessions.start target; checked by validate_target.
            "target",
            // A workflow definition and its input values are objects.
            "workflow",
            "values",
        ]
        .contains(&key.as_str())
            && !value.is_string()
        {
            return Err(format!("{key} must be text"));
        }
    }
    if input.get("modelSettings").is_some_and(|value| {
        value
            .as_object()
            .is_none_or(|values| values.values().any(|v| !v.is_string()))
    }) {
        return Err("modelSettings must be an object of strings".into());
    }
    if input["removeWorktree"] == true && input["providerFiles"] == true {
        return Err(
            "removeWorktree and providerFiles cannot be combined; choose one per call".into(),
        );
    }
    if let Some(account) = input.get("account") {
        let account = account.as_str().unwrap_or_default();
        if account.trim().is_empty() || account.chars().count() > 128 {
            return Err("account must be a non-empty string of at most 128 characters".into());
        }
    }
    if action == "accounts.list" {
        if let Some(provider) = input.get("provider") {
            crate::accounts::parse_provider(provider.as_str().unwrap_or_default())?;
        }
    }
    if let Some(target) = input.get("target") {
        cloud::validate_target(target, input)?;
        if input.get("account").is_some() {
            return Err(ACCOUNT_LOCAL_ONLY.into());
        }
    }
    if action.starts_with("cloud.") {
        cloud::validate_action(action, input)?;
    }
    if input
        .get("cloud")
        .is_some_and(|cloud| !matches!(cloud.as_str(), Some("keep" | "stop" | "delete")))
    {
        return Err("cloud must be \"keep\", \"stop\" or \"delete\"".into());
    }
    if action == "sessions.pullEvidence" {
        if input.get("sessionId").is_none() {
            return Err("Provide sessionId".into());
        }
        if let Some(path) = input.get("path").and_then(Value::as_str) {
            crate::codespaces::valid_remote_path(path)?;
        }
        if input
            .get("taskId")
            .and_then(Value::as_str)
            .is_some_and(|id| id.trim().is_empty() || id.len() > 200)
        {
            return Err("taskId must be a Today task id or key".into());
        }
    }
    if input["draft"] == true && input["notifyOnComplete"] == true {
        return Err("An unsent draft cannot send a completion notification".into());
    }
    if input.get("runtimeMode").is_some_and(|v| {
        !matches!(
            v.as_str(),
            Some("read-only" | "supervised" | "auto-accept-edits" | "auto" | "full-access")
        )
    }) {
        return Err("Unknown runtimeMode".into());
    }
    if input
        .get("workspaceMode")
        .is_some_and(|v| !matches!(v.as_str(), Some("current" | "checkout" | "worktree")))
    {
        return Err("workspaceMode must be current or worktree".into());
    }
    if input
        .get("placement")
        .is_some_and(|v| !matches!(v.as_str(), Some("tab" | "right" | "down")))
    {
        return Err("placement must be tab, right or down".into());
    }
    if input.get("besideSessionId").is_some()
        && !matches!(input["placement"].as_str(), Some("right" | "down"))
    {
        return Err("besideSessionId requires placement right or down".into());
    }
    if !today
        && input.get("tags").is_some_and(|v| {
            v.as_array().is_none_or(|a| {
                a.len() > crate::notes::MAX_TAGS
                    || a.iter().any(|v| {
                        v.as_str()
                            .is_none_or(|s| s.len() > crate::notes::MAX_TAG_LENGTH)
                    })
            })
        })
    {
        return Err("tags must be an array of up to 20 short strings".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn validates_before_mutation() {
        assert!(validate("sessions.start", &json!({"prompt":"x","draft":"false"})).is_err());
        assert!(validate("sessions.delete", &json!({"sessionId":1,"force":true})).is_err());
        assert!(validate("sessions.start", &json!({"runtimeMode":"unsafe"})).is_err());
        assert!(validate(
            "sessions.start",
            &json!({"prompt":"x","project":"/code","draft":true})
        )
        .is_ok());
    }
    #[test]
    fn account_is_bounded_text_for_local_starts_only() {
        let start = |input| validate("sessions.start", &input);
        assert!(start(json!({"prompt":"x","account":"Work"})).is_ok());
        assert!(start(json!({"prompt":"x","account":"a".repeat(128)})).is_ok());
        for bad in [
            json!(""),
            json!("  "),
            json!("a".repeat(129)),
        ] {
            assert_eq!(
                start(json!({"prompt":"x","account":bad})).unwrap_err(),
                "account must be a non-empty string of at most 128 characters"
            );
        }
        for bad in [json!(1), json!(null), json!(true)] {
            assert_eq!(
                start(json!({"prompt":"x","account":bad})).unwrap_err(),
                "account must be text"
            );
        }
        for target in [
            json!({"kind":"codespace","instance":"new"}),
            json!({"kind":"codespace","instance":"my-box"}),
            json!({"kind":"sandbox","instance":{"session":5}}),
            json!({"kind":"sandbox"}),
        ] {
            assert_eq!(
                start(json!({"prompt":"x","account":"Work","target":target})).unwrap_err(),
                ACCOUNT_LOCAL_ONLY
            );
        }
        assert_eq!(
            validate("sessions.send", &json!({"sessionId":1,"account":"Work"})).unwrap_err(),
            "sessions.send does not take account"
        );
    }
    #[test]
    fn accounts_list_takes_an_optional_account_capable_provider() {
        let list = |input| validate("accounts.list", &input);
        assert!(list(json!({})).is_ok());
        assert!(list(json!({"provider":"claude"})).is_ok());
        assert!(list(json!({"provider":"codex"})).is_ok());
        assert!(list(json!({"provider":"devin"})).is_ok());
        for bad in [json!("cursor"), json!("nope"), json!(1), json!("")] {
            assert!(list(json!({"provider":bad})).is_err());
        }
        assert_eq!(
            list(json!({"bogus":1})).unwrap_err(),
            "accounts.list does not take bogus"
        );
    }
    #[test]
    fn provider_files_is_a_boolean_on_delete_only() {
        let delete = |input| validate("sessions.delete", &input);
        assert!(delete(json!({"sessionId":1,"providerFiles":true})).is_ok());
        assert!(delete(json!({"sessionId":1,"providerFiles":false})).is_ok());
        for bad in [json!("true"), json!(1), json!(null)] {
            assert_eq!(
                delete(json!({"sessionId":1,"providerFiles":bad})).unwrap_err(),
                "providerFiles must be true or false"
            );
        }
        for action in ["sessions.stop", "sessions.archive"] {
            assert_eq!(
                validate(action, &json!({"sessionId":1,"providerFiles":true})).unwrap_err(),
                format!("{action} does not take providerFiles")
            );
        }
        assert!(delete(json!({"sessionId":1,"force":true})).is_err());
    }
    #[test]
    fn cloud_is_keep_stop_or_delete_on_archive_only() {
        for value in ["keep", "stop", "delete"] {
            assert!(validate("sessions.archive", &json!({"sessionId":1,"cloud":value})).is_ok());
        }
        assert!(validate("sessions.archive", &json!({"sessionId":1})).is_ok());
        for bad in [json!("remove"), json!("Stop")] {
            assert_eq!(
                validate("sessions.archive", &json!({"sessionId":1,"cloud":bad})).unwrap_err(),
                "cloud must be \"keep\", \"stop\" or \"delete\""
            );
        }
        for bad in [json!(true), json!(null), json!(1)] {
            assert!(validate("sessions.archive", &json!({"sessionId":1,"cloud":bad})).is_err());
        }
        assert_eq!(
            validate("sessions.stop", &json!({"sessionId":1,"cloud":"stop"})).unwrap_err(),
            "sessions.stop does not take cloud"
        );
    }
    #[test]
    fn remove_worktree_is_a_boolean_on_delete_only() {
        let delete = |input| validate("sessions.delete", &input);
        assert!(delete(json!({"sessionId":1,"removeWorktree":true})).is_ok());
        assert!(delete(json!({"sessionId":1,"removeWorktree":false})).is_ok());
        for bad in [json!("true"), json!(1), json!(null)] {
            assert_eq!(
                delete(json!({"sessionId":1,"removeWorktree":bad})).unwrap_err(),
                "removeWorktree must be true or false"
            );
        }
        assert_eq!(
            validate(
                "sessions.archive",
                &json!({"sessionId":1,"removeWorktree":true})
            )
            .unwrap_err(),
            "sessions.archive does not take removeWorktree"
        );
        assert!(delete(json!({"sessionId":1,"removeWorktree":true,"providerFiles":true})).is_err());
    }
    #[test]
    fn today_actions_take_their_own_fields_and_non_text_values() {
        let ok = |action: &str, input| validate(action, &input);
        assert!(ok("today.list", json!({})).is_ok());
        assert!(ok("today.sources", json!({})).is_ok());
        assert!(ok(
            "today.sync",
            json!({"source": "gh", "status": "ok", "items": [{"key": "a", "reopened": true}]})
        )
        .is_ok());
        assert!(ok("today.add", json!({"title": "x", "tags": ["a", "b"]})).is_ok());
        assert_eq!(
            ok("today.sync", json!({"source": "gh", "bogus": 1})).unwrap_err(),
            "today.sync does not take bogus"
        );
        assert_eq!(
            ok("today.sources", json!({"mono": "b"})).unwrap_err(),
            "today.sources does not take mono"
        );
        // Other actions keep wanting text where they did.
        assert!(ok("habits.add", json!({"items": []})).is_err());
        assert_eq!(
            ok("today.nope", json!({})).unwrap_err(),
            "Unknown app action. Run app help"
        );
    }
    #[test]
    fn workflows_actions_take_their_own_fields_and_objects() {
        let actions = [
            "list",
            "get",
            "add",
            "validate",
            "guide",
            "update",
            "setInputs",
            "run",
            "stop",
            "pause",
            "resume",
            "resumeRun",
            "remove",
            "export",
            "import",
            "runs",
        ];
        for action in actions {
            let action = format!("workflows.{action}");
            assert!(validate(&action, &json!({})).is_ok(), "{action}");
            assert_eq!(
                validate(&action, &json!({"bogus": 1})).unwrap_err(),
                format!("{action} does not take bogus")
            );
            assert!(
                validate(&action, &json!({"project": "/p"})).is_ok(),
                "{action}"
            );
            assert!(
                validate(&action, &json!({"project": 5})).is_err(),
                "{action}"
            );
        }
        let ok = |action: &str, input| validate(action, &input);
        assert!(ok(
            "workflows.add",
            json!({"workflow": {"name": "x", "nodes": []}})
        )
        .is_ok());
        assert!(ok(
            "workflows.add",
            json!({"yaml": "a: 1", "values": {"p": "/x", "n": 2}})
        )
        .is_ok());
        assert!(ok("workflows.validate", json!({"id": "wf-1", "workflow": {}})).is_ok());
        assert!(ok(
            "workflows.setInputs",
            json!({"id": "wf-1", "values": {"n": 2}})
        )
        .is_ok());
        assert!(ok(
            "workflows.update",
            json!({"id": "wf-1", "workflow": {"enabled": true}})
        )
        .is_ok());
        assert!(ok("workflows.resumeRun", json!({"id": "wf-1", "runId": "r"})).is_ok());
        assert_eq!(
            ok("workflows.add", json!({"yaml": 5})).unwrap_err(),
            "yaml must be text"
        );
        assert_eq!(
            ok("workflows.resumeRun", json!({"id": "wf-1", "runId": 5})).unwrap_err(),
            "runId must be text"
        );
        assert!(ok("workflows.runs", json!({"id": "wf-1", "limit": 3})).is_ok());
        for bad in [json!("3"), json!(-1), json!(1.5)] {
            assert_eq!(
                ok("workflows.runs", json!({"id": "wf-1", "limit": bad})).unwrap_err(),
                "limit must be a non-negative integer"
            );
        }
        // The objects are only for workflows; other actions still want text.
        assert!(ok("habits.add", json!({"values": {}})).is_err());
        assert!(ok("notes.write", json!({"workflow": {}})).is_err());
        assert_eq!(
            ok("workflows.nope", json!({})).unwrap_err(),
            "Unknown app action. Run app help"
        );
    }
    #[test]
    fn start_fields_for_delivery_parent_and_folders() {
        let ok = |input| validate("sessions.start", &input);
        assert!(ok(json!({"prompt":"x","completionDelivery":"each"})).is_ok());
        assert!(ok(json!({"prompt":"x","completionDelivery":"group"})).is_ok());
        for bad in [json!("all"), json!(true), json!("")] {
            assert_eq!(
                ok(json!({"prompt":"x","completionDelivery":bad})).unwrap_err(),
                "completionDelivery must be \"group\" or \"each\""
            );
        }
        assert!(validate(
            "sessions.send",
            &json!({"sessionId":1,"prompt":"x","completionDelivery":"each"})
        )
        .is_ok());
        assert!(validate(
            "sessions.draft",
            &json!({"sessionId":1,"prompt":"x","completionDelivery":"each"})
        )
        .is_err());
        assert!(ok(json!({"prompt":"x","parentSessionId":7})).is_ok());
        assert!(ok(json!({"prompt":"x","parentSessionId":"7"})).is_ok());
        for bad in [
            json!(0),
            json!(-3),
            json!(1.5),
            json!(""),
            json!("abc"),
            json!(true),
            json!(null),
        ] {
            assert_eq!(
                ok(json!({"prompt":"x","parentSessionId":bad})).unwrap_err(),
                "parentSessionId must be a session ID (a positive number)"
            );
        }
        assert!(ok(json!({"prompt":"x","folderId":"f1","folderName":"Ticket 1"})).is_ok());
        assert!(ok(json!({"prompt":"x","folderName":"n".repeat(100)})).is_ok());
        // Limits count characters, not bytes.
        assert!(ok(json!({"prompt":"x","folderName":"\u{e9}".repeat(100)})).is_ok());
        assert!(ok(json!({"prompt":"x","folderName":"\u{e9}".repeat(101)})).is_err());
        for (key, bad) in [
            ("folderName", json!("n".repeat(101))),
            ("folderName", json!("")),
            ("folderName", json!("   ")),
            ("folderName", json!(5)),
            ("folderId", json!(5)),
            ("folderId", json!(" ")),
            ("folderId", json!("i".repeat(257))),
        ] {
            let error = ok(json!({"prompt":"x", key: bad})).unwrap_err();
            assert!(
                error.starts_with(&format!("{key} must be a non-empty string")),
                "{error}"
            );
        }
    }
    #[test]
    fn cloud_target_is_validated_before_anything_is_created() {
        let ok = |input| validate("sessions.start", &input);
        // Instance forms: omitted and "new" create, a name or {session} reuse.
        assert!(ok(json!({"prompt":"x","target":{"kind":"codespace"}})).is_ok());
        assert!(ok(json!({"prompt":"x","target":{"kind":"codespace","instance":"new"}})).is_ok());
        assert!(ok(json!({"prompt":"x","harness":"claude","target":{"kind":"codespace","instance":"new","branch":"feat/x","machine":"standardLinux32gb"}})).is_ok());
        assert!(
            ok(json!({"prompt":"x","target":{"kind":"codespace","instance":"octo-abc123"}}))
                .is_ok()
        );
        assert!(ok(json!({"prompt":"x","target":{"kind":"sandbox","instance":"new"}})).is_ok());
        assert!(ok(
            json!({"prompt":"x","target":{"kind":"sandbox","instance":"monoptah-fix-1234"}})
        )
        .is_ok());
        assert!(
            ok(json!({"prompt":"x","target":{"kind":"sandbox","instance":"sbx_abc123"}})).is_ok()
        );
        assert!(ok(json!({"prompt":"x","target":{"kind":"sandbox","instance":"new","branch":"feat/x","cpus":4,"memory":"8g","ttl":"1h"}})).is_ok());
        for session in [json!(-5), json!(7), json!("-5"), json!("12")] {
            for kind in ["codespace", "sandbox"] {
                assert!(
                    ok(json!({"prompt":"x","target":{"kind":kind,"instance":{"session":session}}}))
                        .is_ok(),
                    "{kind} {session}"
                );
            }
        }
        assert!(
            ok(json!({"prompt":"x","workspaceMode":"current","target":{"kind":"codespace","instance":"new"}}))
                .is_ok()
        );
        for (bad, why) in [
            (json!("new"), "must be an object"),
            (json!({}), "target.kind must be codespace or sandbox"),
            (json!({"kind":"vm"}), "target.kind must be"),
            (json!({"kind":5}), "target.kind must be"),
            (json!({"codespace":"new"}), "target.codespace was replaced"),
            (
                json!({"kind":"codespace","instance":5}),
                "target.instance must be",
            ),
            (
                json!({"kind":"codespace","instance":"-x"}),
                "letters, digits and hyphens",
            ),
            (
                json!({"kind":"codespace","instance":"a b"}),
                "letters, digits and hyphens",
            ),
            (
                json!({"kind":"codespace","instance":"a;rm"}),
                "letters, digits and hyphens",
            ),
            (
                json!({"kind":"sandbox","instance":"a;rm"}),
                "target.instance must be",
            ),
            (
                json!({"kind":"sandbox","instance":"sbx_UPPER"}),
                "target.instance must be",
            ),
            (
                json!({"kind":"sandbox","instance":"x"}),
                "target.instance must be",
            ),
            (
                json!({"kind":"codespace","instance":{}}),
                "target.instance must be",
            ),
            (
                json!({"kind":"codespace","instance":{"session":0}}),
                "target.instance must be",
            ),
            (
                json!({"kind":"codespace","instance":{"session":"abc"}}),
                "target.instance must be",
            ),
            (
                json!({"kind":"codespace","instance":{"session":1.5}}),
                "target.instance must be",
            ),
            (
                json!({"kind":"codespace","instance":{"session":1,"x":2}}),
                "target.instance must be",
            ),
            (
                json!({"kind":"codespace","extra":1}),
                "target does not take extra",
            ),
            (
                json!({"kind":"codespace","cpus":2}),
                "target does not take cpus for a codespace",
            ),
            (
                json!({"kind":"sandbox","machine":"basicLinux32gb"}),
                "target does not take machine for a sandbox",
            ),
            (
                json!({"kind":"codespace","instance":"new","branch":"--force"}),
                "target.branch",
            ),
            (
                json!({"kind":"codespace","instance":"new","branch":"a b"}),
                "target.branch",
            ),
            (
                json!({"kind":"codespace","instance":"new","branch":3}),
                "target.branch",
            ),
            (
                json!({"kind":"codespace","instance":"new","machine":"bad machine"}),
                "target.machine",
            ),
            (
                json!({"kind":"sandbox","instance":"new","branch":"a b"}),
                "target.branch",
            ),
            (
                json!({"kind":"sandbox","instance":"new","cpus":0}),
                "target.cpus",
            ),
            (
                json!({"kind":"sandbox","instance":"new","cpus":"2"}),
                "target.cpus",
            ),
            (
                json!({"kind":"sandbox","instance":"new","memory":"lots"}),
                "target.memory",
            ),
            (
                json!({"kind":"sandbox","instance":"new","ttl":"48h"}),
                "target.ttl",
            ),
            (
                json!({"kind":"codespace","instance":"octo-abc","branch":"main"}),
                "only applies when creating",
            ),
            (
                json!({"kind":"codespace","instance":"octo-abc","machine":"basicLinux32gb"}),
                "only applies when creating",
            ),
            (
                json!({"kind":"codespace","instance":{"session":-5},"branch":"main"}),
                "only applies when creating",
            ),
            (
                json!({"kind":"sandbox","instance":"box-1","cpus":4}),
                "only applies when creating",
            ),
            (
                json!({"kind":"sandbox","instance":{"session":-5},"ttl":"1h"}),
                "only applies when creating",
            ),
        ] {
            let error = ok(json!({"prompt":"x","target":bad.clone()})).unwrap_err();
            assert!(error.contains(why), "{bad}: {error}");
        }
    }
    #[test]
    fn cloud_target_rejects_options_that_need_a_local_checkout() {
        for kind in ["codespace", "sandbox"] {
            let target = json!({"kind":kind,"instance":"new"});
            for (extra, why) in [
                (json!({"harness":"codex"}), "Claude only"),
                (json!({"draft":true}), "cannot be started as a draft"),
                (json!({"workspaceMode":"worktree"}), "workspaceMode"),
                (json!({"workspaceMode":"checkout"}), "workspaceMode"),
                (json!({"worktreeBase":"main"}), "workspaceMode"),
                (json!({"worktreeCwd":"/x"}), "workspaceMode"),
            ] {
                let mut input = json!({"prompt":"x","target":target});
                input
                    .as_object_mut()
                    .unwrap()
                    .extend(extra.as_object().unwrap().clone());
                let error = validate("sessions.start", &input).unwrap_err();
                assert!(error.contains(why), "{error}");
            }
            // Only sessions.start takes a target.
            assert_eq!(
                validate(
                    "sessions.send",
                    &json!({"sessionId":1,"prompt":"x","target":target})
                )
                .unwrap_err(),
                "sessions.send does not take target"
            );
        }
    }
    #[test]
    fn session_ids_may_be_negative_because_cloud_sessions_are_remote() {
        assert!(validate("sessions.read", &json!({"sessionId":-123456})).is_ok());
        assert!(validate("sessions.send", &json!({"sessionId":-5,"prompt":"x"})).is_ok());
        assert!(validate("sessions.stop", &json!({"sessionId":"-5"})).is_ok());
        assert!(validate("sessions.read", &json!({"sessionId":1.5})).is_err());
        assert!(validate("sessions.read", &json!({"sessionId":null})).is_err());
        // Note IDs and parents stay as they were.
        assert!(validate("notes.read", &json!({"id":-1})).is_err());
        assert!(validate(
            "sessions.start",
            &json!({"prompt":"x","parentSessionId":-4})
        )
        .is_err());
    }
    #[test]
    fn pull_evidence_takes_a_session_and_checked_optional_fields() {
        assert!(validate("sessions.pullEvidence", &json!({"sessionId":-5})).is_ok());
        assert!(validate(
            "sessions.pullEvidence",
            &json!({"sessionId":-5,"path":"out/shots","taskId":"manual:7","project":"/p"})
        )
        .is_ok());
        for bad in [
            json!({}),
            json!({"sessionId":-5,"path":"-r"}),
            json!({"sessionId":-5,"path":"../x"}),
            json!({"sessionId":-5,"path":5}),
            json!({"sessionId":-5,"taskId":" "}),
            json!({"sessionId":-5,"taskId":7}),
            json!({"sessionId":-5,"force":true}),
        ] {
            assert!(validate("sessions.pullEvidence", &bad).is_err(), "{bad}");
        }
    }
    #[test]
    fn cloud_actions_take_exactly_their_fields() {
        for ok in [
            ("cloud.list", json!({})),
            ("cloud.list", json!({"project":"/p","kind":"sandbox"})),
            ("cloud.status", json!({})),
            ("cloud.status", json!({"kind":"codespace"})),
            (
                "cloud.create",
                json!({"kind":"codespace","branch":"main","machine":"basicLinux32gb","displayName":"mine"}),
            ),
            ("cloud.create", json!({"kind":"codespace"})),
            // A sandbox cannot be created alone; the capability error comes at run time.
            ("cloud.create", json!({"kind":"sandbox"})),
            ("cloud.create", json!({"kind":"sandbox","branch":"x"})),
            ("cloud.stop", json!({"sessionId":-5})),
            ("cloud.stop", json!({"sessionId":"-5","project":"/p"})),
            (
                "cloud.resume",
                json!({"kind":"codespace","instance":"octo-abc"}),
            ),
            ("cloud.delete", json!({"kind":"sandbox","instance":"box-1"})),
            ("cloud.delete", json!({"sessionId":7})),
        ] {
            assert!(validate(ok.0, &ok.1).is_ok(), "{} {}", ok.0, ok.1);
        }
        for (action, input) in [
            ("cloud.list", json!({"name":"x"})),
            ("cloud.status", json!({"value":"s"})),
            ("cloud.list", json!({"kind":"vm"})),
            ("cloud.list", json!({"kind":5})),
            ("cloud.create", json!({})),
            ("cloud.create", json!({"kind":"vm"})),
            ("cloud.create", json!({"kind":"codespace","name":"x"})),
            ("cloud.create", json!({"kind":"codespace","branch":"--x"})),
            ("cloud.create", json!({"kind":"codespace","machine":"a;b"})),
            ("cloud.create", json!({"kind":"codespace","displayName":""})),
            (
                "cloud.create",
                json!({"kind":"codespace","displayName":"x".repeat(49)}),
            ),
            ("cloud.stop", json!({})),
            ("cloud.stop", json!({"kind":"codespace"})),
            ("cloud.stop", json!({"instance":"octo-abc"})),
            (
                "cloud.stop",
                json!({"sessionId":1,"kind":"codespace","instance":"octo-abc"}),
            ),
            ("cloud.stop", json!({"sessionId":1,"instance":"octo-abc"})),
            ("cloud.stop", json!({"kind":"codespace","instance":"a b"})),
            ("cloud.delete", json!({"kind":"codespace","instance":"-rf"})),
            ("cloud.delete", json!({"kind":"codespace","instance":5})),
            ("cloud.delete", json!({"kind":"sandbox","instance":"a;b"})),
            ("cloud.delete", json!({"sessionId":null})),
            ("cloud.resume", json!({"sessionId":1,"force":true})),
            ("cloud.nope", json!({})),
            // The old codespace-only actions are gone, without aliases.
            ("codespaces.list", json!({})),
            ("codespaces.create", json!({})),
            ("codespaces.stop", json!({"name":"octo-abc"})),
            ("codespaces.delete", json!({"name":"octo-abc"})),
            ("codespaces.secretsStatus", json!({})),
        ] {
            assert!(validate(action, &input).is_err(), "{action} {input}");
        }
    }
}
