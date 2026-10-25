//! Saved Ticket Contributors preferences. The Tauri app keeps them in
//! localStorage under `monocode.*` keys that are never renamed; the native app
//! reads those as defaults and saves its own changes under
//! `native-ticket-contributors-*`. Port of `ticket-contributors/model/prefs.ts`.

use chrono::{DateTime, Local};
use serde::Serialize;
use serde_json::{Map, Value};

use super::tickets::{ExcludedMember, Points, WorkspaceScope, DEFAULT_TICKET_POINTS, LEVELS};
use crate::contributors::model::js_trim;
use crate::contributors::model::period::{PeriodKind, PeriodPick};
use crate::contributors::model::prefs::{desktop_value, parse_pick, serialize_pick};
use crate::store::Store;

// The `monocode.` prefix is deliberately kept, like every other preference key.
pub const SETTINGS_KEY: &str = "monocode.ticketContributors.settings";
pub const RANGE_KEY: &str = "monocode.ticketContributors.range";
pub const NATIVE_SETTINGS: &str = "native-ticket-contributors-settings";
pub const NATIVE_RANGE: &str = "native-ticket-contributors-range";

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct TicketBoardSettings {
    pub scope: Option<WorkspaceScope>,
    pub points: Points,
    #[serde(serialize_with = "crate::contributors::model::serialize_number")]
    pub percent: f64,
}

fn strings(value: Option<&Value>) -> Vec<String> {
    match value {
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|v| v.as_str().map(String::from))
            .collect(),
        _ => vec![],
    }
}

/// Old scopes stored bare ids; those read as a member named by its id.
fn parse_excluded_members(value: Option<&Value>) -> Vec<ExcludedMember> {
    let Some(Value::Array(entries)) = value else {
        return vec![];
    };
    let mut members: Vec<ExcludedMember> = Vec::new();
    for entry in entries {
        let member = match entry {
            Value::String(id) => Some(ExcludedMember {
                id: id.clone(),
                name: id.clone(),
            }),
            Value::Object(map) => map.get("id").and_then(Value::as_str).map(|id| {
                let name = map.get("name").and_then(Value::as_str).map_or("", js_trim);
                ExcludedMember {
                    id: id.to_string(),
                    name: if name.is_empty() { id } else { name }.to_string(),
                }
            }),
            _ => None,
        };
        if let Some(member) = member {
            if !member.id.is_empty() && !members.iter().any(|m| m.id == member.id) {
                members.push(member);
            }
        }
    }
    members
}

fn parse_scope(value: Option<&Value>) -> Option<WorkspaceScope> {
    let map = value?.as_object()?;
    let workspace_id = map
        .get("workspaceId")?
        .as_str()
        .filter(|id| !id.is_empty())?;
    let space_ids: Vec<String> = strings(map.get("spaceIds"))
        .iter()
        .map(|s| js_trim(s).to_string())
        .filter(|s| !s.is_empty())
        .collect();
    Some(WorkspaceScope {
        workspace_id: workspace_id.to_string(),
        name: map
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        // Every space of the workspace: nothing (or only blanks) was picked.
        space_ids: (!space_ids.is_empty()).then_some(space_ids),
        excluded_statuses: strings(map.get("excludedStatuses")),
        // Scopes saved before delivered statuses existed (or a stored `null`) add none.
        delivered_statuses: strings(map.get("deliveredStatuses")),
        // Scopes saved before excluded members existed (or a stored `null`) exclude nobody.
        excluded_members: parse_excluded_members(map.get("excludedMembers")),
    })
}

fn parse_points(value: Option<&Value>) -> Points {
    let mut points = DEFAULT_TICKET_POINTS;
    if let Some(Value::Object(map)) = value {
        for level in LEVELS {
            if let Some(n) = map.get(level.as_str()).and_then(Value::as_f64) {
                if n.is_finite() && n >= 0.0 {
                    points[level] = n;
                }
            }
        }
    }
    points
}

fn parse_percent(value: Option<&Value>) -> f64 {
    match value.and_then(Value::as_f64) {
        Some(n) if n.is_finite() => n.clamp(0.0, 100.0),
        _ => 100.0,
    }
}

