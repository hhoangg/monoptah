use super::*;
use crate::mobile::denylist::DenyList;
use crate::mobile::file_transfer::{keystore::KeyStore, ObjectStore, StoreError};
use crate::mobile::mentioned_file::{budget, Project, Worktree};
use crate::mobile::policy::{code_of_failure, DeviceContext, DeviceKind};
use serde_json::json;
use std::{collections::HashMap, fs, path::PathBuf, sync::Mutex};

/// A temporary home with a project inside and files outside it.
struct World {
    _dir: tempfile::TempDir,
    root: PathBuf,
}

impl World {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        for folder in ["home/project", "home/Documents", "home/.ssh", "tmp"] {
            fs::create_dir_all(root.join(folder)).unwrap();
        }
        Self { _dir: dir, root }
    }

    fn write(&self, relative: &str, content: &str) -> PathBuf {
        let path = self.root.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, content).unwrap();
        path
    }

    fn project(&self, selected: bool) -> Vec<Project> {
        vec![Project::new(
            "p1",
            &self.root.join("home/project"),
            selected,
            DenyList::default(),
        )]
    }

    fn context<'a>(
        &self,
        projects: &'a [Project],
        no_texts: &'a [String],
        cwd: &'a str,
    ) -> Context<'a> {
        let mut ctx = Context::new(no_texts, cwd, &[], projects);
        ctx.home = Some(self.root.join("home"));
        ctx
    }
}

fn asked(path: &Path) -> Params {
    Params {
        path: path.to_string_lossy().into_owned(),
        asset: None,
        source: false,
    }
}

fn code_of(result: &Result<Prepared, Refusal>) -> Option<Code> {
    result
        .as_ref()
        .err()
        .and_then(|refusal| code_of_failure(&refusal.failure))
}

fn rule_of(result: &Result<Prepared, Refusal>) -> Option<&'static str> {
    result.as_ref().err().map(|refusal| refusal.rule)
}

fn run(
    world: &World,
    permission: Permission,
    params: &Params,
    selected: bool,
) -> Result<Prepared, Refusal> {
    let projects = world.project(selected);
    let no_texts: [String; 0] = [];
    let cwd = world
        .root
        .join("home/project")
        .to_string_lossy()
        .into_owned();
    let ctx = world.context(&projects, &no_texts, &cwd);
    // One device name per call: a transform in flight is limited per device, and tests run in parallel.
    static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let device = format!(
        "t{}",
        NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
    );
    resolve(&device, permission, params, &ctx)
}

#[test]
fn a_control_device_opens_any_readable_file_and_it_is_marked_outside() {
    let world = World::new();
    let tmp = world.write("tmp/report.txt", "hello");
    let docs = world.write("home/Documents/notes.md", "# notes");
    for path in [&tmp, &docs] {
        let prepared = run(&world, Permission::Control, &asked(path), true).unwrap();
        assert!(prepared.outside_projects, "{}", path.display());
    }
    let inside = world.write("home/project/src/a.rs", "fn main() {}");
    let prepared = run(&world, Permission::Control, &asked(&inside), true).unwrap();
    assert!(!prepared.outside_projects);
    assert_eq!(prepared.kind, Kind::Text);
    assert_eq!(
        prepared.project,
        Some(("p1".to_owned(), "src/a.rs".to_owned()))
    );
}

#[test]
fn a_read_device_stays_inside_the_shared_projects() {
    let world = World::new();
    let outside = world.write("tmp/report.txt", "hello");
    let inside = world.write("home/project/a.txt", "inside");
    let refused = run(&world, Permission::Read, &asked(&outside), true);
    assert_eq!(code_of(&refused), Some(Code::Forbidden));
    assert_eq!(rule_of(&refused), Some(rule::NOT_IN_PROJECT));
    assert!(run(&world, Permission::Read, &asked(&inside), true).is_ok());
    // A project the device was not given is outside for it.
    let unshared = run(&world, Permission::Read, &asked(&inside), false);
    assert_eq!(code_of(&unshared), Some(Code::Forbidden));
    // A path that does not exist outside the projects answers like one that does.
    let missing = run(
        &world,
        Permission::Read,
        &asked(&world.root.join("tmp/nope")),
        true,
    );
    assert_eq!(code_of(&missing), Some(Code::Forbidden));
}

