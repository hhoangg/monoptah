use super::docker_config::*;
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
