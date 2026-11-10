use super::kit::*;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

// Every value below is fake.
const FAKE_TOKEN: &str = "fake-token-AAAA1111";
const FAKE_HEADER: &str = "Bearer fake-header-BBBB2222";
const FAKE_QUERY: &str = "fake-query-CCCC3333";

struct Env {
    dir: tempfile::TempDir,
    roots: KitRoots,
}

fn env() -> Env {
    let dir = tempfile::tempdir().unwrap();
    let home = std::fs::canonicalize(dir.path()).unwrap();
    let project = home.join("work/app");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::create_dir_all(home.join(".claude")).unwrap();
    let roots = KitRoots {
        account_dir: None,
        claude_home: home.join(".claude"),
        project,
        key_dir: Some(home.join("app-data")),
        home,
    };
    Env { dir, roots }
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn write_json(path: &Path, value: Value) {
    write(path, &value.to_string());
}

fn skill(root: &Path, name: &str, body: &str) {
    write(&root.join(name).join("SKILL.md"), body);
}

fn git(project: &Path, args: &[&str]) {
    let status = std::process::Command::new("git")
        .arg("-C")
        .arg(project)
        .args(args)
        .status()
        .unwrap();
    assert!(status.success());
}

fn servers_json(env: &Env, servers: Value) {
    write_json(
        &env.roots.home.join(".claude.json"),
        json!({ "mcpServers": servers }),
    );
}

fn compat_of(servers: Value, workspace: Workspace) -> Vec<(String, McpCompat)> {
    let env = env();
    servers_json(&env, servers);
    build_inventory(&env.roots, workspace)
        .mcp_servers
        .into_iter()
        .map(|s| (s.name, s.compat))
        .collect()
}

fn compat_one(config: Value) -> McpCompat {
    compat_of(json!({ "s": config }), Workspace::Codespace)[0].1
}

#[test]
fn stdio_classification_table() {
    use LocalOnlyReason::*;
    use McpCompat::*;
    let cases = [
        (
            json!({"command": "/Users/me/bin/server"}),
            LocalOnly(AbsoluteCommand),
        ),
        (
            json!({"command": "/opt/homebrew/bin/node", "args": ["x.js"]}),
            LocalOnly(AbsoluteCommand),
        ),
        (
            json!({"command": "/Applications/Tool.app/run"}),
            LocalOnly(AbsoluteCommand),
        ),
        (
            json!({"command": "~/bin/server"}),
            LocalOnly(AbsoluteCommand),
        ),
        (
            json!({"command": "C:\\tools\\server.exe"}),
            LocalOnly(AbsoluteCommand),
        ),
        (
            json!({"command": "npx", "args": ["-y", "pkg"]}),
            NeedsRuntime(Runtime::Npx),
        ),
        (json!({"command": "uvx"}), NeedsRuntime(Runtime::Uvx)),
        (json!({"command": "bunx"}), NeedsRuntime(Runtime::Bunx)),
        (json!({"command": "node"}), NeedsRuntime(Runtime::Node)),
        (json!({"command": "python3"}), NeedsRuntime(Runtime::Python)),
        (json!({"command": "python"}), NeedsRuntime(Runtime::Python)),
        (
            json!({"command": "docker", "args": ["run"]}),
            NeedsRuntime(Runtime::Docker),
        ),
        (json!({"type": "stdio", "command": "my-server"}), Ok),
    ];
    for (config, expected) in cases {
        assert_eq!(compat_one(config.clone()), expected, "{config}");
    }
}

#[test]
fn docker_is_local_only_on_a_sandbox() {
    let result = compat_of(
        json!({"d": {"command": "docker"}, "n": {"command": "npx"}}),
        Workspace::Sandbox,
    );
    assert_eq!(
        result,
        vec![
            (
                "d".to_string(),
                McpCompat::LocalOnly(LocalOnlyReason::DockerOnSandbox)
            ),
            ("n".to_string(), McpCompat::NeedsRuntime(Runtime::Npx)),
        ]
    );
}

#[test]
fn url_classification_table() {
    use LocalOnlyReason::PrivateUrl;
    use McpCompat::*;
    let private = [
        "http://localhost:3000/mcp",
        "http://LOCALHOST./mcp",
        "http://127.0.0.1/mcp",
        "http://127.9.9.9:80/mcp",
        "http://[::1]:8080/mcp",
        "https://box.local/mcp",
        "http://10.1.2.3/mcp",
        "http://172.16.0.1/mcp",
        "http://172.31.255.255/mcp",
        "http://192.168.1.5/mcp",
        "http://169.254.169.254/mcp",
        "http://[fe80::1]/mcp",
        "http://0.0.0.0:9/mcp",
        "http://[::]/mcp",
        "http://100.64.0.1/mcp",
        "http://100.127.255.255/mcp",
        // The URL parser normalizes short and hex IPv4 forms, so these are loopback.
        "http://127.1/mcp",
        "http://0x7f000001/mcp",
        "http://[::ffff:127.0.0.1]/mcp",
        "http://[::ffff:10.0.0.1]/mcp",
        // The host is what follows the userinfo.
        "http://user:pass@localhost/mcp",
    ];
    for url in private {
        assert_eq!(
            compat_one(json!({"type": "http", "url": url})),
            LocalOnly(PrivateUrl),
            "{url}"
        );
    }
    let public = [
        "https://mcp.example.com/mcp",
        "http://172.32.0.1/mcp",
        "http://11.0.0.1/mcp",
        "https://localhost.example.com/mcp",
        // Parses as a domain named "${host}".
        "https://${HOST}/mcp",
        // No scheme: unparseable, so not judged.
        "${BASE_URL}/mcp",
        "http://100.63.255.255/mcp",
        "http://100.128.0.1/mcp",
        // The userinfo is "localhost"; the host is evil.com.
        "http://localhost@evil.com/mcp",
    ];
    for url in public {
        assert_eq!(compat_one(json!({"type": "sse", "url": url})), Ok, "{url}");
    }
}

#[test]
fn authorization_header_skips_the_sign_in_probe() {
    let env = env();
    servers_json(
        &env,
        json!({
            "a": {"type": "http", "url": "https://a.example.com", "headers": {"authorization": FAKE_HEADER}},
            "b": {"type": "http", "url": "https://b.example.com"},
        }),
    );
    let inventory = build_inventory_with(&env.roots, Workspace::Codespace, &|_| Some(true));
    let compat: Vec<_> = inventory
        .mcp_servers
        .iter()
        .map(|s| (&*s.name, s.compat))
        .collect();
    assert_eq!(
        compat,
        vec![("a", McpCompat::Ok), ("b", McpCompat::SignedIn)]
    );
    let inventory = build_inventory_with(&env.roots, Workspace::Codespace, &|_| Some(false));
    assert_eq!(inventory.mcp_servers[1].compat, McpCompat::NotSignedIn);
    // The shipped seam knows nothing yet, so the server stays Ok.
    let inventory = build_inventory(&env.roots, Workspace::Codespace);
    assert_eq!(inventory.mcp_servers[1].compat, McpCompat::Ok);
}

#[test]
fn secret_rules() {
    let env = env();
    servers_json(
        &env,
        json!({
            "s": {
                "command": "npx",
                "env": {
                    "REF": "${TOKEN}",
                    "DEFAULTED": "${TOKEN:-fallback}",
                    "MIXED": "Bearer ${TOKEN}",
                    "PLAIN": FAKE_TOKEN,
                    "EMPTY": ""
                }
            },
            "h": {
                "type": "http",
                "url": format!("https://x.example.com/mcp?key={FAKE_QUERY}"),
                "headers": {"Authorization": FAKE_HEADER, "X-Ref": "${KEY}"}
            },
            "plain-url": {"type": "http", "url": "https://x.example.com/mcp"}
        }),
    );
    let inventory = build_inventory(&env.roots, Workspace::Codespace);
    let by = |name: &str| {
        inventory
            .mcp_servers
            .iter()
            .find(|s| s.name == name)
            .unwrap()
    };
    let locations = |name: &str| -> Vec<SecretLocation> {
        by(name)
            .secrets
            .iter()
            .map(|s| s.location.clone())
            .collect()
    };
    assert_eq!(
        locations("s"),
        vec![
            SecretLocation::Env("DEFAULTED".into()),
            SecretLocation::Env("EMPTY".into()),
            SecretLocation::Env("MIXED".into()),
            SecretLocation::Env("PLAIN".into()),
        ]
    );
    assert_eq!(
        locations("h"),
        vec![
            SecretLocation::Header("Authorization".into()),
            SecretLocation::UrlQuery
        ]
    );
    assert!(locations("plain-url").is_empty());
    let plain = by("s")
        .secrets
        .iter()
        .find(|s| s.location == SecretLocation::Env("PLAIN".into()));
    assert_eq!(plain.unwrap().value.expose(), FAKE_TOKEN);
}

#[test]
fn debug_and_display_never_show_secret_values() {
    let env = env();
    servers_json(
        &env,
        json!({
            "s": {"command": "npx", "env": {"KEY": FAKE_TOKEN}},
            "h": {"type": "http", "url": format!("https://x.example.com/?k={FAKE_QUERY}"),
                  "headers": {"Authorization": FAKE_HEADER}}
        }),
    );
    let inventory = build_inventory(&env.roots, Workspace::Codespace);
    assert_eq!(
        inventory
            .mcp_servers
            .iter()
            .map(|s| s.secrets.len())
            .sum::<usize>(),
        3
    );
    for text in [format!("{inventory:?}"), format!("{inventory:#?}")] {
        for secret in [FAKE_TOKEN, FAKE_HEADER, FAKE_QUERY, "fake-header-BBBB2222"] {
            assert!(!text.contains(secret), "leaked {secret}");
        }
    }
    let secret = Secret::new(FAKE_TOKEN);
    assert!(!format!("{secret}").contains(FAKE_TOKEN));
    assert!(!format!("{secret:?}").contains(FAKE_TOKEN));
}

#[test]
fn mcp_scopes_and_precedence() {
    let mut env = env();
    let account = env.dir.path().join("account");
    std::fs::create_dir_all(&account).unwrap();
    env.roots.account_dir = Some(account.clone());
    let project = env.roots.project.to_string_lossy().into_owned();
    write_json(
        &env.roots.home.join(".claude.json"),
        json!({
            "mcpServers": {"shared": {"command": "uvx"}, "user-only": {"command": "node"}},
            "projects": {
                project: {"mcpServers": {"here": {"command": "bunx"}}},
                "/some/other/project": {"mcpServers": {"elsewhere": {"command": "node"}}}
            }
        }),
    );
    write_json(
        &account.join(".claude.json"),
        json!({"mcpServers": {"shared": {"command": "npx"}}}),
    );
    write_json(
        &env.roots.project.join(".mcp.json"),
        json!({"mcpServers": {"from-repo": {"command": "/Users/x/y", "env": {"K": FAKE_TOKEN}}}}),
    );
    let inventory = build_inventory(&env.roots, Workspace::Codespace);
    let rows: Vec<_> = inventory
        .mcp_servers
        .iter()
        .map(|s| (s.name.as_str(), s.scope, s.source, s.compat))
        .collect();
    assert_eq!(
        rows,
        vec![
            (
                "from-repo",
                McpScope::Project,
                KitSource::Project,
                McpCompat::Project
            ),
            (
                "here",
                McpScope::Local,
                KitSource::User,
                McpCompat::NeedsRuntime(Runtime::Bunx)
            ),
            (
                "shared",
                McpScope::User,
                KitSource::Account,
                McpCompat::NeedsRuntime(Runtime::Npx)
            ),
            (
                "user-only",
                McpScope::User,
                KitSource::User,
                McpCompat::NeedsRuntime(Runtime::Node)
            ),
        ]
    );
}

#[test]
fn user_skills_account_wins_and_sources_are_labelled() {
    let mut env = env();
    let account = env.dir.path().join("account");
    env.roots.account_dir = Some(account.clone());
    skill(&account.join("skills"), "dup", "account version");
    skill(&env.roots.claude_home.join("skills"), "dup", "user version");
    skill(&env.roots.claude_home.join("skills"), "solo", "solo");
    // Not a skill: no SKILL.md, and a hidden folder.
    write(&env.roots.claude_home.join("skills/synced/readme.txt"), "x");
    skill(&env.roots.claude_home.join("skills"), ".hidden", "x");
    let inventory = build_inventory(&env.roots, Workspace::Codespace);
    let rows: Vec<_> = inventory
        .skills
        .iter()
        .map(|s| (s.name.as_str(), s.scope, s.source))
        .collect();
    assert_eq!(
        rows,
        vec![
            ("dup", SkillScope::User, KitSource::Account),
            ("solo", SkillScope::User, KitSource::User),
        ]
    );
}

#[test]
fn committed_project_skills_are_skipped() {
    let env = env();
    let project = &env.roots.project;
    git(project, &["init", "-q"]);
    skill(&project.join(".claude/skills"), "committed", "in the repo");
    skill(&project.join(".claude/skills"), "untracked", "only here");
    skill(&project.join(".claude/skills"), "ignored", "ignored");
    write(&project.join(".gitignore"), ".claude/skills/ignored/\n");
    git(project, &["add", ".gitignore", ".claude/skills/committed"]);
    let inventory = build_inventory(&env.roots, Workspace::Codespace);
    let rows: Vec<_> = inventory
        .skills
        .iter()
        .map(|s| (s.name.as_str(), s.scope, s.source))
        .collect();
    assert_eq!(
        rows,
        vec![
            ("ignored", SkillScope::ProjectUntracked, KitSource::Project),
            (
                "untracked",
                SkillScope::ProjectUntracked,
                KitSource::Project
            ),
        ]
    );
}

#[test]
fn project_skills_outside_a_git_repo_are_all_offered() {
    let env = env();
    skill(&env.roots.project.join(".claude/skills"), "loose", "x");
    let inventory = build_inventory(&env.roots, Workspace::Codespace);
    assert_eq!(inventory.skills.len(), 1);
    assert_eq!(inventory.skills[0].scope, SkillScope::ProjectUntracked);
}

fn install_plugin(env: &Env, id: &str, scope: Value, enabled: Option<bool>) -> PathBuf {
    let home = &env.roots.home;
    let name = id.split('@').next().unwrap();
    let path = home.join(format!(".claude/plugins/cache/mk/{name}/1.0.0"));
    write(&path.join(".claude-plugin/plugin.json"), "{}");
    let registry = home.join(".claude/plugins/installed_plugins.json");
    let mut value: Value = std::fs::read_to_string(&registry)
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or(json!({"version": 2, "plugins": {}}));
    let mut entry = json!({"version": "1.0.0", "installPath": path});
    entry
        .as_object_mut()
        .unwrap()
        .extend(scope.as_object().unwrap().clone());
    value["plugins"][id] = json!([entry]);
    write_json(&registry, value);
    if let Some(enabled) = enabled {
        let settings = home.join(".claude/settings.json");
        let mut current: Value = std::fs::read_to_string(&settings)
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or(json!({}));
        current["enabledPlugins"][id] = json!(enabled);
        write_json(&settings, current);
    }
    path
}

#[test]
fn only_enabled_plugins_are_offered() {
    let env = env();
    install_plugin(&env, "on@mk", json!({"scope": "user"}), Some(true));
    install_plugin(&env, "off@mk", json!({"scope": "user"}), Some(false));
    install_plugin(&env, "unlisted@mk", json!({"scope": "user"}), None);
    let inventory = build_inventory(&env.roots, Workspace::Codespace);
    let ids: Vec<_> = inventory.plugins.iter().map(|p| p.id.as_str()).collect();
    assert_eq!(ids, vec!["on@mk"]);
    let plugin = &inventory.plugins[0];
    assert_eq!(plugin.version, "1.0.0");
    assert_eq!(plugin.source, KitSource::User);
    assert!(plugin.size > 0);
    assert!(!plugin.has_native_bin);
    assert!(plugin.exclusion.is_none());
}

#[test]
fn project_scoped_plugins_follow_their_project() {
    let env = env();
    let mine = env.roots.project.clone();
    install_plugin(
        &env,
        "mine@mk",
        json!({"scope": "project", "projectPath": mine}),
        Some(true),
    );
    install_plugin(
        &env,
        "theirs@mk",
        json!({"scope": "local", "projectPath": "/somewhere/else"}),
        Some(true),
    );
    let inventory = build_inventory(&env.roots, Workspace::Codespace);
    let ids: Vec<_> = inventory.plugins.iter().map(|p| p.id.as_str()).collect();
    assert_eq!(ids, vec!["mine@mk"]);
}

#[test]
fn native_bin_detection_and_node_modules_rules() {
    let env = env();
    let with_bin = install_plugin(&env, "bin@mk", json!({"scope": "user"}), Some(true));
    write(&with_bin.join("bin/tool"), "x");
    let with_lsp = install_plugin(&env, "lsp@mk", json!({"scope": "user"}), Some(true));
    write(&with_lsp.join(".lsp.json"), "{}");
    let with_node = install_plugin(&env, "native@mk", json!({"scope": "user"}), Some(true));
    write(&with_node.join("node_modules/dep/build/addon.node"), "x");
    let plain = install_plugin(&env, "plain@mk", json!({"scope": "user"}), Some(true));
    write(&plain.join("node_modules/dep/index.js"), "kept");
    write(&plain.join(".git/config"), "skipped");
    write(&plain.join(".DS_Store"), "skipped");
    skill(&env.roots.claude_home.join("skills"), "s", "x");
    write(
        &env.roots
            .claude_home
            .join("skills/s/node_modules/dep/index.js"),
        "skipped in skills",
    );
    write(
        &env.roots.claude_home.join("skills/s/.git/config"),
        "skipped",
    );
    let inventory = build_inventory(&env.roots, Workspace::Codespace);
    let flags: Vec<_> = inventory
        .plugins
        .iter()
        .map(|p| (p.id.as_str(), p.has_native_bin))
        .collect();
    assert_eq!(
        flags,
        vec![
            ("bin@mk", true),
            ("lsp@mk", true),
            ("native@mk", true),
            ("plain@mk", false)
        ]
    );
    let plain_plugin = inventory
        .plugins
        .iter()
        .find(|p| p.id == "plain@mk")
        .unwrap();
    // plugin.json (2) + index.js (4)
    assert_eq!(plain_plugin.size, 2 + 4);
    let user_skill = inventory.skills.iter().find(|s| s.name == "s").unwrap();
    assert_eq!(user_skill.size, "x".len() as u64);
}

#[test]
fn plugin_skills_are_listed_with_their_plugin() {
    let env = env();
    let path = install_plugin(&env, "kit@mk", json!({"scope": "user"}), Some(true));
    skill(&path.join("skills"), "inside", "x");
    let inventory = build_inventory(&env.roots, Workspace::Codespace);
    let found = inventory
        .skills
        .iter()
        .find(|s| s.name == "inside")
        .unwrap();
    assert_eq!(found.scope, SkillScope::Plugin);
    assert_eq!(found.plugin.as_deref(), Some("kit@mk"));
}

#[test]
fn size_caps_exclude_items() {
    let env = env();
    let big_skill = env.roots.claude_home.join("skills/big");
    write(&big_skill.join("SKILL.md"), "x");
    std::fs::write(
        big_skill.join("blob.bin"),
        vec![0u8; MAX_SKILL_BYTES as usize],
    )
    .unwrap();
    skill(&env.roots.claude_home.join("skills"), "ok", "x");
    // Exactly at the cap is allowed.
    let edge = env.roots.claude_home.join("skills/edge");
    std::fs::create_dir_all(&edge).unwrap();
    std::fs::write(edge.join("SKILL.md"), vec![0u8; MAX_SKILL_BYTES as usize]).unwrap();
    let big_plugin = install_plugin(&env, "big@mk", json!({"scope": "user"}), Some(true));
    std::fs::write(
        big_plugin.join("blob.bin"),
        vec![0u8; MAX_PLUGIN_BYTES as usize],
    )
    .unwrap();
    let inventory = build_inventory(&env.roots, Workspace::Codespace);
    let skill_rows: Vec<_> = inventory
        .skills
        .iter()
        .map(|s| (s.name.as_str(), s.exclusion))
        .collect();
    assert_eq!(
        skill_rows,
        vec![
            ("big", Some(Exclusion::TooLarge)),
            ("edge", None),
            ("ok", None)
        ]
    );
    assert_eq!(inventory.plugins[0].exclusion, Some(Exclusion::TooLarge));
    assert!(inventory.plugins[0].hash.is_empty());
    // Excluded items do not count toward the total, and zero bytes compress small.
    assert!(!inventory.over_total_cap);
}

#[test]
fn total_cap_is_measured_after_compression() {
    let env = env();
    // 10 MB of zeros compresses to a few KB: three of them stay far below 50 MB.
    for name in ["a", "b", "c"] {
        let dir = env.roots.claude_home.join("skills").join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("SKILL.md"), vec![0u8; MAX_SKILL_BYTES as usize]).unwrap();
    }
    let inventory = build_inventory(&env.roots, Workspace::Codespace);
    assert!(inventory.total_compressed > 0);
    assert!(inventory.total_compressed < 1024 * 1024);
    assert!(!inventory.over_total_cap);
}

