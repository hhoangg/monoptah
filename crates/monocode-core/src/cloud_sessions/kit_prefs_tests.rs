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
            server("linear", McpCompat::SignedIn, false),
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
fn defaults_check_everything_that_can_be_sent_secrets_and_sign_ins_included() {
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
        "sign-ins are on by default"
    );
    assert!(row(&servers, "fetch").checked);
    assert!(row(&servers, "github").badges.contains(&Badge::HasSecrets));
    assert!(row(&servers, "linear").badges.contains(&Badge::SignedIn));

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
    assert_eq!(label("linear"), ["Works in the cloud", "Signed in (OAuth)"]);
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
    assert_eq!(KitSummary::of(&inventory, &forced), KitSummary::default());
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
        "Your MCP secrets and sign-ins will be copied to your cloud workspaces. They will be visible to the agent, to every MCP server and to every command the agent runs there."
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
