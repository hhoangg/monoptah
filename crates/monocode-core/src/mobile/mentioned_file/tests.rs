use super::*;
use crate::transcript::Row;
use std::fs;

struct Fx {
    _dir: tempfile::TempDir,
    root: PathBuf,
}

impl Fx {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        Self { _dir: dir, root }
    }
    fn path(&self, relative: &str) -> PathBuf {
        self.root.join(relative)
    }
    fn write(&self, relative: &str, content: impl AsRef<[u8]>) -> PathBuf {
        let path = self.path(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, content).unwrap();
        path
    }
    fn text(&self, relative: &str) -> String {
        self.path(relative).to_string_lossy().into_owned()
    }
}

fn code(result: &Result<Reply, Failure>) -> Option<Code> {
    result
        .as_ref()
        .err()
        .and_then(super::super::policy::code_of_failure)
}

fn params(path: &str) -> Params {
    Params {
        path: path.to_owned(),
        asset: None,
        offset: 0,
    }
}

fn asset(path: &str, asset: &str) -> Params {
    Params {
        path: path.to_owned(),
        asset: Some(asset.to_owned()),
        offset: 0,
    }
}

/// One call with `texts` as the assistant's messages and `cwd` as the session folder.
fn run(texts: &[&str], cwd: &Path, params: &Params) -> Result<Reply, Failure> {
    run_with(texts, cwd, &[], params, |_| {})
}

fn run_with(
    texts: &[&str],
    cwd: &Path,
    projects: &[Project],
    params: &Params,
    tune: impl FnOnce(&mut Context),
) -> Result<Reply, Failure> {
    let texts: Vec<String> = texts.iter().map(|t| (*t).to_owned()).collect();
    let cwd = cwd.to_string_lossy().into_owned();
    let mut ctx = Context::new(&texts, &cwd, &[], projects);
    ctx.home = None;
    tune(&mut ctx);
    serve(params, &ctx)
}

#[test]
fn an_unmentioned_path_is_forbidden_whether_or_not_it_exists() {
    let fx = Fx::new();
    let there = fx.write("secret.txt", "x");
    let gone = fx.text("missing.txt");
    let said = ["see `report.md`"];
    for path in [
        there.to_string_lossy().into_owned(),
        gone,
        "secret.txt".into(),
    ] {
        let result = run(&said, &fx.root, &params(&path));
        assert_eq!(code(&result), Some(Code::Forbidden), "{path}");
    }
    // No assistant message at all authorizes nothing.
    assert_eq!(
        code(&run(&[], &fx.root, &params("secret.txt"))),
        Some(Code::Forbidden)
    );
}

#[test]
fn only_assistant_rows_make_mentions() {
    let rows = vec![
        Row {
            role: "user".into(),
            text: "read `/etc/hosts`".into(),
            first: true,
            sent_at: None,
        },
        Row {
            role: "assistant".into(),
            text: "ok `a.md`".into(),
            first: true,
            sent_at: None,
        },
        Row {
            role: "mono-system".into(),
            text: "`b.md`".into(),
            first: true,
            sent_at: None,
        },
        Row {
            role: "tool".into(),
            text: "`c.md`".into(),
            first: true,
            sent_at: None,
        },
    ];
    assert_eq!(assistant_texts(&rows), ["ok `a.md`"]);
    // A mention in a user message does not authorize: only the assistant text is handed over.
    let fx = Fx::new();
    fx.write("a.md", "# a");
    let texts = assistant_texts(&rows);
    let cwd = fx.text("");
    let ctx = Context::new(&texts, &cwd, &[], &[]);
    assert_eq!(
        code(&serve(&params("/etc/hosts"), &ctx)),
        Some(Code::Forbidden)
    );
    assert!(serve(&params("a.md"), &ctx).is_ok());
}