#[cfg(unix)]
#[test]
fn item_roots_may_be_links_under_home_but_content_links_must_stay_in_the_item() {
    use std::os::unix::fs::symlink;
    let env = env();
    let outside = tempfile::tempdir().unwrap();
    write(&outside.path().join("secret.txt"), "outside home");

    // A skill that is itself a link to another place inside home.
    let shared = env.roots.home.join("shared/linked-skill");
    write(&shared.join("SKILL.md"), "inside");
    let skills = env.roots.claude_home.join("skills");
    std::fs::create_dir_all(&skills).unwrap();
    symlink(&shared, skills.join("linked-skill")).unwrap();

    // A skill with a file linking out of home.
    skill(&skills, "leaky", "x");
    symlink(
        outside.path().join("secret.txt"),
        skills.join("leaky/out.txt"),
    )
    .unwrap();

    // A skill whose directory links out of home.
    write(&outside.path().join("far/SKILL.md"), "far");
    symlink(outside.path().join("far"), skills.join("far")).unwrap();

    // A link loop inside a skill must terminate.
    skill(&skills, "loop", "x");
    symlink(skills.join("loop"), skills.join("loop/again")).unwrap();

    let inventory = build_inventory(&env.roots, Workspace::Codespace);
    let rows: Vec<_> = inventory
        .skills
        .iter()
        .map(|s| (s.name.as_str(), s.exclusion))
        .collect();
    assert_eq!(
        rows,
        vec![
            ("far", Some(Exclusion::LinkOutsideHome)),
            ("leaky", Some(Exclusion::LinkLeavesItem)),
            ("linked-skill", None),
            ("loop", None),
        ]
    );
}