/// The saved settings; a missing or malformed value reads as the defaults, field by field.
pub fn read_settings(text: Option<&str>) -> TicketBoardSettings {
    let saved: Value = text
        .and_then(|t| serde_json::from_str(t).ok())
        .unwrap_or(Value::Null);
    let empty = Map::new();
    let obj = saved.as_object().unwrap_or(&empty);
    TicketBoardSettings {
        scope: parse_scope(obj.get("scope")),
        points: parse_points(obj.get("points")),
        percent: parse_percent(obj.get("percent")),
    }
}

/// The text a settings value is saved as: what `JSON.stringify` writes.
pub fn write_settings(settings: &TicketBoardSettings) -> String {
    serde_json::to_string(settings).expect("settings serialize")
}

/// The saved range. There is no 7d here (tickets close too rarely), so it reads as the month.
pub fn read_range(text: Option<&str>, now: &DateTime<Local>) -> PeriodPick {
    match parse_pick(text, now) {
        Some(pick) if pick.kind != PeriodKind::Days7 => pick,
        _ => PeriodPick {
            kind: PeriodKind::Month,
            id: None,
        },
    }
}

pub fn write_range(pick: &PeriodPick) -> String {
    serialize_pick(pick)
}

/// The native value wins over the desktop one, even when it does not parse.
pub(crate) fn pick_text<'a>(native: Option<&'a str>, desktop: Option<&'a str>) -> Option<&'a str> {
    native.or(desktop)
}

pub fn load_settings(store: &Store) -> TicketBoardSettings {
    let native = store.setting(NATIVE_SETTINGS).ok().flatten();
    let desktop = desktop_value(store, SETTINGS_KEY);
    read_settings(pick_text(native.as_deref(), desktop.as_deref()))
}

pub fn save_settings(store: &Store, settings: &TicketBoardSettings) -> Result<(), String> {
    store.save_setting(NATIVE_SETTINGS, &write_settings(settings))
}

pub fn load_range(store: &Store, now: &DateTime<Local>) -> PeriodPick {
    let native = store.setting(NATIVE_RANGE).ok().flatten();
    let desktop = desktop_value(store, RANGE_KEY);
    read_range(pick_text(native.as_deref(), desktop.as_deref()), now)
}