#[test]
fn relative_dot_absolute_and_suffixed_mentions_resolve_against_the_session_folder() {
    let fx = Fx::new();
    fx.write("report.html", "<p>hi</p>");
    fx.write("docs/notes.md", "# notes");
    let abs = fx.text("abs.txt");
    fs::write(&abs, "abs").unwrap();
    let said = [
        "open `report.html` and `./docs/notes.md:12:3`",
        &format!("also {abs}."),
    ];
    assert!(run(&said, &fx.root, &params("report.html")).is_ok());
    assert!(run(&said, &fx.root, &params("./report.html")).is_ok());
    assert!(run(&said, &fx.root, &params("report.html:7")).is_ok());
    assert!(run(&said, &fx.root, &params("docs/notes.md#L3")).is_ok());
    assert!(run(&said, &fx.root, &params(&abs)).is_ok());
    // A dot-dot detour that lands on a mentioned file is the same file.
    assert!(run(&said, &fx.root, &params("docs/../report.html")).is_ok());
    // A sibling nobody mentioned is not.
    fx.write("other.md", "x");
    assert_eq!(
        code(&run(&said, &fx.root, &params("other.md"))),
        Some(Code::Forbidden)
    );
}

#[test]
fn a_tilde_mention_is_expanded_before_it_is_compared() {
    let fx = Fx::new();
    let said = ["`~/zz-no-such-folder/report.html`"];
    // Mentioned and absent: found-or-not is the disk's answer, not the mention check's.
    let result = run(&said, &fx.root, &params("~/zz-no-such-folder/report.html"));
    assert_eq!(code(&result), Some(Code::NotFound));
    let other = run(&said, &fx.root, &params("~/zz-no-such-folder/other.html"));
    assert_eq!(code(&other), Some(Code::Forbidden));
}

#[test]
fn a_mono_mention_is_found_under_its_project_roots() {
    let fx = Fx::new();
    let project = fx.root.join("proj");
    fx.write("proj/docs/a.md", "# a");
    let home = fx.root.join("home");
    fs::create_dir_all(&home).unwrap();
    let texts = vec!["see `docs/a.md`".to_owned()];
    let roots = vec![project.to_string_lossy().into_owned()];
    let cwd = home.to_string_lossy().into_owned();
    let ctx = Context::new(&texts, &cwd, &roots, &[]);
    let reply = serve(&params("docs/a.md"), &ctx).unwrap();
    assert_eq!(reply.path, project.join("docs/a.md").to_string_lossy());
    assert_eq!(reply.kind, Kind::Markdown);
}

#[cfg(unix)]
#[test]
fn a_mentioned_symlink_to_a_secret_is_refused() {
    use std::os::unix::fs::symlink;
    let fx = Fx::new();
    fx.write("work/.keep", "");
    let key = fx.write("home/.ssh/id_rsa", "fake key");
    let env = fx.write("elsewhere/.env", "FAKE=1");
    symlink(&key, fx.path("work/notes.txt")).unwrap();
    symlink(&env, fx.path("work/config.txt")).unwrap();
    let work = fx.path("work");
    let said = ["`notes.txt` and `config.txt`"];
    for name in ["notes.txt", "config.txt"] {
        let result = run(&said, &work, &params(name));
        assert_eq!(code(&result), Some(Code::PathDenied), "{name}");
    }
}

#[cfg(unix)]
#[test]
fn a_directory_symlink_into_a_secret_folder_is_refused() {
    use std::os::unix::fs::symlink;
    let fx = Fx::new();
    fx.write("home/.aws/credentials", "fake");
    fx.write("work/.keep", "");
    symlink(fx.path("home/.aws"), fx.path("work/cloud")).unwrap();
    let said = ["`./cloud/credentials`"];
    assert_eq!(
        code(&run(
            &said,
            &fx.path("work"),
            &params("./cloud/credentials")
        )),
        Some(Code::PathDenied)
    );
}

