use super::kit::*;
use super::kit_pack::*;
use super::kit_prefs::CloudKitPrefs;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};

// Every value below is fake.
const FAKE_TOKEN: &str = "fake-token-AAAA1111";
const FAKE_HEADER: &str = "Bearer fake-header-BBBB2222";
const FAKE_QUERY: &str = "fake-query-CCCC3333";

struct Env {
    _dir: tempfile::TempDir,
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
    Env { _dir: dir, roots }
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn skill(env: &Env, name: &str, body: &str) -> PathBuf {
    let dir = env.roots.claude_home.join("skills").join(name);
    write(&dir.join("SKILL.md"), body);
    dir
}

fn servers(env: &Env, servers: Value) {
    write(
        &env.roots.home.join(".claude.json"),
        &json!({ "mcpServers": servers }).to_string(),
    );
}

fn on() -> CloudKitPrefs {
    CloudKitPrefs {
        enabled: true,
        notice_acknowledged: true,
        selection: None,
        ..Default::default()
    }
}

fn all_present() -> Present {
    Present::known(PROBE_COMMANDS.iter().map(|c| c.to_string()))
}

fn pack_env(env: &Env, present: &Present) -> Packed {
    let inventory = build_inventory(&env.roots, Workspace::Codespace);
    pack(&inventory, &env.roots, &on(), present).unwrap()
}

/// Every entry of the tar: path -> (is dir, mtime, uid, mode, bytes).
fn read_tar(packed: &Packed) -> BTreeMap<String, (bool, u64, u64, u32, Vec<u8>)> {
    let mut archive = tar::Archive::new(std::fs::File::open(packed.file.path()).unwrap());
    let mut out = BTreeMap::new();
    for entry in archive.entries().unwrap() {
        let mut entry = entry.unwrap();
        let path = entry
            .path()
            .unwrap()
            .to_string_lossy()
            .trim_end_matches('/')
            .to_owned();
        let header = entry.header().clone();
        let mut data = Vec::new();
        entry.read_to_end(&mut data).unwrap();
        out.insert(
            path,
            (
                header.entry_type().is_dir(),
                header.mtime().unwrap(),
                header.uid().unwrap(),
                header.mode().unwrap(),
                data,
            ),
        );
    }
    out
}

fn mcp_json(packed: &Packed) -> Value {
    let tar = read_tar(packed);
    serde_json::from_slice(&tar["mcp.json"].4).unwrap()
}

fn contains(haystack: &[u8], needle: &str) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle.as_bytes())
}

