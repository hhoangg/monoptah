use super::kit::*;
use super::kit_prefs::*;
use serde_json::json;
use std::path::PathBuf;

// Every value below is fake.
const FAKE_SECRET: &str = "fake-secret-ZZZZ9999";

fn skill(name: &str, scope: SkillScope, exclusion: Option<Exclusion>) -> KitSkill {
    KitSkill {
        name: name.into(),
        scope,
        source: KitSource::User,
        plugin: None,
        path: PathBuf::from("/x").join(name),
        size: 2048,
        hash: "h".into(),
        compressed: 100,
        exclusion,
        files: Vec::new(),
    }
}

fn server(name: &str, compat: McpCompat, secret: bool) -> KitMcpServer {
    KitMcpServer {
        name: name.into(),
        scope: McpScope::User,
        source: KitSource::User,
        config_path: PathBuf::from("/x/.claude.json"),
        transport: "stdio".into(),
        compat,
        secrets: if secret {
            vec![SecretField {
                location: SecretLocation::Env("API_KEY".into()),
                value: Secret::new(FAKE_SECRET),
            }]
        } else {
            Vec::new()
        },
        config_hash: "c".into(),
        secrets_digest: None,
    }
}

fn plugin(id: &str, exclusion: Option<Exclusion>, native: bool) -> KitPlugin {
    KitPlugin {
        id: id.into(),
        version: "1.0.0".into(),
        source: KitSource::User,
        install_path: PathBuf::from("/x/plugins").join(id),
        size: 4096,
        has_native_bin: native,
        hash: "h".into(),
        compressed: 200,
        exclusion,
        files: Vec::new(),
    }
}

fn fixture() -> KitInventory {
    KitInventory {
        skills: vec![
            skill("review", SkillScope::User, None),
            skill("local-only-draft", SkillScope::ProjectUntracked, None),
            skill("huge", SkillScope::User, Some(Exclusion::TooLarge)),
            // Travels with its plugin: no row of its own.
            skill("from-plugin", SkillScope::Plugin, None),
        ],
        mcp_servers: vec![
            server("fetch", McpCompat::NeedsRuntime(Runtime::Npx), false),
            server("github", McpCompat::Ok, true),
            server(
                "mine",
                McpCompat::LocalOnly(LocalOnlyReason::AbsoluteCommand),
                true,
            ),
            server("linear", McpCompat::OAuthStyle, false),
            server("repo", McpCompat::Project, false),
        ],
        plugins: vec![
            plugin("fmt@market", None, true),
            plugin("big@market", Some(Exclusion::TooLarge), false),
        ],
        ..Default::default()
    }
}

fn row<'a>(rows: &'a [KitRow], name: &str) -> &'a KitRow {
    rows.iter().find(|row| row.name == name).unwrap()
}

#[test]
fn defaults_check_everything_that_can_be_sent_secrets_and_oauth_style_servers() {
    let inventory = fixture();
    let prefs = CloudKitPrefs::default();
    assert_eq!(prefs.selection, None);

    let skills = rows(&inventory, &prefs, KitTab::Skills);
    assert_eq!(skills.len(), 3, "the plugin skill has no row");
    assert!(row(&skills, "review").checked);
    assert!(row(&skills, "local-only-draft").checked);
    assert!(!row(&skills, "huge").checked);

    let servers = rows(&inventory, &prefs, KitTab::McpServers);
    assert!(row(&servers, "github").checked, "secrets are on by default");
    assert!(
        row(&servers, "linear").checked,
        "OAuth-style servers are on by default"
    );
    assert!(row(&servers, "fetch").checked);
    assert!(row(&servers, "github").badges.contains(&Badge::HasSecrets));
    assert!(row(&servers, "linear")
        .badges
        .contains(&Badge::SignInInCloud));

    let plugins = rows(&inventory, &prefs, KitTab::Plugins);
    assert!(row(&plugins, "fmt@market").checked);
    assert!(row(&plugins, "fmt@market")
        .badges
        .contains(&Badge::MayNotRun));

    assert_eq!(
        KitSummary::of(&inventory, &prefs),
        KitSummary {
            skills: 2,
            mcp_servers: 3,
            plugins: 1
        }
    );
}

