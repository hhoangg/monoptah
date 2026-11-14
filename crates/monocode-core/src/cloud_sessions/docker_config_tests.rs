use super::docker_config::*;
use crate::sandboxes::SandboxSource;
use crate::store::Store;

fn store() -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("monocode.db")).unwrap();
    (dir, store)
}
type Mutation = Box<dyn Fn(&mut DockerProjectConfig)>;

fn config() -> DockerProjectConfig {
    DockerProjectConfig {
        repo_url: "https://github.com/o/r.git".into(),
        ..Default::default()
    }
}

#[test]
fn serde_defaults_and_camel_case() {
    let cfg: DockerProjectConfig =
        serde_json::from_str(r#"{"repoUrl":"https://github.com/o/r"}"#).unwrap();
    assert_eq!(cfg.repo_url, "https://github.com/o/r");
    assert_eq!(cfg.devcontainer_path, ".devcontainer/devcontainer.json");
    assert_eq!(
        (cfg.cpus, cfg.memory.as_str(), cfg.ttl.as_str()),
        (2, "4g", "2h")
    );
    assert!(cfg.credential_id.is_none() && cfg.platform.is_none() && cfg.claude_account.is_none());
    assert_eq!(cfg.base_ref, "");
    let json = serde_json::to_value(&cfg).unwrap();
    for key in [
        "repoUrl",
        "credentialId",
        "baseRef",
        "devcontainerPath",
        "claudeAccount",
    ] {
        assert!(json.get(key).is_some(), "{key}");
    }
}

#[test]
fn save_and_load_round_trip_per_project() {
    let (_dir, store) = store();
    assert!(load(&store, "/p/a").unwrap().is_none());
    let mut cfg = config();
    cfg.credential_id = Some("abc".into());
    cfg.base_ref = " main ".into();
    let saved = save(&store, "/p/a", &cfg).unwrap();
    assert_eq!(saved.base_ref, "main");
    save(&store, "/p/b", &config()).unwrap();
    assert_eq!(load(&store, "/p/a").unwrap().unwrap(), saved);
    assert_eq!(load(&store, "/p/b").unwrap().unwrap().credential_id, None);
    assert!(store.setting(SETTING).unwrap().is_some());
    clear(&store, "/p/a").unwrap();
    assert!(load(&store, "/p/a").unwrap().is_none());
    assert!(load(&store, "/p/b").unwrap().is_some());
}

#[test]
fn invalid_configs_are_rejected_and_not_stored() {
    let (_dir, store) = store();
    let mutations: Vec<(&str, Mutation)> = vec![
        ("empty url", Box::new(|c| c.repo_url = String::new())),
        (
            "ssh url",
            Box::new(|c| c.repo_url = "git@github.com:o/r.git".into()),
        ),
        (
            "url with token",
            Box::new(|c| c.repo_url = "https://tok@github.com/o/r".into()),
        ),
        (
            "url with user and password",
            Box::new(|c| c.repo_url = "https://u:p@github.com/o/r".into()),
        ),
        (
            "other host",
            Box::new(|c| c.repo_url = "https://example.com/o/r".into()),
        ),
        (
            "abs devcontainer",
            Box::new(|c| c.devcontainer_path = "/etc/x.json".into()),
        ),
        (
            "dotdot devcontainer",
            Box::new(|c| c.devcontainer_path = "../x.json".into()),
        ),
        (
            "empty devcontainer",
            Box::new(|c| c.devcontainer_path = " ".into()),
        ),
        ("zero cpus", Box::new(|c| c.cpus = 0)),
        ("too many cpus", Box::new(|c| c.cpus = 999)),
        ("bad memory", Box::new(|c| c.memory = "lots".into())),
        ("tiny memory", Box::new(|c| c.memory = "1m".into())),
        ("bad ttl", Box::new(|c| c.ttl = "soon".into())),
        ("zero ttl", Box::new(|c| c.ttl = "0m".into())),
        ("long ttl", Box::new(|c| c.ttl = "48h".into())),
        (
            "bad platform",
            Box::new(|c| c.platform = Some("amd64".into())),
        ),
        (
            "bad ref",
            Box::new(|c| c.base_ref = "--upload-pack=x".into()),
        ),
        ("spaced ref", Box::new(|c| c.base_ref = "a b".into())),
        (
            "multibyte memory",
            Box::new(|c| c.memory = "4\u{e9}".into()),
        ),
        ("multibyte ttl", Box::new(|c| c.ttl = "2\u{447}".into())),
        ("signed memory", Box::new(|c| c.memory = "+4g".into())),
        ("signed ttl", Box::new(|c| c.ttl = "+2h".into())),
        ("unit only", Box::new(|c| c.ttl = "h".into())),
        ("ref semicolon", Box::new(|c| c.base_ref = "main;id".into())),
        ("ref subshell", Box::new(|c| c.base_ref = "$(id)".into())),
        ("ref backtick", Box::new(|c| c.base_ref = "`id`".into())),
        ("ref quote", Box::new(|c| c.base_ref = "a'b".into())),
        ("ref reflog", Box::new(|c| c.base_ref = "main@{1}".into())),
        ("ref dotdot", Box::new(|c| c.base_ref = "a..b".into())),
        ("ref dslash", Box::new(|c| c.base_ref = "a//b".into())),
        ("ref lead", Box::new(|c| c.base_ref = "/a".into())),
        ("ref trail", Box::new(|c| c.base_ref = "a/".into())),
        ("ref lock", Box::new(|c| c.base_ref = "a.lock".into())),
        ("ref dot", Box::new(|c| c.base_ref = "a.".into())),
        ("ref comp", Box::new(|c| c.base_ref = "a/.b".into())),
        (
            "dc semicolon",
            Box::new(|c| c.devcontainer_path = "a;b.json".into()),
        ),
        (
            "dc subshell",
            Box::new(|c| c.devcontainer_path = "$(id)/d.json".into()),
        ),
        (
            "dc backtick",
            Box::new(|c| c.devcontainer_path = "`id`.json".into()),
        ),
        (
            "dc quote",
            Box::new(|c| c.devcontainer_path = "a\"b.json".into()),
        ),
        (
            "dc space",
            Box::new(|c| c.devcontainer_path = "a b.json".into()),
        ),
        (
            "dc dot",
            Box::new(|c| c.devcontainer_path = "./a.json".into()),
        ),
        (
            "id semicolon",
            Box::new(|c| c.credential_id = Some("a;b".into())),
        ),
        (
            "id subshell",
            Box::new(|c| c.credential_id = Some("$(id)".into())),
        ),
        (
            "id space",
            Box::new(|c| c.credential_id = Some("a b".into())),
        ),
        (
            "id quote",
            Box::new(|c| c.credential_id = Some("a'b".into())),
        ),
    ];
    for (name, mutate) in mutations {
        let mut cfg = config();
        mutate(&mut cfg);
        assert!(save(&store, "/p", &cfg).is_err(), "{name}");
    }
    assert!(load(&store, "/p").unwrap().is_none());
}

#[test]
fn valid_variants_are_accepted_and_normalised() {
    let (_dir, store) = store();
    let mut cfg = config();
    cfg.memory = "512M".into();
    cfg.ttl = "30M".into();
    cfg.platform = Some("linux/amd64".into());
    cfg.claude_account = Some("  ".into());
    cfg.devcontainer_path = "sub/dev.json".into();
    let saved = save(&store, "/p", &cfg).unwrap();
    assert_eq!((saved.memory.as_str(), saved.ttl.as_str()), ("512m", "30m"));
    assert_eq!(saved.claude_account, None);
    let mut public = config();
    public.credential_id = None;
    public.ttl = "24h".into();
    assert!(save(&store, "/q", &public).is_ok());
}

#[test]
fn a_corrupt_setting_is_an_error_and_is_never_overwritten() {
    let (_dir, store) = store();
    store.save_setting(SETTING, "{not json").unwrap();
    assert!(load(&store, "/p").is_err());
    assert!(save(&store, "/p", &config()).is_err());
    assert!(clear(&store, "/p").is_err());
    assert_eq!(
        store.setting(SETTING).unwrap().as_deref(),
        Some("{not json")
    );
}

#[test]
fn a_stored_config_that_no_longer_validates_is_an_error_on_load() {
    let (_dir, store) = store();
    store
        .save_setting(
            SETTING,
            r#"{"/p":{"repoUrl":"https://github.com/o/r","baseRef":"$(id)"}}"#,
        )
        .unwrap();
    assert!(load(&store, "/p").is_err());
}

#[test]
fn safe_refs_and_ids_are_accepted() {
    let (_dir, store) = store();
    let mut cfg = config();
    cfg.base_ref = "feature/a-b_c.1".into();
    cfg.credential_id = Some("1b2c-3d".into());
    assert!(save(&store, "/p", &cfg).is_ok());
}

#[test]
fn old_stored_json_loads_without_docker_account() {
    let (_dir, store) = store();
    store
        .save_setting(
            SETTING,
            r#"{"/p":{"repoUrl":"https://github.com/o/r","claudeAccount":"acc-1","credentialId":"cred-1","cpus":4,"memory":"8g","ttl":"4h","baseRef":"main","devcontainerPath":".devcontainer.json"}}"#,
        )
        .unwrap();
    let cfg = load(&store, "/p").unwrap().unwrap();
    assert_eq!(cfg.docker_account, None);
    assert_eq!(cfg.claude_account.as_deref(), Some("acc-1"));
    assert_eq!(
        (cfg.cpus, cfg.memory.as_str(), cfg.base_ref.as_str()),
        (4, "8g", "main")
    );
}

#[test]
fn docker_account_is_trimmed_and_checked() {
    let mut cfg = config();
    cfg.docker_account = Some("  ".into());
    assert_eq!(cfg.validated().unwrap().docker_account, None);
    cfg.docker_account = Some(" acc-1 ".into());
    assert_eq!(
        cfg.validated().unwrap().docker_account.as_deref(),
        Some("acc-1")
    );
    for bad in ["a/b", "a b", "a;b", &"x".repeat(65)] {
        cfg.docker_account = Some(bad.into());
        assert!(cfg.validated().is_err(), "{bad}");
    }
}
// ------------------------------------------------------- template and image

fn template_config(name: &str) -> DockerProjectConfig {
    DockerProjectConfig {
        environment: SandboxSource::Template { name: name.into() },
        ..config()
    }
}

#[test]
fn an_old_config_without_an_environment_is_the_default_sandbox() {
    // Exactly what an earlier build stored.
    let old = r#"{"repoUrl":"https://github.com/o/r","credentialId":null,"baseRef":"","devcontainerPath":".devcontainer/devcontainer.json","cpus":4,"memory":"8g","ttl":"4h","platform":null,"claudeAccount":null}"#;
    let cfg: DockerProjectConfig = serde_json::from_str(old).unwrap();
    assert_eq!(cfg.environment, SandboxSource::Default);
    assert_eq!(cfg.use_devcontainer, None);
    assert!(cfg.uses_devcontainer() && !cfg.size_from_template());
    let checked = cfg.validated().unwrap();
    assert_eq!((checked.cpus, checked.memory.as_str()), (4, "8g"));
    // A default config writes nothing new, so an older build reads it back unchanged.
    let json = serde_json::to_value(&checked).unwrap();
    assert!(json.get("environment").is_none() && json.get("useDevcontainer").is_none());
}

#[test]
fn a_template_or_image_round_trips_through_the_store() {
    let (_dir, store) = store();
    let mut template = template_config(" rust-small ");
    template.use_devcontainer = Some(true);
    let saved = save(&store, "/p/t", &template).unwrap();
    assert_eq!(
        saved.environment,
        SandboxSource::Template {
            name: "rust-small".into()
        }
    );
    let image = DockerProjectConfig {
        environment: SandboxSource::Image {
            reference: "docker.io/library/rust:slim".into(),
        },
        ..config()
    };
    save(&store, "/p/i", &image).unwrap();
    assert_eq!(load(&store, "/p/t").unwrap().unwrap(), saved);
    let loaded = load(&store, "/p/i").unwrap().unwrap();
    assert_eq!(loaded.environment, image.environment);
    let raw = store.setting(SETTING).unwrap().unwrap();
    assert!(raw.contains(r#""environment":{"kind":"template","name":"rust-small"}"#));
    assert!(raw.contains(r#""useDevcontainer":true"#));
}

#[test]
fn the_dev_container_is_off_for_a_template_or_image_unless_asked_for() {
    assert!(config().uses_devcontainer());
    let mut cfg = template_config("t");
    assert_eq!(cfg.use_devcontainer, None);
    assert!(!cfg.uses_devcontainer());
    cfg.use_devcontainer = Some(true);
    assert!(cfg.uses_devcontainer());
    cfg.environment = SandboxSource::Image {
        reference: "rust:slim".into(),
    };
    cfg.use_devcontainer = Some(false);
    assert!(!cfg.uses_devcontainer());
    // The flag means nothing for the default sandbox, which keeps today's behaviour.
    let mut default = config();
    default.use_devcontainer = Some(false);
    let checked = default.validated().unwrap();
    assert_eq!(checked.use_devcontainer, None);
    assert!(checked.uses_devcontainer());
}

#[test]
fn a_template_skips_the_size_checks_and_an_image_keeps_them() {
    let mut cfg = template_config("t");
    cfg.cpus = 0;
    cfg.memory = "banana".into();
    assert!(cfg.size_from_template());
    assert!(cfg.validated().is_ok());
    let mut image = cfg.clone();
    image.environment = SandboxSource::Image {
        reference: "rust:slim".into(),
    };
    assert!(image.validated().is_err());
    // The lifetime still applies to a template.
    cfg.ttl = "forever".into();
    assert!(cfg.validated().is_err());
}

#[test]
fn bad_template_names_and_image_references_are_rejected_and_not_stored() {
    let (_dir, store) = store();
    for bad in [
        "",
        "   ",
        "-t",
        "--cpus=64",
        "a b",
        "a\tb",
        "a\nb",
        "a\u{7}b",
    ] {
        for environment in [
            SandboxSource::Template { name: bad.into() },
            SandboxSource::Image {
                reference: bad.into(),
            },
        ] {
            let cfg = DockerProjectConfig {
                environment,
                ..config()
            };
            let error = cfg.validated().unwrap_err();
            assert!(
                error.contains("Template name") || error.contains("Image reference"),
                "{bad:?}: {error}"
            );
            assert!(save(&store, "/p/x", &cfg).is_err());
        }
    }
    assert!(store.setting(SETTING).unwrap().is_none());
}