#[test]
fn the_tar_is_deterministic_and_has_only_the_host_layout() {
    let env = env();
    skill(&env, "review", "# review");
    write(
        &env.roots.claude_home.join("skills/review/scripts/run.sh"),
        "#!/bin/sh\n",
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let run = env.roots.claude_home.join("skills/review/scripts/run.sh");
        std::fs::set_permissions(&run, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    skill(&env, "deploy", "# deploy");
    servers(&env, json!({"fetch": {"command": "npx", "args": ["x"]}}));
    let first = pack_env(&env, &all_present());
    // Same files a moment later, different mtimes: same bytes.
    std::thread::sleep(std::time::Duration::from_millis(1100));
    for name in ["review", "deploy"] {
        let file = env
            .roots
            .claude_home
            .join("skills")
            .join(name)
            .join("SKILL.md");
        let text = std::fs::read_to_string(&file).unwrap();
        std::fs::write(&file, text).unwrap();
    }
    let second = pack_env(&env, &all_present());
    assert_eq!(first.sha256, second.sha256);
    assert_eq!(
        std::fs::read(first.file.path()).unwrap(),
        std::fs::read(second.file.path()).unwrap()
    );
    let tar = read_tar(&first);
    let paths: Vec<&str> = tar.keys().map(String::as_str).collect();
    assert_eq!(
        paths,
        [
            "kit.json",
            "mcp.json",
            "skills",
            "skills/deploy",
            "skills/deploy/SKILL.md",
            "skills/review",
            "skills/review/SKILL.md",
            "skills/review/scripts",
            "skills/review/scripts/run.sh",
        ]
    );
    for (path, (dir, mtime, uid, mode, _)) in &tar {
        assert_eq!((*mtime, *uid), (0, 0), "{path}");
        if !dir {
            let want = if path.ends_with("run.sh") && cfg!(unix) {
                0o755
            } else {
                0o644
            };
            assert_eq!(*mode, want, "{path}");
        }
    }
    assert_eq!(first.skills, ["deploy", "review"]);
}

#[test]
fn secrets_become_references_and_never_enter_the_tar() {
    let env = env();
    servers(
        &env,
        json!({
            "github": {"command": "npx", "env": {"GITHUB_TOKEN": FAKE_TOKEN, "REGION": "${REGION}"}},
            "site": {"type": "http", "url": "https://x.example/mcp",
                     "headers": {"Authorization": FAKE_HEADER}},
            "q": {"type": "http", "url": format!("https://q.example/mcp?key={FAKE_QUERY}")},
        }),
    );
    let packed = pack_env(&env, &all_present());
    let bytes = std::fs::read(packed.file.path()).unwrap();
    for secret in [FAKE_TOKEN, FAKE_HEADER, FAKE_QUERY] {
        assert!(!contains(&bytes, secret), "{secret} is in the tar");
    }
    let mcp = mcp_json(&packed);
    let servers = &mcp["mcpServers"];
    assert_eq!(
        servers["github"]["env"]["GITHUB_TOKEN"],
        "${MONOPTAH_KIT_GITHUB_GITHUB_TOKEN}"
    );
    // A pure reference is not a secret and stays as it is.
    assert_eq!(servers["github"]["env"]["REGION"], "${REGION}");
    assert_eq!(
        servers["site"]["headers"]["Authorization"],
        "${MONOPTAH_KIT_SITE_HEADER_AUTHORIZATION}"
    );
    assert_eq!(servers["q"]["url"], "${MONOPTAH_KIT_Q_URL}");
    let secrets: BTreeMap<_, _> = packed
        .secrets
        .iter()
        .map(|(n, v)| (n.clone(), v.as_str().to_owned()))
        .collect();
    assert_eq!(secrets["MONOPTAH_KIT_GITHUB_GITHUB_TOKEN"], FAKE_TOKEN);
    assert_eq!(
        secrets["MONOPTAH_KIT_SITE_HEADER_AUTHORIZATION"],
        FAKE_HEADER
    );
    assert_eq!(
        secrets["MONOPTAH_KIT_Q_URL"],
        format!("https://q.example/mcp?key={FAKE_QUERY}")
    );
    for name in secrets.keys() {
        assert!(name.starts_with("MONOPTAH_KIT_"), "{name}");
        assert!(
            name.bytes()
                .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_'),
            "{name}"
        );
        assert!(name.len() <= "MONOPTAH_KIT_".len() + 128);
    }
    // Names are stable between runs.
    let again = pack_env(&env, &all_present());
    let names = |p: &Packed| p.secrets.iter().map(|s| s.0.clone()).collect::<Vec<_>>();
    assert_eq!(names(&packed), names(&again));
    // Nothing prints a value.
    let debug = format!(
        "{packed:?} {:?}",
        build_inventory(&env.roots, Workspace::Codespace)
    );
    for secret in [FAKE_TOKEN, FAKE_HEADER, FAKE_QUERY] {
        assert!(!debug.contains(secret));
    }
}

#[test]
fn secret_names_that_collide_stay_unique() {
    let env = env();
    servers(
        &env,
        json!({
            "a-b": {"command": "npx", "env": {"KEY": "value-one-1111"}},
            "a_b": {"command": "npx", "env": {"KEY": "value-two-2222"}},
        }),
    );
    let packed = pack_env(&env, &all_present());
    let names: Vec<_> = packed.secrets.iter().map(|s| s.0.as_str()).collect();
    assert_eq!(names.len(), 2);
    assert_ne!(names[0], names[1]);
}

#[test]
fn an_oauth_style_server_is_carried_config_only() {
    let env = env();
    servers(
        &env,
        json!({"linear": {"type": "http", "url": "https://mcp.linear.example/mcp"}}),
    );
    let packed = pack_env(&env, &all_present());
    assert!(packed.secrets.is_empty());
    assert_eq!(
        packed.servers,
        [PackedServer {
            name: "linear".into(),
            oauth_style: true,
            hosts: vec!["mcp.linear.example".into()],
        }]
    );
    assert_eq!(
        mcp_json(&packed)["mcpServers"]["linear"],
        json!({"type": "http", "url": "https://mcp.linear.example/mcp"})
    );
}

#[test]
fn a_missing_runtime_drops_the_server_with_a_warning() {
    let env = env();
    servers(
        &env,
        json!({
            "fetch": {"command": "npx", "args": ["x"]},
            "py": {"command": "python3"},
        }),
    );
    let without_npx = Present::known(["python3".to_string()]);
    let packed = pack_env(&env, &without_npx);
    let names: Vec<_> = packed.servers.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, ["py"]);
    assert!(
        packed
            .warnings
            .iter()
            .any(|w| w.contains("fetch") && w.contains("npx")),
        "{:?}",
        packed.warnings
    );
    // A probe that failed drops nothing: the health check will say.
    let unknown = pack_env(&env, &Present::unknown());
    assert_eq!(unknown.servers.len(), 2);
}

#[test]
fn a_local_server_replaces_a_personal_one_of_the_same_name() {
    let env = env();
    let project = env.roots.project.to_string_lossy().into_owned();
    write(
        &env.roots.home.join(".claude.json"),
        &json!({
            "mcpServers": {"docs": {"command": "npx", "args": ["personal"]}},
            "projects": {project: {"mcpServers": {"docs": {"command": "npx", "args": ["local"]}}}},
        })
        .to_string(),
    );
    let packed = pack_env(&env, &all_present());
    assert_eq!(
        mcp_json(&packed)["mcpServers"]["docs"]["args"],
        json!(["local"])
    );
    assert_eq!(packed.servers.len(), 1);
}

#[test]
fn a_server_whose_config_changed_after_the_scan_is_not_sent() {
    let env = env();
    servers(&env, json!({"fetch": {"command": "npx", "args": ["one"]}}));
    let inventory = build_inventory(&env.roots, Workspace::Codespace);
    servers(&env, json!({"fetch": {"command": "npx", "args": ["two"]}}));
    let packed = pack(&inventory, &env.roots, &on(), &all_present()).unwrap();
    assert!(packed.servers.is_empty());
    assert!(packed.warnings.iter().any(|w| w.contains("changed")));
}

#[test]
fn unsendable_names_are_refused_with_a_warning() {
    let env = env();
    skill(&env, "Bad Name", "# x");
    servers(&env, json!({"fine": {"command": "npx"}}));
    let packed = pack_env(&env, &all_present());
    assert!(packed.skills.is_empty());
    assert!(packed.warnings.iter().any(|w| w.contains("Bad Name")));
    assert!(valid_skill_name("my-skill-2") && !valid_skill_name("My") && !valid_skill_name("a--b"));
    assert!(valid_plugin_id("fmt@market") && !valid_plugin_id("a/b") && !valid_plugin_id(""));
    assert!(valid_server_name("a.b_c@d-e") && !valid_server_name("a b"));
}

#[test]
fn a_file_rewritten_to_the_same_size_with_another_time_is_refused() {
    let env = env();
    let dir = skill(&env, "review", "# review");
    let inventory = build_inventory(&env.roots, Workspace::Codespace);
    // Same inode and same length: only the modification time tells it apart.
    let path = dir.join("SKILL.md");
    let file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
    let later = std::time::SystemTime::now() + std::time::Duration::from_secs(3600);
    file.set_modified(later).unwrap();
    drop(file);
    let packed = pack(&inventory, &env.roots, &on(), &all_present()).unwrap();
    assert!(packed.skills.is_empty());
    assert!(packed.warnings.iter().any(|w| w.contains("changed")));
}

#[cfg(unix)]
mod links {
    use super::*;
    use std::os::unix::fs::symlink;

    fn private_file(env: &Env) -> PathBuf {
        let path = env.roots.home.join("private/secret.txt");
        write(&path, "private-data-ZZZZ");
        path
    }

    #[test]
    fn links_inside_a_skill_keep_their_own_names_in_the_tar() {
        let env = env();
        let dir = skill(&env, "review", "# review");
        write(&dir.join("shared/docs/a.md"), "doc a");
        symlink("SKILL.md", dir.join("README.md")).unwrap();
        symlink("shared/docs", dir.join("docs")).unwrap();
        let packed = pack_env(&env, &all_present());
        assert_eq!(packed.skills, ["review"], "{:?}", packed.warnings);
        let tar = read_tar(&packed);
        let paths: Vec<&str> = tar.keys().map(String::as_str).collect();
        // The file link and the folder link exist under their own names, with the bytes of
        // their targets, and the targets are there as well.
        for want in [
            "skills/review/SKILL.md",
            "skills/review/README.md",
            "skills/review/shared/docs/a.md",
            "skills/review/docs/a.md",
        ] {
            assert!(paths.contains(&want), "{want} missing in {paths:?}");
        }
        assert_eq!(tar["skills/review/README.md"].4, b"# review");
        assert_eq!(tar["skills/review/docs/a.md"].4, b"doc a");
    }

    #[test]
    fn a_link_to_a_folder_that_contains_it_ends_the_walk() {
        let env = env();
        let dir = skill(&env, "review", "# review");
        symlink(".", dir.join("again")).unwrap();
        let packed = pack_env(&env, &all_present());
        assert_eq!(packed.skills, ["review"]);
        let tar = read_tar(&packed);
        assert!(
            !tar.keys().any(|p| p.contains("again/again")),
            "{:?}",
            tar.keys()
        );
    }

    #[test]
    fn a_file_swapped_for_a_link_after_the_scan_is_refused() {
        let env = env();
        let dir = skill(&env, "review", "# review");
        write(&dir.join("notes.md"), "notes");
        let inventory = build_inventory(&env.roots, Workspace::Codespace);
        let private = private_file(&env);
        std::fs::remove_file(dir.join("notes.md")).unwrap();
        symlink(&private, dir.join("notes.md")).unwrap();
        let packed = pack(&inventory, &env.roots, &on(), &all_present()).unwrap();
        assert!(packed.skills.is_empty(), "{:?}", packed.skills);
        assert!(packed.warnings.iter().any(|w| w.contains("review")));
        assert!(!contains(
            &std::fs::read(packed.file.path()).unwrap(),
            "private-data"
        ));
    }

    #[test]
    fn a_folder_swapped_for_a_link_after_the_scan_is_refused() {
        let env = env();
        let dir = skill(&env, "review", "# review");
        write(&dir.join("sub/a.md"), "inside");
        let inventory = build_inventory(&env.roots, Workspace::Codespace);
        let outside = env.roots.home.join("elsewhere");
        write(&outside.join("a.md"), "private-data-YYYY");
        std::fs::remove_dir_all(dir.join("sub")).unwrap();
        symlink(&outside, dir.join("sub")).unwrap();
        let packed = pack(&inventory, &env.roots, &on(), &all_present()).unwrap();
        assert!(packed.skills.is_empty());
        assert!(!contains(
            &std::fs::read(packed.file.path()).unwrap(),
            "private-data"
        ));
    }

    #[test]
    fn a_replaced_file_is_refused() {
        let env = env();
        let dir = skill(&env, "review", "# review");
        let inventory = build_inventory(&env.roots, Workspace::Codespace);
        // Same path, a different file (another inode).
        std::fs::remove_file(dir.join("SKILL.md")).unwrap();
        write(&dir.join("SKILL.md"), "# replaced");
        let packed = pack(&inventory, &env.roots, &on(), &all_present()).unwrap();
        assert!(packed.skills.is_empty());
        assert!(packed.warnings.iter().any(|w| w.contains("changed")));
    }

    #[test]
    fn a_link_inside_a_skill_never_reaches_the_tar() {
        let env = env();
        let dir = skill(&env, "review", "# review");
        symlink(private_file(&env), dir.join("creds")).unwrap();
        let packed = pack_env(&env, &all_present());
        // The scan already excludes the item; the builder sends nothing of it.
        assert!(packed.skills.is_empty());
        assert!(!contains(
            &std::fs::read(packed.file.path()).unwrap(),
            "private-data"
        ));
    }

    #[test]
    fn a_path_that_escapes_its_item_is_refused() {
        let env = env();
        let dir = skill(&env, "review", "# review");
        let mut inventory = build_inventory(&env.roots, Workspace::Codespace);
        let private = private_file(&env);
        // A scan record that names a file outside the item (a corrupted inventory).
        inventory.skills[0].files.push(KitFile {
            path: private.clone(),
            id: None,
            stamp: None,
            name: "stolen.txt".into(),
        });
        let packed = pack(&inventory, &env.roots, &on(), &all_present()).unwrap();
        assert!(packed.skills.is_empty());
        assert!(dir.join("SKILL.md").exists());
        assert!(!contains(
            &std::fs::read(packed.file.path()).unwrap(),
            "private-data"
        ));
    }

    #[test]
    fn a_file_over_the_per_file_cap_is_refused() {
        let env = env();
        let dir = skill(&env, "review", "# review");
        let mut inventory = build_inventory(&env.roots, Workspace::Codespace);
        let big = std::fs::canonicalize(&dir).unwrap().join("big.bin");
        let file = std::fs::File::create(&big).unwrap();
        file.set_len(MAX_FILE_BYTES + 1).unwrap();
        inventory.skills[0].files.push(KitFile {
            path: big,
            id: None,
            stamp: None,
            name: "big.bin".into(),
        });
        let packed = pack(&inventory, &env.roots, &on(), &all_present()).unwrap();
        assert!(packed.skills.is_empty());
        assert!(packed.warnings.iter().any(|w| w.contains("64 MiB")));
    }
}

#[test]
fn nothing_is_sent_when_the_credential_index_is_incomplete() {
    let env = env();
    skill(&env, "review", "# review");
    let mut inventory = build_inventory(&env.roots, Workspace::Codespace);
    inventory.file_index_incomplete = true;
    let packed = pack(&inventory, &env.roots, &on(), &all_present()).unwrap();
    assert!(packed.skills.is_empty());
    assert!(packed.warnings.iter().any(|w| w.contains("credentials")));
}

#[test]
fn the_selection_decides_what_is_sent_and_the_token_follows_it() {
    let env = env();
    skill(&env, "review", "# review");
    skill(&env, "deploy", "# deploy");
    servers(
        &env,
        json!({"github": {"command": "npx", "env": {"T": FAKE_TOKEN}}}),
    );
    let inventory = build_inventory(&env.roots, Workspace::Codespace);
    let mut prefs = on();
    prefs.set_selected(&inventory, "skill:user:deploy", false);
    let packed = pack(&inventory, &env.roots, &prefs, &all_present()).unwrap();
    assert_eq!(packed.skills, ["review"]);
    let all = selection_token(&inventory, &on());
    assert_ne!(all, selection_token(&inventory, &prefs));
    // The token covers secret values, and prints nothing about them.
    servers(
        &env,
        json!({"github": {"command": "npx", "env": {"T": "another-value-9999"}}}),
    );
    let changed = build_inventory(&env.roots, Workspace::Codespace);
    assert_ne!(all, selection_token(&changed, &on()));
    assert!(!all.contains(FAKE_TOKEN));
}

#[test]
fn an_empty_selection_packs_nothing() {
    let env = env();
    let packed = pack_env(&env, &all_present());
    assert!(packed.is_empty());
}

#[test]
fn a_server_with_a_headers_helper_is_not_sent() {
    let env = env();
    servers(
        &env,
        json!({
            "helper": {"type": "http", "url": "https://h.example/mcp", "headersHelper": "my-helper"},
            "fine": {"command": "npx"},
        }),
    );
    let packed = pack_env(&env, &all_present());
    let names: Vec<_> = packed.servers.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, ["fine"]);
    assert!(packed
        .warnings
        .iter()
        .any(|w| w.contains("helper") && w.contains("headers helper")));
}