#[cfg(unix)]
#[test]
fn a_plugin_install_path_that_links_out_of_home_is_refused() {
    let env = env();
    let target = tempfile::tempdir().unwrap();
    write(&target.path().join(".claude-plugin/plugin.json"), "{}");
    let outside = env.roots.home.join("link-to-outside");
    std::os::unix::fs::symlink(target.path(), &outside).unwrap();
    let registry = env.roots.claude_home.join("plugins/installed_plugins.json");
    write_json(
        &registry,
        json!({"version": 2, "plugins": {"far@mk": [{"scope": "user", "version": "1", "installPath": outside}]}}),
    );
    write_json(
        &env.roots.claude_home.join("settings.json"),
        json!({"enabledPlugins": {"far@mk": true}}),
    );
    let inventory = build_inventory(&env.roots, Workspace::Codespace);
    assert_eq!(
        inventory.plugins[0].exclusion,
        Some(Exclusion::LinkOutsideHome)
    );
}

fn fixture(env: &Env) {
    skill(&env.roots.claude_home.join("skills"), "b", "second");
    skill(&env.roots.claude_home.join("skills"), "a", "first");
    let plugin = install_plugin(env, "p@mk", json!({"scope": "user"}), Some(true));
    write(&plugin.join("skills/ps/SKILL.md"), "plugin skill");
    servers_json(
        env,
        json!({
            "z": {"command": "npx", "env": {"K": FAKE_TOKEN}},
            "y": {"type": "http", "url": "https://y.example.com"}
        }),
    );
}

