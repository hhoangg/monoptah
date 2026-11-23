use super::*;
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};

/// A fake machine: files with text, executable files, folders, a home, a PATH and a shell.
#[derive(Default)]
struct Fake {
    home: Option<PathBuf>,
    shell: Option<PathBuf>,
    path: String,
    exec: BTreeSet<PathBuf>,
    dirs: BTreeSet<PathBuf>,
    text: BTreeMap<PathBuf, String>,
    /// What the login shell prints; `None` is a timeout or a failure.
    shell_output: Option<String>,
    shell_runs: RefCell<Vec<PathBuf>>,
}
impl Fake {
    fn new() -> Self {
        let mut f = Self {
            home: Some("/home/u".into()),
            shell: Some("/bin/zsh".into()),
            ..Self::default()
        };
        f.exec.insert("/bin/zsh".into());
        f
    }
    /// `npx` and `node` in `dir`.
    fn node_in(&mut self, dir: &str) -> &mut Self {
        for tool in ["npx", "node"] {
            self.exec.insert(Path::new(dir).join(tool));
        }
        // Every folder above it exists too, so a listing finds the version folders.
        for ancestor in Path::new(dir).ancestors() {
            self.dirs.insert(ancestor.to_path_buf());
        }
        self
    }
    fn file(&mut self, path: &str, text: &str) -> &mut Self {
        self.text.insert(path.into(), text.into());
        self
    }
    fn dir(&mut self, path: &str) -> &mut Self {
        self.dirs.insert(path.into());
        self
    }
    /// Installed nvm versions with `bin/{node,npx}`.
    fn nvm(&mut self, versions: &[&str]) -> &mut Self {
        for v in versions {
            let bin = format!("/home/u/.nvm/versions/node/{v}/bin");
            self.node_in(&bin);
            self.dirs
                .insert(format!("/home/u/.nvm/versions/node/{v}").into());
        }
        self.dirs.insert("/home/u/.nvm/versions/node".into());
        self
    }
}
impl Probe for Fake {
    fn home(&self) -> Option<PathBuf> {
        self.home.clone()
    }
    fn shell(&self) -> Option<PathBuf> {
        self.shell.clone()
    }
    fn path_var(&self) -> String {
        self.path.clone()
    }
    fn is_executable(&self, path: &Path) -> bool {
        self.exec.contains(path)
    }
    fn is_dir(&self, path: &Path) -> bool {
        self.dirs.contains(path)
    }
    fn read_to_string(&self, path: &Path) -> Option<String> {
        self.text.get(path).cloned()
    }
    fn list_dir(&self, path: &Path) -> Vec<String> {
        let mut names: BTreeSet<String> = BTreeSet::new();
        for p in self.dirs.iter().chain(self.exec.iter()) {
            if p.parent() == Some(path) {
                names.insert(p.file_name().unwrap().to_string_lossy().into_owned());
            }
        }
        names.into_iter().collect()
    }
    fn run_login_shell(&self, shell: &Path) -> Option<String> {
        self.shell_runs.borrow_mut().push(shell.to_path_buf());
        self.shell_output.clone()
    }
}

fn dir_of(tools: &Option<NodeTools>) -> String {
    tools
        .as_ref()
        .map(|t| t.bin_dir.to_string_lossy().into_owned())
        .unwrap_or_default()
}

#[test]
fn login_shell_answer_wins_over_everything_after_it() {
    let mut f = Fake::new();
    f.node_in("/home/u/.volta/bin").node_in("/opt/homebrew/bin");
    f.shell_output = Some("/home/u/.volta/bin/npx\n".into());
    let tools = resolve_with(&f, None).unwrap();
    assert_eq!(tools.source, NodeSource::LoginShell);
    assert_eq!(tools.npx, Path::new("/home/u/.volta/bin/npx"));
    assert_eq!(tools.node, Path::new("/home/u/.volta/bin/node"));
}