pub fn save_range(store: &Store, pick: &PeriodPick) -> Result<(), String> {
    store.save_setting(NATIVE_RANGE, &write_range(pick))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contributors::model::period::PeriodKind;
    use crate::contributors::model::test_fixtures::local_at;
    use crate::contributors::tickets::test_fixtures::strings;
    use crate::contributors::tickets::tickets::{ByLevel, Level, DEFAULT_TICKET_POINTS};
    use crate::store::Store;
    use serde_json::{json, Value};
    use std::path::Path;

    fn now() -> DateTime<Local> {
        local_at(2026, 9, 15, 12, 0)
    }

    fn pick(kind: PeriodKind, id: Option<&str>) -> PeriodPick {
        PeriodPick {
            kind,
            id: id.map(String::from),
        }
    }

    fn defaults() -> TicketBoardSettings {
        TicketBoardSettings {
            scope: None,
            points: DEFAULT_TICKET_POINTS,
            percent: 100.0,
        }
    }

    fn read(saved: &Value) -> TicketBoardSettings {
        read_settings(Some(&saved.to_string()))
    }

    fn member(id: &str, name: &str) -> ExcludedMember {
        ExcludedMember {
            id: id.into(),
            name: name.into(),
        }
    }

    #[test]
    fn settings_use_the_monocode_prefix_keys() {
        assert_eq!(SETTINGS_KEY, "monocode.ticketContributors.settings");
        assert_eq!(RANGE_KEY, "monocode.ticketContributors.range");
        assert_eq!(NATIVE_SETTINGS, "native-ticket-contributors-settings");
        assert_eq!(NATIVE_RANGE, "native-ticket-contributors-range");
    }

    #[test]
    fn settings_default_when_nothing_is_saved() {
        assert_eq!(read_settings(None), defaults());
    }

    #[test]
    fn settings_round_trip() {
        let saved = TicketBoardSettings {
            scope: Some(WorkspaceScope {
                workspace_id: "w1".into(),
                name: "Workspace".into(),
                space_ids: Some(strings(&["s1", "s2"])),
                excluded_statuses: strings(&["cancelled"]),
                delivered_statuses: strings(&["ready for testing"]),
                excluded_members: vec![member("u1", "Una"), member("u2", "Uri")],
            }),
            points: ByLevel([1.0, 2.0, 4.0, 8.0, 16.0]),
            percent: 40.0,
        };
        let text = write_settings(&saved);
        assert_eq!(read_settings(Some(&text)), saved);
        // The same text `JSON.stringify` writes, so the desktop app reads it back.
        assert_eq!(
            text,
            r#"{"scope":{"workspaceId":"w1","name":"Workspace","spaceIds":["s1","s2"],"excludedStatuses":["cancelled"],"deliveredStatuses":["ready for testing"],"excludedMembers":[{"id":"u1","name":"Una"},{"id":"u2","name":"Uri"}]},"points":{"XS":1,"S":2,"M":4,"L":8,"XL":16},"percent":40}"#
        );
        assert_eq!(
            write_settings(&defaults()),
            r#"{"scope":null,"points":{"XS":3,"S":8,"M":16,"L":32,"XL":48},"percent":100}"#
        );
    }

    #[test]
    fn settings_fall_back_to_defaults_on_malformed_json() {
        for bad in ["{not json", "null", "[]", "\"text\"", "42"] {
            assert_eq!(read_settings(Some(bad)), defaults(), "{bad}");
        }
    }

    #[test]
    fn settings_fall_back_per_field_when_a_field_is_invalid() {
        let got = read(&json!({
            "scope": { "workspaceId": 7 },
            "points": { "XS": "x", "S": -1, "M": 20 },
            "percent": "lots",
        }));
        let mut points = DEFAULT_TICKET_POINTS;
        points[Level::M] = 20.0;
        assert_eq!(
            got,
            TicketBoardSettings {
                scope: None,
                points,
                percent: 100.0
            }
        );
    }

    #[test]
    fn settings_clamp_the_saved_percent_into_0_to_100() {
        assert_eq!(read(&json!({ "percent": 250 })).percent, 100.0);
        assert_eq!(read(&json!({ "percent": -5 })).percent, 0.0);
    }

    #[test]
    fn settings_read_empty_or_all_blank_space_ids_as_none() {
        for space_ids in [json!([]), json!(["", "  "])] {
            let got = read(&json!({
                "scope": {
                    "workspaceId": "w1",
                    "name": "Workspace",
                    "spaceIds": space_ids,
                    "excludedStatuses": [],
                },
            }));
            assert_eq!(
                got.scope,
                Some(WorkspaceScope {
                    workspace_id: "w1".into(),
                    name: "Workspace".into(),
                    space_ids: None,
                    excluded_statuses: vec![],
                    delivered_statuses: vec![],
                    excluded_members: vec![],
                })
            );
        }
    }

    #[test]
    fn settings_keep_the_non_blank_space_ids() {
        let got = read(&json!({
            "scope": {
                "workspaceId": "w1",
                "name": "Workspace",
                "spaceIds": [" s1 ", "", "s2"],
                "excludedStatuses": ["x"],
            },
        }));
        assert_eq!(got.scope.unwrap().space_ids, Some(strings(&["s1", "s2"])));
    }

    fn scope_with(extra: Value) -> WorkspaceScope {
        let mut scope = json!({
            "workspaceId": "w1",
            "name": "Workspace",
            "spaceIds": null,
            "excludedStatuses": [],
        });
        for (key, value) in extra.as_object().unwrap() {
            scope[key] = value.clone();
        }
        read(&json!({ "scope": scope })).scope.unwrap()
    }

    #[test]
    fn delivered_statuses_read_an_old_scope_without_them_as_an_empty_list() {
        assert!(scope_with(json!({})).delivered_statuses.is_empty());
    }

    #[test]
    fn delivered_statuses_migrate_a_stored_null_to_an_empty_list() {
        assert!(scope_with(json!({ "deliveredStatuses": null }))
            .delivered_statuses
            .is_empty());
    }

    #[test]
    fn delivered_statuses_read_a_non_array_value_as_an_empty_list() {
        for bad in [json!("done"), json!(7), json!({}), json!(true)] {
            assert!(
                scope_with(json!({ "deliveredStatuses": bad }))
                    .delivered_statuses
                    .is_empty(),
                "{bad}"
            );
        }
    }

    #[test]
    fn delivered_statuses_keep_a_saved_list_dropping_non_strings_and_an_empty_list_stays_empty() {
        assert_eq!(
            scope_with(json!({ "deliveredStatuses": ["a", 3, "b"] })).delivered_statuses,
            strings(&["a", "b"])
        );
        assert!(scope_with(json!({ "deliveredStatuses": [] }))
            .delivered_statuses
            .is_empty());
    }

    #[test]
    fn excluded_members_read_an_old_scope_without_them_as_an_empty_list() {
        assert!(scope_with(json!({})).excluded_members.is_empty());
    }

    #[test]
    fn excluded_members_migrate_a_stored_null_and_non_array_values_to_an_empty_list() {
        for bad in [json!(null), json!("u1"), json!(7), json!({}), json!(true)] {
            assert!(
                scope_with(json!({ "excludedMembers": bad }))
                    .excluded_members
                    .is_empty(),
                "{bad}"
            );
        }
    }

    #[test]
    fn excluded_members_read_old_bare_ids_as_members_named_by_their_id_dropping_junk() {
        let got = scope_with(json!({ "excludedMembers": ["u1", 3, "u2", "u1", "", null] }));
        assert_eq!(
            got.excluded_members,
            [member("u1", "u1"), member("u2", "u2")]
        );
    }

    #[test]
    fn excluded_members_keep_stored_names_and_fall_back_to_the_id_for_a_blank_or_missing_one() {
        let got = scope_with(json!({
            "excludedMembers": [
                { "id": "u1", "name": " Una " },
                { "id": "u2", "name": "" },
                { "id": "u3" },
                { "name": "No id" },
            ],
        }));
        assert_eq!(
            got.excluded_members,
            [member("u1", "Una"), member("u2", "u2"), member("u3", "u3")]
        );
    }

    #[test]
    fn range_defaults_to_the_current_month() {
        assert_eq!(read_range(None, &now()), pick(PeriodKind::Month, None));
    }

    #[test]
    fn range_round_trips_a_kind_and_a_pinned_period() {
        for p in [
            pick(PeriodKind::Month, None),
            pick(PeriodKind::Month, Some("2026-08")),
            pick(PeriodKind::Quarter, Some("2026-Q2")),
            pick(PeriodKind::Year, Some("2025")),
            pick(PeriodKind::All, None),
        ] {
            assert_eq!(read_range(Some(&write_range(&p)), &now()), p);
        }
    }

    #[test]
    fn range_reads_a_saved_7d_as_month() {
        assert_eq!(
            read_range(Some("7d"), &now()),
            pick(PeriodKind::Month, None)
        );
    }

    #[test]
    fn range_falls_back_to_month_on_garbage() {
        assert_eq!(
            read_range(Some("fortnight"), &now()),
            pick(PeriodKind::Month, None)
        );
    }

    // not in the TS suite: the native settings win over the desktop values

    #[test]
    fn a_native_value_wins_over_the_desktop_one() {
        let desktop = json!({ "percent": 10 }).to_string();
        let native = json!({ "percent": 20 }).to_string();
        assert_eq!(
            pick_text(Some(&native), Some(&desktop)),
            Some(native.as_str())
        );
        assert_eq!(pick_text(None, Some(&desktop)), Some(desktop.as_str()));
        assert_eq!(pick_text(None, None), None);
    }

    #[test]
    fn a_garbage_native_value_does_not_fall_back_to_the_desktop_one() {
        let desktop = json!({ "percent": 10 }).to_string();
        assert_eq!(
            read_settings(pick_text(Some("{nope"), Some(&desktop))),
            defaults()
        );
    }

    #[test]
    fn a_pinned_future_range_reads_as_month() {
        assert_eq!(
            read_range(Some("month:2027-01"), &now()),
            pick(PeriodKind::Month, None)
        );
    }

    #[test]
    fn the_store_round_trips_the_settings_and_the_range() {
        let store = Store::open(Path::new(":memory:")).unwrap();
        assert_eq!(load_settings(&store), defaults());
        assert_eq!(load_range(&store, &now()), pick(PeriodKind::Month, None));
        let mut saved = defaults();
        saved.percent = 55.0;
        save_settings(&store, &saved).unwrap();
        save_range(&store, &pick(PeriodKind::Quarter, Some("2026-Q2"))).unwrap();
        assert_eq!(load_settings(&store), saved);
        assert_eq!(
            store.setting(NATIVE_RANGE).unwrap().as_deref(),
            Some("quarter:2026-Q2")
        );
        assert_eq!(
            load_range(&store, &now()),
            pick(PeriodKind::Quarter, Some("2026-Q2"))
        );
    }
}