#[test]
fn a_reference_mixed_into_text_is_sent_literally_with_a_warning() {
    let env = env();
    servers(
        &env,
        json!({"mixed": {"command": "npx", "env": {"HOME_DIR": "${HOME}/x", "DEF": "${X:-d}", "OK": "${PURE}"}}}),
    );
    let packed = pack_env(&env, &all_present());
    let names: Vec<_> = packed.secrets.iter().map(|s| s.0.as_str()).collect();
    assert_eq!(names.len(), 2, "{names:?}");
    let warned = packed
        .warnings
        .iter()
        .filter(|w| w.contains("variable reference"))
        .count();
    assert_eq!(warned, 2, "{:?}", packed.warnings);
    // A lone reference is still left alone.
    assert_eq!(
        mcp_json(&packed)["mcpServers"]["mixed"]["env"]["OK"],
        "${PURE}"
    );
}

#[test]
fn url_secrets_are_tracked_by_name_even_when_the_name_was_made_unique() {
    let env = env();
    servers(
        &env,
        json!({
            "a-b": {"type": "http", "url": format!("https://q.example/a?key={FAKE_QUERY}")},
            "a_b": {"type": "http", "url": "https://q.example/b?key=other-query-9999"},
        }),
    );
    let packed = pack_env(&env, &all_present());
    assert_eq!(packed.secrets.len(), 2);
    assert_eq!(packed.url_secrets.len(), 2);
    for (name, _) in &packed.secrets {
        assert!(packed.url_secrets.contains(name), "{name}");
    }
}