#[test]
fn shell_noise_around_the_answer_is_ignored() {
    let mut f = Fake::new();
    f.node_in("/x/bin");
    f.shell_output = Some(
        "Last login: Mon\nWelcome!\n/not/a/real/file\n/x/bin/npx\nnvm: ready\n/also/missing\n"
            .into(),
    );
    let tools = resolve_with(&f, None).unwrap();
    assert_eq!(tools.npx, Path::new("/x/bin/npx"));
    // A function or alias answer is not a path, so the shell step yields nothing.
    f.shell_output = Some("npx\nalias npx=foo\n".into());
    assert_eq!(resolve_with(&f, None), None);
}

#[test]
fn a_shell_timeout_falls_through_to_known_locations() {
    let mut f = Fake::new();
    f.shell_output = None;
    f.node_in("/opt/homebrew/bin");
    let tools = resolve_with(&f, None).unwrap();
    assert_eq!(tools.source, NodeSource::WellKnown);
    assert_eq!(dir_of(&Some(tools)), "/opt/homebrew/bin");
}

#[test]
fn the_shell_must_be_an_absolute_existing_executable() {
    let mut f = Fake::new();
    f.node_in("/x/bin");
    f.shell_output = Some("/x/bin/npx".into());
    f.shell = Some("zsh".into());
    assert!(resolve_with(&f, None).is_none_or(|t| t.source != NodeSource::LoginShell));
    f.shell = Some("/bin/missing".into());
    assert!(resolve_with(&f, None).is_none_or(|t| t.source != NodeSource::LoginShell));
    assert!(f.shell_runs.borrow().is_empty());
    f.shell = Some("/bin/zsh".into());
    assert_eq!(
        resolve_with(&f, None).unwrap().source,
        NodeSource::LoginShell
    );
    assert_eq!(
        f.shell_runs.borrow().as_slice(),
        [PathBuf::from("/bin/zsh")]
    );
}

#[test]
fn node_must_sit_beside_the_shells_npx() {
    let mut f = Fake::new();
    f.exec.insert("/x/bin/npx".into());
    f.shell_output = Some("/x/bin/npx".into());
    assert_eq!(resolve_with(&f, None), None);
}

#[test]
fn nvm_follows_the_default_alias_to_a_version() {
    let mut f = Fake::new();
    f.nvm(&["v18.20.0", "v20.11.1", "v22.3.0"])
        .file("/home/u/.nvm/alias/default", "20\n");
    assert_eq!(
        dir_of(&resolve_with(&f, None)),
        "/home/u/.nvm/versions/node/v20.11.1/bin"
    );
    f.file("/home/u/.nvm/alias/default", "v18.20.0");
    assert_eq!(
        dir_of(&resolve_with(&f, None)),
        "/home/u/.nvm/versions/node/v18.20.0/bin"
    );
}

#[test]
fn nvm_resolves_lts_aliases_through_their_files() {
    let mut f = Fake::new();
    f.nvm(&["v18.20.0", "v20.11.1", "v22.3.0"])
        .file("/home/u/.nvm/alias/default", "lts/*")
        .file("/home/u/.nvm/alias/lts/*", "lts/iron")
        .file("/home/u/.nvm/alias/lts/iron", "v20.11.1");
    assert_eq!(
        dir_of(&resolve_with(&f, None)),
        "/home/u/.nvm/versions/node/v20.11.1/bin"
    );
}

#[test]
fn nvm_node_alias_and_a_missing_alias_pick_the_newest_by_semver() {
    let mut f = Fake::new();
    // 9 is less than 10 as a number, though not as text.
    f.nvm(&["v9.11.2", "v10.2.0", "v8.0.0"]);
    f.file("/home/u/.nvm/alias/default", "node");
    assert_eq!(
        dir_of(&resolve_with(&f, None)),
        "/home/u/.nvm/versions/node/v10.2.0/bin"
    );
    f.text.clear();
    assert_eq!(
        dir_of(&resolve_with(&f, None)),
        "/home/u/.nvm/versions/node/v10.2.0/bin"
    );
    // An alias for a version that is not installed falls back to the newest one.
    f.file("/home/u/.nvm/alias/default", "v99.0.0");
    assert_eq!(
        dir_of(&resolve_with(&f, None)),
        "/home/u/.nvm/versions/node/v10.2.0/bin"
    );
}