#[test]
fn deny_list_names_are_refused_in_any_case_and_anywhere() {
    let fx = Fx::new();
    for name in [
        ".ENV",
        ".env",
        ".Env.Local",
        "id_rsa",
        "server.PEM",
        ".ssh/config",
        ".git/config",
    ] {
        fx.write(name, "fake");
        let said = [format!("`./{name}`")];
        let said: Vec<&str> = said.iter().map(String::as_str).collect();
        let result = run(&said, &fx.root, &params(name));
        assert_eq!(code(&result), Some(Code::PathDenied), "{name}");
    }
    // The example file stays readable.
    fx.write(".env.example", "FAKE=");
    assert!(run(&["`.env.example`"], &fx.root, &params(".env.example")).is_ok());
}

#[test]
fn a_project_adds_its_own_patterns_and_the_app_folders_are_never_served() {
    let fx = Fx::new();
    fx.write("proj/private/plan.md", "x");
    fx.write("appdata/state.json", "{}");
    fx.write("home/.config/gh/hosts.yml", "x: 1");
    fx.write("home/.config/monoptah/updater.key.txt", "x");
    let project = Project::new(
        "p1",
        &fx.path("proj"),
        true,
        DenyList::new(&["private/**".to_owned()]),
    );
    let said = [format!(
        "`{}` `{}` `{}` `{}`",
        fx.text("proj/private/plan.md"),
        fx.text("appdata/state.json"),
        fx.text("home/.config/gh/hosts.yml"),
        fx.text("home/.config/monoptah/updater.key.txt")
    )];
    let said: Vec<&str> = said.iter().map(String::as_str).collect();
    let tune = |ctx: &mut Context| {
        ctx.private_dirs = vec![fx.path("appdata")];
        ctx.home = Some(fx.path("home"));
    };
    for path in [
        "proj/private/plan.md",
        "appdata/state.json",
        "home/.config/gh/hosts.yml",
        "home/.config/monoptah/updater.key.txt",
    ] {
        let result = run_with(
            &said,
            &fx.root,
            std::slice::from_ref(&project),
            &params(&fx.text(path)),
            tune,
        );
        assert_eq!(code(&result), Some(Code::PathDenied), "{path}");
    }
}

#[test]
fn a_directory_is_never_served() {
    let fx = Fx::new();
    fx.write("sub/a.txt", "x");
    let said = ["look in ./sub and `docs.d`"];
    fs::create_dir_all(fx.path("docs.d")).unwrap();
    assert_eq!(
        code(&run(&said, &fx.root, &params("./sub"))),
        Some(Code::Forbidden)
    );
    assert_eq!(
        code(&run(&said, &fx.root, &params("docs.d"))),
        Some(Code::Forbidden)
    );
}

#[test]
fn kinds_follow_the_extension_then_the_content() {
    let fx = Fx::new();
    fx.write("a.html", "<p>x</p>");
    fx.write("b.md", "# b");
    fx.write("c.png", [0x89, b'P', b'N', b'G', 1, 2, 3]);
    fx.write("d.rs", "fn main() {}");
    fx.write("e.unknown", "plain words");
    fx.write("f.unknown", [0u8, 1, 2, 3]);
    fx.write("g.pdf", "%PDF-1.4");
    fx.write("LICENSE", "MIT");
    let said = ["`a.html` `b.md` `c.png` `d.rs` `e.unknown` `f.unknown` `g.pdf` `LICENSE`"];
    let kind = |name: &str| run(&said, &fx.root, &params(name)).unwrap();
    assert_eq!(kind("a.html").kind, Kind::Html);
    assert_eq!(kind("b.md").kind, Kind::Markdown);
    let png = kind("c.png");
    assert_eq!(
        (png.kind, png.mime, png.encoding),
        (Kind::Image, Some("image/png"), "base64")
    );
    assert_eq!(kind("d.rs").kind, Kind::Text);
    assert_eq!(kind("e.unknown").kind, Kind::Text);
    let binary = kind("f.unknown");
    assert_eq!(
        (binary.kind, binary.encoding, binary.data.as_str()),
        (Kind::Binary, "none", "")
    );
    assert_eq!(binary.size, 4);
    assert_eq!(kind("g.pdf").kind, Kind::Binary);
    // `LICENSE` has no extension and no `.`: it is only a mention when written as a path.
    assert_eq!(
        code(&run(&said, &fx.root, &params("LICENSE"))),
        Some(Code::Forbidden)
    );
    let said = ["`./LICENSE`"];
    assert_eq!(
        run(&said, &fx.root, &params("./LICENSE")).unwrap().kind,
        Kind::Text
    );
}

