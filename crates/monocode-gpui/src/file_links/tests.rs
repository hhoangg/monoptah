use super::*;

/// A fake home folder and fake projects under one temp dir; removed when dropped.
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root =
            std::env::temp_dir().join(format!("file-links-{}", monocode_core::mono::new_id()));
        std::fs::create_dir_all(root.join("home")).unwrap();
        Self(root)
    }
    fn dir(&self, name: &str) -> String {
        let dir = self.0.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        dir.to_string_lossy().into_owned()
    }
    fn file(&self, project: &str, relative: &str) -> String {
        let file = Path::new(project).join(relative);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, "fake").unwrap();
        file.to_string_lossy().into_owned()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// What the app used before: the link joined onto the chat's base, whatever is on disk.
fn old(raw: &str, base: &str) -> (String, Option<(usize, usize)>) {
    crate::file_editor::file_location(raw, base)
}

#[test]
fn a_mono_chat_in_the_home_folder_finds_a_relative_link_in_its_project() {
    let fx = Fixture::new();
    let (home, project) = (fx.dir("home"), fx.dir("project"));
    let file = fx.file(&project, "crates/x/y.rs");
    let roots = candidate_roots(None, std::slice::from_ref(&project));
    let (path, location) = resolve_link("crates/x/y.rs", &home, &roots);
    assert_eq!(path, file);
    assert_eq!(location, None);
    assert_ne!(
        path,
        old("crates/x/y.rs", &home).0,
        "the home join was the bug"
    );
}

#[test]
fn a_link_already_bound_to_the_first_project_moves_to_the_project_that_has_the_file() {
    let fx = Fixture::new();
    let (first, second) = (fx.dir("first"), fx.dir("second"));
    let file = fx.file(&second, "src/lib.rs");
    let roots = candidate_roots(None, &[first.clone(), second.clone()]);
    // The chat binds `src/lib.rs` to its base while drawing, with no disk access.
    let bound = old("src/lib.rs", &first).0;
    assert_eq!(resolve_link(&bound, &first, &roots).0, file);
    assert_eq!(resolve_link("src/lib.rs", &first, &roots).0, file);
}

#[test]
fn line_and_column_suffixes_survive_the_move() {
    let fx = Fixture::new();
    let (home, project) = (fx.dir("home"), fx.dir("project"));
    let file = fx.file(&project, "crates/x/y.rs");
    let roots = candidate_roots(None, &[project]);
    assert_eq!(
        resolve_link("crates/x/y.rs:12:3", &home, &roots),
        (file.clone(), Some((12, 3)))
    );
    assert_eq!(
        resolve_link("crates/x/y.rs#L12", &home, &roots),
        (file.clone(), Some((12, 1)))
    );
    assert_eq!(
        resolve_link("crates/x/y.rs:40", &home, &roots),
        (file, Some((40, 1)))
    );
}

#[test]
fn a_file_in_two_projects_comes_from_the_first_root_and_the_windows_project_leads() {
    let fx = Fixture::new();
    let (home, first, second) = (fx.dir("home"), fx.dir("first"), fx.dir("second"));
    let in_first = fx.file(&first, "docs/a.md");
    let in_second = fx.file(&second, "docs/a.md");
    let projects = [first.clone(), second.clone()];
    let roots = candidate_roots(None, &projects);
    assert_eq!(resolve_link("docs/a.md", &home, &roots).0, in_first);
    // The window shows the second project, which the Mono works on: it is tried first.
    let roots = candidate_roots(Some(&second), &projects);
    assert_eq!(roots, [second.clone(), first.clone()]);
    assert_eq!(resolve_link("docs/a.md", &home, &roots).0, in_second);
    // A window on a project the Mono does not work on never becomes a root.
    let other = fx.dir("other");
    fx.file(&other, "docs/a.md");
    assert_eq!(candidate_roots(Some(&other), &projects), [first, second]);
}

#[test]
fn a_file_found_nowhere_keeps_the_old_result_and_error() {
    let fx = Fixture::new();
    let (home, project) = (fx.dir("home"), fx.dir("project"));
    let roots = candidate_roots(None, &[project]);
    for raw in [
        "missing/none.rs",
        "missing/none.rs:7:2",
        "../escape.rs",
        ".",
    ] {
        assert_eq!(resolve_link(raw, &home, &roots), old(raw, &home), "{raw}");
    }
}

#[test]
fn a_remote_base_and_remote_roots_are_never_looked_up() {
    let fx = Fixture::new();
    let project = fx.dir("project");
    fx.file(&project, "crates/x/y.rs");
    let remote = "remote://env-1/home/me/repo".to_owned();
    assert_eq!(
        resolve_link(
            "crates/x/y.rs",
            &remote,
            &candidate_roots(None, std::slice::from_ref(&project))
        ),
        old("crates/x/y.rs", &remote),
        "a remote base keeps today's behaviour"
    );
    assert!(candidate_roots(Some(&remote), std::slice::from_ref(&remote)).is_empty());
    assert_eq!(candidate_roots(None, &[remote, project.clone()]), [project]);
}

#[test]
fn a_chat_that_is_not_a_monos_has_no_roots_and_is_unchanged() {
    let fx = Fixture::new();
    let (cwd, project) = (fx.dir("session"), fx.dir("project"));
    fx.file(&project, "crates/x/y.rs");
    assert_eq!(
        resolve_link("crates/x/y.rs", &cwd, &[]),
        old("crates/x/y.rs", &cwd)
    );
    let there = fx.file(&cwd, "a.rs");
    assert_eq!(resolve_link("a.rs", &cwd, &[]), (there, None));
}

#[test]
fn an_existing_path_and_an_absolute_link_are_unchanged() {
    let fx = Fixture::new();
    let (home, first, second) = (fx.dir("home"), fx.dir("first"), fx.dir("second"));
    let roots = candidate_roots(None, &[first.clone(), second.clone()]);
    let absolute = fx.file(&home, "notes/today.md");
    fx.file(&first, "notes/today.md");
    assert_eq!(
        resolve_link(&absolute, &first, &roots),
        (absolute.clone(), None)
    );
    // A bound path that exists under its own base is not moved, even if another root has it.
    let bound = fx.file(&second, "same.rs");
    fx.file(&first, "same.rs");
    assert_eq!(resolve_link(&bound, &home, &roots).0, bound);
    // An absolute link outside every root, missing or not, stays as written.
    let outside = Path::new(&home).join("elsewhere/gone.rs");
    let outside = outside.to_string_lossy().into_owned();
    assert_eq!(
        resolve_link(&outside, &first, &roots),
        (outside.clone(), None)
    );
    assert_eq!(root_of(&outside, &roots), None);
    assert_eq!(root_of(&bound, &roots), Some(second.as_str()));
}