#[test]
fn a_missing_file_is_not_found_for_control() {
    let world = World::new();
    let missing = run(
        &world,
        Permission::Control,
        &asked(&world.root.join("tmp/nope.txt")),
        true,
    );
    assert_eq!(code_of(&missing), Some(Code::NotFound));
}

#[test]
fn the_deny_list_holds_for_every_device_inside_and_outside_the_projects() {
    let world = World::new();
    let secrets = [
        world.write("home/project/.env", "TOKEN=x"),
        world.write("tmp/.env", "TOKEN=x"),
        world.write("tmp/server.pem", "pem"),
        world.write("home/Documents/deploy.key", "key"),
        world.write("home/.ssh/id_ed25519", "ssh"),
        world.write("home/.ssh/config", "Host x"),
        world.write("home/.aws/credentials", "[default]"),
        world.write("home/.git-credentials", "https://u:p@example.test"),
    ];
    for permission in [Permission::Control, Permission::Read] {
        for secret in &secrets {
            let refused = run(&world, permission, &asked(secret), true);
            assert!(
                refused.is_err(),
                "{permission:?} opened {}",
                secret.display()
            );
        }
    }
    // For Control the refusal is the coded "hidden" one, and the audit rule says which list.
    let ssh = run(&world, Permission::Control, &asked(&secrets[4]), true);
    assert_eq!(code_of(&ssh), Some(Code::PathDenied));
    // A credential folder that is no default pattern is held by the protected folders.
    let azure = world.write("home/.azure/profile.txt", "azure");
    let azure = run(&world, Permission::Control, &asked(&azure), true);
    assert_eq!(code_of(&azure), Some(Code::PathDenied));
    assert_eq!(rule_of(&azure), Some(rule::PROTECTED_DIR));
    let env = run(&world, Permission::Control, &asked(&secrets[1]), true);
    assert_eq!(code_of(&env), Some(Code::PathDenied));
    assert_eq!(rule_of(&env), Some(rule::DENY_LIST));
    // The `.env.example` family stays readable, as for the project routes.
    let example = world.write("tmp/.env.example", "TOKEN=");
    assert!(run(&world, Permission::Control, &asked(&example), true).is_ok());
}

#[test]
fn the_app_data_folder_is_never_served() {
    let world = World::new();
    let private = world.root.join("tmp/appdata");
    let file = world.write("tmp/appdata/store.db", "db");
    let projects = world.project(true);
    let no_texts: [String; 0] = [];
    let mut ctx = world.context(&projects, &no_texts, "");
    ctx.private_dirs = vec![private];
    let refused = resolve("private-dir", Permission::Control, &asked(&file), &ctx);
    assert_eq!(code_of(&refused), Some(Code::PathDenied));
}

