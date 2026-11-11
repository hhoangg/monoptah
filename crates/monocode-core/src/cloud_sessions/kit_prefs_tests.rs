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
            plugins: 1,
            ..Default::default()
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
        ..Default::default()
    };
    assert_eq!(summary.text(), "6 skills, 4 MCP servers, 3 plugins");
    let one = KitSummary {
        skills: 1,
        mcp_servers: 1,
        plugins: 1,
        ..Default::default()
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
        ..Default::default()
    };
    let saved = serde_json::to_value(&prefs).unwrap();
    assert_eq!(
        saved,
        json!({"selection":["skill:user:review"],"enabled":true,"notice_acknowledged":true,"instructions":true})
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

/// Every non-test `.rs` file under `dir`, recursively (`tests.rs` and `*_tests.rs` are tests).
fn production_sources(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            production_sources(&path, out);
            continue;
        }
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        if name.ends_with(".rs") && name != "tests.rs" && !name.ends_with("_tests.rs") {
            out.push(path);
        }
    }
}

#[test]
fn cloud_sources_never_read_the_keychain_or_claude_credentials() {
    // D12: the app never reads MCP sign-ins. Every production file of the cloud sessions
    // module, and the kit UI files, are scanned (test files are not); the Guard denylist in
    // kit.rs names the credentials file as a protected path on purpose, and those exact
    // lines are the only allowed hits. Spawning a process is also banned in the kit files
    // (the other cloud files run `gh`, `ssh` and `sbx` legitimately).
    let core = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/cloud_sessions");
    let gpui = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../monocode-gpui/src");
    let mut files = Vec::new();
    production_sources(&core, &mut files);
    let kit_ui = [
        gpui.join("cloud_kit_ui.rs"),
        gpui.join("settings/pages/cloud.rs"),
        gpui.join("shell/claude_setup.rs"),
        gpui.join("shell.rs"),
        gpui.join("panel_root.rs"),
        gpui.join("shell/cloud_picker.rs"),
        gpui.join("conversation/cloud_target.rs"),
        gpui.join("settings/preferences.rs"),
    ];
    files.extend(kit_ui.iter().cloned());
    assert!(files.len() >= 20, "the scan found {files:?}");
    for must in [
        "kit.rs",
        "kit_pack.rs",
        "claude_setup.rs",
        "docker_start.rs",
    ] {
        assert!(
            files.iter().any(|f| f.file_name().unwrap() == must),
            "{must} is not scanned"
        );
    }
    let allowed = "    \".claude/.credentials.json\",";
    let keychain = [
        "find-generic-password",
        "\"security\"",
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
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let is_guard = name == "kit.rs";
        let is_kit = name.starts_with("kit") || kit_ui.contains(&path);
        for (ix, line) in text.lines().enumerate() {
            let spawns = is_kit && line.contains("Command::new");
            for needle in keychain.into_iter().chain(spawns.then_some("Command::new")) {
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
fn the_sign_in_hint_names_real_commands_and_says_nothing_is_copied() {
    use crate::cloud_sessions::claude_setup::SIGN_IN_HINT;
    assert!(SIGN_IN_HINT.contains("not copied"));
    assert!(SIGN_IN_HINT.contains("`claude mcp login --no-browser <name>`"));
    assert!(SIGN_IN_HINT.contains("`/mcp`"));
}

#[test]
fn an_oauth_style_server_is_carried_config_only_and_wears_the_sign_in_badge() {
    use crate::cloud_sessions::claude_setup::SIGN_IN_LABEL;
    let inventory = KitInventory {
        mcp_servers: vec![server("linear", McpCompat::OAuthStyle, false)],
        ..Default::default()
    };
    let prefs = CloudKitPrefs::default();
    let servers = rows(&inventory, &prefs, KitTab::McpServers);
    let linear = row(&servers, "linear");
    // Sent (checked, no secret, nothing local-only or excluded), badge in the D12 words.
    assert!(linear.checked);
    assert_eq!(linear.badges, vec![Badge::Works, Badge::SignInInCloud]);
    assert_eq!(Badge::SignInInCloud.label(), SIGN_IN_LABEL);
    // The start panel's count includes it, and nothing flags it as a secret.
    assert_eq!(KitSummary::of(&inventory, &prefs).mcp_servers, 1);
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

#[test]
fn instructions_are_on_for_old_and_new_preferences_and_round_trip_off() {
    assert!(CloudKitPrefs::default().instructions);
    let old: CloudKitPrefs =
        serde_json::from_str(r#"{"enabled":true,"notice_acknowledged":true}"#).unwrap();
    assert!(old.instructions);
    let empty: CloudKitPrefs = serde_json::from_str("{}").unwrap();
    assert!(empty.instructions);
    let off = CloudKitPrefs {
        instructions: false,
        ..Default::default()
    };
    let back: CloudKitPrefs = serde_json::from_str(&serde_json::to_string(&off).unwrap()).unwrap();
    assert!(!back.instructions);
    assert_eq!(back, off);
}

#[test]
fn the_summary_counts_instructions_only_when_they_are_on_and_carried() {
    let ins = |exclusion| KitInstructions {
        source: KitSource::User,
        root: PathBuf::from("/x"),
        files: vec![KitFile {
            path: PathBuf::from("/x/CLAUDE.md"),
            id: None,
            stamp: None,
            name: PathBuf::from("CLAUDE.md"),
        }],
        size: 3,
        hash: "h".into(),
        exclusion,
        not_carried: Vec::new(),
        notes: Vec::new(),
    };
    let mut inventory = KitInventory {
        instructions: Some(ins(None)),
        ..Default::default()
    };
    let on = CloudKitPrefs::default();
    let summary = KitSummary::of(&inventory, &on);
    assert!(summary.instructions && !summary.is_empty());
    assert_eq!(summary.text(), "your instructions");
    let off = CloudKitPrefs {
        instructions: false,
        ..Default::default()
    };
    assert!(KitSummary::of(&inventory, &off).is_empty());
    inventory.instructions = Some(ins(Some(Exclusion::TooLarge)));
    assert!(!KitSummary::of(&inventory, &on).instructions);
}
