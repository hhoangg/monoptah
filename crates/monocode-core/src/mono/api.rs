use serde_json::Value;
/// Reject misspelled fields and ambiguous types before any app mutation.
pub fn validate(action: &str, input: &Value) -> Result<(), String> {
    let fields=match action{
        "models.list"|"soul.read"|"habits.list"=>"",
        "sessions.list"|"folders.list"|"worktrees.list"=>"project",
        "sessions.read"=>"sessionId project before limit maxChars",
        "sessions.start"=>"prompt project harness model modelSettings effort runtimeMode title draft reveal workspaceMode worktreeBase worktreeCwd placement besideSessionId notifyOnComplete completionDelivery parentSessionId folderName folderId target",
        "codespaces.list"|"codespaces.secretsStatus"=>"project",
        "codespaces.create"=>"branch machine displayName project",
        "codespaces.stop"|"codespaces.delete"=>"name project",
        "sessions.send"=>"sessionId prompt project notifyOnComplete completionDelivery",
        "sessions.draft"=>"sessionId prompt project",
        "sessions.stop"|"sessions.archive"=>"sessionId project",
        "sessions.delete"=>"sessionId project providerFiles",
        "worktrees.create"=>"branch base existing project",
        "folders.move"=>"sessionId folderId newFolderName project",
        "notes.list"=>"limit offset","notes.read"=>"id","notes.write"=>"id title body tags",
        "artifacts.list"=>"kind limit offset","artifacts.read"=>"id","artifacts.write"=>"id kind title body summary",
        "soul.update"=>"text expectedHash","memory.read"=>"topic","memory.search"=>"query since",
        "memory.add"=>"fact topic until","memory.replace"=>"find fact topic until","memory.remove"=>"find topic",
        "habits.add"=>"name instructions schedule","habits.update"=>"id name instructions schedule enabled","habits.run"|"habits.remove"=>"id",
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
    if let Some(target) = input.get("target") {
        validate_target(target, input)?;
    }
    if action.starts_with("codespaces.") {
        validate_codespace_fields(input)?;
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

/// Valid codespace names are letters, digits and hyphens (they become `gh` argv).
fn codespace_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 100
        && !value.starts_with('-')
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
}
fn branch_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 250
        && !value.starts_with('-')
        && !value.chars().any(|c| c.is_whitespace() || c.is_control())
}
fn machine_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// `target` of sessions.start: `{codespace:"new"|"<name>", branch?, machine?}`. A cloud session
/// is a Claude chat in a codespace of the project's GitHub repository, so options that only make
/// sense for a local checkout are refused here, before anything is created.
fn validate_target(target: &Value, input: &Value) -> Result<(), String> {
    let target = target.as_object().ok_or(
        "target must be an object like {\"codespace\":\"new\"} or {\"codespace\":\"<name>\"}",
    )?;
    for key in target.keys() {
        if !["codespace", "branch", "machine"].contains(&key.as_str()) {
            return Err(format!("target does not take {key}"));
        }
    }
    let name = target
        .get("codespace")
        .and_then(Value::as_str)
        .ok_or("target.codespace must be \"new\" or the name of an existing codespace")?;
    if name != "new" && !codespace_name(name) {
        return Err(
            "target.codespace must be \"new\" or a codespace name (letters, digits and hyphens)"
                .into(),
        );
    }
    for (key, valid) in [
        ("branch", branch_name as fn(&str) -> bool),
        ("machine", machine_name),
    ] {
        if let Some(value) = target.get(key) {
            if !value.as_str().is_some_and(valid) {
                return Err(format!("target.{key} is not a valid {key} name"));
            }
            if name != "new" {
                return Err(format!(
                    "target.{key} only applies when creating a codespace (codespace \"new\")"
                ));
            }
        }
    }
    if input
        .get("harness")
        .is_some_and(|h| h.as_str() != Some("claude"))
    {
        return Err("Cloud sessions support Claude only for now (harness \"claude\")".into());
    }
    if input["draft"] == true {
        return Err("A cloud session cannot be started as a draft".into());
    }
    if input
        .get("workspaceMode")
        .is_some_and(|v| v.as_str() != Some("current"))
        || input.get("worktreeBase").is_some()
        || input.get("worktreeCwd").is_some()
    {
        return Err("A cloud session runs in its codespace; workspaceMode, worktreeBase and worktreeCwd do not apply".into());
    }
    Ok(())
}

fn validate_codespace_fields(input: &Value) -> Result<(), String> {
    if let Some(name) = input.get("name").and_then(Value::as_str) {
        if !codespace_name(name) {
            return Err("name must be a codespace name (letters, digits and hyphens)".into());
        }
    }
    if input
        .get("branch")
        .and_then(Value::as_str)
        .is_some_and(|b| !branch_name(b))
    {
        return Err("branch is not a valid branch name".into());
    }
    if input
        .get("machine")
        .and_then(Value::as_str)
        .is_some_and(|m| !machine_name(m))
    {
        return Err("machine is not a valid machine type name".into());
    }
    if input
        .get("displayName")
        .and_then(Value::as_str)
        .is_some_and(|d| {
            d.trim().is_empty() || d.chars().count() > 48 || d.chars().any(char::is_control)
        })
    {
        return Err("displayName must be 1 to 48 characters".into());
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
        assert!(ok(json!({"prompt":"x","target":{"codespace":"new"}})).is_ok());
        assert!(ok(json!({"prompt":"x","harness":"claude","target":{"codespace":"new","branch":"feat/x","machine":"standardLinux32gb"}})).is_ok());
        assert!(ok(json!({"prompt":"x","target":{"codespace":"octo-abc123"}})).is_ok());
        assert!(
            ok(json!({"prompt":"x","workspaceMode":"current","target":{"codespace":"new"}}))
                .is_ok()
        );
        for (bad, why) in [
            (json!("new"), "must be an object"),
            (json!({}), "target.codespace must be"),
            (json!({"codespace":5}), "target.codespace must be"),
            (json!({"codespace":"-x"}), "letters, digits and hyphens"),
            (json!({"codespace":"a b"}), "letters, digits and hyphens"),
            (json!({"codespace":"a;rm"}), "letters, digits and hyphens"),
            (json!({"codespace":"new","extra":1}), "does not take extra"),
            (
                json!({"codespace":"new","branch":"--force"}),
                "target.branch",
            ),
            (json!({"codespace":"new","branch":"a b"}), "target.branch"),
            (json!({"codespace":"new","branch":3}), "target.branch"),
            (
                json!({"codespace":"new","machine":"bad machine"}),
                "target.machine",
            ),
            (
                json!({"codespace":"octo-abc","branch":"main"}),
                "only applies when creating",
            ),
            (
                json!({"codespace":"octo-abc","machine":"basicLinux32gb"}),
                "only applies when creating",
            ),
        ] {
            let error = ok(json!({"prompt":"x","target":bad})).unwrap_err();
            assert!(error.contains(why), "{error}");
        }
    }
    #[test]
    fn cloud_target_rejects_options_that_need_a_local_checkout() {
        let target = json!({"codespace":"new"});
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
    fn codespace_actions_take_exactly_their_fields() {
        assert!(validate("codespaces.list", &json!({})).is_ok());
        assert!(validate("codespaces.list", &json!({"project":"/p"})).is_ok());
        assert!(validate("codespaces.secretsStatus", &json!({})).is_ok());
        assert!(validate(
            "codespaces.create",
            &json!({"branch":"main","machine":"basicLinux32gb","displayName":"mine"})
        )
        .is_ok());
        assert!(validate("codespaces.create", &json!({})).is_ok());
        assert!(validate("codespaces.stop", &json!({"name":"octo-abc"})).is_ok());
        assert!(validate("codespaces.delete", &json!({"name":"octo-abc"})).is_ok());
        for (action, input) in [
            ("codespaces.list", json!({"name":"x"})),
            ("codespaces.secretsStatus", json!({"value":"s"})),
            ("codespaces.create", json!({"name":"x"})),
            ("codespaces.stop", json!({"branch":"main"})),
            ("codespaces.delete", json!({"force":true})),
            ("codespaces.nope", json!({})),
        ] {
            assert!(validate(action, &input).is_err(), "{action}");
        }
        for (action, input) in [
            ("codespaces.stop", json!({"name":"a b"})),
            ("codespaces.delete", json!({"name":"-rf"})),
            ("codespaces.delete", json!({"name":5})),
            ("codespaces.create", json!({"branch":"--x"})),
            ("codespaces.create", json!({"machine":"a;b"})),
            ("codespaces.create", json!({"displayName":""})),
            ("codespaces.create", json!({"displayName":"x".repeat(49)})),
        ] {
            assert!(validate(action, &input).is_err(), "{action} {input}");
        }
    }
}
