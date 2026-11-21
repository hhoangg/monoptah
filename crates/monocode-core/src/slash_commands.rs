//! The `/` menu of a session composer: Monoptah's own commands and the skills of the project.
//!
//! The native composer (`monocode-gpui`, `composer_completion.rs`) and a paired web device
//! (`commands.list`) build their list from the same table and the same rules, so the two
//! menus cannot drift apart: what a command is called, what it says, in which order it comes
//! and when a skill or command is left out.
use crate::{provider::Kind, skills::DiscoveredSkill};
use serde::Serialize;
use std::collections::BTreeMap;

/// One of Monoptah's own commands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Builtin {
    pub id: &'static str,
    pub label: &'static str,
    pub detail: &'static str,
    /// A paired web device runs it. The others need the desktop's own pickers (an MCP server,
    /// the git panel, an inline visual) and stay on the desktop.
    pub web: bool,
}

/// The built-in commands in the order the bare `/` menu shows them.
pub const BUILTINS: &[Builtin] = &[
    Builtin {
        id: "draft",
        label: "Draft",
        detail: "Save this message without starting the agent",
        web: true,
    },
    Builtin {
        id: "orchestrator",
        label: "Orchestrator",
        detail: "Plan and coordinate agent work",
        web: true,
    },
    Builtin {
        id: "operator",
        label: "Operator",
        detail: "Give this thread access to Monoptah",
        web: true,
    },
    Builtin {
        id: "plan",
        label: "Plan mode",
        detail: "Review a plan before building",
        web: true,
    },
    Builtin {
        id: "btw",
        label: "Side question",
        detail: "Ask a read-only side question about the current turn",
        web: true,
    },
    Builtin {
        id: "compact",
        label: "Compact context",
        detail: "Summarize older conversation context to free space",
        web: true,
    },
    Builtin {
        id: "add-to-folder",
        label: "Add to folder",
        detail: "Place this session in an existing or new sidebar folder",
        web: true,
    },
    Builtin {
        id: "mcp",
        label: "MCP server",
        detail: "Find an MCP server for this message",
        web: false,
    },
    Builtin {
        id: "stage",
        label: "Stage",
        detail: "Stage only this session's changes",
        web: false,
    },
    Builtin {
        id: "commit",
        label: "Commit",
        detail: "Stage and commit this session's changes",
        web: false,
    },
    Builtin {
        id: "visualize",
        label: "Visualize",
        detail: "Create an inline visual of this topic or a specific request",
        web: false,
    },
];

/// The commands that switch a mode of the next message instead of running at once.
pub const MODES: &[&str] = &["draft", "plan", "operator", "orchestrator"];

/// Skill names that would shadow a command of the app: never offered as skills.
const RESERVED_SKILLS: &[&str] = &[
    "plan",
    "compact",
    "mcp",
    "operator",
    "orchestrator",
    "add-to-folder",
    "draft",
    "btw",
    "mono",
    "monocode",
];

/// Whether `id` is a built-in the composer lists for `provider`. `has_user_command` says
/// whether a skill or provider command of that name exists, which wins over `stage` and
/// `commit`. A side question is not offered by providers that cannot run one.
pub fn builtin_listed(id: &str, provider: Kind, has_user_command: impl Fn(&str) -> bool) -> bool {
    if matches!(id, "stage" | "commit") && has_user_command(id) {
        return false;
    }
    !(id == "btw" && matches!(provider, Kind::Fx | Kind::Hermes | Kind::Antigravity))
}

/// Whether a skill of this name is offered in the `/` menu.
pub fn skill_listed(name: &str) -> bool {
    !RESERVED_SKILLS.contains(&name)
}

/// What kind of row an entry is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum EntryKind {
    Builtin,
    Skill,
}

/// One row of the menu, as a paired device receives it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    pub id: String,
    pub kind: EntryKind,
    pub label: String,
    pub detail: String,
    /// A name for the device's icon set: the command id, or `skill`.
    pub icon: String,
    /// Whether it can be used now. A device that may only read gets every row disabled.
    pub available: bool,
    /// Why it cannot, in a plain sentence.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// A mode of the next message (Draft, Plan, Operator, Orchestrator).
    pub mode: bool,
    /// The mode is on for this session now.
    pub active: bool,
}

/// What the desktop knows about the conversation the menu is for.
#[derive(Clone, Debug, Default)]
pub struct Context {
    pub provider: Option<Kind>,
    /// The device may only read: every row is disabled.
    pub read_only: bool,
    /// Why a built-in is unavailable now, by id (busy turn, nothing sent yet, remote project).
    pub blocked: BTreeMap<String, String>,
    /// The modes that are on now.
    pub active: Vec<String>,
}

/// The reason shown to a device that cannot control.
pub const READ_ONLY_REASON: &str = "This device can only read on this desktop";

