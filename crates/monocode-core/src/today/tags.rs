//! Tags: one app wide, ordered list of named, colored labels. A task holds the
//! ids of its tags ([`super::Task::tags`]); names and colors live here only, so
//! a rename or recolor shows everywhere at once. Stored as a setting, like the
//! workflow statuses.
//!
//! Tasks saved before tags had definitions hold free text names. The store
//! turns those into definitions once (see `Store::today_tags`), and
//! [`resolve`] still reads a leftover name by matching it ignoring case, so a
//! past day record written before the migration shows its tags too.
use serde::{Deserialize, Serialize};

pub use super::statuses::{format_color, parse_color};

/// Longest tag name, in characters.
pub const MAX_NAME: usize = 40;
/// Most tags one task can hold.
pub const MAX_PER_TASK: usize = 20;
/// Ids of tags start with this; a task entry without it is a legacy name.
pub const ID_PREFIX: &str = "tag-";

/// The preset swatches the Tags dialog offers; new and migrated tags cycle
/// through them.
pub const PRESETS: [u32; 5] = [0x4da3f5, 0x10b981, 0xe5a23c, 0xf472b6, 0xa78bfa];

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Tag {
    /// `tag-<uuid>`.
    pub id: String,
    pub name: String,
    /// `#rrggbb`, lowercase.
    pub color: String,
}
impl Tag {
    /// The color as `0xRRGGBB`; a malformed one reads as the first preset.
    pub fn rgb(&self) -> u32 {
        parse_color(&self.color).unwrap_or(PRESETS[0])
    }
}

/// What a tag edit may change. Order changes through a move.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TagPatch {
    pub name: Option<String>,
    /// `#rrggbb`.
    pub color: Option<String>,
}

/// The preset for the `n`th tag created, cycling.
pub fn preset_for(n: usize) -> u32 {
    PRESETS[n % PRESETS.len()]
}

/// Repairs a stored list: drops entries with an empty or repeated id and ones
/// whose name is empty or repeats an earlier one ignoring case, trims names
/// and fixes malformed colors. Keeps the order. Never fails.
pub fn normalize(list: Vec<Tag>) -> Vec<Tag> {
    let mut out: Vec<Tag> = Vec::new();
    for (index, mut tag) in list.into_iter().enumerate() {
        tag.name = tag.name.trim().to_string();
        if tag.id.is_empty()
            || tag.name.is_empty()
            || out
                .iter()
                .any(|kept| kept.id == tag.id || same_name(&kept.name, &tag.name))
        {
            continue;
        }
        tag.color = match parse_color(tag.color.trim()) {
            Some(rgb) => format_color(rgb),
            None => format_color(preset_for(index)),
        };
        out.push(tag);
    }
    out
}

/// The tags a task's entries name, in the order of `defs`. An entry is an id;
/// an entry that is no known id but matches a name ignoring case (a legacy free
/// text tag) reads as that tag. Anything else is skipped. No duplicates.
pub fn resolve(entries: &[String], defs: &[Tag]) -> Vec<Tag> {
    let mut out: Vec<Tag> = Vec::new();
    for def in defs {
        let named = entries.iter().any(|entry| {
            entry == &def.id || (!is_id(entry) && same_name(&legacy_key(entry), &def.name))
        });
        if named {
            out.push(def.clone());
        }
    }
    out
}

/// Whether a task entry is a tag id: `tag-` and a UUID. Anything else is a
/// legacy free text name, even one that merely starts with `tag-`.
pub fn is_id(entry: &str) -> bool {
    entry
        .strip_prefix(ID_PREFIX)
        .is_some_and(|rest| uuid::Uuid::parse_str(rest).is_ok())
}

/// A legacy entry as a definition name is stored: trimmed and cut to
/// [`MAX_NAME`] characters.
pub fn legacy_key(entry: &str) -> String {
    let cut: String = entry.trim().chars().take(MAX_NAME).collect();
    cut.trim().to_string()
}

/// Two tag names are the same when they match trimmed, ignoring case.
pub fn same_name(a: &str, b: &str) -> bool {
    a.trim().to_lowercase() == b.trim().to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tag(id: &str, name: &str, color: &str) -> Tag {
        Tag {
            id: id.into(),
            name: name.into(),
            color: color.into(),
        }
    }

    #[test]
    fn normalize_drops_empty_and_repeated_ids_and_names() {
        let list = vec![
            tag("tag-1", "Bug", "#4da3f5"),
            tag("", "NoId", "#4da3f5"),
            tag("tag-1", "Again", "#4da3f5"),
            tag("tag-2", "  ", "#4da3f5"),
            tag("tag-3", " bug ", "#4da3f5"),
            tag("tag-4", "UI", "#10b981"),
        ];
        let got = normalize(list);
        let names: Vec<_> = got.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, ["Bug", "UI"]);
    }

    #[test]
    fn normalize_trims_names_and_fixes_colors() {
        let got = normalize(vec![
            tag("tag-1", "  Bug ", "#ABCDEF"),
            tag("tag-2", "UI", "red"),
            tag("tag-3", "Docs", " #10B981 "),
        ]);
        assert_eq!(got[0].name, "Bug");
        assert_eq!(got[0].color, "#abcdef");
        assert_eq!(got[1].color, format_color(preset_for(1)));
        assert_eq!(got[2].color, "#10b981");
    }

    #[test]
    fn normalize_keeps_order() {
        let got = normalize(vec![
            tag("tag-b", "B", "#000000"),
            tag("tag-a", "A", "#000000"),
        ]);
        assert_eq!(got[0].id, "tag-b");
        assert_eq!(got[1].id, "tag-a");
    }

    #[test]
    fn only_tag_followed_by_a_uuid_is_an_id() {
        let id = format!("{ID_PREFIX}{}", uuid::Uuid::new_v4());
        assert!(is_id(&id));
        assert!(!is_id("tag-cleanup"));
        assert!(!is_id("tag-"));
        assert!(!is_id("Bug"));
        let defs = vec![tag("tag-1", "tag-cleanup", "#000000")];
        let got = resolve(&["Tag-Cleanup".to_string()], &defs);
        assert_eq!(got.len(), 1);
    }

    #[test]
    fn long_legacy_names_resolve_through_the_same_cut() {
        let long = "x".repeat(60);
        let defs = vec![tag("tag-1", &"x".repeat(MAX_NAME), "#000000")];
        assert_eq!(resolve(&[long], &defs).len(), 1);
    }

    #[test]
    fn resolve_reads_ids_and_legacy_names_in_definition_order() {
        let defs = vec![
            tag("tag-1", "Bug", "#000000"),
            tag("tag-2", "UI", "#000000"),
        ];
        let entries = vec![
            "ui".to_string(),
            "tag-1".to_string(),
            "tag-gone".to_string(),
        ];
        let got = resolve(&entries, &defs);
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].id, "tag-1");
        assert_eq!(got[1].id, "tag-2");
    }
}