/// One file of each secret store a remote device must never read, by group.
const SECRET_STORES: &[(&str, &[&str])] = &[
    (
        "browser profiles",
        &[
            "Library/Application Support/Firefox/Profiles/x.default/logins.json",
            "Library/Application Support/Firefox/Profiles/x.default/key4.db",
            "Library/Application Support/Firefox/Profiles/x.default/cookies.sqlite",
            "Library/Application Support/Google/Chrome/Default/Cookies",
            "Library/Application Support/Google/Chrome/Default/Login Data",
            "Library/Application Support/Chromium/Default/Cookies",
            "Library/Application Support/BraveSoftware/Brave-Browser/Default/Login Data",
            "Library/Application Support/Microsoft Edge/Default/Cookies",
            "Library/Application Support/Arc/User Data/Default/Cookies",
            "Library/Safari/History.db",
            ".mozilla/firefox/x.default/logins.json",
            ".config/google-chrome/Default/Login Data",
            ".config/chromium/Default/Cookies",
            ".config/BraveSoftware/Brave-Browser/Default/Cookies",
            ".config/microsoft-edge/Default/Cookies",
            "snap/firefox/common/.mozilla/firefox/x.default/cookies.sqlite",
            "AppData/Local/Google/Chrome/User Data/Default/Login Data",
            "Library/Cookies/Cookies.binarycookies",
        ],
    ),
    (
        "keyrings and password managers",
        &[
            ".local/share/keyrings/login.keyring",
            ".password-store/work/mail.gpg",
            ".gnupg/pubring.kbx",
            ".config/op/config",
            ".config/1Password/data.db",
            "Library/Group Containers/2BUA8C4S2C.com.1password/Library/Application Support/1Password/data.sqlite",
            "Library/Application Support/Bitwarden/data.json",
            ".config/Bitwarden/data.json",
        ],
    ),
    (
        "chat app token stores",
        &[
            "Library/Application Support/Slack/Local Storage/leveldb/000003.log",
            "Library/Application Support/discord/Local Storage/leveldb/000005.ldb",
            ".config/Slack/Local Storage/leveldb/000003.log",
            ".config/discord/Local Storage/leveldb/000005.ldb",
            "AppData/Roaming/Slack/Local Storage/leveldb/000003.log",
        ],
    ),
];

#[test]
fn secret_stores_of_other_programs_are_refused_to_every_device() {
    let world = World::new();
    for (group, files) in SECRET_STORES {
        for relative in *files {
            let path = world.write(&format!("home/{relative}"), "secret");
            // Control, with the file outside every project.
            let control = run(&world, Permission::Control, &asked(&path), true);
            assert_eq!(
                code_of(&control),
                Some(Code::PathDenied),
                "{group}: {relative} (control)"
            );
            // Read, even when its project is the whole home folder: the file is inside what it was given.
            let projects = vec![Project::new(
                "home",
                &world.root.join("home"),
                true,
                DenyList::default(),
            )];
            let no_texts: [String; 0] = [];
            let cwd = world.root.join("home").to_string_lossy().into_owned();
            let ctx = world.context(&projects, &no_texts, &cwd);
            let read = resolve("secret-store-reader", Permission::Read, &asked(&path), &ctx);
            assert_eq!(
                code_of(&read),
                Some(Code::PathDenied),
                "{group}: {relative} (read)"
            );
        }
    }
}

#[test]
fn profile_file_names_are_refused_outside_projects_but_not_inside_one() {
    let world = World::new();
    // A fixture of the project: the project routes and the transfer both serve it.
    let fixture = world.write("home/project/test/fixtures/logins.json", "{}");
    for permission in [Permission::Control, Permission::Read] {
        assert!(
            run(&world, permission, &asked(&fixture), true).is_ok(),
            "{permission:?}"
        );
    }
    // The same names in a copy of a profile outside every project.
    for name in ["logins.json", "key4.db", "cookies.sqlite", "Login Data"] {
        let copy = world.write(&format!("tmp/profile-copy/{name}"), "secret");
        let refused = run(&world, Permission::Control, &asked(&copy), true);
        assert_eq!(code_of(&refused), Some(Code::PathDenied), "{name}");
    }
}

#[test]
fn a_plain_file_next_to_a_secret_store_still_opens() {
    let world = World::new();
    let plain = world.write(
        "home/Library/Application Support/Notes/todo.txt",
        "buy milk",
    );
    assert!(run(&world, Permission::Control, &asked(&plain), true).is_ok());
}