#[test]
fn ordering_and_hash_are_deterministic() {
    let env = env();
    fixture(&env);
    let first = build_inventory(&env.roots, Workspace::Codespace);
    let second = build_inventory(&env.roots, Workspace::Codespace);
    assert_eq!(first, second);
    assert_eq!(first.content_hash.len(), 64);
    let skills: Vec<_> = first.skills.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(skills, vec!["a", "b", "ps"]);
    let servers: Vec<_> = first.mcp_servers.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(servers, vec!["y", "z"]);
}

#[test]
fn hash_ignores_key_order_and_follows_content() {
    let env = env();
    fixture(&env);
    let before = build_inventory(&env.roots, Workspace::Codespace).content_hash;

    // Same servers written in another key order: same hash.
    write(
        &env.roots.home.join(".claude.json"),
        r#"{"mcpServers":{"y":{"url":"https://y.example.com","type":"http"},"z":{"env":{"K":"fake-token-AAAA1111"},"command":"npx"}}}"#,
    );
    assert_eq!(
        build_inventory(&env.roots, Workspace::Codespace).content_hash,
        before
    );

    // A changed skill file, secret value or plugin file changes it.
    write(&env.roots.claude_home.join("skills/a/SKILL.md"), "changed");
    let skill_changed = build_inventory(&env.roots, Workspace::Codespace).content_hash;
    assert_ne!(skill_changed, before);
    write(
        &env.roots.home.join(".claude.json"),
        r#"{"mcpServers":{"y":{"url":"https://y.example.com","type":"http"},"z":{"env":{"K":"fake-other-value"},"command":"npx"}}}"#,
    );
    // The secret value is no longer in `content_hash`: only `change_token` follows it.
    let secret_inventory = build_inventory(&env.roots, Workspace::Codespace);
    let secret_changed = secret_inventory.content_hash;
    assert_eq!(secret_changed, skill_changed);
    let plugin = env.roots.claude_home.join("plugins/cache/mk/p/1.0.0");
    write(&plugin.join("extra.txt"), "new file");
    assert_ne!(
        build_inventory(&env.roots, Workspace::Codespace).content_hash,
        secret_changed
    );
}

#[test]
fn empty_home_gives_an_empty_inventory() {
    let env = env();
    let inventory = build_inventory(&env.roots, Workspace::Codespace);
    assert!(inventory.skills.is_empty());
    assert!(inventory.plugins.is_empty());
    assert!(inventory.mcp_servers.is_empty());
    assert!(!inventory.over_total_cap);
}

#[test]
fn account_source_reads_its_own_plugins_and_settings() {
    let mut env = env();
    let account = env.dir.path().join("account");
    env.roots.account_dir = Some(account.clone());
    let path = account.join("plugins/cache/mk/acc/1.0.0");
    write(&path.join(".claude-plugin/plugin.json"), "{}");
    write_json(
        &account.join("plugins/installed_plugins.json"),
        json!({"version": 2, "plugins": {"acc@mk": [{"scope": "user", "version": "1.0.0", "installPath": path}]}}),
    );
    // Without a settings.json in the account dir nothing is enabled there.
    assert!(build_inventory(&env.roots, Workspace::Codespace)
        .plugins
        .is_empty());
    write_json(
        &account.join("settings.json"),
        json!({"enabledPlugins": {"acc@mk": true}}),
    );
    let inventory = build_inventory(&env.roots, Workspace::Codespace);
    assert_eq!(inventory.plugins[0].source, KitSource::Account);
}

#[cfg(unix)]
#[test]
fn dead_symlinks_are_skipped_not_refused() {
    use std::os::unix::fs::symlink;
    let env = env();
    let skills = env.roots.claude_home.join("skills");
    skill(&skills, "dead", "x");
    symlink(env.roots.home.join("nowhere"), skills.join("dead/gone.txt")).unwrap();
    // A dead link loop too.
    symlink(skills.join("dead/a"), skills.join("dead/b")).unwrap();
    symlink(skills.join("dead/b"), skills.join("dead/a")).unwrap();
    let inventory = build_inventory(&env.roots, Workspace::Codespace);
    assert_eq!(inventory.skills.len(), 1);
    assert_eq!(inventory.skills[0].exclusion, None);
    assert_eq!(inventory.skills[0].size, 1);
}

#[test]
fn a_project_outside_home_still_offers_its_skills() {
    let env = env();
    let elsewhere = tempfile::tempdir().unwrap();
    let project = std::fs::canonicalize(elsewhere.path()).unwrap();
    skill(&project.join(".claude/skills"), "local", "abc");
    let roots = KitRoots {
        project,
        ..env.roots.clone()
    };
    let inventory = build_inventory(&roots, Workspace::Codespace);
    assert_eq!(inventory.skills.len(), 1);
    assert_eq!(inventory.skills[0].exclusion, None);
    assert_eq!(inventory.skills[0].size, 3);
}

#[test]
fn a_plugin_installed_outside_home_as_a_real_dir_is_offered() {
    let env = env();
    // Not canonicalized on purpose: on macOS the temp dir sits behind /var -> /private/var,
    // and a root whose own path is outside home must still be offered.
    let outside = tempfile::tempdir().unwrap();
    write(&outside.path().join(".claude-plugin/plugin.json"), "{}");
    write_json(
        &env.roots.claude_home.join("plugins/installed_plugins.json"),
        json!({"version": 2, "plugins": {"far@mk": [{"scope": "user", "version": "1", "installPath": outside.path()}]}}),
    );
    write_json(
        &env.roots.claude_home.join("settings.json"),
        json!({"enabledPlugins": {"far@mk": true}}),
    );
    let inventory = build_inventory(&env.roots, Workspace::Codespace);
    assert_eq!(inventory.plugins[0].exclusion, None);
    assert_eq!(inventory.plugins[0].size, 2);
}

#[test]
fn a_timed_out_scan_is_incomplete_and_never_under_the_cap() {
    let env = env();
    fixture(&env);
    let complete = build_inventory(&env.roots, Workspace::Codespace);
    assert!(!complete.incomplete);
    let timed_out = build_inventory_budgeted(
        &env.roots,
        Workspace::Codespace,
        &|_| None,
        std::time::Duration::ZERO,
    );
    assert!(timed_out.incomplete);
    assert!(timed_out.over_total_cap);
    assert!(timed_out
        .skills
        .iter()
        .any(|s| s.exclusion == Some(Exclusion::ScanTimedOut)));
    assert_ne!(timed_out.content_hash, complete.content_hash);
    assert!(!timed_out.content_hash.is_empty());
}