#[test]
fn token_looking_parts_of_args_and_urls_are_collected_for_the_scrub() {
    let env = env();
    servers(
        &env,
        json!({
            "tool": {"command": "npx", "args": ["-y", "@scope/pkg", "--key=argvalue-EEEE5555", "tokenmix1234567890ABCD", "${REF}"]},
            "path": {"type": "http", "url": "https://p.example/mcp/pathtoken1234567890XYZ"},
        }),
    );
    let packed = pack_env(&env, &all_present());
    let extra: Vec<&str> = packed.scrub_extra.iter().map(|s| s.as_str()).collect();
    for want in [
        "argvalue-EEEE5555",
        "tokenmix1234567890ABCD",
        "pathtoken1234567890XYZ",
    ] {
        assert!(extra.contains(&want), "{want} not in {extra:?}");
    }
    // Plain words stay out, so the log stays readable.
    for plain in ["-y", "@scope/pkg", "${REF}", "npx"] {
        assert!(!extra.contains(&plain), "{plain}");
    }
}

#[cfg(unix)]
#[test]
fn files_that_differ_only_in_case_are_refused() {
    let env = env();
    let dir = skill(&env, "review", "# review");
    write(&dir.join("one.txt"), "one");
    write(&dir.join("two.txt"), "two");
    let mut inventory = build_inventory(&env.roots, Workspace::Codespace);
    let real = std::fs::canonicalize(&dir).unwrap();
    let files = &mut inventory.skills[0].files;
    files.clear();
    files.push(KitFile {
        path: real.join("one.txt"),
        id: None,
        stamp: None,
        name: "Notes.md".into(),
    });
    files.push(KitFile {
        path: real.join("two.txt"),
        id: None,
        stamp: None,
        name: "notes.md".into(),
    });
    let packed = pack(&inventory, &env.roots, &on(), &all_present()).unwrap();
    assert!(packed.skills.is_empty());
    assert!(
        packed.warnings.iter().any(|w| w.contains("letter case")),
        "{:?}",
        packed.warnings
    );
}

