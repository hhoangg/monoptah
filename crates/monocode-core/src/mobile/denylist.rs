//! Sensitive-file deny-list applied to every remote file route (H-04).
//!
//! One matcher, shared by `project.file`, `files`, `search`, `fileHighlight`
//! and `changes`. Paths are compared case-insensitively, with `\` read as `/`.
//! A pattern without `/` matches any single path component. A pattern with `/`
//! matches that run of components at any depth. `*` matches within one
//! component, `**` matches any number of components (also none). A match on a
//! component also denies everything below it, so `.git` denies `.git/HEAD`.
//! The defaults can only be extended, never removed.

use std::{
    fmt,
    path::{Component, Path, PathBuf},
};

use super::policy::Code;

/// Patterns that are always denied.
pub const DEFAULT_PATTERNS: &[&str] = &[
    ".env",
    ".env.*",
    "*.pem",
    "*.key",
    "*.p12",
    "*.pfx",
    "*.keystore",
    "*.jks",
    "id_rsa*",
    "id_ed25519*",
    "id_ecdsa*",
    ".git/**",
    ".ssh/**",
    ".aws/**",
    ".gnupg/**",
    ".kube/config",
    ".docker/config.json",
    ".npmrc",
    ".pypirc",
    ".netrc",
    ".htpasswd",
    // Also `.credentials.json` (a leading dot is not matched by `credentials*`).
    "*credentials*.json",
    "secrets.*",
    "*.tfstate",
    "*.kdbx",
    ".git-credentials",
    ".gitconfig",
    ".cargo/credentials*",
    ".vault-token",
    ".pgpass",
    ".my.cnf",
    // Shell histories (`.zsh_history`, `.bash_history`) hold pasted secrets.
    ".*_history",
];

/// Final file names exempt from the `.env.*` pattern (and only from it).
pub const ALLOW: &[&str] = &[".env.example", ".env.sample", ".env.template"];

/// The pattern the allow-list relaxes.
const ENV_PATTERN: &str = ".env.*";

/// A path was refused. Displays as the fixed `path_denied` text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Denied;

impl fmt::Display for Denied {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(Code::PathDenied.message())
    }
}

impl std::error::Error for Denied {}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Segment {
    /// `**`: any number of components, including none.
    Any,
    /// A glob matched against one component, lowercased.
    Glob(Vec<char>),
}

#[derive(Debug, Clone)]
struct Pattern {
    text: String,
    segments: Vec<Segment>,
}

/// The defaults plus a project's extra patterns.
#[derive(Debug, Clone)]
pub struct DenyList {
    patterns: Vec<Pattern>,
}

impl Default for DenyList {
    fn default() -> Self {
        Self::new(&[])
    }
}

/// The path components as compared: lowercased, `.` and empty parts dropped,
/// trailing dots and spaces trimmed (Windows ignores them), `\` as `/`.
fn components(path: &str) -> Vec<String> {
    path.replace('\\', "/")
        .split('/')
        .filter(|part| !part.is_empty() && *part != ".")
        .map(fold)
        .collect()
}

fn fold(part: &str) -> String {
    let lower = part.to_lowercase();
    if lower.chars().all(|c| c == '.') {
        return lower;
    }
    lower.trim_end_matches(['.', ' ']).to_string()
}

/// The same components cut at `:`, so an NTFS stream such as `.env::$DATA`
/// is judged by the file it belongs to.
fn stream_free(parts: &[String]) -> Vec<String> {
    parts
        .iter()
        .map(|part| match part.split_once(':') {
            Some((head, _)) if !head.is_empty() => fold(head),
            _ => part.clone(),
        })
        .collect()
}

fn glob(pattern: &[char], text: &[char]) -> bool {
    match pattern.split_first() {
        None => text.is_empty(),
        Some(('*', rest)) => (0..=text.len()).any(|skip| glob(rest, &text[skip..])),
        Some(('?', rest)) => !text.is_empty() && glob(rest, &text[1..]),
        Some((c, rest)) => text.first() == Some(c) && glob(rest, &text[1..]),
    }
}

/// Whether `segments` match the start of `parts`; leftover components are fine.
fn matches_prefix(segments: &[Segment], parts: &[String]) -> bool {
    match segments.split_first() {
        None => true,
        Some((Segment::Any, rest)) => {
            (0..=parts.len()).any(|skip| matches_prefix(rest, &parts[skip..]))
        }
        Some((Segment::Glob(g), rest)) => match parts.split_first() {
            Some((part, tail)) => {
                let chars: Vec<char> = part.chars().collect();
                glob(g, &chars) && matches_prefix(rest, tail)
            }
            None => false,
        },
    }
}