#[cfg(unix)]
#[test]
fn symlinks_are_followed_before_every_check() {
    use std::os::unix::fs::symlink;
    let world = World::new();
    let secret = world.write("home/.ssh/id_rsa", "ssh");
    let outside = world.write("tmp/plain.txt", "plain");
    let env = world.write("tmp/.env", "TOKEN=x");
    let project = world.root.join("home/project");
    symlink(&secret, project.join("innocent.txt")).unwrap();
    symlink(&env, project.join("notes.txt")).unwrap();
    symlink(&outside, project.join("shortcut.txt")).unwrap();
    symlink(world.root.join("home/.ssh"), project.join("folder")).unwrap();
    for permission in [Permission::Control, Permission::Read] {
        for name in ["innocent.txt", "notes.txt", "folder/id_rsa"] {
            let refused = run(&world, permission, &asked(&project.join(name)), true);
            assert!(refused.is_err(), "{permission:?} followed {name}");
        }
    }
    // A link out of the project to a plain file: Control may open it, Read may not.
    assert!(run(
        &world,
        Permission::Control,
        &asked(&project.join("shortcut.txt")),
        true
    )
    .is_ok());
    let read = run(
        &world,
        Permission::Read,
        &asked(&project.join("shortcut.txt")),
        true,
    );
    assert_eq!(rule_of(&read), Some(rule::NOT_IN_PROJECT));
}

#[cfg(unix)]
#[test]
fn directories_and_kernel_folders_are_refused() {
    let world = World::new();
    let dir = run(
        &world,
        Permission::Control,
        &asked(&world.root.join("tmp")),
        true,
    );
    assert_eq!(rule_of(&dir), Some(rule::NOT_REGULAR));
    for path in [
        "/proc/self/environ",
        "/proc/1/environ",
        "/dev/null",
        "/sys/kernel",
    ] {
        let refused = run(&world, Permission::Control, &asked(Path::new(path)), true);
        assert_eq!(rule_of(&refused), Some(rule::PROTECTED_DIR), "{path}");
    }
}

#[test]
fn a_file_over_the_cap_is_too_large() {
    let world = World::new();
    let path = world.root.join("tmp/huge.bin");
    let file = fs::File::create(&path).unwrap();
    file.set_len(MAX_PLAINTEXT_BYTES + 1).unwrap();
    let refused = run(&world, Permission::Control, &asked(&path), true);
    assert_eq!(code_of(&refused), Some(Code::TooLarge));
}

#[test]
fn kinds_and_mimes_follow_the_extension_or_the_content() {
    let world = World::new();
    let kinds = [
        ("tmp/a.png", "x", Kind::Image, "image/png"),
        ("tmp/a.md", "# x", Kind::Markdown, "text/markdown"),
        ("tmp/a.html", "<p>x</p>", Kind::Html, "text/html"),
        ("tmp/a.txt", "x", Kind::Text, "text/plain"),
        ("tmp/a.pdf", "x", Kind::Binary, "application/octet-stream"),
        ("tmp/noext", "plain words", Kind::Text, "text/plain"),
    ];
    for (name, content, kind, mime) in kinds {
        let path = world.write(name, content);
        let prepared = run(&world, Permission::Control, &asked(&path), true).unwrap();
        assert_eq!((prepared.kind, prepared.mime), (kind, mime), "{name}");
    }
    let binary = world.root.join("tmp/blob");
    fs::write(&binary, [0u8, 1, 2, 3]).unwrap();
    let prepared = run(&world, Permission::Control, &asked(&binary), true).unwrap();
    assert_eq!(prepared.kind, Kind::Binary);
}