#[test]
fn the_reply_names_the_file_and_the_selected_project() {
    let fx = Fx::new();
    fx.write("proj/docs/a.md", "# a");
    let said = ["`proj/docs/a.md`"];
    let selected = Project::new("p1", &fx.path("proj"), true, DenyList::default());
    let reply = run_with(
        &said,
        &fx.root,
        std::slice::from_ref(&selected),
        &params("proj/docs/a.md"),
        |_| {},
    )
    .unwrap();
    assert_eq!(reply.name, "a.md");
    assert_eq!(reply.size, 3);
    assert_eq!(
        reply.project,
        Some(("p1".to_owned(), "docs/a.md".to_owned()))
    );
    let json = reply.to_json();
    assert_eq!(
        json["project"],
        json!({"projectId": "p1", "path": "docs/a.md"})
    );
    assert_eq!(json["kind"], "markdown");
    assert_eq!(json["encoding"], "utf8");
    assert_eq!(json["data"], "# a");
    assert!(json.get("nextOffset").is_none());
    let unselected = Project::new("p1", &fx.path("proj"), false, DenyList::default());
    let reply = run_with(
        &said,
        &fx.root,
        &[unselected],
        &params("proj/docs/a.md"),
        |_| {},
    )
    .unwrap();
    assert_eq!(reply.project, None);
}

#[test]
fn a_file_over_the_cap_is_too_large_but_a_big_binary_still_answers() {
    let fx = Fx::new();
    let big = fx.write("big.txt", "");
    fs::File::options()
        .write(true)
        .open(&big)
        .unwrap()
        .set_len(MENTIONED_FILE_MAX_BYTES + 1)
        .unwrap();
    let bin = fx.write("big.bin", "");
    fs::File::options()
        .write(true)
        .open(&bin)
        .unwrap()
        .set_len(MENTIONED_FILE_MAX_BYTES + 1)
        .unwrap();
    let at_cap = fx.write("cap.txt", "");
    fs::File::options()
        .write(true)
        .open(&at_cap)
        .unwrap()
        .set_len(MENTIONED_FILE_MAX_BYTES)
        .unwrap();
    let said = ["`big.txt` `big.bin` `cap.txt`"];
    assert_eq!(
        code(&run(&said, &fx.root, &params("big.txt"))),
        Some(Code::TooLarge)
    );
    let binary = run(&said, &fx.root, &params("big.bin")).unwrap();
    assert_eq!(
        (binary.kind, binary.size, binary.encoding),
        (Kind::Binary, MENTIONED_FILE_MAX_BYTES + 1, "none")
    );
    // Exactly at the cap is served, one page of it.
    let page = run(&said, &fx.root, &params("cap.txt")).unwrap();
    assert_eq!(page.served as usize, MENTIONED_FILE_PAGE_BYTES);
    assert_eq!(page.next_offset, Some(MENTIONED_FILE_PAGE_BYTES as u64));
    // A tighter cap in the context is honoured too.
    fx.write("small.txt", "0123456789");
    let result = run_with(
        &["`small.txt`"],
        &fx.root,
        &[],
        &params("small.txt"),
        |ctx| ctx.max_bytes = 5,
    );
    assert_eq!(code(&result), Some(Code::TooLarge));
}

