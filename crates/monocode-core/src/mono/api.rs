use serde_json::Value;
/// Reject misspelled fields and ambiguous types before any app mutation.
pub fn validate(action: &str, input: &Value) -> Result<(), String> {
    let fields=match action{
        "models.list"|"soul.read"|"habits.list"=>"",
        "sessions.list"|"folders.list"|"worktrees.list"=>"project",
        "sessions.read"=>"sessionId project before limit maxChars",
        "sessions.start"=>"prompt project harness model modelSettings runtimeMode title draft reveal workspaceMode worktreeBase worktreeCwd notifyOnComplete completionDelivery parentSessionId folderName folderId",
        "sessions.send"=>"sessionId prompt project notifyOnComplete completionDelivery",
        "sessions.draft"=>"sessionId prompt project",
        "sessions.stop"|"sessions.archive"|"sessions.delete"=>"sessionId project",
        "worktrees.create"=>"branch base existing project",
        "folders.move"=>"sessionId folderId newFolderName project",
        "notes.list"=>"limit offset","notes.read"=>"id","notes.write"=>"id title body tags",
        "artifacts.list"=>"kind limit offset","artifacts.read"=>"id","artifacts.write"=>"id kind title body summary",
        "soul.update"=>"text expectedHash","memory.read"=>"topic","memory.search"=>"query since",
        "memory.add"=>"fact topic until","memory.replace"=>"find fact topic until","memory.remove"=>"find topic",
        "habits.add"=>"name instructions schedule","habits.update"=>"id name instructions schedule enabled","habits.run"|"habits.remove"=>"id",
        "chat.card"=>"type repo number note sessionId options name instructions schedule",
        _=>return Err("Unknown app action. Run app help".into()),
    };
    for (key, value) in input.as_object().ok_or("Input must be an object")? {
        if !fields.split_whitespace().any(|f| f == key) {
            return Err(format!("{action} does not take {key}"));
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
        if ["draft", "reveal", "notifyOnComplete", "existing", "enabled"].contains(&key.as_str())
            && !value.is_boolean()
        {
            return Err(format!("{key} must be true or false"));
        }
        if ["before", "limit", "offset", "maxChars", "number"].contains(&key.as_str())
            && value.as_u64().is_none()
        {
            return Err(format!("{key} must be a non-negative integer"));
        }
        if ![
            "draft",
            "reveal",
            "notifyOnComplete",
            "existing",
            "enabled",
            "before",
            "limit",
            "offset",
            "maxChars",
            "number",
            "id",
            "sessionId",
            "modelSettings",
            "schedule",
            "tags",
            "options",
            "parentSessionId",
        ]
        .contains(&key.as_str())
            && !value.is_string()
        {
            return Err(format!("{key} must be text"));
        }
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
    if input.get("tags").is_some_and(|v| {
        v.as_array().is_none_or(|a| {
            a.len() > 32 || a.iter().any(|v| v.as_str().is_none_or(|s| s.len() > 160))
        })
    }) {
        return Err("tags must be an array of up to 32 short strings".into());
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
}
