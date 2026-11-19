//! `device.selection` for a web device: the list of everything the desktop
//! published (names only for what the device does not have), the opaque refs
//! that stand for the unshared items, and the plan of a change (narrow or
//! widen). The host decides who may apply a plan; this module only computes.
//!
//! A project id is an absolute path (G13), so an unshared item is described by
//! its name and a ref: the ref is a keyed hash that changes whenever the
//! desktop restarts, and never reveals the id.
use super::policy::{requested, Code, DeviceContext, Failure, Requested, Selection};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::HashMap;

const INVALID: &str = "Invalid mobile parameters";
/// Most refs one call may carry, and the longest one.
const MAX_ADD: usize = 1000;
const MAX_REF_CHARS: usize = 64;
/// The longest ticket the host accepts.
pub(crate) const MAX_TICKET_CHARS: usize = 256;
const NAME_CHARS: usize = 200;
/// Longest text of an access change.
const CHANGE_CHARS: usize = 400;

/// Which list of the catalog an id belongs to. Part of the ref, so a project
/// and a Mono with the same id get different refs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Project,
    Mono,
}

impl Kind {
    fn key(self) -> &'static str {
        match self {
            Kind::Project => "projects",
            Kind::Mono => "monos",
        }
    }
    fn label(self) -> &'static str {
        match self {
            Kind::Project => "project",
            Kind::Mono => "mono",
        }
    }
    fn unnamed(self) -> &'static str {
        match self {
            Kind::Project => "Unnamed project",
            Kind::Mono => "Unnamed Mono",
        }
    }
}

/// The first 32 hex characters of SHA-256(secret || kind || 0 || id).
pub(crate) fn make_ref(secret: &[u8; 32], kind: Kind, id: &str) -> String {
    let mut hash = Sha256::new();
    hash.update(secret);
    hash.update(kind.label().as_bytes());
    hash.update([0u8]);
    hash.update(id.as_bytes());
    hash.finalize()[..16]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// A catalog name as text for the web: no control characters, capped, and a
/// generic name when there is none. Never the id.
pub(crate) fn clean_name(name: Option<&str>, kind: Kind) -> String {
    let cleaned: String = name
        .unwrap_or_default()
        .chars()
        .filter(|c| !c.is_control())
        .take(NAME_CHARS)
        .collect();
    let cleaned = cleaned.trim();
    if cleaned.is_empty() {
        kind.unnamed().to_owned()
    } else {
        cleaned.to_owned()
    }
}

fn entries(catalog: &Value, kind: Kind) -> impl Iterator<Item = (&str, Option<&str>)> {
    catalog[kind.key()]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|entry| Some((entry["id"].as_str()?, entry["name"].as_str())))
}

/// The call's own fields, validated.
pub(crate) struct Request {
    projects: Requested,
    monos: Requested,
    add_projects: Vec<String>,
    add_monos: Vec<String>,
    pub(crate) ticket: Option<String>,
}

fn refs(params: &Value, key: &str) -> Result<Vec<String>, Failure> {
    let Some(value) = params.get(key) else {
        return Ok(Vec::new());
    };
    let items = value.as_array().filter(|a| a.len() <= MAX_ADD);
    let mut list: Vec<String> = Vec::new();
    for item in items.ok_or(INVALID)? {
        let text = item
            .as_str()
            .filter(|t| t.len() <= MAX_REF_CHARS)
            .ok_or(INVALID)?;
        if !list.iter().any(|v| v == text) {
            list.push(text.to_owned());
        }
    }
    Ok(list)
}

pub(crate) fn parse(params: &Value) -> Result<Request, Failure> {
    let ticket = match params.get("ticket") {
        None | Some(Value::Null) => None,
        Some(Value::String(t)) if t.chars().count() <= MAX_TICKET_CHARS => Some(t.clone()),
        Some(_) => return Err(INVALID.to_owned().into()),
    };
    Ok(Request {
        projects: requested(params, "projects")?,
        monos: requested(params, "monos")?,
        add_projects: refs(params, "addProjects")?,
        add_monos: refs(params, "addMonos")?,
        ticket,
    })
}

/// What one field becomes.
struct FieldPlan {
    value: Option<Vec<String>>,
    widens: bool,
    /// The device now has everything, including what is published later.
    all: bool,
    /// Names of the items that are new to the device.
    added: Vec<String>,
}