#[test]
fn committed_symlink_skills_count_as_committed() {
    let env = env();
    let project = &env.roots.project;
    git(project, &["init", "-q"]);
    let skills = project.join(".claude/skills");
    skill(&project.join("shared"), "linked", "x");
    std::fs::create_dir_all(&skills).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(project.join("shared/linked"), skills.join("linked")).unwrap();
    skill(&skills, "plain", "x");
    git(project, &["add", ".claude/skills/linked"]);
    let inventory = build_inventory(&env.roots, Workspace::Codespace);
    let names: Vec<_> = inventory.skills.iter().map(|s| s.name.as_str()).collect();
    #[cfg(unix)]
    assert_eq!(names, vec!["plain"]);
    #[cfg(not(unix))]
    let _ = names;
}

#[cfg(unix)]
mod links {
    use super::*;
    use std::os::unix::fs::symlink;

    fn only_skill(env: &Env) -> KitSkill {
        let inventory = build_inventory(&env.roots, Workspace::Codespace);
        assert_eq!(inventory.skills.len(), 1, "{inventory:?}");
        inventory.skills.into_iter().next().unwrap()
    }

    #[test]
    fn parent_link_of_a_skills_root_out_of_home_is_refused() {
        let env = env();
        let outside = tempfile::tempdir().unwrap();
        skill(&outside.path().join("skills"), "far", "x");
        // ~/.claude/skills -> outside
        symlink(
            outside.path().join("skills"),
            env.roots.claude_home.join("skills"),
        )
        .unwrap();
        assert_eq!(only_skill(&env).exclusion, Some(Exclusion::LinkOutsideHome));
    }

    #[test]
    fn parent_link_of_a_plugin_install_path_out_of_home_is_refused() {
        let env = env();
        let outside = tempfile::tempdir().unwrap();
        write(&outside.path().join("foo/.claude-plugin/plugin.json"), "{}");
        // ~/plug -> outside, installPath ~/plug/foo
        symlink(outside.path(), env.roots.home.join("plug")).unwrap();
        write_json(
            &env.roots.claude_home.join("plugins/installed_plugins.json"),
            json!({"version": 2, "plugins": {"p@mk": [{"scope": "user", "version": "1", "installPath": env.roots.home.join("plug/foo")}]}}),
        );
        write_json(
            &env.roots.claude_home.join("settings.json"),
            json!({"enabledPlugins": {"p@mk": true}}),
        );
        let inventory = build_inventory(&env.roots, Workspace::Codespace);
        assert_eq!(
            inventory.plugins[0].exclusion,
            Some(Exclusion::LinkOutsideHome)
        );
    }

    #[test]
    fn relative_link_escaping_with_dotdot_is_refused() {
        let env = env();
        let skills = env.roots.claude_home.join("skills");
        skill(&skills, "esc", "x");
        let outside = tempfile::tempdir().unwrap();
        write(&outside.path().join("f.txt"), "outside");
        // A relative target that climbs out of home.
        let depth = env
            .roots
            .claude_home
            .join("skills/esc")
            .components()
            .count();
        let mut target = PathBuf::new();
        for _ in 0..depth {
            target.push("..");
        }
        target.push(outside.path().strip_prefix("/").unwrap());
        target.push("f.txt");
        symlink(&target, skills.join("esc/rel.txt")).unwrap();
        assert_eq!(only_skill(&env).exclusion, Some(Exclusion::LinkLeavesItem));
    }

    #[test]
    fn link_chain_ending_outside_home_is_refused() {
        let env = env();
        let skills = env.roots.claude_home.join("skills");
        skill(&skills, "chain", "x");
        let outside = tempfile::tempdir().unwrap();
        write(&outside.path().join("f.txt"), "outside");
        // inside-home link -> inside-home link -> outside file
        symlink(outside.path().join("f.txt"), env.roots.home.join("hop2")).unwrap();
        symlink(env.roots.home.join("hop2"), env.roots.home.join("hop1")).unwrap();
        symlink(env.roots.home.join("hop1"), skills.join("chain/c.txt")).unwrap();
        assert_eq!(only_skill(&env).exclusion, Some(Exclusion::LinkLeavesItem));
    }

    // Replaces `project_outside_home_may_link_inside_itself_but_not_out`: a link from a
    // project skill to a sibling path inside the project used to be followed. It now
    // excludes the skill, because it leaves the skill's own root.
    #[test]
    fn project_skill_linking_to_elsewhere_in_the_project_is_excluded() {
        let env = env();
        let elsewhere = tempfile::tempdir().unwrap();
        let project = elsewhere.path().to_path_buf();
        write(&project.join("shared/lib/util.txt"), "abc");
        skill(&project.join(".claude/skills"), "foo", "x");
        symlink(
            "../../../shared/lib",
            project.join(".claude/skills/foo/lib"),
        )
        .unwrap();
        let roots = KitRoots {
            project: project.clone(),
            ..env.roots.clone()
        };
        let inventory = build_inventory(&roots, Workspace::Codespace);
        assert_eq!(inventory.skills.len(), 1);
        assert_eq!(
            inventory.skills[0].exclusion,
            Some(Exclusion::LinkLeavesItem)
        );
    }

    #[test]
    fn links_to_other_places_under_home_exclude_the_item() {
        let env = env();
        let skills = env.roots.claude_home.join("skills");
        skill(&skills, "creds", "x");
        write(&env.roots.home.join(".aws/credentials"), "fake");
        symlink(env.roots.home.join(".aws"), skills.join("creds/aws")).unwrap();
        assert_eq!(only_skill(&env).exclusion, Some(Exclusion::LinkLeavesItem));
    }

    #[test]
    fn links_inside_the_item_are_followed() {
        let env = env();
        let skills = env.roots.claude_home.join("skills");
        skill(&skills, "tidy", "x");
        write(&skills.join("tidy/real/data.txt"), "abcd");
        symlink("real", skills.join("tidy/alias")).unwrap();
        symlink("real/data.txt", skills.join("tidy/data-link.txt")).unwrap();
        let found = only_skill(&env);
        assert_eq!(found.exclusion, None);
        assert!(found.size >= 9);
    }

    // The documented dotfiles rule: the ROOT may be a link to anywhere under home, but
    // the content links are judged against the canonical root it resolves to.
    #[test]
    fn a_dotfiles_skill_root_may_be_a_link_but_its_content_links_stay_in_its_target() {
        let env = env();
        let skills = env.roots.claude_home.join("skills");
        std::fs::create_dir_all(&skills).unwrap();
        let target = env.roots.home.join("dotfiles/skills/foo");
        write(&target.join("SKILL.md"), "x");
        write(&target.join("lib/a.txt"), "abc");
        symlink("lib/a.txt", target.join("a-link.txt")).unwrap();
        symlink(&target, skills.join("foo")).unwrap();
        assert_eq!(only_skill(&env).exclusion, None);

        // A sibling in dotfiles is inside home but outside the item: excluded.
        write(&env.roots.home.join("dotfiles/other/o.txt"), "o");
        symlink(env.roots.home.join("dotfiles/other"), target.join("other")).unwrap();
        assert_eq!(only_skill(&env).exclusion, Some(Exclusion::LinkLeavesItem));
    }

    #[test]
    fn a_plugin_with_a_link_out_of_its_install_path_is_excluded() {
        let env = env();
        let install = env.roots.home.join("plugins-store/p");
        write(&install.join(".claude-plugin/plugin.json"), "{}");
        write(&env.roots.home.join(".ssh/id"), "fake");
        symlink(env.roots.home.join(".ssh"), install.join("x")).unwrap();
        write_json(
            &env.roots.claude_home.join("plugins/installed_plugins.json"),
            json!({"version": 2, "plugins": {"p@mk": [{"scope": "user", "version": "1", "installPath": install}]}}),
        );
        write_json(
            &env.roots.claude_home.join("settings.json"),
            json!({"enabledPlugins": {"p@mk": true}}),
        );
        let inventory = build_inventory(&env.roots, Workspace::Codespace);
        assert_eq!(
            inventory.plugins[0].exclusion,
            Some(Exclusion::LinkLeavesItem)
        );
    }
}

mod keyed_secrets {
    use super::*;