#[test]
fn an_html_page_is_sanitized_and_its_assets_resolve_through_the_same_policy() {
    let world = World::new();
    world.write(
        "tmp/site/index.html",
        r#"<html><body><script>alert(1)</script><img src="img/chart.png"><img src="../secret.png"></body></html>"#,
    );
    world.write("tmp/site/img/chart.png", "png-bytes");
    world.write("tmp/secret.png", "outside-the-root");
    world.write("tmp/site/img/other.png", "not referenced");
    let page = world.root.join("tmp/site/index.html");
    let prepared = run(&world, Permission::Control, &asked(&page), true).unwrap();
    assert_eq!(prepared.kind, Kind::Html);
    let assets = prepared.assets.clone().unwrap();
    assert!(assets.contains(&"img/chart.png".to_owned()), "{assets:?}");
    match prepared.source {
        Source::Bytes(bytes) => {
            let html = String::from_utf8(bytes).unwrap();
            assert!(!html.contains("<script"), "the page is the sanitized one");
        }
        Source::File(_) => panic!("html is served from the sanitized document"),
    }
    let with = |asset: &str| Params {
        path: page.to_string_lossy().into_owned(),
        asset: Some(asset.to_owned()),
        source: false,
    };
    let image = run(&world, Permission::Control, &with("img/chart.png"), true).unwrap();
    assert_eq!((image.kind, image.mime), (Kind::Image, "image/png"));
    // Listed only: another image of the folder, and one outside the page's root, are refused.
    assert!(run(&world, Permission::Control, &with("img/other.png"), true).is_err());
    assert!(run(&world, Permission::Control, &with("../secret.png"), true).is_err());
    // A plain text file has no assets.
    let text = world.write("tmp/a.txt", "x");
    let on_text = Params {
        path: text.to_string_lossy().into_owned(),
        asset: Some("img/chart.png".to_owned()),
        source: false,
    };
    assert!(run(&world, Permission::Control, &on_text, true).is_err());
}

#[test]
fn a_relative_path_is_read_against_the_conversation_folder() {
    let world = World::new();
    world.write("home/project/docs/a.txt", "x");
    let relative = Params {
        path: "docs/a.txt".to_owned(),
        asset: None,
        source: false,
    };
    assert!(run(&world, Permission::Read, &relative, true).is_ok());
    let escape = Params {
        path: "../../tmp/report".to_owned(),
        asset: None,
        source: false,
    };
    world.write("tmp/report", "x");
    assert!(run(&world, Permission::Read, &escape, true).is_err());
}

#[test]
fn the_source_flag_gives_the_raw_text_of_a_page() {
    let world = World::new();
    let page = world.write("tmp/p.html", "<h1>Title</h1><script>1</script>");
    let md = world.write("tmp/p.md", "# Title");
    for path in [&page, &md] {
        let raw = Params {
            source: true,
            ..asked(path)
        };
        let prepared = run(&world, Permission::Control, &raw, true).unwrap();
        assert_eq!(prepared.kind, Kind::Text, "{}", path.display());
        assert!(matches!(prepared.source, Source::File(_)));
        // The very file on disk, byte for byte, not the sanitized page: this is what the web's Download of an html file saves.
        assert_eq!(
            prepared.source.len_hint(),
            std::fs::metadata(path).unwrap().len(),
            "{}",
            path.display()
        );
    }
    // Without the flag an html page is the sanitized document (its images are placeholders), never the file.
    let rendered = run(&world, Permission::Control, &asked(&page), true).unwrap();
    assert_eq!(rendered.kind, Kind::Html);
    assert!(matches!(rendered.source, Source::Bytes(_)));
}

#[test]
fn request_parameters_are_checked() {
    for bad in [
        json!({}),
        json!({"path": ""}),
        json!({"path": 3}),
        json!({"path": "a\0b"}),
        json!({"path": "a", "asset": ""}),
    ] {
        assert!(Params::from_json(&bad).is_err(), "{bad}");
    }
    let ok = Params::from_json(&json!({"path": "a/b.txt", "asset": "x.png"})).unwrap();
    assert_eq!(ok.asset.as_deref(), Some("x.png"));
}

#[derive(Default)]
struct Memory(Mutex<HashMap<String, Vec<u8>>>);

impl ObjectStore for Memory {
    fn head(&self, object: &str) -> Result<Option<u64>, StoreError> {
        Ok(self.0.lock().unwrap().get(object).map(|o| o.len() as u64))
    }

