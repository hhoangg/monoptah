//! Saved Contributors preferences. The Tauri app keeps them in localStorage
//! under `monocode.*` keys that are never renamed; the native app reads those
//! as defaults and saves its own changes under `native-contributors-*`.

use super::period::{period_from_id, PeriodKind, PeriodPick};
use crate::store::Store;
use chrono::{DateTime, Local};

// The `monocode.` prefix is deliberately kept: renaming it would drop every
// saved preference on existing installs.
pub const RANGE_KEY: &str = "monocode.contributors.range";
pub const MERGE_KEY: &str = "monocode.contributors.mergeIdentities";
pub const NATIVE_RANGE_SETTING: &str = "native-contributors-range";
pub const NATIVE_MERGE_SETTING: &str = "native-contributors-merge";

/// What a saved preference holds: `<kind>` (follows the current period) or `<kind>:<id>`.
pub fn serialize_pick(pick: &PeriodPick) -> String {
    match &pick.id {
        None => pick.kind.as_str().into(),
        Some(id) => format!("{}:{id}", pick.kind.as_str()),
    }
}

/// The pick a saved text stands for, or `None` when it is not one. That
/// covers the removed ranges (`30d`, `custom:...`, `90d`, `1y`), a pinned id
/// that does not parse, and one that starts after today.
pub fn parse_pick(value: Option<&str>, now: &DateTime<Local>) -> Option<PeriodPick> {
    let value = value.filter(|v| !v.is_empty())?;
    let (kind, id) = match value.split_once(':') {
        Some((kind, id)) => (kind, Some(id)),
        None => (value, None),
    };
    let kind = PeriodKind::parse(kind)?;
    let Some(id) = id else {
        return Some(PeriodPick { kind, id: None });
    };
    // The last 7 days always end today and All time always starts at the first
    // commit, so there is nothing to pin.
    if matches!(kind, PeriodKind::Days7 | PeriodKind::All) {
        return None;
    }
    period_from_id(kind, id, now).map(|_| PeriodPick {
        kind,
        id: Some(id.into()),
    })
}

/// A saved value that is no longer a period reads as the last 7 days. The
/// native value wins over the desktop one.
pub fn read_period_pick(
    native: Option<&str>,
    desktop: Option<&str>,
    now: &DateTime<Local>,
) -> PeriodPick {
    parse_pick(native.or(desktop), now).unwrap_or(PeriodPick {
        kind: PeriodKind::Days7,
        id: None,
    })
}

/// Anything except the text "false" means on.
pub fn read_merge(native: Option<&str>, desktop: Option<&str>) -> bool {
    native.or(desktop) != Some("false")
}

pub fn merge_text(merge: bool) -> &'static str {
    if merge {
        "true"
    } else {
        "false"
    }
}

pub(crate) fn desktop_value(store: &Store, key: &str) -> Option<String> {
    store
        .desktop_preferences()
        .and_then(|prefs| prefs.get(key))
        .map(String::from)
}

pub fn load_period_pick(store: &Store, now: &DateTime<Local>) -> PeriodPick {
    let native = store.setting(NATIVE_RANGE_SETTING).ok().flatten();
    let desktop = desktop_value(store, RANGE_KEY);
    read_period_pick(native.as_deref(), desktop.as_deref(), now)
}

pub fn save_period_pick(store: &Store, pick: &PeriodPick) -> Result<(), String> {
    store.save_setting(NATIVE_RANGE_SETTING, &serialize_pick(pick))
}

pub fn load_merge(store: &Store) -> bool {
    let native = store.setting(NATIVE_MERGE_SETTING).ok().flatten();
    let desktop = desktop_value(store, MERGE_KEY);
    read_merge(native.as_deref(), desktop.as_deref())
}