    fn with_secret(env: &Env, token: &str) {
        servers_json(
            env,
            json!({
                "a": {"command": "npx", "env": {"K": token, "REF": "${HOME}"}},
                "b": {"type": "http", "url": "https://b.example.com/mcp?key=abc"},
                "plain": {"command": "npx"}
            }),
        );
    }

    #[test]
    fn a_secret_change_changes_the_keyed_digest_and_token_but_not_the_plain_hashes() {
        let env = env();
        with_secret(&env, "fake-one-1111");
        let one = build_inventory(&env.roots, Workspace::Codespace);
        with_secret(&env, "fake-two-2222");
        let two = build_inventory(&env.roots, Workspace::Codespace);
        let (a1, a2) = (&one.mcp_servers[0], &two.mcp_servers[0]);
        assert_eq!(a1.name, "a");
        assert_eq!(a1.config_hash, a2.config_hash);
        assert_eq!(one.content_hash, two.content_hash);
        assert!(a1.secrets_digest.is_some());
        assert_ne!(a1.secrets_digest, a2.secrets_digest);
        assert_ne!(one.change_token(), two.change_token());
        // Same inputs, same token.
        assert_eq!(
            two.change_token(),
            build_inventory(&env.roots, Workspace::Codespace).change_token()
        );
        // A server without secrets has no digest.
        assert_eq!(two.mcp_servers[2].name, "plain");
        assert_eq!(two.mcp_servers[2].secrets_digest, None);
    }

    #[test]
    fn a_url_query_secret_is_masked_in_the_plain_hash_and_keyed() {
        let env = env();
        let hash_with = |query: &str| {
            servers_json(
                &env,
                json!({"b": {"type": "http", "url": format!("https://b.example.com/mcp?key={query}")}}),
            );
            build_inventory(&env.roots, Workspace::Codespace).mcp_servers[0].clone()
        };
        let (x, y) = (hash_with("fake-x"), hash_with("fake-y"));
        assert_eq!(x.config_hash, y.config_hash);
        assert_ne!(x.secrets_digest, y.secrets_digest);
    }

    #[test]
    fn two_installs_with_different_keys_produce_unrelated_digests() {
        let env = env();
        with_secret(&env, "fake-one-1111");
        let first = build_inventory(&env.roots, Workspace::Codespace);
        let other = KitRoots {
            key_dir: Some(env.roots.home.join("other-install")),
            ..env.roots.clone()
        };
        let second = build_inventory(&other, Workspace::Codespace);
        assert_eq!(first.content_hash, second.content_hash);
        assert_ne!(
            first.mcp_servers[0].secrets_digest,
            second.mcp_servers[0].secrets_digest
        );
        assert_ne!(first.change_token(), second.change_token());
    }

    #[test]
    fn debug_output_has_no_secret_digest() {
        let env = env();
        with_secret(&env, FAKE_TOKEN);
        let inventory = build_inventory(&env.roots, Workspace::Codespace);
        let digest = inventory.mcp_servers[0].secrets_digest.clone().unwrap();
        let hex = digest.hex_for_test();
        let token = inventory.change_token();
        for text in [format!("{inventory:?}"), format!("{inventory:#?}")] {
            assert!(!text.contains(hex));
            assert!(!text.contains(&token));
            assert!(!text.contains(FAKE_TOKEN));
            assert!(text.contains("KeyedDigest(<redacted>)"));
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_missing_key_file_is_created_with_mode_0600_and_reused() {
        use std::os::unix::fs::PermissionsExt;
        let env = env();
        let key_path = env.roots.home.join("app-data/kit-hash.key");
        assert!(!key_path.exists());
        with_secret(&env, "fake-one-1111");
        let first = build_inventory(&env.roots, Workspace::Codespace);
        let mode = std::fs::metadata(&key_path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        let saved = std::fs::read_to_string(&key_path).unwrap();
        assert_eq!(saved.trim().len(), 64);
        // A second run reuses the same key, so the digests match.
        let second = build_inventory(&env.roots, Workspace::Codespace);
        assert_eq!(std::fs::read_to_string(&key_path).unwrap(), saved);
        assert_eq!(first.change_token(), second.change_token());
        assert!(!first.incomplete);
    }

    #[test]
    fn an_unusable_key_degrades_without_panicking() {
        let env = env();
        with_secret(&env, "fake-one-1111");
        let key_path = env.roots.home.join("app-data/kit-hash.key");

        // Corrupt content: not replaced, secrets left out, inventory flagged.
        write(&key_path, "not a key");
        let corrupt = build_inventory(&env.roots, Workspace::Codespace);
        assert_eq!(std::fs::read_to_string(&key_path).unwrap(), "not a key");
        assert!(corrupt.incomplete);
        assert!(corrupt
            .mcp_servers
            .iter()
            .all(|s| s.secrets_digest.is_none()));
        assert!(!corrupt.content_hash.is_empty());

        // No key location at all.
        let none = KitRoots {
            key_dir: None,
            ..env.roots.clone()
        };
        assert!(build_inventory(&none, Workspace::Codespace).incomplete);

        // A key location that is a file, so the key cannot be created.
        let blocked = env.roots.home.join("blocked");
        write(&blocked, "file");
        let roots = KitRoots {
            key_dir: Some(blocked),
            ..env.roots.clone()
        };
        assert!(build_inventory(&roots, Workspace::Codespace).incomplete);

        // Without secrets nothing needs a key, so nothing is flagged.
        servers_json(&env, json!({"plain": {"command": "npx"}}));
        assert!(!build_inventory(&none, Workspace::Codespace).incomplete);
    }

    #[cfg(unix)]
    #[test]
    fn an_unreadable_key_file_degrades_without_panicking() {
        use std::os::unix::fs::PermissionsExt;
        let env = env();
        with_secret(&env, "fake-one-1111");
        let key_path = env.roots.home.join("app-data/kit-hash.key");
        write(&key_path, &"ab".repeat(32));
        std::fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o000)).unwrap();
        let readable = std::fs::read_to_string(&key_path).is_ok(); // true when running as root
        let inventory = build_inventory(&env.roots, Workspace::Codespace);
        std::fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o600)).unwrap();
        if !readable {
            assert!(inventory.incomplete);
            assert!(inventory
                .mcp_servers
                .iter()
                .all(|s| s.secrets_digest.is_none()));
        }
    }

    #[test]
    fn hmac_matches_the_rfc_4231_vector() {
        // RFC 4231 test case 2.
        let mac = super::super::kit::hmac_for_test(b"Jefe", b"what do ya want for nothing?");
        assert_eq!(
            mac,
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
    }
}

mod key_file {
    use super::*;

    fn key_path(env: &Env) -> PathBuf {
        env.roots.home.join("app-data/kit-hash.key")
    }

    fn secret_env() -> Env {
        let env = env();
        servers_json(
            &env,
            json!({"a": {"command": "npx", "env": {"K": "fake-k"}}}),
        );
        env
    }