#[test]
fn badges_use_the_words_of_the_design() {
    let inventory = fixture();
    let servers = rows(&inventory, &CloudKitPrefs::default(), KitTab::McpServers);
    let label = |name: &str| -> Vec<String> {
        row(&servers, name)
            .badges
            .iter()
            .map(|b| b.label())
            .collect()
    };
    assert_eq!(label("github"), ["Works in the cloud", "Has secrets"]);
    assert_eq!(label("fetch"), ["Needs npx on the instance"]);
    assert_eq!(label("mine"), ["Local only - will not work", "Has secrets"]);
    assert_eq!(
        label("linear"),
        ["Works in the cloud", "Sign in in the cloud"]
    );
}

#[test]
fn local_only_items_are_not_selectable_and_say_why() {
    let inventory = fixture();
    let mut prefs = CloudKitPrefs::default();
    let servers = rows(&inventory, &prefs, KitTab::McpServers);
    let mine = row(&servers, "mine");
    assert!(!mine.selectable && !mine.checked);
    assert!(mine.reason.as_deref().is_some_and(|r| !r.is_empty()));
    assert!(
        !row(&servers, "repo").selectable,
        "project servers are information only"
    );

    // Ticking one does nothing.
    prefs.set_selected(&inventory, "mcp:user:mine", true);
    assert_eq!(prefs.selection, None);
    let servers = rows(&inventory, &prefs, KitTab::McpServers);
    assert!(!row(&servers, "mine").checked);

    // Even a stored id cannot make one checked.
    let forced = CloudKitPrefs {
        selection: Some(["mcp:user:mine".to_owned()].into()),
        ..Default::default()
    };
    let servers = rows(&inventory, &forced, KitTab::McpServers);
    assert!(!row(&servers, "mine").checked);
    // Only the untracked project skill counts: it follows its project, whatever is stored.
    assert_eq!(
        KitSummary::of(&inventory, &forced),
        KitSummary {
            skills: 1,
            ..Default::default()
        }
    );
}

#[test]
fn unchecking_turns_the_defaults_into_an_explicit_selection() {
    let inventory = fixture();
    let mut prefs = CloudKitPrefs::default();
    prefs.set_selected(&inventory, "mcp:user:github", false);
    let servers = rows(&inventory, &prefs, KitTab::McpServers);
    assert!(!row(&servers, "github").checked);
    assert!(row(&servers, "linear").checked);
    assert_eq!(KitSummary::of(&inventory, &prefs).mcp_servers, 2);

    prefs.set_selected(&inventory, "mcp:user:github", true);
    assert_eq!(KitSummary::of(&inventory, &prefs).mcp_servers, 3);

    prefs.reset_selection();
    assert_eq!(prefs.selection, None);
}

#[test]
fn the_notice_is_shown_once() {
    let mut prefs = CloudKitPrefs::default();
    assert!(!prefs.enabled && prefs.needs_notice());
    assert_eq!(prefs.request_enable(), Enable::NeedsNotice);
    assert!(!prefs.enabled, "not on until the notice is accepted");

    prefs.acknowledge_notice();
    assert!(prefs.enabled && !prefs.needs_notice());

    prefs.disable();
    assert_eq!(prefs.request_enable(), Enable::Enabled);
    assert!(prefs.enabled && prefs.notice_acknowledged);
}

#[test]
fn the_notice_text_is_the_designed_text() {
    assert_eq!(
        NOTICE,
        "Your MCP secrets will be copied to your cloud workspaces. They will be visible to the agent, to every MCP server and to every command the agent runs there. MCP servers that use OAuth sign-in are not copied with a sign-in: sign in to them in the cloud."
    );
}

#[test]
fn api_kit_none_disables_the_kit_and_default_cannot_widen_it() {
    let mut prefs = CloudKitPrefs::default();
    assert!(
        !prefs.applies(ApiKit::Default),
        "off until the user turns it on"
    );
    prefs.acknowledge_notice();
    assert!(prefs.applies(ApiKit::Default));
    assert!(!prefs.applies(ApiKit::None));
    prefs.disable();
    assert!(!prefs.applies(ApiKit::Default), "the API cannot turn it on");
}