/// The menu for `context`: the web built-ins in the bare `/` order, then the skills by name.
pub fn list(context: &Context, skills: &[DiscoveredSkill]) -> Vec<Entry> {
    let provider = context.provider.unwrap_or(Kind::Claude);
    // Only the skills count here, unlike the native menu, which also counts commands the provider
    // discovered. The difference is harmless: `has_user_command` only hides `stage` and `commit`
    // (a skill or provider command of that name wins), and neither is a web command.
    let has_user_command = |name: &str| {
        skills.iter().any(|skill| {
            skill
                .name
                .trim_start_matches('/')
                .eq_ignore_ascii_case(name)
        })
    };
    let state = |id: &str| -> (bool, Option<String>) {
        if context.read_only {
            return (false, Some(READ_ONLY_REASON.into()));
        }
        match context.blocked.get(id) {
            Some(reason) => (false, Some(reason.clone())),
            None => (true, None),
        }
    };
    let mut entries: Vec<Entry> = BUILTINS
        .iter()
        .filter(|builtin| builtin.web && builtin_listed(builtin.id, provider, has_user_command))
        .map(|builtin| {
            let (available, reason) = state(builtin.id);
            Entry {
                id: builtin.id.into(),
                kind: EntryKind::Builtin,
                label: builtin.label.into(),
                detail: builtin.detail.into(),
                icon: builtin.id.into(),
                available,
                reason,
                mode: MODES.contains(&builtin.id),
                active: context.active.iter().any(|id| id == builtin.id),
            }
        })
        .collect();
    let mut found: Vec<&DiscoveredSkill> = skills
        .iter()
        .filter(|skill| skill_listed(&skill.name))
        .collect();
    found.sort_by(|a, b| a.name.cmp(&b.name));
    entries.extend(found.into_iter().map(|skill| {
        let (available, reason) = if context.read_only {
            (false, Some(READ_ONLY_REASON.to_owned()))
        } else {
            (true, None)
        };
        Entry {
            id: skill.name.trim_start_matches('/').into(),
            kind: EntryKind::Skill,
            label: skill.name.trim_start_matches('/').into(),
            detail: if skill.description.is_empty() {
                format!("{} \u{b7} {}", skill.scope, skill.source)
            } else {
                skill.description.clone()
            },
            icon: "skill".into(),
            available,
            reason,
            mode: false,
            active: false,
        }
    }));
    entries
}

#[cfg(test)]
mod tests {
    use super::*;

    fn skill(name: &str, description: &str) -> DiscoveredSkill {
        DiscoveredSkill {
            name: name.into(),
            description: description.into(),
            path: format!("/skills/{name}/SKILL.md"),
            scope: "project".into(),
            source: "agents".into(),
        }
    }

    fn ids(entries: &[Entry]) -> Vec<&str> {
        entries.iter().map(|entry| entry.id.as_str()).collect()
    }

    #[test]
    fn the_bare_menu_lists_the_web_commands_in_the_native_order_then_skills() {
        let entries = list(
            &Context::default(),
            &[skill("review", "Review a diff"), skill("deploy", "")],
        );
        assert_eq!(
            ids(&entries),
            [
                "draft",
                "orchestrator",
                "operator",
                "plan",
                "btw",
                "compact",
                "add-to-folder",
                "deploy",
                "review"
            ]
        );
        assert!(entries.iter().all(|entry| entry.available));
        assert_eq!(entries[3].label, "Plan mode");
        assert!(entries[3].mode && !entries[4].mode);
        assert_eq!(entries[7].detail, "project \u{b7} agents");
        assert_eq!(entries[8].detail, "Review a diff");
        assert_eq!(entries[8].icon, "skill");
    }

    #[test]
    fn the_native_picker_and_the_relay_share_one_table() {
        let native: Vec<_> = BUILTINS.iter().map(|builtin| builtin.id).collect();
        assert_eq!(
            native,
            [
                "draft",
                "orchestrator",
                "operator",
                "plan",
                "btw",
                "compact",
                "add-to-folder",
                "mcp",
                "stage",
                "commit",
                "visualize"
            ]
        );
    }

    #[test]
    fn a_skill_named_like_a_command_is_not_offered_and_a_provider_without_btw_loses_it() {
        let skills = [skill("plan", "x"), skill("mono", "x"), skill("lint", "x")];
        let entries = list(&Context::default(), &skills);
        assert_eq!(
            entries
                .iter()
                .filter(|e| e.kind == EntryKind::Skill)
                .count(),
            1
        );
        let hermes = list(
            &Context {
                provider: Some(Kind::Hermes),
                ..Default::default()
            },
            &[],
        );
        assert!(!ids(&hermes).contains(&"btw"));
        assert!(ids(&hermes).contains(&"compact"));
        assert!(builtin_listed("btw", Kind::Claude, |_| false));
        assert!(!builtin_listed("stage", Kind::Claude, |name| name == "stage"));
        assert!(builtin_listed("commit", Kind::Claude, |_| false));
    }

    #[test]
    fn blocked_commands_say_why_and_active_modes_are_marked() {
        let mut blocked = BTreeMap::new();
        blocked.insert("compact".to_owned(), "Wait for the turn to end".to_owned());
        let entries = list(
            &Context {
                blocked,
                active: vec!["plan".into()],
                ..Default::default()
            },
            &[],
        );
        let compact = entries.iter().find(|e| e.id == "compact").unwrap();
        assert!(!compact.available);
        assert_eq!(compact.reason.as_deref(), Some("Wait for the turn to end"));
        assert!(entries.iter().find(|e| e.id == "plan").unwrap().active);
        assert!(!entries.iter().find(|e| e.id == "draft").unwrap().active);
    }

    #[test]
    fn a_read_only_device_gets_every_row_disabled() {
        let entries = list(
            &Context {
                read_only: true,
                ..Default::default()
            },
            &[skill("lint", "x")],
        );
        assert!(!entries.is_empty());
        for entry in &entries {
            assert!(!entry.available, "{}", entry.id);
            assert_eq!(entry.reason.as_deref(), Some(READ_ONLY_REASON));
        }
    }

    #[test]
    fn an_entry_serializes_in_camel_case_without_an_empty_reason() {
        let entries = list(&Context::default(), &[]);
        let value = serde_json::to_value(&entries[0]).unwrap();
        assert_eq!(value["kind"], "builtin");
        assert_eq!(value["available"], true);
        assert!(value.get("reason").is_none());
    }
}