pub fn save_merge(store: &Store, merge: bool) -> Result<(), String> {
    store.save_setting(NATIVE_MERGE_SETTING, merge_text(merge))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contributors::model::test_fixtures::local_at;
    use crate::store::Store;
    use std::path::Path;

    fn now() -> chrono::DateTime<chrono::Local> {
        local_at(2026, 9, 15, 12, 0)
    }

    fn pick(kind: PeriodKind, id: Option<&str>) -> PeriodPick {
        PeriodPick {
            kind,
            id: id.map(String::from),
        }
    }

    fn memory_store() -> Store {
        Store::open(Path::new(":memory:")).unwrap()
    }

    #[test]
    fn prefs_default_to_7d_and_merging_on() {
        assert_eq!(
            read_period_pick(None, None, &now()),
            pick(PeriodKind::Days7, None)
        );
        assert!(read_merge(None, None));
    }

    #[test]
    fn prefs_use_the_monocode_prefix_keys() {
        assert_eq!(RANGE_KEY, "monocode.contributors.range");
        assert_eq!(MERGE_KEY, "monocode.contributors.mergeIdentities");
        assert_eq!(NATIVE_RANGE_SETTING, "native-contributors-range");
        assert_eq!(NATIVE_MERGE_SETTING, "native-contributors-merge");
    }

    #[test]
    fn prefs_store_a_kind_that_follows_the_current_period_as_the_bare_kind() {
        for kind in [
            PeriodKind::Days7,
            PeriodKind::Month,
            PeriodKind::Quarter,
            PeriodKind::Year,
            PeriodKind::All,
        ] {
            let text = serialize_pick(&pick(kind, None));
            assert_eq!(text, kind.as_str());
            assert_eq!(
                read_period_pick(Some(&text), None, &now()),
                pick(kind, None)
            );
        }
    }

    #[test]
    fn prefs_store_a_pinned_period_as_kind_colon_id_and_read_it_back() {
        for (kind, id, text) in [
            (PeriodKind::Month, "2026-08", "month:2026-08"),
            (PeriodKind::Quarter, "2026-Q2", "quarter:2026-Q2"),
            (PeriodKind::Year, "2025", "year:2025"),
        ] {
            let p = pick(kind, Some(id));
            assert_eq!(serialize_pick(&p), text);
            assert_eq!(read_period_pick(Some(text), None, &now()), p);
        }
    }

    #[test]
    fn prefs_fall_back_to_7d_for_the_removed_ranges() {
        for removed in [
            "30d",
            "90d",
            "1y",
            "forever",
            "custom",
            "custom:2026-10-01:2026-10-10",
        ] {
            assert_eq!(
                read_period_pick(Some(removed), None, &now()),
                pick(PeriodKind::Days7, None),
                "{removed}"
            );
        }
    }

    #[test]
    fn prefs_read_all_as_all_time_which_has_no_id_to_pin() {
        assert_eq!(
            read_period_pick(Some("all"), None, &now()),
            pick(PeriodKind::All, None)
        );
        for pinned in ["all:all", "all:2026", "all:"] {
            assert_eq!(
                read_period_pick(Some(pinned), None, &now()),
                pick(PeriodKind::Days7, None),
                "{pinned}"
            );
        }
    }

    #[test]
    fn prefs_fall_back_to_7d_for_garbage_and_unusable_ids() {
        for bad in [
            "forever",
            "",
            ":",
            "month:",
            "month:2026-13",
            "month:2026-Q3",
            "quarter:2026-Q9",
            "year:soon",
            "7d:7d",
            "7d:x",
            "month:2027-01",
            "Month",
        ] {
            assert_eq!(
                read_period_pick(Some(bad), None, &now()),
                pick(PeriodKind::Days7, None),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn prefs_serialize_and_parse_symmetrically() {
        let p = pick(PeriodKind::Quarter, Some("2026-Q1"));
        assert_eq!(parse_pick(Some(&serialize_pick(&p)), &now()), Some(p));
        assert_eq!(parse_pick(None, &now()), None);
    }

    #[test]
    fn prefs_round_trip_the_merge_toggle() {
        assert!(!read_merge(Some("false"), None));
        assert!(read_merge(Some("true"), None));
        assert!(read_merge(Some("maybe"), None));
        assert_eq!(merge_text(false), "false");
        assert_eq!(merge_text(true), "true");
    }

    // not in the TS suite: the native setting wins over the desktop value

    #[test]
    fn a_native_value_wins_over_the_desktop_one() {
        assert_eq!(
            read_period_pick(Some("year"), Some("month"), &now()),
            pick(PeriodKind::Year, None)
        );
        assert_eq!(
            read_period_pick(None, Some("month"), &now()),
            pick(PeriodKind::Month, None)
        );
        assert!(!read_merge(Some("false"), Some("true")));
        assert!(read_merge(Some("true"), Some("false")));
        assert!(!read_merge(None, Some("false")));
    }

    #[test]
    fn the_store_round_trips_the_pick_and_the_merge_toggle() {
        let store = memory_store();
        assert_eq!(
            load_period_pick(&store, &now()),
            pick(PeriodKind::Days7, None)
        );
        assert!(load_merge(&store));
        save_period_pick(&store, &pick(PeriodKind::Month, Some("2026-08"))).unwrap();
        save_merge(&store, false).unwrap();
        assert_eq!(
            store.setting(NATIVE_RANGE_SETTING).unwrap().as_deref(),
            Some("month:2026-08")
        );
        assert_eq!(
            load_period_pick(&store, &now()),
            pick(PeriodKind::Month, Some("2026-08"))
        );
        assert!(!load_merge(&store));
        save_merge(&store, true).unwrap();
        assert!(load_merge(&store));
    }
}
