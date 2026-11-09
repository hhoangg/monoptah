//! Input checks of the cloud app API, called from `mono::api::validate` before anything runs.
//!
//! Nothing here names a backend: instance names, option keys and option values come from the
//! [`CloudKindSpec`] registry.
use super::provider::{spec_of, specs, CloudKindSpec};
use crate::cloud_sessions::CloudBackendKind;
use serde_json::{Map, Value};

/// Accepted values of `kind`, for error text: `codespace or sandbox`.
fn kind_choices() -> String {
    CloudBackendKind::ALL
        .iter()
        .map(|kind| kind.api_name())
        .collect::<Vec<_>>()
        .join(" or ")
}

/// Space separated field names of a `cloud.*` action; `None` for an unknown action.
pub fn fields(action: &str) -> Option<String> {
    match action {
        "cloud.list" | "cloud.status" => Some("project kind".into()),
        "cloud.create" => {
            let mut fields = String::from("kind project");
            for spec in specs() {
                for option in spec.create_options() {
                    if !fields.split_whitespace().any(|f| f == *option) {
                        fields.push(' ');
                        fields.push_str(option);
                    }
                }
            }
            Some(fields)
        }
        "cloud.stop" | "cloud.resume" | "cloud.delete" => {
            Some("sessionId project kind instance".into())
        }
        _ => None,
    }
}

fn kind_of(value: &Value, what: &str) -> Result<CloudBackendKind, String> {
    value
        .as_str()
        .and_then(CloudBackendKind::from_api_name)
        .ok_or_else(|| format!("{what} must be {}", kind_choices()))
}

/// Checks the values of a `cloud.*` action after the generic field check passed.
pub fn validate_action(action: &str, input: &Value) -> Result<(), String> {
    let kind = input
        .get("kind")
        .map(|value| kind_of(value, "kind"))
        .transpose()?;
    match action {
        "cloud.list" | "cloud.status" => Ok(()),
        "cloud.create" => {
            let kind = kind.ok_or_else(|| format!("Provide kind ({})", kind_choices()))?;
            let spec = spec_of(kind);
            // A backend that cannot create on its own answers with a capability error at run
            // time, which says what to do instead; do not hide it behind a field error.
            if !spec.supports_standalone_create() {
                return Ok(());
            }
            for (key, value) in input.as_object().into_iter().flatten() {
                if matches!(key.as_str(), "kind" | "project") {
                    continue;
                }
                if !spec.create_options().contains(&key.as_str()) {
                    return Err(format!(
                        "cloud.create does not take {key} for a {}",
                        kind.noun()
                    ));
                }
                spec.check_create_option(key, value)?;
            }
            Ok(())
        }
        _ => validate_address(input, kind),
    }
}

/// `sessionId` XOR (`kind` + `instance`).
fn validate_address(input: &Value, kind: Option<CloudBackendKind>) -> Result<(), String> {
    let session = input.get("sessionId").is_some();
    let instance = input.get("instance");
    if session && (kind.is_some() || instance.is_some()) {
        return Err("Provide either sessionId or kind and instance, not both".into());
    }
    if session {
        return Ok(());
    }
    match (kind, instance) {
        (None, None) => Err("Provide sessionId, or kind and instance".into()),
        (Some(_), None) | (None, Some(_)) => Err("Provide both kind and instance".into()),
        (Some(kind), Some(instance)) => {
            let name = instance.as_str().ok_or("instance must be text")?;
            spec_of(kind).instance_name(name)
        }
    }
}

/// A `{session: <id>}` instance: a number or numeric text, positive or negative (cloud
/// sessions live in the remote store, whose ids are negative), never zero.
pub fn parse_session_ref(value: &Value) -> Option<i64> {
    value
        .as_i64()
        .or_else(|| value.as_str()?.trim().parse().ok())
        .filter(|id| *id != 0)
}

/// `target` of `sessions.start`: `{kind, instance?, ...options}`. A cloud session is a Claude
/// chat in a codespace or sandbox of the project, so options that only make sense for a local
/// checkout are refused here, before anything is created.
pub fn validate_target(target: &Value, input: &Value) -> Result<(), String> {
    let target = target
        .as_object()
        .ok_or("target must be an object like {\"kind\":\"codespace\",\"instance\":\"new\"}")?;
    if target.contains_key("codespace") && !target.contains_key("kind") {
        return Err(
            "target.codespace was replaced: use {\"kind\":\"codespace\",\"instance\":\"new\"|\"<name>\"}"
                .into(),
        );
    }
    let kind = target
        .get("kind")
        .ok_or_else(|| format!("target.kind must be {}", kind_choices()))
        .and_then(|value| kind_of(value, "target.kind"))?;
    let spec = spec_of(kind);
    for key in target.keys() {
        if !["kind", "instance"].contains(&key.as_str())
            && !spec.start_options().contains(&key.as_str())
        {
            return Err(format!("target does not take {key} for a {}", kind.noun()));
        }
    }
    let creating = validate_instance(target, spec)?;
    for (key, value) in target {
        if matches!(key.as_str(), "kind" | "instance") {
            continue;
        }
        if !creating {
            return Err(format!(
                "target.{key} only applies when creating a {} (instance \"new\")",
                kind.noun()
            ));
        }
        spec.check_start_option(key, value)?;
    }
    validate_session_options(input)
}

/// Checks `target.instance`; true when it asks for a new instance.
fn validate_instance(
    target: &Map<String, Value>,
    spec: &dyn CloudKindSpec,
) -> Result<bool, String> {
    let bad = || {
        format!(
            "target.instance must be \"new\", the name of an existing {} or {{\"session\":<sessionId>}}",
            spec.kind().noun()
        )
    };
    match target.get("instance") {
        None => Ok(true),
        Some(Value::String(name)) if name == "new" => Ok(true),
        Some(Value::String(name)) => spec
            .instance_name(name)
            .map(|()| false)
            .map_err(|e| format!("{} ({e})", bad())),
        Some(Value::Object(map)) => {
            if map.len() == 1 && map.get("session").and_then(parse_session_ref).is_some() {
                Ok(false)
            } else {
                Err(bad())
            }
        }
        Some(_) => Err(bad()),
    }
}

/// Options of `sessions.start` that a cloud session cannot honour.
fn validate_session_options(input: &Value) -> Result<(), String> {
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
        return Err("A cloud session runs in its cloud workspace; workspaceMode, worktreeBase and worktreeCwd do not apply".into());
    }
    Ok(())
}