// ---------------------------------------------------------------------------
// Instructions
// ---------------------------------------------------------------------------

fn instructions_env() -> Env {
    let env = env();
    let root = &env.roots.claude_home;
    write(&root.join("CLAUDE.md"), "@RTK.md\nBe brief.\n");
    write(&root.join("RTK.md"), "use rtk");
    write(&root.join("rules/style.md"), "style");
    env
}

fn pack_with(env: &Env, prefs: &CloudKitPrefs, present: &Present) -> Packed {
    let inventory = build_inventory(&env.roots, Workspace::Codespace);
    pack(&inventory, &env.roots, prefs, present).unwrap()
}

fn kit_json(packed: &Packed) -> Value {
    serde_json::from_slice(&read_tar(packed)["kit.json"].4).unwrap()
}

#[test]
fn instructions_travel_when_enabled_and_the_host_can_take_them() {
    let env = instructions_env();
    let packed = pack_with(&env, &on(), &all_present().with_instructions(true));
    let tar = read_tar(&packed);
    let paths: Vec<&str> = tar.keys().map(String::as_str).collect();
    assert_eq!(
        paths,
        [
            "instructions",
            "instructions/CLAUDE.md",
            "instructions/RTK.md",
            "instructions/rules",
            "instructions/rules/style.md",
            "kit.json",
        ]
    );
    assert_eq!(tar["instructions/RTK.md"].4, b"use rtk");
    assert_eq!(
        kit_json(&packed)["instructions"],
        json!(["CLAUDE.md", "RTK.md", "rules/style.md"])
    );
    assert_eq!(
        packed.instructions,
        ["CLAUDE.md", "RTK.md", "rules/style.md"]
    );
    assert!(!packed.is_empty());
}