    #[test]
    fn an_empty_or_malformed_key_file_is_recreated_as_a_valid_one() {
        for garbage in ["", "   \n", "not a key", &"zz".repeat(32), &"ab".repeat(10)] {
            let env = secret_env();
            write(&key_path(&env), garbage);
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(key_path(&env), std::fs::Permissions::from_mode(0o600))
                    .unwrap();
            }
            let inventory = build_inventory(&env.roots, Workspace::Codespace);
            assert!(!inventory.incomplete, "{garbage:?}");
            assert!(inventory.mcp_servers[0].secrets_digest.is_some());
            let saved = std::fs::read_to_string(key_path(&env)).unwrap();
            assert_eq!(saved.trim().len(), 64);
            // A valid key is never replaced.
            build_inventory(&env.roots, Workspace::Codespace);
            assert_eq!(std::fs::read_to_string(key_path(&env)).unwrap(), saved);
            // No temp files are left behind.
            let names: Vec<_> = std::fs::read_dir(key_path(&env).parent().unwrap())
                .unwrap()
                .map(|e| e.unwrap().file_name())
                .collect();
            assert_eq!(names.len(), 1, "{names:?}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_group_or_world_readable_key_is_refused_and_left_alone() {
        use std::os::unix::fs::PermissionsExt;
        let env = secret_env();
        let valid = "ab".repeat(32);
        write(&key_path(&env), &valid);
        std::fs::set_permissions(key_path(&env), std::fs::Permissions::from_mode(0o644)).unwrap();
        let inventory = build_inventory(&env.roots, Workspace::Codespace);
        assert!(inventory.key_unavailable && inventory.incomplete);
        assert_eq!(std::fs::read_to_string(key_path(&env)).unwrap(), valid);
    }

    #[test]
    fn a_missing_key_does_not_count_as_a_timed_out_scan() {
        let env = secret_env();
        let none = KitRoots {
            key_dir: None,
            ..env.roots.clone()
        };
        let inventory = build_inventory(&none, Workspace::Codespace);
        assert!(inventory.incomplete && inventory.key_unavailable);
        assert!(!inventory.scan_timed_out, "caching must stay on");
        assert!(!inventory.over_total_cap);

        skill(&env.roots.claude_home.join("skills"), "s", "x");
        let timed = build_inventory_budgeted(
            &env.roots,
            Workspace::Codespace,
            &|_| None,
            std::time::Duration::ZERO,
        );
        assert!(timed.scan_timed_out && timed.incomplete);
        assert!(!timed.key_unavailable);
    }

    #[test]
    fn empty_or_relative_data_dirs_are_ignored() {
        let home = Path::new("/home/u");
        let default = home.join(".cache/monocode-native");
        let dir = |v: &str| data_dir_from(Some(v.into()), home);
        assert_eq!(dir(""), default);
        assert_eq!(dir("relative/dir"), default);
        assert_eq!(dir("/abs/data"), PathBuf::from("/abs/data"));
        assert_eq!(data_dir_from(None, home), default);
    }
}

#[test]
fn a_url_query_secret_masks_only_the_query_in_the_plain_hash() {
    let env = env();
    let hash = |url: &str| {
        servers_json(&env, json!({"b": {"type": "http", "url": url}}));
        build_inventory(&env.roots, Workspace::Codespace).mcp_servers[0].clone()
    };
    let base = hash("https://one.example.com/mcp?key=aaa");
    let other_host = hash("https://two.example.com/mcp?key=aaa");
    let other_path = hash("https://one.example.com/other?key=aaa");
    let other_secret = hash("https://one.example.com/mcp?key=bbb");
    assert_ne!(base.config_hash, other_host.config_hash);
    assert_ne!(base.config_hash, other_path.config_hash);
    assert_eq!(base.config_hash, other_secret.config_hash);
    assert_ne!(base.secrets_digest, other_secret.secrets_digest);
}

#[cfg(unix)]
#[test]
fn a_hard_link_to_a_credentials_file_is_refused_and_other_hard_links_are_a_known_limit() {
    let env = env();
    let skills = env.roots.claude_home.join("skills");
    skill(&skills, "secret", "x");
    skill(&skills, "plain", "x");
    write(&env.roots.home.join(".aws/credentials"), "fake");
    write(&env.roots.home.join(".claude/.credentials.json"), "fake");
    write(&env.roots.home.join("other/file.txt"), "abc");
    std::fs::hard_link(
        env.roots.home.join(".aws/credentials"),
        skills.join("secret/c.txt"),
    )
    .unwrap();
    skill(&skills, "oauth", "x");
    std::fs::hard_link(
        env.roots.home.join(".claude/.credentials.json"),
        skills.join("oauth/c.json"),
    )
    .unwrap();
    std::fs::hard_link(
        env.roots.home.join("other/file.txt"),
        skills.join("plain/file.txt"),
    )
    .unwrap();
    let inventory = build_inventory(&env.roots, Workspace::Codespace);
    let rows: Vec<_> = inventory
        .skills
        .iter()
        .map(|s| (s.name.as_str(), s.exclusion))
        .collect();
    assert_eq!(
        rows,
        vec![
            ("oauth", Some(Exclusion::HardLinkElsewhere)),
            ("plain", None),
            ("secret", Some(Exclusion::HardLinkElsewhere)),
        ]
    );
}

#[cfg(unix)]
#[test]
fn a_folder_named_like_a_credentials_folder_inside_a_skill_excludes_it() {
    for dir in [".ssh", ".aws", ".gnupg", ".kube", ".docker"] {
        let env = env();
        let skills = env.roots.claude_home.join("skills");
        skill(&skills, "s", "x");
        write(&skills.join("s").join(dir).join("f"), "fake");
        let inventory = build_inventory(&env.roots, Workspace::Codespace);
        assert_eq!(
            inventory.skills[0].exclusion,
            Some(Exclusion::SensitiveContent),
            "{dir}"
        );
    }
}

#[test]
fn a_url_password_is_a_secret_with_or_without_a_query() {
    for url in [
        "https://bot:{}@mcp.example.com/mcp?x=1",
        "https://bot:{}@mcp.example.com/mcp",
        "https://{}@mcp.example.com:8443/mcp",
    ] {
        let env = env();
        let build = |secret: &str| {
            servers_json(
                &env,
                json!({"b": {"type": "http", "url": url.replace("{}", secret)}}),
            );
            let inventory = build_inventory(&env.roots, Workspace::Codespace);
            (inventory.mcp_servers[0].clone(), inventory.content_hash)
        };
        let (one, content_one) = build("s3cr3t-one");
        let (two, content_two) = build("s3cr3t-two");
        assert!(!one.secrets.is_empty(), "{url}");
        assert_eq!(one.config_hash, two.config_hash, "{url}");
        assert_eq!(content_one, content_two, "{url}");
        assert_ne!(one.secrets_digest, two.secrets_digest, "{url}");
        assert!(!format!("{one:?}").contains("s3cr3t"), "{url}");
    }
}

#[cfg(unix)]
mod key_race {
    use super::super::kit::retire_garbage_for_test as retire;
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    const VALID: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    fn write_key(path: &std::path::Path, text: &str) {
        std::fs::write(path, text).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }

    fn id(path: &std::path::Path) -> Option<(u64, u64)> {
        let meta = std::fs::symlink_metadata(path).unwrap();
        Some((meta.dev(), meta.ino()))
    }

    #[test]
    fn garbage_replaced_by_a_valid_key_before_removal_is_kept() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("kit-hash.key");
        write_key(&path, "garbage");
        let seen = id(&path);
        // Another writer replaces the garbage with a valid key (new inode).
        let other = dir.path().join("other");
        write_key(&other, VALID);
        std::fs::rename(&other, &path).unwrap();
        assert!(retire(&path, seen).is_some());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), VALID);
    }

    #[test]
    fn a_valid_key_moved_aside_by_mistake_is_put_back() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("kit-hash.key");
        write_key(&path, VALID);
        // Identity matches (as if the valid key landed in place after we read garbage).
        assert!(retire(&path, id(&path)).is_some());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), VALID);
        let names: Vec<_> = std::fs::read_dir(dir.path()).unwrap().flatten().collect();
        assert_eq!(names.len(), 1);
    }

    #[test]
    fn real_garbage_is_cleared_and_leaves_nothing_behind() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("kit-hash.key");
        write_key(&path, "garbage");
        assert!(retire(&path, id(&path)).is_none());
        assert!(!path.exists());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }
}

#[cfg(unix)]
mod roots {
    use super::*;
    use std::os::unix::fs::symlink;

    fn first_skill(env: &Env) -> KitSkill {
        build_inventory(&env.roots, Workspace::Codespace)
            .skills
            .into_iter()
            .next()
            .expect("a skill")
    }

