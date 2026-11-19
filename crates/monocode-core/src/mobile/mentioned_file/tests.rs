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
        source: false,
    }
}

fn asset(path: &str, asset: &str) -> Params {
    Params {
        path: path.to_owned(),
        asset: Some(asset.to_owned()),
        offset: 0,
        source: false,
    }
}

/// A tool row of a file tool (`Write`) that was given `path`.
fn write_row(path: &str) -> String {
    let mut call = crate::tool::ToolCall::new("1", "Write");
    call.state = crate::tool::ToolState::Completed;
    call.input = json!({ "file_path": path, "content": "x" }).to_string();
    serde_json::to_string(&call).unwrap()
}

/// A tool row of `name` with `input` and `output`.
fn tool_row_of(name: &str, input: &str, output: &str) -> String {
    let mut call = crate::tool::ToolCall::new("1", name);
    call.state = crate::tool::ToolState::Completed;
    call.input = input.to_owned();
    call.output = output.to_owned();
    serde_json::to_string(&call).unwrap()
}

/// `Write` rows for everything `texts` mention.
fn write_rows_for(texts: &[String]) -> Vec<String> {
    texts
        .iter()
        .flat_map(|text| mentions::extract_mentions(text))
        .map(|mention| write_row(&mention))
        .collect()
}

/// One call with `texts` as the assistant's messages and `cwd` as the session folder. The agent
/// is taken to have run a file tool on everything it names, so a file outside a project is
/// judged on the mention alone; [`run_evidence`] says otherwise.
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
    let tool = write_rows_for(&texts);
    let mut ctx = Context::new(&texts, &cwd, &[], projects);
    ctx.tool_texts = &tool;
    ctx.home = None;
    tune(&mut ctx);
    serve(params, &ctx)
}

/// One call with separate assistant messages and tool rows.
fn run_evidence(
    assistant: &[&str],
    tool: &[&str],
    cwd: &Path,
    projects: &[Project],
    params: &Params,
) -> Result<Reply, Failure> {
    let assistant: Vec<String> = assistant.iter().map(|t| (*t).to_owned()).collect();
    let tool: Vec<String> = tool.iter().map(|t| (*t).to_owned()).collect();
    let cwd = cwd.to_string_lossy().into_owned();
    let mut ctx = Context::new(&assistant, &cwd, &[], projects);
    ctx.tool_texts = &tool;
    ctx.home = None;
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
            text: write_row("c.md"),
            first: true,
            sent_at: None,
        },
    ];
    assert_eq!(assistant_texts(&rows), ["ok `a.md`"]);
    assert_eq!(tool_texts(&rows), [write_row("c.md")]);
    // A mention in a user message does not authorize: only the assistant text is handed over.
    let fx = Fx::new();
    fx.write("a.md", "# a");
    let texts = assistant_texts(&rows);
    let tool = tool_texts(&rows);
    let cwd = fx.text("");
    let mut ctx = Context::new(&texts, &cwd, &[], &[]);
    ctx.tool_texts = &tool;
    ctx.home = None;
    let hosts = fx.write("hosts", "x");
    assert_eq!(
        code(&serve(&params(&hosts.to_string_lossy()), &ctx)),
        Some(Code::Forbidden)
    );
    // `a.md` is mentioned by the assistant but no tool row names it: outside a project that is
    // not enough (see the tool evidence tests).
    assert_eq!(code(&serve(&params("a.md"), &ctx)), Some(Code::Forbidden));
    // A tool row that names another file called `a.md` is not that file.
    let other = vec![write_row("/nowhere/a.md")];
    ctx.tool_texts = &other;
    assert_eq!(code(&serve(&params("a.md"), &ctx)), Some(Code::Forbidden));
    let named = vec![write_row("a.md")];
    ctx.tool_texts = &named;
    assert!(serve(&params("a.md"), &ctx).is_ok());
}

// POSIX paths in the text (`/tmp/...`, `~/`).
#[cfg(unix)]
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

// POSIX paths in the text (`/tmp/...`, `~/`).
#[cfg(unix)]
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
    let tool = write_rows_for(&texts);
    let mut ctx = Context::new(&texts, &cwd, &roots, &[]);
    ctx.tool_texts = &tool;
    ctx.home = None;
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