fn plan_field(
    kind: Kind,
    current: &Option<Vec<String>>,
    base: &Requested,
    adds: &[String],
    catalog: &Value,
    secret: &[u8; 32],
) -> Result<FieldPlan, Failure> {
    let mut resolved: Vec<String> = Vec::new();
    if !adds.is_empty() {
        let by_ref: HashMap<String, &str> = entries(catalog, kind)
            .map(|(id, _)| (make_ref(secret, kind, id), id))
            .collect();
        for add in adds {
            let id = by_ref.get(add).ok_or_else(|| Code::NotFound.failure())?;
            resolved.push((*id).to_owned());
        }
    }
    // An explicit id list can only narrow: new ids enter through refs, which
    // the catalog vouches for. Against "all" a list is a narrowing as given.
    let base: Option<Vec<String>> = match (base, current) {
        (Requested::Keep, _) => current.clone(),
        (Requested::All, _) => None,
        (Requested::List(list), None) => Some(list.clone()),
        (Requested::List(list), Some(have)) => Some(
            list.iter()
                .filter(|id| have.contains(id))
                .cloned()
                .collect(),
        ),
    };
    let value = base.map(|mut list| {
        for id in resolved {
            if !list.contains(&id) {
                list.push(id);
            }
        }
        list
    });
    let (widens, all, new_ids) = match (&value, current) {
        (_, None) => (false, false, Vec::new()),
        (None, Some(_)) => (true, true, Vec::new()),
        (Some(list), Some(have)) => {
            let new: Vec<&String> = list.iter().filter(|id| !have.contains(id)).collect();
            (
                !new.is_empty(),
                false,
                new.into_iter().cloned().collect::<Vec<_>>(),
            )
        }
    };
    let added = new_ids
        .iter()
        .map(|id| {
            let name = entries(catalog, kind).find(|(have, _)| have == id);
            clean_name(name.and_then(|(_, name)| name), kind)
        })
        .collect();
    Ok(FieldPlan {
        value,
        widens,
        all,
        added,
    })
}

/// A change to a web device's selection, computed against one catalog.
#[derive(Debug, PartialEq)]
pub(crate) struct Plan {
    pub(crate) selection: Selection,
    /// Differs from the current selection.
    pub(crate) changed: bool,
    pub(crate) widens: bool,
    /// "all projects", "all Monos" or the names added, for the audit line and
    /// the access change.
    pub(crate) summary: String,
}

pub(crate) fn plan(
    device: &DeviceContext,
    request: &Request,
    catalog: &Value,
    secret: &[u8; 32],
) -> Result<Plan, Failure> {
    let projects = plan_field(
        Kind::Project,
        &device.projects,
        &request.projects,
        &request.add_projects,
        catalog,
        secret,
    )?;
    let monos = plan_field(
        Kind::Mono,
        &device.monos,
        &request.monos,
        &request.add_monos,
        catalog,
        secret,
    )?;
    let mut parts: Vec<String> = Vec::new();
    if projects.all {
        parts.push("all projects, including new ones".into());
    }
    if monos.all {
        parts.push("all Monos, including new ones".into());
    }
    parts.extend(projects.added.iter().cloned());
    parts.extend(monos.added.iter().cloned());
    let selection = Selection {
        projects: projects.value,
        monos: monos.value,
    };
    Ok(Plan {
        changed: selection.projects != device.projects || selection.monos != device.monos,
        widens: projects.widens || monos.widens,
        selection,
        summary: parts.join(", "),
    })
}

/// "projects 3 -> 1, monos all -> 2": counts only, no names.
pub(crate) fn narrow_detail(device: &DeviceContext, selection: &Selection) -> String {
    let count = |list: &Option<Vec<String>>| {
        list.as_ref()
            .map_or("all".to_owned(), |l| l.len().to_string())
    };
    let mut parts: Vec<String> = Vec::new();
    if selection.projects != device.projects {
        parts.push(format!(
            "projects {} -> {}",
            count(&device.projects),
            count(&selection.projects)
        ));
    }
    if selection.monos != device.monos {
        parts.push(format!(
            "monos {} -> {}",
            count(&device.monos),
            count(&selection.monos)
        ));
    }
    parts.join(", ")
}

/// The reply to `device.selection` for a web device, after any change.
pub(crate) fn reply(
    selection: &Selection,
    can_widen: bool,
    catalog: &Value,
    secret: &[u8; 32],
) -> Value {
    let list = |kind: Kind, chosen: &Option<Vec<String>>| -> Vec<Value> {
        entries(catalog, kind)
            .map(|(id, name)| {
                let shared = chosen
                    .as_ref()
                    .is_none_or(|ids| ids.iter().any(|v| v == id));
                let mut item = json!({
                    "ref": make_ref(secret, kind, id),
                    "name": clean_name(name, kind),
                    "shared": shared,
                });
                if shared {
                    item["id"] = json!(id);
                }
                item
            })
            .collect()
    };
    json!({
        "projects": selection.projects,
        "monos": selection.monos,
        "available": {
            "projects": list(Kind::Project, &selection.projects),
            "monos": list(Kind::Mono, &selection.monos),
        },
        "canWiden": can_widen,
    })
}

/// "<device> now has access to <summary>", cleaned and capped.
pub(crate) fn change_text(device: &str, summary: &str) -> String {
    let text = format!("{device} now has access to {summary}");
    text.chars()
        .filter(|c| !c.is_control())
        .take(CHANGE_CHARS)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_cleaned_and_never_fall_back_to_the_id() {
        assert_eq!(clean_name(Some("a\u{0}b\n"), Kind::Project), "ab");
        assert_eq!(clean_name(Some("  "), Kind::Mono), "Unnamed Mono");
        assert_eq!(clean_name(None, Kind::Project), "Unnamed project");
        assert_eq!(clean_name(Some(&"x".repeat(500)), Kind::Project).len(), 200);
    }

    #[test]
    fn the_text_of_a_change_is_capped_and_has_no_control_characters() {
        let text = change_text("Lap\ntop", &"n".repeat(1000));
        assert!(text.starts_with("Laptop now has access to "));
        assert_eq!(text.chars().count(), 400);
    }
}