impl Pattern {
    fn parse(raw: &str) -> Option<Self> {
        let parts = components(raw);
        if parts.is_empty() {
            return None;
        }
        let segments = parts
            .iter()
            .map(|part| {
                if part == "**" {
                    Segment::Any
                } else {
                    Segment::Glob(part.chars().collect())
                }
            })
            .collect();
        Some(Self {
            text: parts.join("/"),
            segments,
        })
    }

    /// Matches `parts` starting at any depth. `allowed_last` marks that the
    /// final component is on the allow-list, which exempts the env pattern.
    fn matches(&self, parts: &[String], allowed_last: bool) -> bool {
        if allowed_last && self.text == ENV_PATTERN && matches_env_last(&self.segments, parts) {
            return false;
        }
        (0..parts.len()).any(|start| matches_prefix(&self.segments, &parts[start..]))
    }
}

/// The env pattern is a single component, so the allow-list can only apply
/// when its single match is the final component and no other component hits.
fn matches_env_last(segments: &[Segment], parts: &[String]) -> bool {
    let Some((last, before)) = parts.split_last() else {
        return false;
    };
    let hit_before = (0..before.len()).any(|start| matches_prefix(segments, &before[start..]));
    !hit_before && matches_prefix(segments, std::slice::from_ref(last))
}

impl DenyList {
    /// The defaults plus `extra` (project patterns). Empty extras are skipped.
    pub fn new(extra: &[String]) -> Self {
        let patterns = DEFAULT_PATTERNS
            .iter()
            .copied()
            .chain(extra.iter().map(String::as_str))
            .filter_map(Pattern::parse)
            .collect();
        Self { patterns }
    }

    /// The defaults plus the extra patterns saved for `project` in the policy
    /// settings (matched by `files::path_key`).
    pub fn for_project(project: &str) -> Self {
        let key = crate::files::path_key(project);
        let settings = super::policy::settings();
        let extra: Vec<String> = settings
            .deny_patterns
            .iter()
            .filter(|(path, _)| crate::files::path_key(path) == key)
            .flat_map(|(_, patterns)| patterns.iter().cloned())
            .collect();
        Self::new(&extra)
    }

    /// Whether the path, as written (relative to a project root, or any path),
    /// is denied. Lexical only: see [`DenyList::check`] for the filesystem.
    pub fn is_denied_rel(&self, rel: &str) -> bool {
        let parts = components(rel);
        if parts.is_empty() {
            return false;
        }
        let streamless = stream_free(&parts);
        [&parts, &streamless].iter().any(|parts| self.denies(parts))
    }

    fn denies(&self, parts: &[String]) -> bool {
        let allowed_last = parts
            .last()
            .is_some_and(|last| ALLOW.contains(&last.as_str()));
        self.patterns
            .iter()
            .any(|pattern| pattern.matches(parts, allowed_last))
    }

    /// Whether `requested` (relative to `root`, or absolute) may be reached.
    /// Denies when the written path, the canonical target relative to the
    /// canonical root, or (for a target outside the root) the target's own
    /// components match; also when an existing path cannot be canonicalised.
    pub fn check(&self, root: &Path, requested: &Path) -> Result<(), Denied> {
        let root_canonical = root.canonicalize().map_err(|_| Denied)?;
        let joined = root_canonical.join(requested);
        // As written: relative to the root when it lies under it.
        let written = joined
            .strip_prefix(&root_canonical)
            .map(Path::to_path_buf)
            .or_else(|_| {
                requested
                    .strip_prefix(root)
                    .map(Path::to_path_buf)
                    .map_err(|_| ())
            })
            .unwrap_or_else(|_| requested.to_path_buf());
        if self.is_denied_rel(&lossy(&written)) {
            return Err(Denied);
        }
        // Each existing ancestor, then the target itself.
        let lexical = normalize(&joined);
        let mut chain: Vec<&Path> = lexical.ancestors().collect();
        chain.reverse();
        for candidate in chain {
            // Only entries below the root: the root and its parents are the
            // project's own location, not something a device named.
            if candidate == root_canonical || !candidate.starts_with(&root_canonical) {
                continue;
            }
            match candidate.canonicalize() {
                Ok(target) => self.check_target(&root_canonical, &target)?,
                Err(_) => {
                    // A dangling link or an unreadable entry exists but cannot
                    // be resolved: refuse. A missing one is simply absent.
                    if std::fs::symlink_metadata(candidate).is_ok() {
                        return Err(Denied);
                    }
                    break;
                }
            }
        }
        Ok(())
    }