    fn put(&self, object: &str, _len: u64, body: &mut dyn std::io::Read) -> Result<(), StoreError> {
        let mut data = Vec::new();
        body.read_to_end(&mut data)
            .map_err(|_| StoreError::Failed)?;
        self.0.lock().unwrap().insert(object.to_owned(), data);
        Ok(())
    }
}

fn device(permission: Permission, id: &str) -> DeviceContext {
    DeviceContext {
        id: id.to_owned(),
        kind: DeviceKind::Web,
        permission,
        projects: None,
        monos: None,
    }
}

#[test]
fn handle_answers_with_the_object_the_key_and_the_audit_facts() {
    let world = World::new();
    let file = world.write("tmp/report.txt", "the report");
    let keys_dir = tempfile::tempdir().unwrap();
    let keys = KeyStore::new(keys_dir.path());
    let store = Memory::default();
    let projects = world.project(true);
    let no_texts: [String; 0] = [];
    let ctx = world.context(&projects, &no_texts, "");
    let params = json!({"path": file.to_string_lossy()});

    let control = device(Permission::Control, "handle-control");
    let call = handle(&control, &params, &ctx, &store, &keys);
    let reply = call.result.clone().unwrap();
    assert!(call.outside_projects && !call.reused);
    assert_eq!(call.bytes, 10);
    assert_eq!(reply["kind"], "text");
    assert_eq!(reply["size"], 10);
    let object = reply["object"].as_str().unwrap();
    assert!(
        !object.contains("report"),
        "the object name has no part of the path"
    );
    assert!(crate::mobile::web_link::desktop_api::valid_object_name(
        object
    ));
    // The object opens with the key of the reply.
    let key = crate::mobile::file_transfer::crypto::ObjectKey::from_base64(
        reply["key"].as_str().unwrap(),
        reply["nonce"].as_str().unwrap(),
    )
    .unwrap();
    let held = store.0.lock().unwrap().get(object).cloned().unwrap();
    assert_eq!(key.open_all(&held).unwrap(), b"the report");

    // A second device gets the same object and key without an upload.
    let other = device(Permission::Control, "handle-control-2");
    let again = handle(&other, &params, &ctx, &store, &keys);
    assert!(again.reused);
    assert_eq!(again.result.unwrap()["key"], reply["key"]);

    // A read-only device is refused for the same path and the refusal names the rule.
    let reader = device(Permission::Read, "handle-reader");
    let refused = handle(&reader, &params, &ctx, &store, &keys);
    assert_eq!(refused.rule, Some(rule::NOT_IN_PROJECT));
    assert!(!refused.outside_projects);
    assert_eq!(refused.path, file.to_string_lossy());
}

#[test]
fn an_upload_counts_its_bytes_against_the_budget_and_a_reuse_counts_none() {
    let world = World::new();
    let file = world.write("tmp/a.bin", &"x".repeat(1024 * 1024));
    let keys_dir = tempfile::tempdir().unwrap();
    let keys = KeyStore::new(keys_dir.path());
    let store = Memory::default();
    let projects = world.project(true);
    let no_texts: [String; 0] = [];
    let ctx = world.context(&projects, &no_texts, "");
    let params = json!({"path": file.to_string_lossy()});
    let window = budget::BYTES_PER_WINDOW;

    // The device that uploaded 1 MiB has spent it: with the rest of the window used, the next call is busy.
    let uploader = device(Permission::Control, "bytes-uploader");
    let first = handle(&uploader, &params, &ctx, &store, &keys);
    assert!(!first.reused && first.result.is_ok());
    budget::served(&uploader.id, window - 1024 * 1024);
    let busy = handle(&uploader, &params, &ctx, &store, &keys);
    assert_eq!(busy.rule, Some(rule::BUDGET));

    // A device that only reused the object spent nothing: the same remainder leaves it room.
    let reuser = device(Permission::Control, "bytes-reuser");
    let reused = handle(&reuser, &params, &ctx, &store, &keys);
    assert!(reused.reused && reused.result.is_ok());
    budget::served(&reuser.id, window - 1024 * 1024);
    assert_eq!(handle(&reuser, &params, &ctx, &store, &keys).rule, None);
}

