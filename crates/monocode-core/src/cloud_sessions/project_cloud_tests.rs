use super::docker_config::{self, DockerProjectConfig};
use super::project_cloud::*;
use crate::store::Store;

fn store() -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("monocode.db")).unwrap();
    (dir, store)
}
fn sandbox() -> DockerProjectConfig {
    DockerProjectConfig {
        repo_url: "https://github.com/o/r.git".into(),
        ..Default::default()
    }
}

#[test]
fn defaults_are_todays_behavior() {
    let cfg = CodespaceProjectConfig::default();
    assert_eq!(
        (cfg.base_ref.as_str(), cfg.devcontainer_path.as_str()),
        ("", "")
    );
    assert!(cfg.machine.is_none() && cfg.claude_account.is_none());
    assert_eq!(cfg.idle_timeout_minutes, 30);
    let parsed: CodespaceProjectConfig = serde_json::from_str("{}").unwrap();
    assert_eq!(parsed, cfg);
}

#[test]
fn kind_serializes_as_codespace_and_sandbox() {
    assert_eq!(
        serde_json::to_string(&DefaultBackend::Codespace).unwrap(),
        r#""codespace""#
    );
    assert_eq!(
        serde_json::from_str::<DefaultBackend>(r#""sandbox""#).unwrap(),
        DefaultBackend::Sandbox
    );
}

#[test]
fn missing_project_loads_defaults() {
    let (_dir, store) = store();
    let loaded = load(&store, "/p").unwrap();
    assert_eq!(loaded, ProjectCloud::default());
    assert_eq!(
        loaded.effective_default_backend(),
        DefaultBackend::Codespace
    );
}

#[test]
fn round_trip_keeps_both_parts() {
    let (_dir, store) = store();
    let project = ProjectCloud {
        default_backend: Some(DefaultBackend::Sandbox),
        codespace: CodespaceProjectConfig {
            base_ref: " main ".into(),
            devcontainer_path: ".devcontainer/web/devcontainer.json".into(),
            machine: Some("standardLinux32gb".into()),
            idle_timeout_minutes: 60,
            claude_account: Some("acc-1".into()),
        },
        sandbox: Some(DockerProjectConfig {
            docker_account: Some("dock-1".into()),
            ..sandbox()
        }),
    };
    let saved = save(&store, "/p", &project).unwrap();
    assert_eq!(saved.codespace.base_ref, "main");
    assert_eq!(load(&store, "/p").unwrap(), saved);
    // The sandbox stays in its own setting.
    assert!(docker_config::load(&store, "/p").unwrap().is_some());
    assert_eq!(load(&store, "/other").unwrap(), ProjectCloud::default());
    let raw = store.setting(SETTING).unwrap().unwrap();
    assert!(raw.contains("defaultBackend") && raw.contains("idleTimeoutMinutes"));
}

#[test]
fn old_stored_json_without_default_backend_loads() {
    let (_dir, store) = store();
    store
        .save_setting(
            SETTING,
            r#"{"/p":{"codespace":{"machine":"basicLinux32gb"}}}"#,
        )
        .unwrap();
    let loaded = load(&store, "/p").unwrap();
    assert_eq!(loaded.default_backend, None);
    assert_eq!(loaded.codespace.idle_timeout_minutes, 30);
}

#[test]
fn unreadable_setting_is_an_error_and_never_overwritten() {
    let (_dir, store) = store();
    store.save_setting(SETTING, "{not json").unwrap();
    assert!(load(&store, "/p").is_err());
    assert!(save(&store, "/p", &ProjectCloud::default()).is_err());
    assert_eq!(store.setting(SETTING).unwrap().unwrap(), "{not json");
}

#[test]
fn unreadable_docker_setting_blocks_the_save_before_anything_is_written() {
    let (_dir, store) = store();
    store
        .save_setting(docker_config::SETTING, "garbage")
        .unwrap();
    assert!(save(&store, "/p", &ProjectCloud::default()).is_err());
    assert!(store.setting(SETTING).unwrap().is_none());
    assert_eq!(
        store.setting(docker_config::SETTING).unwrap().unwrap(),
        "garbage"
    );
}

#[test]
fn invalid_input_writes_nothing() {
    let (_dir, store) = store();
    let bad_codespace = ProjectCloud {
        sandbox: Some(sandbox()),
        codespace: CodespaceProjectConfig {
            idle_timeout_minutes: 4,
            ..Default::default()
        },
        ..Default::default()
    };
    assert!(save(&store, "/p", &bad_codespace).is_err());
    let bad_sandbox = ProjectCloud {
        sandbox: Some(DockerProjectConfig {
            repo_url: "ftp://nope".into(),
            ..Default::default()
        }),
        ..Default::default()
    };
    assert!(save(&store, "/p", &bad_sandbox).is_err());
    assert!(store.setting(SETTING).unwrap().is_none());
    assert!(store.setting(docker_config::SETTING).unwrap().is_none());
}

#[test]
fn codespace_validation_rules() {
    let ok = |f: &dyn Fn(&mut CodespaceProjectConfig)| {
        let mut c = CodespaceProjectConfig::default();
        f(&mut c);
        c.validated()
    };
    assert!(ok(&|_| {}).is_ok());
    assert!(ok(&|c| c.idle_timeout_minutes = 5).is_ok());
    assert!(ok(&|c| c.idle_timeout_minutes = 240).is_ok());
    assert!(ok(&|c| c.idle_timeout_minutes = 4).is_err());
    assert!(ok(&|c| c.idle_timeout_minutes = 241).is_err());
    assert!(ok(&|c| c.base_ref = "a b".into()).is_err());
    assert!(ok(&|c| c.base_ref = "feature/x".into()).is_ok());
    for bad in ["/abs", "../x", "a//b", "a b", "a/./b"] {
        assert!(ok(&|c| c.devcontainer_path = bad.into()).is_err(), "{bad}");
    }
    assert!(ok(&|c| c.devcontainer_path = ".devcontainer.json".into()).is_ok());
    for bad in ["a b", "x;y", "m/n", &"x".repeat(65)] {
        assert!(ok(&|c| c.machine = Some(bad.into())).is_err(), "{bad}");
    }
    assert!(ok(&|c| c.machine = Some("standard_Linux-32".into())).is_ok());
    assert!(ok(&|c| c.claude_account = Some("a/b".into())).is_err());
    assert!(ok(&|c| c.claude_account = Some("abc-123".into())).is_ok());
    let cleaned = ok(&|c| {
        c.machine = Some("  ".into());
        c.claude_account = Some(" ".into());
    })
    .unwrap();
    assert!(cleaned.machine.is_none() && cleaned.claude_account.is_none());
}

#[test]
fn effective_default_backend_cases() {
    let mut p = ProjectCloud::default();
    assert_eq!(p.effective_default_backend(), DefaultBackend::Codespace);
    p.sandbox = Some(sandbox());
    assert_eq!(p.effective_default_backend(), DefaultBackend::Sandbox);
    p.default_backend = Some(DefaultBackend::Codespace);
    assert_eq!(p.effective_default_backend(), DefaultBackend::Codespace);
    p.sandbox = None;
    p.default_backend = Some(DefaultBackend::Sandbox);
    assert_eq!(p.effective_default_backend(), DefaultBackend::Sandbox);
}

#[test]
fn saving_without_a_sandbox_leaves_the_existing_sandbox_alone() {
    let (_dir, store) = store();
    docker_config::save(&store, "/p", &sandbox()).unwrap();
    let saved = save(&store, "/p", &ProjectCloud::default()).unwrap();
    assert!(saved.sandbox.is_some());
    assert!(docker_config::load(&store, "/p").unwrap().is_some());
    docker_config::clear(&store, "/p").unwrap();
    assert!(load(&store, "/p").unwrap().sandbox.is_none());
}

#[test]
fn projects_using_accounts() {
    let (_dir, store) = store();
    let with = |docker: Option<&str>, claude: Option<&str>, cs_claude: Option<&str>| ProjectCloud {
        codespace: CodespaceProjectConfig {
            claude_account: cs_claude.map(str::to_owned),
            ..Default::default()
        },
        sandbox: Some(DockerProjectConfig {
            docker_account: docker.map(str::to_owned),
            claude_account: claude.map(str::to_owned),
            ..sandbox()
        }),
        ..Default::default()
    };
    save(&store, "/b", &with(Some("d1"), Some("c1"), None)).unwrap();
    save(&store, "/a", &with(Some("d1"), None, Some("c1"))).unwrap();
    save(&store, "/c", &with(Some("d2"), None, None)).unwrap();
    save(
        &store,
        "/d",
        &ProjectCloud {
            codespace: CodespaceProjectConfig {
                claude_account: Some("c1".into()),
                ..Default::default()
            },
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(
        projects_using_docker_account(&store, "d1").unwrap(),
        ["/a", "/b"]
    );
    assert_eq!(
        projects_using_docker_account(&store, "nope").unwrap(),
        Vec::<String>::new()
    );
}

fn project_with(file: &str, text: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(file);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
    dir
}

#[test]
fn devcontainer_missing_file() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(
        devcontainer_info(dir.path(), ""),
        DevcontainerInfo::default()
    );
    assert_eq!(
        devcontainer_info(dir.path(), "x/y.json"),
        DevcontainerInfo::default()
    );
    assert_eq!(
        devcontainer_info(dir.path(), "../escape"),
        DevcontainerInfo::default()
    );
}

#[test]
fn devcontainer_reads_jsonc_with_comments_and_requirements() {
    let dir = project_with(
        ".devcontainer/devcontainer.json",
        r#"{
  // the image
  "image": "x", /* block */
  "hostRequirements": { "cpus": 4, "memory": "16gb", },
  "features": { "ghcr.io/devcontainers/features/sshd:1": {}, },
}"#,
    );
    let info = devcontainer_info(dir.path(), "");
    assert_eq!(
        info.found_path.as_deref(),
        Some(".devcontainer/devcontainer.json")
    );
    assert_eq!(info.host_cpus, Some(4));
    assert_eq!(info.host_memory_mib, Some(16 * 1024));
    assert!(info.has_sshd);
}

#[test]
fn devcontainer_memory_units() {
    for (text, mib) in [
        ("8GB", 8 * 1024),
        ("512mb", 512),
        ("1tb", 1024 * 1024),
        ("1.5gb", 1536),
        // Binary spellings and bare letters, any case, an optional space.
        ("8gib", 8 * 1024),
        ("8 GiB", 8 * 1024),
        ("512MiB", 512),
        ("2g", 2 * 1024),
        ("256m", 256),
        ("1tib", 1024 * 1024),
        ("1024kb", 1),
    ] {
        let dir = project_with(
            ".devcontainer.json",
            &format!(r#"{{"hostRequirements":{{"memory":"{text}"}}}}"#),
        );
        let info = devcontainer_info(dir.path(), "");
        assert_eq!(info.found_path.as_deref(), Some(".devcontainer.json"));
        assert_eq!(info.host_memory_mib, Some(mib), "{text}");
    }
    let dir = project_with(
        ".devcontainer.json",
        r#"{"hostRequirements":{"memory":"lots"}}"#,
    );
    assert_eq!(devcontainer_info(dir.path(), "").host_memory_mib, None);
}

#[test]
fn devcontainer_sshd_variants() {
    let case = |features: &str| {
        let dir = project_with(
            ".devcontainer/devcontainer.json",
            &format!(r#"{{"features":{features}}}"#),
        );
        devcontainer_info(dir.path(), "").has_sshd
    };
    assert!(case(r#"{"ghcr.io/devcontainers/features/sshd:1":{}}"#));
    assert!(case(r#"{"ghcr.io/devcontainers/features/sshd":{}}"#));
    assert!(case(r#"{"sshd:1":{}}"#));
    assert!(case(
        r#"{"ghcr.io/devcontainers/features/sshd@sha256:abc":{}}"#
    ));
    assert!(!case(r#"{"ghcr.io/devcontainers/features/not-sshd:1":{}}"#));
    assert!(!case(
        r#"{"ghcr.io/devcontainers/features/sshd-extras":{}}"#
    ));
    assert!(!case(r#"{"ghcr.io/devcontainers/features/node:1":{}}"#));
    assert!(!case("{}"));
}

#[test]
fn devcontainer_explicit_path_and_unparsable_file() {
    let dir = project_with(
        ".devcontainer/web/devcontainer.json",
        r#"{"hostRequirements":{"cpus":2}}"#,
    );
    let info = devcontainer_info(dir.path(), ".devcontainer/web/devcontainer.json");
    assert_eq!(info.host_cpus, Some(2));
    // The default lookup does not see a nested config.
    assert_eq!(devcontainer_info(dir.path(), "").found_path, None);
    let bad = project_with(".devcontainer.json", "{ nope");
    let info = devcontainer_info(bad.path(), "");
    assert_eq!(info.found_path.as_deref(), Some(".devcontainer.json"));
    assert_eq!(
        (info.host_cpus, info.host_memory_mib, info.has_sshd),
        (None, None, false)
    );
}

#[test]
fn default_backend_converts_to_and_from_the_session_backend_kind() {
    use crate::cloud_sessions::backend::CloudBackendKind;
    for (default, kind) in [
        (DefaultBackend::Codespace, CloudBackendKind::Codespaces),
        (DefaultBackend::Sandbox, CloudBackendKind::DockerSandbox),
    ] {
        assert_eq!(CloudBackendKind::from(default), kind);
        assert_eq!(DefaultBackend::from(kind), default);
    }
}

#[test]
fn codespace_settings_load_even_when_the_sandbox_setting_is_invalid() {
    let (_dir, store) = store();
    save(
        &store,
        "/p",
        &ProjectCloud {
            default_backend: Some(DefaultBackend::Codespace),
            codespace: CodespaceProjectConfig {
                machine: Some("standardLinux32gb".into()),
                idle_timeout_minutes: 90,
                ..Default::default()
            },
            ..Default::default()
        },
    )
    .unwrap();
    store
        .save_setting(docker_config::SETTING, r#"{"/p":{"repoUrl":"ftp://nope"}}"#)
        .unwrap();
    assert!(load(&store, "/p").is_err());
    let loaded = load_codespace(&store, "/p").unwrap();
    assert_eq!(loaded.default_backend, Some(DefaultBackend::Codespace));
    assert_eq!(
        loaded.codespace.machine.as_deref(),
        Some("standardLinux32gb")
    );
    assert_eq!(loaded.codespace.idle_timeout_minutes, 90);
    let shared = std::sync::Mutex::new(store);
    let cfg = codespace_config_or_default(&shared, "/p");
    assert_eq!(cfg.idle_timeout_minutes, 90);
    // An unreadable cloud-projects.v1 still falls back to the defaults.
    shared
        .lock()
        .unwrap()
        .save_setting(SETTING, "{nope")
        .unwrap();
    assert_eq!(
        codespace_config_or_default(&shared, "/p"),
        CodespaceProjectConfig::default()
    );
}

#[test]
fn a_failed_cloud_projects_write_leaves_the_sandbox_setting_untouched() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("monocode.db");
    let store = Store::open(&path).unwrap();
    let old = DockerProjectConfig {
        cpus: 2,
        ..sandbox()
    };
    docker_config::save(&store, "/p", &old).unwrap();
    let before = store.setting(docker_config::SETTING).unwrap();
    // Make writes of cloud-projects.v1 fail from a second connection.
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute_batch(&format!(
            "CREATE TRIGGER fail_cloud_projects BEFORE INSERT ON native_settings \
             WHEN NEW.key = '{SETTING}' BEGIN SELECT RAISE(ABORT, 'injected'); END;"
        ))
        .unwrap();
    let next = ProjectCloud {
        sandbox: Some(DockerProjectConfig {
            cpus: 4,
            ..sandbox()
        }),
        ..Default::default()
    };
    assert!(save(&store, "/p", &next).is_err());
    assert_eq!(store.setting(docker_config::SETTING).unwrap(), before);
    assert_eq!(docker_config::load(&store, "/p").unwrap().unwrap().cpus, 2);
}

#[test]
fn what_a_devcontainer_asks_for_reads_as_one_phrase() {
    let ask = |cpus, memory_mib| {
        DevcontainerInfo {
            host_cpus: cpus,
            host_memory_mib: memory_mib,
            ..Default::default()
        }
        .asks_for()
    };
    assert_eq!(
        ask(Some(4), Some(16 * 1024)).as_deref(),
        Some("4 CPUs and 16 GB")
    );
    assert_eq!(ask(Some(1), None).as_deref(), Some("1 CPU"));
    assert_eq!(ask(None, Some(1536)).as_deref(), Some("1.5 GB"));
    assert_eq!(ask(None, None), None);
}