#[test]
fn instructions_are_omitted_when_off_or_when_the_host_lacks_the_capability() {
    let env = instructions_env();
    let off = CloudKitPrefs {
        instructions: false,
        ..on()
    };
    let packed = pack_with(&env, &off, &all_present().with_instructions(true));
    assert!(packed.is_empty());
    assert!(read_tar(&packed).is_empty());

    let skills_too = {
        skill(&env, "review", "# review");
        pack_with(&env, &off, &all_present().with_instructions(true))
    };
    assert!(kit_json(&skills_too).get("instructions").is_none());

    let packed = pack_with(&env, &on(), &all_present());
    assert!(packed.instructions.is_empty());
    assert!(kit_json(&packed).get("instructions").is_none());
    assert!(!read_tar(&packed)
        .keys()
        .any(|p| p.starts_with("instructions")));
}

#[test]
fn an_instructions_only_kit_is_packed_and_is_deterministic() {
    let env = instructions_env();
    let first = pack_with(&env, &on(), &Present::unknown().with_instructions(true));
    assert!(read_tar(&first).contains_key("kit.json"));
    assert!(first.skills.is_empty() && first.plugins.is_empty() && first.servers.is_empty());
    std::thread::sleep(std::time::Duration::from_millis(1100));
    for name in ["CLAUDE.md", "RTK.md"] {
        let file = env.roots.claude_home.join(name);
        let text = std::fs::read_to_string(&file).unwrap();
        std::fs::write(&file, text).unwrap();
    }
    let second = pack_with(&env, &on(), &Present::unknown().with_instructions(true));
    assert_eq!(first.sha256, second.sha256);
    assert_eq!(
        std::fs::read(first.file.path()).unwrap(),
        std::fs::read(second.file.path()).unwrap()
    );
}