// POSIX paths in the text (`/tmp/...`, `~/`).
#[cfg(unix)]
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
    fx.write("NOTES", "n");
    let said = ["`a.html` `b.md` `c.png` `d.rs` `e.unknown` `f.unknown` `g.pdf` `LICENSE` `NOTES`"];
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
    // `LICENSE` is one of the names the web chat also makes a chip (a known file name without
    // an extension); `NOTES` has no extension and is no known name: only a path mentions it.
    assert_eq!(kind("LICENSE").kind, Kind::Text);
    assert_eq!(
        code(&run(&said, &fx.root, &params("NOTES"))),
        Some(Code::Forbidden)
    );
    let said = ["`./NOTES`"];
    assert_eq!(
        run(&said, &fx.root, &params("./NOTES")).unwrap().kind,
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
    let at_cap = fx.write("cap.png", "");
    fs::File::options()
        .write(true)
        .open(&at_cap)
        .unwrap()
        .set_len(MENTIONED_FILE_MAX_BYTES)
        .unwrap();
    let said = ["`big.txt` `big.bin` `cap.png`"];
    assert_eq!(
        code(&run(&said, &fx.root, &params("big.txt"))),
        Some(Code::TooLarge)
    );
    let binary = run(&said, &fx.root, &params("big.bin")).unwrap();
    assert_eq!(
        (binary.kind, binary.size, binary.encoding),
        (Kind::Binary, MENTIONED_FILE_MAX_BYTES + 1, "none")
    );
    // Exactly at the cap is served, one page of it (an image: a page of text is cut shorter).
    let page = run(&said, &fx.root, &params("cap.png")).unwrap();
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
fn spellings_of_one_stylesheet_path_apply_it_and_count_once() {
    let fx = Fx::new();
    fx.write("site/a.css", "h1 { color: red }");
    fx.write(
        "site/p.html",
        concat!(
            "<html><head><link rel=stylesheet href=a.css>",
            "<link rel=stylesheet href=./a.css><link rel=stylesheet href=././a.css></head>",
            "<body><h1>T</h1></body></html>",
        ),
    );
    let site = fx.path("site");
    let reply = run(&["`p.html`"], &site, &params("p.html")).unwrap();
    assert!(reply.data.contains("color: red"), "{}", reply.data);
}

#[test]
fn a_call_without_a_device_counts_toward_the_global_transform_limit() {
    let mut state = FlightState::default();
    for _ in 0..MAX_TRANSFORMS {
        assert!(state.admit("", MAX_TRANSFORMS));
    }
    assert!(!state.admit("", MAX_TRANSFORMS));
    assert!(!state.admit("phone", MAX_TRANSFORMS));
}

#[test]
fn html_pages_cut_on_character_boundaries() {
    let fx = Fx::new();
    fx.write("p.html", format!("<p>{}</p>", "é€😀".repeat(30)));
    let replies = pages(&["`p.html`"], &fx.root, "p.html", 7);
    assert!(replies.len() > 3);
    let joined: String = replies.iter().map(|r| r.data.as_str()).collect();
    assert!(joined.ends_with("</p></div></body></html>"));
    assert!(!joined.contains('\u{fffd}'));
    assert_eq!(joined.len() as u64, replies[0].total);
}

fn site() -> Fx {
    let fx = Fx::new();
    fx.write(
        "site/report.html",
        concat!(
            "<img src=img/a.png><img src=.git/x.png><img src=img/none.png>",
            "<img src=img/leak.png><img src=img/env.png><img src=img/text.png>",
        ),
    );
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
    assert_eq!(
        (ok.path.as_str(), ok.asset, ok.offset, ok.source),
        ("a.md", None, 5, false)
    );
    let source = Params::from_json(&json!({"path": "a.md", "source": true})).unwrap();
    assert!(source.source);
    assert!(Params::from_json(&json!({"path": "a.md", "source": null})).is_ok());
    assert!(Params::from_json(&json!({"path": "a.md", "source": "yes"})).is_err());
    assert!(Params::from_json(&json!({"path": "a.md", "source": 1})).is_err());
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
    let tool = write_rows_for(&texts);
    let mut ctx = Context::new(&texts, &cwd, &[], &[]);
    ctx.tool_texts = &tool;
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

// ---- R1: tool evidence outside the device's projects ----

#[cfg(unix)]
#[test]
fn outside_a_project_the_assistant_alone_does_not_authorize_a_file() {
    let fx = Fx::new();
    let secret = fx.write("home/private-notes.txt", "private");
    let path = secret.to_string_lossy().into_owned();
    // A control device asked the assistant to "reply with <path>": a mention, but no tool ran.
    let said = [format!("Sure: `{path}`")];
    let said: Vec<&str> = said.iter().map(String::as_str).collect();
    let result = run_evidence(&said, &[], &fx.root, &[], &params(&path));
    assert_eq!(code(&result), Some(Code::Forbidden));
    // A tool row that is not about this file does not help either.
    let other = tool_row_of(
        "Bash",
        &json!({ "command": format!("cat {}", fx.text("home/other.txt")) }).to_string(),
        "",
    );
    let result = run_evidence(&said, &[&other], &fx.root, &[], &params(&path));
    assert_eq!(code(&result), Some(Code::Forbidden));
    // A tool row that wrote it does.
    let write = write_row(&path);
    // Echoing the path in a command, or in the output of a tool, is not a tool run on it.
    let echo = tool_row_of(
        "Bash",
        &json!({ "command": format!("echo {path}") }).to_string(),
        &path,
    );
    let result = run_evidence(&said, &[&echo], &fx.root, &[], &params(&path));
    assert_eq!(code(&result), Some(Code::Forbidden));
    // Neither is the answer a control device typed to a question.
    let answer = tool_row_of(
        "AskUserQuestion",
        &json!({ "path": &path }).to_string(),
        &path,
    );
    let result = run_evidence(&said, &[&answer], &fx.root, &[], &params(&path));
    assert_eq!(code(&result), Some(Code::Forbidden));
    assert!(run_evidence(&said, &[&write], &fx.root, &[], &params(&path)).is_ok());
    // A tool row alone (no assistant mention) does not authorize.
    let result = run_evidence(&["nothing"], &[&write], &fx.root, &[], &params(&path));
    assert_eq!(code(&result), Some(Code::Forbidden));
}

#[test]
fn a_tool_row_names_a_file_relative_to_the_session_folder_or_through_a_link() {
    let fx = Fx::new();
    fx.write("work/out/report.html", "<p>x</p>");
    let work = fx.path("work");
    let said = ["`out/report.html`"];
    // A relative tool path is the same file as the relative mention.
    assert!(run_evidence(
        &said,
        &[&write_row("out/report.html")],
        &work,
        &[],
        &params("out/report.html")
    )
    .is_ok());
    assert!(run_evidence(
        &said,
        &[&write_row("./out/report.html")],
        &work,
        &[],
        &params("out/report.html")
    )
    .is_ok());
    // The same name in another folder is another file.
    let result = run_evidence(
        &said,
        &[&write_row("other/report.html")],
        &work,
        &[],
        &params("out/report.html"),
    );
    assert_eq!(code(&result), Some(Code::Forbidden));
}

#[test]
fn inside_a_selected_project_the_mention_is_enough_and_an_unselected_one_is_outside() {
    let fx = Fx::new();
    fx.write("proj/docs/a.md", "# a");
    let said = ["`proj/docs/a.md`"];
    let selected = Project::new("p1", &fx.path("proj"), true, DenyList::default());
    assert!(run_evidence(
        &said,
        &[],
        &fx.root,
        std::slice::from_ref(&selected),
        &params("proj/docs/a.md")
    )
    .is_ok());
    let unselected = Project::new("p1", &fx.path("proj"), false, DenyList::default());
    let result = run_evidence(
        &said,
        &[],
        &fx.root,
        &[unselected],
        &params("proj/docs/a.md"),
    );
    assert_eq!(code(&result), Some(Code::Forbidden));
}

#[test]
fn sufficient_evidence_stops_the_history_scan_only_when_the_call_is_decided() {
    let fx = Fx::new();
    fx.write("a.md", "# a");
    let cwd = fx.text("");
    let assistant = vec!["`a.md`".to_owned()];
    let none: Vec<String> = Vec::new();
    let mut ctx = Context::new(&assistant, &cwd, &[], &[]);
    ctx.home = None;
    // Mentioned, outside a project, no tool row yet: keep reading older turns.
    assert!(!sufficient(&ctx, &params("a.md")));
    let tool = vec![write_row("a.md")];
    ctx.tool_texts = &tool;
    assert!(sufficient(&ctx, &params("a.md")));
    // Not mentioned at all: older turns may still mention it.
    let mut ctx = Context::new(&none, &cwd, &[], &[]);
    ctx.tool_texts = &tool;
    assert!(!sufficient(&ctx, &params("a.md")));
}

// ---- R2: the mention and the request must be the same canonical file ----

#[test]
fn a_relative_mention_authorizes_the_one_file_it_resolves_to_not_the_name_under_every_root() {
    let fx = Fx::new();
    let (one, two) = (fx.root.join("one"), fx.root.join("two"));
    fx.write("one/a.md", "# one");
    fx.write("two/a.md", "# two");
    fx.write("home/.keep", "");
    let texts = vec!["see `a.md`".to_owned()];
    let roots = vec![
        one.to_string_lossy().into_owned(),
        two.to_string_lossy().into_owned(),
    ];
    let cwd = fx.text("home");
    let tool = write_rows_for(&texts);
    let mut ctx = Context::new(&texts, &cwd, &roots, &[]);
    ctx.tool_texts = &tool;
    ctx.home = None;
    // The mention re-roots to the first root that has it.
    let reply = serve(&params("a.md"), &ctx).unwrap();
    assert_eq!(reply.path, one.join("a.md").to_string_lossy());
    // The same name under the second root is another file: forbidden, though lexically a candidate.
    let other = two.join("a.md").to_string_lossy().into_owned();
    assert_eq!(code(&serve(&params(&other), &ctx)), Some(Code::Forbidden));
}

#[test]
fn the_same_file_spelled_two_ways_is_one_file() {
    let fx = Fx::new();
    fx.write("work/docs/a.md", "# a");
    let work = fx.path("work");
    let said = ["`./docs/a.md`"];
    assert!(run(&said, &work, &params("docs/a.md")).is_ok());
    assert!(run(&said, &work, &params("docs/./a.md")).is_ok());
}

// ---- R3: more deny-list outside the projects ----

#[cfg(unix)]
#[test]
fn credential_and_app_data_places_under_home_are_refused() {
    let fx = Fx::new();
    let home = fx.path("home");
    let names = [
        ".kube/notes.txt",
        ".docker/notes.txt",
        ".azure/notes.txt",
        ".config/gcloud/notes.txt",
        ".config/github-copilot/hosts.txt",
        ".cargo/credentials.toml",
        ".git-credentials",
        ".gitconfig",
        ".vault-token",
        ".pgpass",
        ".my.cnf",
        ".zsh_history",
        ".bash_history",
        ".claude/.credentials.json",
        ".claude.json",
        ".codex/auth.json",
        "Library/Application Support/com.monoptah.desktop/provider-accounts/a/x.txt",
        "Library/Application Support/Monoptah Native/x.txt",
        "Library/Application Support/Monoptah Development/x.txt",
        "Library/Application Support/com.monoptah.desktop.dev/x.txt",
        "Library/WebKit/com.monoptah.desktop/x.txt",
        ".config/monoptah/updater.key.txt",
    ];
    for name in names {
        let path = fx.write(&format!("home/{name}"), "fake");
        let path = path.to_string_lossy().into_owned();
        // A link destination, because some of these paths hold a space.
        let said = [format!("[x](<{path}>)")];
        let said: Vec<&str> = said.iter().map(String::as_str).collect();
        let result = run_with(&said, &fx.root, &[], &params(&path), |ctx| {
            ctx.home = Some(home.clone());
        });
        assert_eq!(code(&result), Some(Code::PathDenied), "{name}");
    }
    // An ordinary file of the home folder is still served.
    let ok = fx.write("home/Documents/report.md", "# r");
    let path = ok.to_string_lossy().into_owned();
    let said = [format!("`{path}`")];
    let said: Vec<&str> = said.iter().map(String::as_str).collect();
    let result = run_with(&said, &fx.root, &[], &params(&path), |ctx| {
        ctx.home = Some(home.clone());
    });
    assert!(result.is_ok());
}

#[cfg(unix)]
#[test]
fn a_hard_link_to_a_credential_file_is_refused_and_an_ordinary_one_is_not() {
    let fx = Fx::new();
    let home = fx.path("home");
    fx.write("home/.ssh/key", "fake key");
    fx.write("work/shared.txt", "shared");
    std::fs::hard_link(home.join(".ssh/key"), fx.path("work/alias.txt")).unwrap();
    std::fs::hard_link(fx.path("work/shared.txt"), fx.path("work/twin.txt")).unwrap();
    let work = fx.path("work");
    let said = ["`alias.txt` `twin.txt`"];
    let tune = |ctx: &mut Context| ctx.home = Some(home.clone());
    let alias = run_with(&said, &work, &[], &params("alias.txt"), tune);
    assert_eq!(code(&alias), Some(Code::PathDenied));
    let twin = run_with(&said, &work, &[], &params("twin.txt"), tune);
    assert!(twin.is_ok(), "{:?}", code(&twin));
}

// ---- R4: HTML transform limits, cache, escaped page size ----

#[test]
fn an_html_source_over_the_transform_cap_is_too_large() {
    let fx = Fx::new();
    let big = format!(
        "<p>{}</p>",
        "a".repeat(MAX_REFERENCING_SOURCE_BYTES as usize)
    );
    fx.write("big.html", big);
    let result = run(&["`big.html`"], &fx.root, &params("big.html"));
    assert_eq!(code(&result), Some(Code::TooLarge));
    // The raw source of the same file is still served as text, in pages.
    let mut request = params("big.html");
    request.source = true;
    let reply = run(&["`big.html`"], &fx.root, &request).unwrap();
    assert_eq!(reply.kind, Kind::Text);
    assert!(reply.next_offset.is_some());
}

#[test]
fn many_elements_times_many_rules_is_too_large() {
    let fx = Fx::new();
    let elements = "<i>x</i>".repeat(4_000);
    let rules = "i{color:red}".repeat(3_000);
    fx.write("heavy.html", format!("<style>{rules}</style>{elements}"));
    let result = run(&["`heavy.html`"], &fx.root, &params("heavy.html"));
    assert_eq!(code(&result), Some(Code::TooLarge));
}

#[test]
fn one_rule_with_a_huge_declaration_block_is_too_large() {
    let fx = Fx::new();
    // One rule: the old rule count saw 1. 1 MB of declarations times 20k elements would inline
    // far past the output cap, so it is refused before css-inline runs.
    let declarations = "color:red;".repeat(100_000);
    let elements = "<i>x</i>".repeat(20_000);
    fx.write(
        "wide.html",
        format!("<style>i{{{declarations}}}</style>{elements}"),
    );
    let result = run(&["`wide.html`"], &fx.root, &params("wide.html"));
    assert_eq!(code(&result), Some(Code::TooLarge));
    // The same block on a few elements is fine.
    fx.write(
        "ok.html",
        format!("<style>i{{{declarations}}}</style><i>x</i>"),
    );
    assert!(run(&["`ok.html`"], &fx.root, &params("ok.html")).is_ok());
}

// ---- the transform is bounded by structure, not by estimates ----

#[test]
fn a_deeply_nested_page_is_refused_not_a_stack_overflow() {
    let fx = Fx::new();
    // 10 KB, no CSS: aborted the whole process on the 512 KB stack of a background thread.
    fx.write("deep.html", "<div>".repeat(2000));
    let result = run(&["`deep.html`"], &fx.root, &params("deep.html"));
    assert_eq!(code(&result), Some(Code::TooLarge));
}

#[test]
fn cloned_formatting_elements_are_counted_and_refused_fast() {
    let fx = Fx::new();
    // 43 KB of source that html5ever turns into millions of elements.
    let opening: String = (0..200).map(|n| format!("<b class=c{n}>")).collect();
    let bomb = format!("<p>{opening}x</p>{}", "<p>x".repeat(10_000));
    fx.write("bomb.html", &bomb);
    fx.write(
        "bomb-css.html",
        format!("<style>.c1{{color:red}}</style>{bomb}"),
    );
    for name in ["bomb.html", "bomb-css.html"] {
        let started = std::time::Instant::now();
        let result = run(&[&format!("`{name}`")], &fx.root, &params(name));
        assert_eq!(code(&result), Some(Code::TooLarge), "{name}");
        assert!(started.elapsed().as_secs() < 5, "{:?}", started.elapsed());
    }
}

#[test]
fn walking_selectors_without_declarations_are_too_costly() {
    let fx = Fx::new();
    let nest = "<div>".repeat(200);
    let rules = "div div div div div div div div{}div~div~div~div{}";
    fx.write(
        "walk.html",
        format!(
            "<style>{rules}</style>{nest}{}",
            "<div>x</div>".repeat(3000)
        ),
    );
    let result = run(&["`walk.html`"], &fx.root, &params("walk.html"));
    assert_eq!(code(&result), Some(Code::TooLarge));
}

#[test]
fn a_realistic_report_is_still_served() {
    let fx = Fx::new();
    // 3000 elements, 200 KB of class rules (some with a descendant), nested about 20 deep.
    let rules: String = (0..3000)
        .map(|n| format!(".c{n}{{color:#123456;margin:1px}}.t .c{n}{{padding:2px}}"))
        .collect();
    let rows: String = (0..1000)
        .map(|n| format!("<tr class=\"c{n}\"><td class=\"c{n}\">v&amp;{n}</td></tr>"))
        .collect();
    let nest = format!(
        "{}<table class=t>{rows}</table>{}",
        "<div>".repeat(18),
        "</div>".repeat(18)
    );
    assert!(rules.len() > 100_000);
    fx.write("report.html", format!("<style>{rules}</style>{nest}"));
    let reply = run(&["`report.html`"], &fx.root, &params("report.html")).unwrap();
    assert!(reply.data.contains("#123456"));
}

#[test]
fn a_device_has_one_transform_in_flight_and_the_slot_is_freed() {
    let first = Flight::enter("flight-test-device").unwrap();
    assert_eq!(
        Flight::enter("flight-test-device")
            .err()
            .and_then(|f| super::super::policy::code_of_failure(&f)),
        Some(Code::Busy)
    );
    // Another device is not blocked by it.
    let other = Flight::enter("flight-test-other").unwrap();
    drop(first);
    drop(other);
    assert!(Flight::enter("flight-test-device").is_ok());
}

#[test]
fn a_page_of_control_characters_is_cut_so_the_escaped_reply_fits_the_transport() {
    let fx = Fx::new();
    // Each byte escapes to six: two MiB would be twelve.
    fx.write("ctl.txt", vec![1u8; 2 * 1024 * 1024]);
    let reply = run(&["`ctl.txt`"], &fx.root, &params("ctl.txt")).unwrap();
    assert!(json_len(&reply.data) <= MAX_DATA_JSON_BYTES);
    assert!(reply.served as usize <= MAX_DATA_JSON_BYTES / 6);
    assert_eq!(reply.next_offset, Some(reply.served));
    let wire = serde_json::to_string(&reply.to_json()).unwrap();
    assert!(wire.len() < 8 * 1024 * 1024, "{}", wire.len());
    // Paging still reaches the end and loses nothing.
    let mut offset = 0;
    let mut total = 0;
    for _ in 0..10 {
        let mut request = params("ctl.txt");
        request.offset = offset;
        let reply = run(&["`ctl.txt`"], &fx.root, &request).unwrap();
        total += reply.data.len();
        match reply.next_offset {
            Some(next) => offset = next,
            None => break,
        }
    }
    assert_eq!(total, 2 * 1024 * 1024);
}

#[test]
fn text_pages_stay_under_two_mib_even_when_the_context_allows_more() {
    let fx = Fx::new();
    fx.write("t.txt", vec![b'a'; 3 * 1024 * 1024]);
    let reply = run(&["`t.txt`"], &fx.root, &params("t.txt")).unwrap();
    assert_eq!(reply.served as usize, MENTIONED_FILE_UTF8_PAGE_BYTES);
    assert_eq!(
        reply.next_offset,
        Some(MENTIONED_FILE_UTF8_PAGE_BYTES as u64)
    );
}

// ---- R5: an asset must be referenced ----

#[test]
fn an_image_the_page_does_not_reference_is_forbidden_and_gives_no_existence_oracle() {
    let fx = site();
    let site = fx.path("site");
    fx.write("site/img/secret-photo.png", "fake");
    let said = ["`report.html`"];
    let existing = run(&said, &site, &asset("report.html", "img/secret-photo.png"));
    let missing = run(&said, &site, &asset("report.html", "img/not-there.png"));
    assert_eq!(code(&existing), Some(Code::Forbidden));
    assert_eq!(code(&missing), Some(Code::Forbidden));
    // Referenced ones still work (and a referenced missing one is the disk's answer).
    assert!(run(&said, &site, &asset("report.html", "img/a.png")).is_ok());
}

#[test]
fn a_markdown_file_references_its_images_and_only_those() {
    let fx = Fx::new();
    fx.write(
        "docs/guide.md",
        "# g\n\n![one](img/one.png)\n\n![two](./img/two%20x.png?raw=1)\n\n![r][ref]\n\n[ref]: img/three.png\n\n![remote](https://e.test/a.png)\n",
    );
    for name in ["one", "two x", "three", "unlisted"] {
        fx.write(&format!("docs/img/{name}.png"), name);
    }
    let docs = fx.path("docs");
    let said = ["`guide.md`"];
    for (asset_name, ok) in [
        ("img/one.png", true),
        ("img/two x.png", true),
        ("img/three.png", true),
        ("img/unlisted.png", false),
    ] {
        let result = run(&said, &docs, &asset("guide.md", asset_name));
        assert_eq!(result.is_ok(), ok, "{asset_name}: {:?}", code(&result));
        if !ok {
            assert_eq!(code(&result), Some(Code::Forbidden));
        }
    }
    assert_eq!(
        markdown_assets("![a](x/a.png) ![b](https://e.test/b.png) ![c](../c.png) ![d](d.txt)"),
        ["x/a.png"]
    );
}

// ---- R6: nothing but a regular file is opened ----

#[cfg(unix)]
#[test]
fn a_fifo_is_refused_without_blocking_and_a_link_is_not_followed_when_opening() {
    use std::os::unix::ffi::OsStrExt;
    let fx = Fx::new();
    let fifo = fx.path("pipe.txt");
    let name = std::ffi::CString::new(fifo.as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
    let result = run(&["`pipe.txt`"], &fx.root, &params("pipe.txt"));
    assert_eq!(code(&result), Some(Code::Forbidden));
    // The open itself refuses a link in the last component (a swap after the check).
    let target = fx.write("real.txt", "x");
    std::os::unix::fs::symlink(&target, fx.path("swapped.txt")).unwrap();
    assert!(open_regular(&fx.path("swapped.txt")).is_err());
    assert!(open_regular(&target).is_ok());
    // Not even a pipe that is opened directly blocks: it is refused.
    assert!(open_regular(&fifo).is_err());
}

// ---- R15: the raw source of a rendered page ----

#[test]
fn source_true_serves_html_and_markdown_as_raw_text() {
    let fx = Fx::new();
    fx.write("a.html", "<p onclick=x()>hi</p>");
    fx.write("b.md", "# b\n![i](i.png)");
    for (name, expected) in [
        ("a.html", "<p onclick=x()>hi</p>"),
        ("b.md", "# b\n![i](i.png)"),
    ] {
        let mut request = params(name);
        request.source = true;
        let reply = run(&["`a.html` `b.md`"], &fx.root, &request).unwrap();
        assert_eq!((reply.kind, reply.data.as_str()), (Kind::Text, expected));
        assert!(reply.assets.is_none());
        assert_eq!(reply.to_json()["kind"], "text");
    }
    // Without the flag the page is rendered as before.
    assert_eq!(
        run(&["`a.html`"], &fx.root, &params("a.html"))
            .unwrap()
            .kind,
        Kind::Html
    );
    // For a file that is not rendered the flag changes nothing.
    fx.write("c.rs", "fn main() {}");
    let mut request = params("c.rs");
    request.source = true;
    assert_eq!(
        run(&["`c.rs`"], &fx.root, &request).unwrap().kind,
        Kind::Text
    );
}

// ---- N6b: not_found must not reveal whether an unauthorized name exists ----

#[test]
fn a_missing_file_is_forbidden_unless_it_is_the_authorized_mention_itself() {
    let fx = Fx::new();
    let (one, two) = (fx.root.join("one"), fx.root.join("two"));
    fx.write("one/a.md", "# one");
    fs::create_dir_all(&two).unwrap();
    fx.write("home/.keep", "");
    let said = vec!["see `a.md`".to_owned()];
    let tool = write_rows_for(&said);
    let roots = vec![
        one.to_string_lossy().into_owned(),
        two.to_string_lossy().into_owned(),
    ];
    let cwd = fx.text("home");
    let mut ctx = Context::new(&said, &cwd, &roots, &[]);
    ctx.tool_texts = &tool;
    ctx.home = None;
    // The same name under the other root: lexically a candidate, but not the mention's target.
    // It is missing here and present there; either way the answer is the same.
    let missing_elsewhere = two.join("a.md").to_string_lossy().into_owned();
    let existing_elsewhere = fx.text("one/a.md");
    assert_eq!(
        code(&serve(&params(&missing_elsewhere), &ctx)),
        Some(Code::Forbidden)
    );
    assert!(serve(&params(&existing_elsewhere), &ctx).is_ok());
    // An unmentioned name is forbidden whether or not it exists.
    fx.write("one/b.md", "# b");
    for name in ["b.md", "zz-missing.md"] {
        assert_eq!(code(&serve(&params(name), &ctx)), Some(Code::Forbidden));
    }
    // Outside a project a mentioned but never tool-touched path is forbidden missing or not.
    let outside = fx.text("home/gone.md");
    let said = vec![format!("see `{outside}`")];
    let mut ctx = Context::new(&said, &cwd, &[], &[]);
    ctx.home = None;
    assert_eq!(code(&serve(&params(&outside), &ctx)), Some(Code::Forbidden));
    fx.write("home/gone.md", "x");
    assert_eq!(code(&serve(&params(&outside), &ctx)), Some(Code::Forbidden));
    fs::remove_file(fx.path("home/gone.md")).unwrap();
    // With the tool row it is the authorized target itself: the disk's answer.
    let tool = vec![write_row(&outside)];
    ctx.tool_texts = &tool;
    assert_eq!(code(&serve(&params(&outside), &ctx)), Some(Code::NotFound));
}

#[test]
fn sufficiency_over_pages_looks_only_at_what_each_page_added() {
    let fx = Fx::new();
    fx.write("a.md", "# a");
    let cwd = fx.text("");
    let none: Vec<String> = Vec::new();
    let ctx = Context::new(&none, &cwd, &[], &[]);
    let mut enough = Sufficiency::new(&ctx, &params("a.md"));
    let mut evidence = Evidence::default();
    assert!(!enough.check(&ctx, &evidence));
    // A page with other files only.
    evidence = Evidence::from_texts(&["`b.md`".to_owned()], &[write_row("b.md")]);
    assert!(!enough.check(&ctx, &evidence));
    // A page that mentions it: outside a project a tool row is still missing.
    let mut grown = evidence.clone();
    grown.absorb_rows(&[Row {
        role: "assistant".into(),
        text: "see `a.md`".into(),
        first: true,
        sent_at: None,
    }]);
    assert!(!enough.check(&ctx, &grown));
    // The tool row arrives on a later page, the mention was seen earlier and is remembered.
    grown.absorb_rows(&[Row {
        role: "tool".into(),
        text: write_row("a.md"),
        first: true,
        sent_at: None,
    }]);
    assert!(enough.check(&ctx, &grown));
    // The same answer as the one-shot check.
    let mut one_shot = Context::new(&grown.assistant, &cwd, &[], &[]);
    one_shot.evidence = Some(&grown);
    assert!(sufficient(&one_shot, &params("a.md")));
}

#[test]
fn a_realistic_report_with_framework_css_is_served() {
    let fx = Fx::new();
    // 3000 elements and about 200 KB of class-based CSS, each class used by a few elements.
    let css: String = (0..8_000)
        .map(|n| format!(".c{n}{{margin:0;padding:1px}}"))
        .collect();
    let body: String = (0..1_000)
        .map(|n| format!("<div class=c{}><span>x</span><b>y</b></div>", n % 8_000))
        .collect();
    let page = format!("<style>body{{margin:0}}{css}</style>{body}");
    assert!(page.len() > 200_000);
    fx.write("report.html", page);
    let result = run(&["`report.html`"], &fx.root, &params("report.html"));
    assert!(result.is_ok(), "{:?}", code(&result));
}

#[test]
fn decoy_style_openers_do_not_hide_the_elements_from_the_budget() {
    let fx = Fx::new();
    let rules: String = (0..6_000).map(|n| format!("i{{p{n:05}:1}}")).collect();
    let elements = "<i>x</i>".repeat(4_000);
    for (name, decoy) in [
        ("comment.html", "<!--<style>-->"),
        ("tag.html", "<styled>"),
        ("attr.html", "<p title=\"<style\">"),
    ] {
        fx.write(name, format!("{decoy}{elements}<style>{rules}</style>"));
        let result = run(&[&format!("`{name}`")], &fx.root, &params(name));
        assert_eq!(code(&result), Some(Code::TooLarge), "{name}");
    }
}

#[cfg(unix)]
#[test]
fn a_missing_file_behind_a_link_out_of_the_project_is_not_inside_the_project() {
    use std::os::unix::fs::symlink;
    let fx = Fx::new();
    fx.write("proj/.keep", "");
    fx.write("elsewhere/.keep", "");
    symlink(fx.path("elsewhere"), fx.path("proj/out")).unwrap();
    let project = Project::new("p1", &fx.path("proj"), true, DenyList::default());
    let inside = fx.text("proj/gone.md");
    let through_link = fx.text("proj/out/gone.md");
    for (path, expected) in [(inside, Code::NotFound), (through_link, Code::Forbidden)] {
        let said = [format!("`{path}`")];
        let said: Vec<&str> = said.iter().map(String::as_str).collect();
        let result = run_evidence(
            &said,
            &[],
            &fx.root,
            std::slice::from_ref(&project),
            &params(&path),
        );
        assert_eq!(code(&result), Some(expected), "{path}");
    }
}

#[test]
fn a_first_hit_that_names_another_file_does_not_end_the_history_scan() {
    let fx = Fx::new();
    let (one, two) = (fx.root.join("one"), fx.root.join("two"));
    fx.write("one/a.md", "# one");
    fx.write("two/a.md", "# two");
    let roots = vec![
        one.to_string_lossy().into_owned(),
        two.to_string_lossy().into_owned(),
    ];
    let cwd = fx.text("home");
    let project = Project::new("p1", &two, true, DenyList::default());
    let wanted = fx.text("two/a.md");
    let none: Vec<String> = Vec::new();
    let ctx = Context::new(&none, &cwd, &roots, std::slice::from_ref(&project));
    let mut enough = Sufficiency::new(&ctx, &params(&wanted));
    // The newest turn says `a.md`, which is the file under the first root: not this one.
    let mut evidence = Evidence::from_texts(&["`a.md`".to_owned()], &[]);
    assert!(!enough.check(&ctx, &evidence));
    // An older page names the file itself: now the call is decided.
    evidence.absorb_rows(&[Row {
        role: "assistant".into(),
        text: format!("`{wanted}`"),
        first: true,
        sent_at: None,
    }]);
    assert!(enough.check(&ctx, &evidence));
}