#[test]
fn nvm_alias_loops_and_path_tricks_do_not_hang_or_escape() {
    let mut f = Fake::new();
    f.nvm(&["v20.0.0"])
        .file("/home/u/.nvm/alias/default", "a")
        .file("/home/u/.nvm/alias/a", "b")
        .file("/home/u/.nvm/alias/b", "a");
    assert_eq!(
        dir_of(&resolve_with(&f, None)),
        "/home/u/.nvm/versions/node/v20.0.0/bin"
    );
    f.file("/home/u/.nvm/alias/default", "../../../etc/passwd");
    assert_eq!(
        dir_of(&resolve_with(&f, None)),
        "/home/u/.nvm/versions/node/v20.0.0/bin"
    );
}

#[test]
fn fnm_default_alias_then_newest_version() {
    let mut f = Fake::new();
    f.node_in("/home/u/.fnm/node-versions/v20.0.0/installation/bin")
        .node_in("/home/u/.fnm/node-versions/v22.0.0/installation/bin");
    assert_eq!(
        dir_of(&resolve_with(&f, None)),
        "/home/u/.fnm/node-versions/v22.0.0/installation/bin"
    );
    f.node_in("/home/u/.fnm/aliases/default/bin");
    assert_eq!(
        dir_of(&resolve_with(&f, None)),
        "/home/u/.fnm/aliases/default/bin"
    );
}

#[test]
fn shim_and_prefix_managers_are_found() {
    for dir in [
        "/home/u/.volta/bin",
        "/home/u/.asdf/shims",
        "/home/u/.local/share/mise/shims",
        "/home/u/.nodenv/shims",
        "/home/u/n/bin",
        "/usr/local/bin",
    ] {
        let mut f = Fake::new();
        f.node_in(dir);
        let tools = resolve_with(&f, None).unwrap();
        assert_eq!(tools.bin_dir, Path::new(dir), "{dir}");
        assert_eq!(tools.source, NodeSource::WellKnown);
    }
}

#[test]
fn known_locations_come_before_path_and_path_is_the_last_resort() {
    let mut f = Fake::new();
    f.node_in("/opt/homebrew/bin").node_in("/p/bin");
    f.path = "/p/bin".into();
    assert_eq!(dir_of(&resolve_with(&f, None)), "/opt/homebrew/bin");
    let mut f = Fake::new();
    f.node_in("/p/bin");
    f.path = "/q:/p/bin".into();
    let tools = resolve_with(&f, None).unwrap();
    assert_eq!(
        (tools.source, dir_of(&Some(tools.clone()))),
        (NodeSource::Path, "/p/bin".into())
    );
    assert_eq!(resolve_with(&Fake::new(), None), None);
}

#[test]
fn a_valid_override_beats_the_login_shell() {
    let mut f = Fake::new();
    f.node_in("/custom/bin").node_in("/x/bin");
    f.shell_output = Some("/x/bin/npx".into());
    let tools = resolve_with(&f, Some("/custom/bin")).unwrap();
    assert_eq!(tools.source, NodeSource::Override);
    assert_eq!(tools.bin_dir, Path::new("/custom/bin"));
    assert!(f.shell_runs.borrow().is_empty());
    // A file is taken as npx.
    let tools = resolve_with(&f, Some("/custom/bin/npx")).unwrap();
    assert_eq!(tools.npx, Path::new("/custom/bin/npx"));
}