    fn check_target(&self, root: &Path, target: &Path) -> Result<(), Denied> {
        let denied = match target.strip_prefix(root) {
            Ok(rel) => self.is_denied_rel(&lossy(rel)),
            Err(_) => self.is_denied_rel(&lossy(target)),
        };
        if denied {
            Err(Denied)
        } else {
            Ok(())
        }
    }
}

fn lossy(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// The path without `.` parts. `..` stays: `canonicalize` of each ancestor
/// resolves it against the real filesystem, which a lexical fold could not.
fn normalize(path: &Path) -> PathBuf {
    path.components()
        .filter(|component| !matches!(component, Component::CurDir))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list() -> DenyList {
        DenyList::default()
    }

    #[test]
    fn every_default_pattern_denies_a_sample() {
        let samples = [
            ".env",
            ".env.local",
            "a.pem",
            "a.key",
            "a.p12",
            "a.pfx",
            "a.keystore",
            "a.jks",
            "id_rsa",
            "id_rsa.pub",
            "id_ed25519",
            "id_ecdsa",
            ".git",
            ".git/HEAD",
            ".ssh/config",
            ".aws/credentials",
            ".gnupg/pubring.kbx",
            ".kube/config",
            ".docker/config.json",
            ".npmrc",
            ".pypirc",
            ".netrc",
            ".htpasswd",
            "credentials.json",
            "credentials-prod.json",
            "secrets.yml",
            "x.tfstate",
            "vault.kdbx",
            ".credentials.json",
            "a/.claude/.credentials.json",
            "x_credentials_y.json",
            ".git-credentials",
            ".gitconfig",
            ".cargo/credentials",
            ".cargo/credentials.toml",
            ".vault-token",
            ".pgpass",
            ".my.cnf",
            ".zsh_history",
            ".bash_history",
        ];
        for sample in samples {
            assert!(list().is_denied_rel(sample), "{sample} should be denied");
        }
        assert_eq!(DEFAULT_PATTERNS.len(), 32);
    }

    #[test]
    fn nested_paths_and_backslashes_are_denied() {
        let list = list();
        for path in [
            "a/b/.env",
            "a/.git/HEAD",
            "a/.git",
            "pkg/keys/server.key",
            "x/.kube/config",
            "x/.docker/config.json",
            "a\\.ssh\\id_rsa",
            "./a/./.env",
            "a//.env",
            "a/.git/",
        ] {
            assert!(list.is_denied_rel(path), "{path} should be denied");
        }
        assert!(!list.is_denied_rel(".kube/other"));
        assert!(!list.is_denied_rel(".docker/other.json"));
        assert!(!list.is_denied_rel(""));
    }

    #[test]
    fn case_variants_are_denied() {
        let list = list();
        for path in [
            ".ENV",
            "ID_RSA",
            ".Git/config",
            "SERVER.KEY",
            "a/.SSH/x",
            ".Env.Local",
        ] {
            assert!(list.is_denied_rel(path), "{path} should be denied");
        }
    }

    #[test]
    fn windows_tricks_are_denied() {
        let list = list();
        for path in [
            ".env.",
            ".env ",
            ".env::$DATA",
            "a.pem.",
            ".git./HEAD",
            "id_rsa:stream",
        ] {
            assert!(list.is_denied_rel(path), "{path} should be denied");
        }
    }

    #[test]
    fn allow_list_covers_only_the_env_templates() {
        let list = list();
        for path in [
            ".env.example",
            ".env.sample",
            ".env.template",
            "app/.ENV.Example",
        ] {
            assert!(!list.is_denied_rel(path), "{path} should be allowed");
        }
        // Not the final component, or another pattern, or a different name.
        assert!(list.is_denied_rel(".env.example/x"));
        assert!(list.is_denied_rel(".env.example2"));
        assert!(list.is_denied_rel(".env.local"));
        assert!(list.is_denied_rel(".git/.env.example"));
        assert!(list.is_denied_rel(".ssh/.env.example"));
        let extra = DenyList::new(&[".env.example".to_string()]);
        assert!(extra.is_denied_rel(".env.example"));
    }

    #[test]
    fn similar_names_are_not_denied() {
        let list = list();
        for path in [
            ".envrc",
            "src/key.rs",
            "src/keys.rs",
            "environment.rs",
            "README.md",
            "docs/secrets/readme.md",
            ".github/workflows/ci.yml",
            ".gitignore",
            ".gitattributes",
            "my.git/x",
            "credentials.txt",
            "secret.txt",
        ] {
            assert!(!list.is_denied_rel(path), "{path} should be allowed");
        }
        assert!(list.is_denied_rel("server.key"));
    }

    #[test]
    fn extras_add_and_never_remove() {
        let list = DenyList::new(&[
            "private/**".to_string(),
            "*.secret".to_string(),
            "  ".to_string(),
            "/rooted/file.txt".to_string(),
            "a/**/z".to_string(),
        ]);
        assert!(list.is_denied_rel("private"));
        assert!(list.is_denied_rel("x/private/y"));
        assert!(list.is_denied_rel("a.SECRET"));
        assert!(list.is_denied_rel("rooted/file.txt"));
        assert!(list.is_denied_rel("a/b/c/z"));
        assert!(list.is_denied_rel("a/z"));
        assert!(!list.is_denied_rel("a/b/c/y"));
        assert!(list.is_denied_rel(".env") && list.is_denied_rel(".git/HEAD"));
        assert!(!list.is_denied_rel("public/file.txt"));
    }

    #[test]
    fn for_project_reads_the_policy_settings() {
        let _serial = super::super::policy::TEST_SETTINGS_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let mut settings = super::super::policy::settings();
        settings
            .deny_patterns
            .insert("/denylist-test/proj/".into(), vec!["*.private".into()]);
        super::super::policy::install_settings(settings);
        let list = DenyList::for_project("/denylist-test/proj");
        assert!(list.is_denied_rel("x.private"));
        assert!(!DenyList::for_project("/denylist-test/other").is_denied_rel("x.private"));
    }

    #[test]
    fn check_denies_the_written_path_even_when_missing() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let list = list();
        assert_eq!(list.check(root, Path::new(".env")), Err(Denied));
        assert_eq!(list.check(root, Path::new("a/.Git/config")), Err(Denied));
        assert_eq!(list.check(root, Path::new("a/../.env")), Err(Denied));
        assert_eq!(list.check(root, Path::new("missing.txt")), Ok(()));
        std::fs::write(root.join("ok.txt"), "x").unwrap();
        assert_eq!(list.check(root, Path::new("ok.txt")), Ok(()));
        assert_eq!(
            list.check(&root.join("nope"), Path::new("ok.txt")),
            Err(Denied)
        );
        assert_eq!(Denied.to_string(), "Hidden: sensitive file");
    }

    #[test]
    fn check_accepts_absolute_paths_under_the_root() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        std::fs::write(root.join(".env"), "x").unwrap();
        std::fs::write(root.join("a.txt"), "x").unwrap();
        let list = list();
        assert_eq!(list.check(&root, &root.join(".env")), Err(Denied));
        assert_eq!(list.check(&root, &root.join("a.txt")), Ok(()));
    }

    #[test]
    fn a_root_inside_a_denied_looking_folder_is_not_denied() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join(".git-mirror").join(".aws");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("a.txt"), "x").unwrap();
        assert_eq!(list().check(&root, Path::new("a.txt")), Ok(()));
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_to_denied_targets_are_denied() {
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("proj");
        let outside = dir.path().join("outside");
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::create_dir_all(outside.join(".ssh")).unwrap();
        std::fs::write(root.join(".env"), "x").unwrap();
        std::fs::write(root.join(".git/config"), "x").unwrap();
        std::fs::write(root.join("src/a.txt"), "x").unwrap();
        std::fs::write(outside.join(".ssh/id_rsa"), "x").unwrap();
        std::fs::write(outside.join("plain.txt"), "x").unwrap();
        // Inside the root: links to denied files and folders.
        symlink(root.join(".env"), root.join("notes.txt")).unwrap();
        symlink(root.join(".git"), root.join("history")).unwrap();
        symlink("../.env", root.join("src/rel.txt")).unwrap();
        // Outside the root: a link to a key, to a folder of keys, to a plain file.
        symlink(outside.join(".ssh/id_rsa"), root.join("outside.txt")).unwrap();
        symlink(outside.join(".ssh"), root.join("docs")).unwrap();
        symlink(outside.join("plain.txt"), root.join("plain-link.txt")).unwrap();
        // A link to a link, and a dangling link.
        symlink(root.join("notes.txt"), root.join("chain.txt")).unwrap();
        symlink(root.join("gone"), root.join("dangling")).unwrap();
        let list = list();
        for denied in [
            "notes.txt",
            "history",
            "history/config",
            "src/rel.txt",
            "outside.txt",
            "docs",
            "docs/id_rsa",
            "chain.txt",
            "dangling",
        ] {
            assert_eq!(
                list.check(&root, Path::new(denied)),
                Err(Denied),
                "{denied}"
            );
        }
        assert_eq!(list.check(&root, Path::new("src/a.txt")), Ok(()));
        assert_eq!(list.check(&root, Path::new("plain-link.txt")), Ok(()));
    }
}