    #[test]
    fn a_skill_root_linking_to_a_credentials_folder_is_excluded() {
        for dir in [
            ".ssh",
            ".aws",
            ".gnupg",
            ".config/gcloud",
            "Library/Keychains/x",
        ] {
            let env = env();
            let target = env.roots.home.join(dir);
            write(&target.join("SKILL.md"), "x");
            let skills = env.roots.claude_home.join("skills");
            std::fs::create_dir_all(&skills).unwrap();
            symlink(&target, skills.join("foo")).unwrap();
            assert_eq!(
                first_skill(&env).exclusion,
                Some(Exclusion::UnsafeRoot),
                "{dir}"
            );
        }
    }

    #[test]
    fn roots_that_are_home_or_contain_it_or_contain_their_own_path_are_excluded() {
        let env = env();
        let skills = env.roots.claude_home.join("skills");
        std::fs::create_dir_all(&skills).unwrap();
        write(&env.roots.home.join("SKILL.md"), "x");
        symlink(&env.roots.home, skills.join("home")).unwrap();
        // foo -> the skills folder that holds it
        write(&skills.join("SKILL.md"), "x");
        symlink(&skills, skills.join("up")).unwrap();
        let inventory = build_inventory(&env.roots, Workspace::Codespace);
        for skill in &inventory.skills {
            assert_eq!(
                skill.exclusion,
                Some(Exclusion::UnsafeRoot),
                "{}",
                skill.name
            );
        }
        assert_eq!(inventory.skills.len(), 2);
    }

    #[test]
    fn plugin_install_paths_that_are_unsafe_are_excluded() {
        for raw in ["~", "/", "/etc", "~/.ssh"] {
            let env = env();
            write(&env.roots.home.join(".ssh/id"), "fake");
            write_json(
                &env.roots.claude_home.join("plugins/installed_plugins.json"),
                json!({"version": 2, "plugins": {"p@mk": [{"scope": "user", "version": "1", "installPath": raw}]}}),
            );
            write_json(
                &env.roots.claude_home.join("settings.json"),
                json!({"enabledPlugins": {"p@mk": true}}),
            );
            let inventory = build_inventory(&env.roots, Workspace::Codespace);
            assert_eq!(
                inventory.plugins[0].exclusion,
                Some(Exclusion::UnsafeRoot),
                "{raw}"
            );
        }
    }

    #[test]
    fn a_root_outside_home_must_stay_in_the_project_or_beside_its_link() {
        let env = env();
        let elsewhere = tempfile::tempdir().unwrap();
        let project = std::fs::canonicalize(elsewhere.path()).unwrap();
        let skills = project.join(".claude/skills");
        std::fs::create_dir_all(&skills).unwrap();
        // Inside the project: fine.
        write(&project.join("shared/ok/SKILL.md"), "x");
        symlink(project.join("shared/ok"), skills.join("ok")).unwrap();
        // Somewhere else on disk: refused.
        let stray = tempfile::tempdir().unwrap();
        write(&stray.path().join("SKILL.md"), "x");
        symlink(stray.path(), skills.join("stray")).unwrap();
        let roots = KitRoots {
            project,
            ..env.roots.clone()
        };
        let rows: Vec<_> = build_inventory(&roots, Workspace::Codespace)
            .skills
            .iter()
            .map(|s| (s.name.clone(), s.exclusion))
            .collect();
        assert_eq!(
            rows,
            vec![
                ("ok".to_string(), None),
                ("stray".to_string(), Some(Exclusion::UnsafeRoot))
            ]
        );
    }

    #[test]
    fn a_root_that_contains_a_credentials_folder_or_claude_home_is_excluded() {
        for target in [".config", ".claude", "."] {
            let env = env();
            write(&env.roots.home.join(".config/gh/hosts.yml"), "fake");
            write(&env.roots.home.join(".claude/.credentials.json"), "fake");
            write(&env.roots.home.join(".config/SKILL.md"), "x");
            write(&env.roots.home.join(".claude/SKILL.md"), "x");
            write(&env.roots.home.join("SKILL.md"), "x");
            let skills = env.roots.claude_home.join("skills");
            std::fs::create_dir_all(&skills).unwrap();
            symlink(env.roots.home.join(target), skills.join("foo")).unwrap();
            let project_skills = env.roots.project.join(".claude/skills");
            std::fs::create_dir_all(&project_skills).unwrap();
            symlink(env.roots.home.join(target), project_skills.join("bar")).unwrap();
            let inventory = build_inventory(&env.roots, Workspace::Codespace);
            assert!(!inventory.skills.is_empty());
            for skill in &inventory.skills {
                assert_eq!(skill.exclusion, Some(Exclusion::UnsafeRoot), "{target}");
            }
        }
    }

    #[test]
    fn a_plugin_install_path_that_contains_credentials_is_excluded() {
        for rel in [".config", ".claude"] {
            let env = env();
            write(&env.roots.home.join(".config/gh/hosts.yml"), "fake");
            write(&env.roots.home.join(".claude/.credentials.json"), "fake");
            let raw = env.roots.home.join(rel).display().to_string();
            write_json(
                &env.roots.claude_home.join("plugins/installed_plugins.json"),
                json!({"version": 2, "plugins": {"p@mk": [{"scope": "user", "version": "1", "installPath": raw}]}}),
            );
            write_json(
                &env.roots.claude_home.join("settings.json"),
                json!({"enabledPlugins": {"p@mk": true}}),
            );
            let inventory = build_inventory(&env.roots, Workspace::Codespace);
            assert_eq!(
                inventory.plugins[0].exclusion,
                Some(Exclusion::UnsafeRoot),
                "{rel}"
            );
        }
    }

    #[test]
    fn real_skill_and_plugin_dirs_under_claude_home_are_still_accepted() {
        let env = env();
        let skills = env.roots.claude_home.join("skills");
        skill(&skills, "real", "x");
        let inventory = build_inventory(&env.roots, Workspace::Codespace);
        assert_eq!(inventory.skills[0].exclusion, None);
        let install = env.roots.claude_home.join("plugins/cache/mk/p/1");
        write(&install.join("README.md"), "x");
        write_json(
            &env.roots.claude_home.join("plugins/installed_plugins.json"),
            json!({"version": 2, "plugins": {"p@mk": [{"scope": "user", "version": "1", "installPath": install.display().to_string()}]}}),
        );
        write_json(
            &env.roots.claude_home.join("settings.json"),
            json!({"enabledPlugins": {"p@mk": true}}),
        );
        let inventory = build_inventory(&env.roots, Workspace::Codespace);
        assert_eq!(inventory.plugins[0].exclusion, None);
    }

    #[test]
    fn a_parent_link_is_refused_when_home_itself_is_behind_a_link() {
        let env = env();
        let alias = env.dir.path().join("alias-home");
        symlink(&env.roots.home, &alias).unwrap();
        let project = alias.join("work/app");
        let skills = project.join(".claude/skills");
        std::fs::create_dir_all(&skills).unwrap();
        write(&project.join(".claude/SKILL.md"), "x");
        symlink("..", skills.join("up")).unwrap();
        let roots = KitRoots {
            account_dir: None,
            claude_home: alias.join(".claude"),
            project,
            key_dir: Some(alias.join("app-data")),
            home: alias,
        };
        let inventory = build_inventory(&roots, Workspace::Codespace);
        assert_eq!(inventory.skills.len(), 1);
        assert_eq!(inventory.skills[0].exclusion, Some(Exclusion::UnsafeRoot));
    }

    #[test]
    fn a_dotfiles_root_is_still_accepted() {
        let env = env();
        let target = env.roots.home.join("dotfiles/skills/foo");
        write(&target.join("SKILL.md"), "x");
        let skills = env.roots.claude_home.join("skills");
        std::fs::create_dir_all(&skills).unwrap();
        symlink(&target, skills.join("foo")).unwrap();
        assert_eq!(first_skill(&env).exclusion, None);
    }
}