#[test]
fn an_invalid_override_is_reported_and_skipped_by_the_resolver() {
    let mut f = Fake::new();
    f.node_in("/x/bin").dir("/empty");
    f.exec.insert("/lonely/npx".into());
    f.shell_output = Some("/x/bin/npx".into());
    for (raw, why) in [
        ("", "Enter"),
        ("   ", "Enter"),
        ("bin/npx", "absolute"),
        ("~/bin", "absolute"),
        ("/missing", "does not exist"),
        ("/empty", "both npx and node"),
        ("/lonely/npx", "same folder"),
        ("/x/bin/node", "must be npx"),
        ("/x/bin/npx\n/etc/passwd", "Enter"),
    ] {
        let error = validate_override(&f, raw).unwrap_err();
        assert!(error.contains(why), "{raw:?}: {error}");
        assert_eq!(
            resolve_with(&f, Some(raw)).unwrap().source,
            NodeSource::LoginShell,
            "{raw:?}"
        );
    }
}

#[test]
fn the_override_text_is_trimmed_and_never_a_shell_command() {
    let mut f = Fake::new();
    f.node_in("/custom/bin");
    assert!(validate_override(&f, "  /custom/bin \n").is_ok());
    assert!(validate_override(&f, "/custom/bin; rm -rf /").is_err());
    assert!(validate_override(&f, "$(echo /custom/bin)").is_err());
}

#[test]
fn path_with_puts_the_node_folder_first() {
    let tools = NodeTools {
        npx: "/n/bin/npx".into(),
        node: "/n/bin/node".into(),
        bin_dir: "/n/bin".into(),
        source: NodeSource::Path,
    };
    let path = tools.path_with("/usr/bin:/bin");
    let parts: Vec<_> = std::env::split_paths(&path).collect();
    assert_eq!(parts[0], Path::new("/n/bin"));
    assert_eq!(parts.len(), 3);
}

#[test]
fn absolute_nvm_aliases_are_not_followed() {
    let mut f = Fake::new();
    f.nvm(&["v20.0.0"])
        .file("/home/u/.nvm/alias/default", "/etc")
        .file("/etc", "v1.0.0");
    assert_eq!(
        dir_of(&resolve_with(&f, None)),
        "/home/u/.nvm/versions/node/v20.0.0/bin"
    );
}

#[test]
fn a_recheck_during_a_lookup_keeps_the_stale_answer_out_of_the_cache() {
    /// The user presses Recheck while the shell is still running.
    struct Racing<'a>(Fake, &'a State);
    impl Probe for Racing<'_> {
        fn home(&self) -> Option<PathBuf> {
            self.0.home()
        }
        fn shell(&self) -> Option<PathBuf> {
            self.0.shell()
        }
        fn path_var(&self) -> String {
            self.0.path_var()
        }
        fn is_executable(&self, p: &Path) -> bool {
            self.0.is_executable(p)
        }
        fn is_dir(&self, p: &Path) -> bool {
            self.0.is_dir(p)
        }
        fn read_to_string(&self, p: &Path) -> Option<String> {
            self.0.read_to_string(p)
        }
        fn list_dir(&self, p: &Path) -> Vec<String> {
            self.0.list_dir(p)
        }
        fn run_login_shell(&self, shell: &Path) -> Option<String> {
            self.1.recheck();
            self.0.run_login_shell(shell)
        }
    }
    let state = State::new();
    let mut f = Fake::new();
    f.node_in("/x/bin");
    f.shell_output = Some("/x/bin/npx".into());
    let racing = Racing(f, &state);
    // The caller still gets its answer, but it is not remembered.
    assert!(state.resolve(&racing).is_some());
    assert_eq!(state.cached(), None);
    // Without a recheck in between it is remembered.
    let mut calm = Fake::new();
    calm.node_in("/x/bin");
    calm.shell_output = Some("/x/bin/npx".into());
    assert!(state.resolve(&calm).is_some());
    assert!(state.cached().flatten().is_some());
}