#[test]
fn api_kit_accepts_only_none_and_default() {
    assert_eq!(ApiKit::parse(&json!("none")), Ok(ApiKit::None));
    assert_eq!(ApiKit::parse(&json!("default")), Ok(ApiKit::Default));
    for bad in [
        json!("all"),
        json!("None"),
        json!(true),
        json!(null),
        json!({"skills":["review"]}),
        json!(["mcp:user:github"]),
    ] {
        assert!(ApiKit::parse(&bad).is_err(), "{bad}");
    }
}

#[test]
fn nothing_secret_reaches_rows_debug_summaries_or_stored_preferences() {
    let inventory = fixture();
    let mut prefs = CloudKitPrefs::default();
    prefs.set_selected(&inventory, "mcp:user:fetch", false);
    let mut shown = format!("{prefs:?}");
    for tab in KitTab::ALL {
        shown.push_str(&format!("{:?}", rows(&inventory, &prefs, tab)));
    }
    shown.push_str(&KitSummary::of(&inventory, &prefs).text());
    shown.push_str(&serde_json::to_string(&prefs).unwrap());
    assert!(!shown.contains(FAKE_SECRET), "{shown}");
    assert_eq!(
        secret_field_names(&inventory.mcp_servers[1]),
        ["env API_KEY"]
    );
}

#[test]
fn summary_text_counts_and_pluralizes() {
    let summary = KitSummary {
        skills: 6,
        mcp_servers: 4,
        plugins: 3,
    };
    assert_eq!(summary.text(), "6 skills, 4 MCP servers, 3 plugins");
    let one = KitSummary {
        skills: 1,
        mcp_servers: 1,
        plugins: 1,
    };
    assert_eq!(one.text(), "1 skill, 1 MCP server, 1 plugin");
    assert_eq!(KitSummary::default().text(), "nothing selected");
}

#[test]
fn preferences_round_trip_and_old_files_get_the_defaults() {
    let old: CloudKitPrefs = serde_json::from_str("{}").unwrap();
    assert_eq!(old, CloudKitPrefs::default());
    let prefs = CloudKitPrefs {
        selection: Some(["skill:user:review".to_owned()].into()),
        enabled: true,
        notice_acknowledged: true,
    };
    let saved = serde_json::to_value(&prefs).unwrap();
    assert_eq!(
        saved,
        json!({"selection":["skill:user:review"],"enabled":true,"notice_acknowledged":true})
    );
    assert_eq!(
        serde_json::from_value::<CloudKitPrefs>(saved).unwrap(),
        prefs
    );
}

#[test]
fn sizes_read_naturally() {
    assert_eq!(size_label(300), "300 B");
    assert_eq!(size_label(12 * 1024), "12 KB");
    assert_eq!(size_label(3 * 1024 * 1024 / 2), "1.5 MB");
}

fn project_inventory(local: &str) -> KitInventory {
    let mut local_server = server(local, McpCompat::Ok, true);
    local_server.scope = McpScope::Local;
    KitInventory {
        skills: vec![
            skill("review", SkillScope::User, None),
            skill("draft", SkillScope::ProjectUntracked, None),
        ],
        mcp_servers: vec![server("github", McpCompat::Ok, false), local_server],
        ..Default::default()
    }
}

#[test]
fn project_scoped_items_follow_their_own_project_and_are_never_frozen() {
    let a = project_inventory("db");
    let b = project_inventory("db");
    let mut c = project_inventory("cache");
    c.skills.retain(|s| s.scope == SkillScope::User);
    let mut prefs = CloudKitPrefs::default();

    // The first edit happens while project A is open.
    prefs.set_selected(&a, "mcp:user:github", false);
    let frozen = prefs.selection.clone().unwrap();
    assert!(frozen.contains("skill:user:review"));
    assert!(
        !frozen.iter().any(|id| follows_project(id)),
        "no project item in the global list: {frozen:?}"
    );

    // Project B has its own server with the same name: on, not inherited from A's state.
    let servers = rows(&b, &prefs, KitTab::McpServers);
    assert!(row(&servers, "db").checked && row(&servers, "db").locked);
    assert!(
        !row(&servers, "github").checked,
        "the user-scope edit applies everywhere"
    );
    // Project C has other project items: also on, though A never listed them.
    let servers = rows(&c, &prefs, KitTab::McpServers);
    assert!(row(&servers, "cache").checked);
    let skills = rows(&a, &prefs, KitTab::Skills);
    assert!(row(&skills, "draft").checked && row(&skills, "draft").locked);

    // They cannot be edited from the global page.
    prefs.set_selected(&a, "mcp:local:db", false);
    prefs.set_selected(&a, "skill:project:draft", false);
    assert_eq!(prefs.selection.as_ref().unwrap(), &frozen);
    assert_eq!(KitSummary::of(&b, &prefs).mcp_servers, 1);
}