#[test]
fn parallel_calls_cannot_together_pass_the_byte_window() {
    let world = World::new();
    let keys_dir = tempfile::tempdir().unwrap();
    let keys = KeyStore::shared(keys_dir.path());
    let store = Memory::default();
    let projects = world.project(true);
    let no_texts: [String; 0] = [];
    let ctx = world.context(&projects, &no_texts, "");
    let files: Vec<PathBuf> = (0..3)
        .map(|i| {
            world.write(
                &format!("tmp/p{i}.bin"),
                &format!("{i}{}", "y".repeat(1536 * 1024)),
            )
        })
        .collect();
    let spender = device(Permission::Control, "parallel-spender");
    // 2 MiB left in the window: one 1.5 MiB file fits, a second one does not, whatever the order.
    budget::served(&spender.id, budget::BYTES_PER_WINDOW - 2 * 1024 * 1024);
    let calls: Vec<TransferCall> = std::thread::scope(|scope| {
        let handles: Vec<_> = files
            .iter()
            .map(|file| {
                let params = json!({"path": file.to_string_lossy()});
                let (spender, ctx, store, keys) = (&spender, &ctx, &store, &*keys);
                scope.spawn(move || handle(spender, &params, ctx, store, keys))
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });
    assert_eq!(calls.iter().filter(|c| c.result.is_ok()).count(), 1);
    assert_eq!(
        calls
            .iter()
            .filter(|c| c.rule == Some(rule::BUDGET))
            .count(),
        2
    );
    // The refused calls uploaded nothing and left the reservation of the one that did.
    assert_eq!(store.0.lock().unwrap().len(), 1);
}

#[test]
fn a_file_of_the_sessions_worktree_is_inside_what_a_read_device_was_given() {
    let world = World::new();
    // A worktree outside the project folder, owned by the selected project.
    let tree = world.root.join("trees/feature");
    fs::create_dir_all(&tree).unwrap();
    let file = tree.join("notes.txt");
    fs::write(&file, "in the worktree").unwrap();
    let projects = world.project(true);
    let no_texts: [String; 0] = [];
    let project_dir = world
        .root
        .join("home/project")
        .to_string_lossy()
        .into_owned();
    let tree_text = tree.to_string_lossy().into_owned();
    let mut ctx = world.context(&projects, &no_texts, &tree_text);
    ctx.worktree = Some(Worktree {
        path: &tree_text,
        project: &project_dir,
    });
    assert!(within_shared(&ctx, &file.canonicalize().unwrap()));
    let read = resolve("worktree-reader", Permission::Read, &asked(&file), &ctx).unwrap();
    assert!(!read.outside_projects);
    // The same file for a project the device was not given: outside.
    let unshared = world.project(false);
    let mut other = world.context(&unshared, &no_texts, &tree_text);
    other.worktree = Some(Worktree {
        path: &tree_text,
        project: &project_dir,
    });
    assert!(!within_shared(&other, &file.canonicalize().unwrap()));
}

#[test]
fn the_budget_stops_a_device_that_asks_too_often() {
    let world = World::new();
    let file = world.write("tmp/a.txt", "x");
    let keys_dir = tempfile::tempdir().unwrap();
    let keys = KeyStore::new(keys_dir.path());
    let store = Memory::default();
    let projects = world.project(true);
    let no_texts: [String; 0] = [];
    let ctx = world.context(&projects, &no_texts, "");
    let params = json!({"path": file.to_string_lossy()});
    let control = device(Permission::Control, "budget-device");
    let mut busy = 0;
    for _ in 0..(budget::CALLS_PER_WINDOW + 5) {
        let call = handle(&control, &params, &ctx, &store, &keys);
        if call.rule == Some(rule::BUDGET) {
            busy += 1;
        }
    }
    assert_eq!(busy, 5);
}