#[test]
fn the_selection_token_follows_the_instructions_and_the_preference() {
    let env = instructions_env();
    let inventory = build_inventory(&env.roots, Workspace::Codespace);
    let before = selection_token(&inventory, &on());
    write(&env.roots.claude_home.join("RTK.md"), "changed");
    let changed = build_inventory(&env.roots, Workspace::Codespace);
    assert_ne!(before, selection_token(&changed, &on()));
    let off = CloudKitPrefs {
        instructions: false,
        ..on()
    };
    assert_ne!(
        selection_token(&changed, &on()),
        selection_token(&changed, &off)
    );
}

#[test]
fn a_file_that_grew_past_the_cap_after_the_scan_is_not_sent() {
    let env = instructions_env();
    let mut inventory = build_inventory(&env.roots, Workspace::Codespace);
    // Same file (device and inode), but it grew after the scan. The scan's size and time are
    // dropped so that only the cap check can refuse it (the stamp would refuse it first).
    for file in &mut inventory.instructions.as_mut().unwrap().files {
        file.stamp = None;
    }
    write(
        &env.roots.claude_home.join("RTK.md"),
        &"x".repeat(300 * 1024),
    );
    let packed = pack(
        &inventory,
        &env.roots,
        &on(),
        &all_present().with_instructions(true),
    )
    .unwrap();
    assert!(packed.instructions.is_empty());
    assert!(packed
        .warnings
        .iter()
        .any(|w| w == "Your instructions were not sent: they are larger than the limit."));
}