fn pages(texts: &[&str], cwd: &Path, name: &str, page: usize) -> Vec<Reply> {
    let mut out = Vec::new();
    let mut offset = 0;
    loop {
        let mut request = params(name);
        request.offset = offset;
        let reply = run_with(texts, cwd, &[], &request, |ctx| ctx.page_bytes = page).unwrap();
        let next = reply.next_offset;
        out.push(reply);
        match next {
            Some(next) => {
                assert!(next > offset, "paging must advance");
                offset = next;
            }
            None => return out,
        }
        assert!(out.len() < 1000);
    }
}

#[test]
fn text_pages_cut_on_character_boundaries_and_join_back() {
    let fx = Fx::new();
    // 1, 2, 3 and 4 byte characters, so every page edge can fall inside one.
    let text = "aé€😀b".repeat(40);
    fx.write("t.txt", &text);
    for page in [4, 5, 6, 7, 11, 64] {
        let replies = pages(&["`t.txt`"], &fx.root, "t.txt", page);
        let joined: String = replies.iter().map(|r| r.data.as_str()).collect();
        assert_eq!(joined, text, "page {page}");
        let mut expect = 0;
        for reply in &replies {
            assert_eq!(reply.offset, expect, "page {page}");
            assert!(reply.data.len() <= page, "page {page}");
            assert!(!reply.data.contains('\u{fffd}'), "page {page}");
            expect += reply.data.len() as u64;
            assert_eq!(reply.total, text.len() as u64);
        }
    }
}

#[test]
fn an_offset_inside_a_character_moves_to_the_next_one() {
    let fx = Fx::new();
    fx.write("t.txt", "a€b");
    let mut request = params("t.txt");
    request.offset = 2; // inside the three bytes of the euro sign
    let reply = run(&["`t.txt`"], &fx.root, &request).unwrap();
    assert_eq!((reply.offset, reply.data.as_str()), (4, "b"));
    request.offset = 99;
    let reply = run(&["`t.txt`"], &fx.root, &request).unwrap();
    assert_eq!((reply.data.as_str(), reply.next_offset), ("", None));
}

#[test]
fn image_pages_are_base64_and_reassemble_to_the_bytes() {
    let fx = Fx::new();
    let bytes: Vec<u8> = (0..=250u8).cycle().take(1000).collect();
    fx.write("i.png", &bytes);
    let replies = pages(&["`i.png`"], &fx.root, "i.png", 333);
    assert_eq!(replies.len(), 4);
    let mut joined = Vec::new();
    for reply in &replies {
        assert_eq!(reply.encoding, "base64");
        assert_eq!(reply.total, 1000);
        joined.extend(STANDARD.decode(&reply.data).unwrap());
    }
    assert_eq!(joined, bytes);
}

#[test]
fn html_is_served_as_a_sanitized_document_with_placeholders() {
    let fx = Fx::new();
    fx.write(
        "site/theme.css",
        "h1 { color: red } .hero { background: url(img/hero.png) }",
    );
    fx.write("site/secrets.css", "p { color: blue }");
    fx.write("outside.css", "p { margin: 9px }");
    fx.write(
        "site/report.html",
        concat!(
            "<html><head><link rel=stylesheet href=theme.css>",
            "<link rel=stylesheet href=secrets.css><link rel=stylesheet href=../outside.css>",
            "<link rel=stylesheet href=https://e.test/x.css><script>alert(1)</script></head>",
            "<body><h1 onclick=x()>T</h1><div class=hero>h</div><img src=img/a.png onerror=x()>",
            "<img src=https://e.test/a.png><a href=javascript:x()>j</a><a href=#end>e</a>",
            "<p>text</p><iframe src=x></iframe></body></html>",
        ),
    );
    let site = fx.path("site");
    let reply = run(&["`report.html`"], &site, &params("report.html")).unwrap();
    assert_eq!(reply.kind, Kind::Html);
    let html = &reply.data;
    for bad in [
        "<script",
        "alert",
        "onclick",
        "onerror",
        "<iframe",
        "e.test",
        "javascript:",
        "<link",
        "<style",
        "margin: 9px",
        "color: blue",
    ] {
        assert!(!html.contains(bad), "{bad} survived: {html}");
    }
    assert!(html.contains("color: red"), "{html}");
    assert!(html.contains("href=\"#end\""), "{html}");
    let assets = reply.assets.clone().unwrap();
    assert_eq!(assets, ["img/hero.png", "img/a.png"]);
    assert!(
        html.contains("monoptah-asset:0") && html.contains("monoptah-asset:1"),
        "{html}"
    );
    assert_eq!(reply.total as usize, html.len());
    assert_eq!(
        reply.size,
        fs::metadata(site.join("report.html")).unwrap().len()
    );
}

