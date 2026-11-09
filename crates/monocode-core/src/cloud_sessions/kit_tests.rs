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
        "https://${HOST}/mcp",
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
fn symlinks_inside_home_are_followed_and_outside_are_refused() {
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
            ("leaky", Some(Exclusion::LinkOutsideHome)),
            ("linked-skill", None),
            ("loop", None),
        ]
    );
}

#[cfg(unix)]
#[test]
fn a_plugin_installed_outside_home_is_refused() {
    let env = env();
    let outside = tempfile::tempdir().unwrap();
    write(&outside.path().join(".claude-plugin/plugin.json"), "{}");
    let registry = env.roots.claude_home.join("plugins/installed_plugins.json");
    write_json(
        &registry,
        json!({"version": 2, "plugins": {"far@mk": [{"scope": "user", "version": "1", "installPath": outside.path()}]}}),
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
    let secret_changed = build_inventory(&env.roots, Workspace::Codespace).content_hash;
    assert_ne!(secret_changed, skill_changed);
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