#[test]
fn imports_that_were_not_carried_are_counted_not_listed() {
    let env = env();
    write(
        &env.roots.claude_home.join("CLAUDE.md"),
        "@missing.md @other.md",
    );
    let packed = pack_with(&env, &on(), &all_present().with_instructions(true));
    assert_eq!(packed.instructions, ["CLAUDE.md"]);
    assert_eq!(packed.instructions_not_carried, 2);
    assert!(packed.warnings.iter().all(|w| !w.contains("missing.md")));
}

#[test]
fn the_token_leaves_instructions_out_when_they_were_not_sent() {
    let env = instructions_env();
    let inventory = build_inventory(&env.roots, Workspace::Codespace);
    let sent = selection_token_for(&inventory, &on(), true);
    let held_back = selection_token_for(&inventory, &on(), false);
    assert_ne!(sent, held_back);
    assert_eq!(sent, selection_token(&inventory, &on()));
    // With no instructions at all both are the same.
    let bare = super::kit::KitInventory::default();
    assert_eq!(
        selection_token_for(&bare, &on(), true),
        selection_token_for(&bare, &on(), false)
    );
}

#[test]
fn a_kit_over_the_size_cap_still_sends_the_instructions() {
    let env = instructions_env();
    skill(&env, "review", "# review");
    let mut inventory = build_inventory(&env.roots, Workspace::Codespace);
    inventory.skills[0].compressed = super::kit::MAX_KIT_COMPRESSED_BYTES + 1;
    let packed = pack(
        &inventory,
        &env.roots,
        &on(),
        &all_present().with_instructions(true),
    )
    .unwrap();
    assert!(packed.skills.is_empty());
    assert_eq!(
        packed.instructions,
        ["CLAUDE.md", "RTK.md", "rules/style.md"]
    );
    assert!(packed
        .warnings
        .iter()
        .any(|w| w.contains("larger than 50 MB")));
}

#[test]
fn an_incomplete_credential_index_drops_the_instructions_and_says_so() {
    let env = instructions_env();
    let mut inventory = build_inventory(&env.roots, Workspace::Codespace);
    inventory.file_index_incomplete = true;
    let packed = pack(
        &inventory,
        &env.roots,
        &on(),
        &all_present().with_instructions(true),
    )
    .unwrap();
    assert!(packed.instructions.is_empty());
    assert!(packed
        .warnings
        .iter()
        .any(|w| w.contains("your instructions were not sent")));
}