#[test]
fn html_pages_cut_on_character_boundaries() {
    let fx = Fx::new();
    fx.write("p.html", format!("<p>{}</p>", "é€😀".repeat(30)));
    let replies = pages(&["`p.html`"], &fx.root, "p.html", 7);
    assert!(replies.len() > 3);
    let joined: String = replies.iter().map(|r| r.data.as_str()).collect();
    assert!(joined.ends_with("</p></body></html>"));
    assert!(!joined.contains('\u{fffd}'));
    assert_eq!(joined.len() as u64, replies[0].total);
}

fn site() -> Fx {
    let fx = Fx::new();
    fx.write("site/report.html", "<img src=img/a.png>");
    fx.write("site/img/a.png", [1u8, 2, 3, 4, 5]);
    fx.write("site/notes.txt", "text");
    fx.write("site/.git/x.png", "fake");
    fx.write("outside.png", "fake");
    fx
}

#[test]
fn an_asset_is_an_image_inside_the_mentioned_files_folder() {
    let fx = site();
    let site = fx.path("site");
    let said = ["`report.html`"];
    let reply = run(&said, &site, &asset("report.html", "img/a.png")).unwrap();
    assert_eq!(
        (reply.kind, reply.mime, reply.encoding),
        (Kind::Image, Some("image/png"), "base64")
    );
    assert_eq!(reply.path, fx.text("site/img/a.png"));
    assert_eq!(STANDARD.decode(&reply.data).unwrap(), [1, 2, 3, 4, 5]);
    assert!(run(&said, &site, &asset("report.html", "./img/a.png")).is_ok());
    // The page the asset belongs to must still be mentioned.
    assert_eq!(
        code(&run(
            &["`other.html`"],
            &site,
            &asset("report.html", "img/a.png")
        )),
        Some(Code::Forbidden)
    );
}

#[test]
fn bad_asset_references_are_refused() {
    let fx = site();
    let site = fx.path("site");
    let said = ["`report.html`"];
    let absolute = fx.text("outside.png");
    for bad in [
        "../outside.png",
        "img/../../outside.png",
        absolute.as_str(),
        "/img/a.png",
        "%2e%2e/outside.png",
        "%2E%2E%2Foutside.png",
        "img%5Ca.png",
        "img\\a.png",
        "~/a.png",
        "file:///etc/a.png",
        "https://e.test/a.png",
        "notes.txt",
        "img/a.png\0",
    ] {
        let result = run(&said, &site, &asset("report.html", bad));
        assert_eq!(code(&result), Some(Code::Forbidden), "{bad:?}");
    }
    assert_eq!(
        code(&run(&said, &site, &asset("report.html", ".git/x.png"))),
        Some(Code::PathDenied)
    );
    assert_eq!(
        code(&run(&said, &site, &asset("report.html", "img/none.png"))),
        Some(Code::NotFound)
    );
    // Assets belong to a page that shows images.
    fx.write("site/data.txt", "x");
    let result = run(&["`data.txt`"], &site, &asset("data.txt", "img/a.png"));
    assert_eq!(code(&result), Some(Code::Forbidden));
}