#[test]
fn the_cap_counts_checked_items_only() {
    let mut inventory = fixture();
    inventory.skills[0].compressed = 1000;
    inventory.skills[1].compressed = 500;
    let mut prefs = CloudKitPrefs::default();
    let (total, exact) = selected_compressed(&inventory, &prefs);
    // review 1000 + local-only-draft 500 + plugin fmt 200 (big and huge are excluded).
    assert_eq!((total, exact), (1700, true));
    prefs.set_selected(&inventory, "skill:user:review", false);
    assert_eq!(selected_compressed(&inventory, &prefs).0, 700);
    inventory.skills[0].exclusion = Some(Exclusion::ScanTimedOut);
    assert!(!selected_compressed(&inventory, &prefs).1);
}

#[test]
fn the_key_note_shows_only_when_the_key_is_unavailable() {
    let mut inventory = fixture();
    assert_eq!(key_unavailable_note(&inventory), None);
    inventory.key_unavailable = true;
    assert_eq!(
        key_unavailable_note(&inventory),
        Some("Secret change detection is unavailable: the local key could not be used.")
    );
}

#[test]
fn kit_sources_never_read_the_keychain_or_claude_credentials() {
    // D12: the app never reads MCP sign-ins. Every production kit file is scanned (the
    // `_tests.rs` files are not); the Guard denylist in kit.rs names the credentials file
    // as a protected path on purpose, and that single exact line is the only allowed hit.
    let core = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/cloud_sessions");
    let gpui = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../monocode-gpui/src");
    let mut files: Vec<std::path::PathBuf> = std::fs::read_dir(&core)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            name.starts_with("kit") && name.ends_with(".rs") && !name.ends_with("_tests.rs")
        })
        .collect();
    files.push(gpui.join("cloud_kit_ui.rs"));
    files.push(gpui.join("settings/pages/cloud.rs"));
    assert!(files.len() >= 4, "the scan found {files:?}");
    let allowed = "    \".claude/.credentials.json\",";
    let forbidden = [
        "find-generic-password",
        "\"security\"",
        "Command::new",
        "SecItem",
        "SecKeychain",
        "security_framework",
        "get_generic_password",
        "keyring",
        "credentials.json",
        "Library/Keychains",
    ];
    for path in files {
        let text = std::fs::read_to_string(&path).unwrap();
        let is_guard = path.file_name().unwrap() == "kit.rs";
        for (ix, line) in text.lines().enumerate() {
            for needle in forbidden {
                if !line.contains(needle) {
                    continue;
                }
                let guard_line = is_guard
                    && ((needle == "credentials.json" && line == allowed)
                        || (needle == "Library/Keychains"
                            && line.contains(
                                "real_root.starts_with(home.join(\"Library/Keychains\"))",
                            )));
                assert!(
                    guard_line,
                    "{} line {} references {needle}: {line}",
                    path.display(),
                    ix + 1
                );
            }
        }
    }
}

#[test]
fn the_kit_is_off_before_the_app_publishes_anything_and_follows_what_it_publishes() {
    // Safe by default: the defaults send nothing, so a worker that runs before the shell
    // applied its preferences cannot send a kit. A private slot keeps the shared one out of
    // the test, so no other test sees these values.
    let slot = PrefsSlot::new();
    assert!(!slot.get().applies(ApiKit::Default));
    let mut prefs = CloudKitPrefs::default();
    prefs.acknowledge_notice();
    slot.publish(prefs.clone());
    assert_eq!(slot.get(), prefs);
    assert!(slot.get().applies(ApiKit::Default));
    slot.publish(CloudKitPrefs::default());
    assert!(!slot.get().applies(ApiKit::Default));
}