#[cfg(unix)]
#[test]
fn an_asset_symlink_out_of_the_folder_or_into_a_secret_is_refused() {
    use std::os::unix::fs::symlink;
    let fx = site();
    let site = fx.path("site");
    symlink(fx.path("outside.png"), fx.path("site/img/leak.png")).unwrap();
    fx.write("site/.env", "FAKE=1");
    symlink(fx.path("site/.env"), fx.path("site/img/env.png")).unwrap();
    symlink(fx.path("site/notes.txt"), fx.path("site/img/text.png")).unwrap();
    let said = ["`report.html`"];
    assert_eq!(
        code(&run(&said, &site, &asset("report.html", "img/leak.png"))),
        Some(Code::Forbidden)
    );
    assert_eq!(
        code(&run(&said, &site, &asset("report.html", "img/env.png"))),
        Some(Code::PathDenied)
    );
    // A link named like an image whose target is not one is not served.
    assert_eq!(
        code(&run(&said, &site, &asset("report.html", "img/text.png"))),
        Some(Code::Forbidden)
    );
}

#[test]
fn an_asset_over_the_cap_is_too_large() {
    let fx = site();
    let result = run_with(
        &["`report.html`"],
        &fx.path("site"),
        &[],
        &asset("report.html", "img/a.png"),
        |ctx| ctx.max_bytes = 3,
    );
    assert_eq!(code(&result), Some(Code::TooLarge));
}

#[test]
fn parameters_are_validated() {
    let ok = Params::from_json(&json!({"path": "a.md", "offset": 5})).unwrap();
    assert_eq!((ok.path.as_str(), ok.asset, ok.offset), ("a.md", None, 5));
    let with_asset = Params::from_json(&json!({"path": "a.html", "asset": "i.png"})).unwrap();
    assert_eq!(with_asset.asset.as_deref(), Some("i.png"));
    let long = "a".repeat(MAX_PATH_CHARS + 1);
    for bad in [
        json!({}),
        json!({"path": ""}),
        json!({"path": 3}),
        json!({"path": long}),
        json!({"path": "a\0b"}),
        json!({"path": "a", "asset": ""}),
        json!({"path": "a", "asset": long}),
        json!({"path": "a", "offset": -1}),
        json!({"path": "a", "offset": 1.5}),
        json!({"path": "a", "offset": "1"}),
    ] {
        assert!(Params::from_json(&bad).is_err(), "{bad}");
    }
    assert!(Params::from_json(&json!({"path": "a".repeat(MAX_PATH_CHARS)})).is_ok());
}

#[test]
fn handle_counts_calls_budgets_bytes_and_reports_what_the_audit_needs() {
    let fx = Fx::new();
    fx.write("a.md", "# hello");
    let texts = vec!["`a.md`".to_owned()];
    let cwd = fx.text("");
    let mut ctx = Context::new(&texts, &cwd, &[], &[]);
    ctx.home = None;
    let device = format!("handle-test-{}", fx.root.display());
    let call = handle(&device, &json!({"path": "a.md"}), &ctx);
    assert!(call.result.is_ok());
    assert_eq!(
        (call.path.as_str(), call.bytes),
        (fx.text("a.md").as_str(), 7)
    );
    assert_eq!(call.outcome(), audit::Outcome::Ok);
    // A refusal names the requested path and serves nothing.
    let call = handle(&device, &json!({"path": "zz.md", "asset": "i.png"}), &ctx);
    assert_eq!(
        (call.path.as_str(), call.asset.as_deref(), call.bytes),
        ("zz.md", Some("i.png"), 0)
    );
    assert_eq!(call.outcome(), audit::Outcome::Denied(Code::Forbidden));
    // A malformed request is an error with no code, still audited by the caller.
    assert_eq!(
        handle(&device, &json!({"path": 1}), &ctx).outcome(),
        audit::Outcome::Error
    );
    // The 121st call within the minute is busy.
    let mut last = None;
    for _ in 0..budget::CALLS_PER_WINDOW {
        last = Some(handle(&device, &json!({"path": "a.md"}), &ctx));
    }
    assert_eq!(last.unwrap().outcome(), audit::Outcome::Denied(Code::Busy));
}
