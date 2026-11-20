//! Which folders a Mono may add to the app as a project (`projects.add`).
//!
//! A person adds a project by picking a folder in a dialog. A Mono names one in text, so the
//! path is checked before it is trusted: absolute (or `~`), an existing directory, resolved
//! through symlinks, and never the home folder or what holds it, a system folder, or a place
//! the app already guards against remote reads (credential folders, secret stores, the app's
//! own data).
use crate::{credential_guard, mobile::denylist::DenyList};
use std::path::{Path, PathBuf};

/// Longest `name` accepted.
pub const NAME_MAX: usize = 200;

/// System trees: the folder and everything under it is refused.
#[cfg(unix)]
const SYSTEM_TREES: &[&str] = &[
    "/System",
    "/usr",
    "/bin",
    "/sbin",
    "/etc",
    "/private/etc",
    "/Library",
    "/Applications",
    "/dev",
    "/proc",
    "/sys",
    "/boot",
    "/lib",
    "/lib64",
    "/run",
];
#[cfg(not(unix))]
const SYSTEM_TREES: &[&str] = &[];

/// Folders that only hold other things (a user's, a volume's): refused as a project themselves,
/// while the folders under them are fine.
#[cfg(unix)]
const SYSTEM_CONTAINERS: &[&str] = &[
    "/var",
    "/private",
    "/private/var",
    "/Users",
    "/home",
    "/Volumes",
    "/opt",
    "/mnt",
    "/media",
    "/tmp",
    "/private/tmp",
    "/srv",
    "/root",
];
#[cfg(not(unix))]
const SYSTEM_CONTAINERS: &[&str] = &[];

/// Folders of the home folder that are refused whole (and whatever is under them).
const HOME_TREES: &[&str] = &["Library", "AppData"];

/// The environment's home folder.
pub fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .filter(|h| !h.is_empty())
        .map(PathBuf::from)
}

/// `~` and `~/rest` expanded against `home`; anything else is returned as written.
pub fn expand_home(raw: &str, home: Option<&Path>) -> Result<PathBuf, String> {
    let raw = raw.trim();
    let rest = if raw == "~" {
        Some("")
    } else {
        raw.strip_prefix("~/").or_else(|| raw.strip_prefix("~\\"))
    };
    match rest {
        Some(rest) => {
            let home = home.ok_or("The home folder is unknown, so ~ cannot be expanded")?;
            Ok(if rest.is_empty() {
                home.to_path_buf()
            } else {
                home.join(rest)
            })
        }
        None => Ok(PathBuf::from(raw)),
    }
}

/// Validates the `name` of `projects.add`: text of 1 to [`NAME_MAX`] characters once trimmed.
pub fn check_name(name: &str) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > NAME_MAX || name.chars().any(char::is_control) {
        return Err(format!(
            "name must be 1 to {NAME_MAX} characters of plain text"
        ));
    }
    Ok(name.to_owned())
}

/// The canonical folder for `raw`, or why it may not become a project.
pub fn resolve(raw: &str, home: Option<&Path>) -> Result<PathBuf, String> {
    if raw.trim().is_empty() {
        return Err("Provide path".into());
    }
    let path = expand_home(raw, home)?;
    if !path.is_absolute() {
        return Err("path must be absolute (or start with ~/)".into());
    }
    let canonical = path
        .canonicalize()
        .map_err(|e| format!("Cannot open {}: {e}", path.display()))?;
    let canonical = plain(canonical);
    if !canonical.is_dir() {
        return Err(format!("{} is not a directory", canonical.display()));
    }
    if let Some(why) = refusal(&canonical, home) {
        return Err(format!(
            "{} cannot be added as a project: {why}",
            canonical.display()
        ));
    }
    Ok(canonical)
}

/// Windows canonicalizes to a `\\?\` path; the app keeps plain ones.
fn plain(path: PathBuf) -> PathBuf {
    #[cfg(windows)]
    {
        let text = path.to_string_lossy();
        if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
            return PathBuf::from(format!(r"\\{rest}"));
        }
        if let Some(rest) = text.strip_prefix(r"\\?\") {
            return PathBuf::from(rest);
        }
    }
    path
}

fn lower(path: &Path) -> Vec<String> {
    path.components()
        .filter_map(|c| match c {
            std::path::Component::Normal(n) => Some(n.to_string_lossy().to_lowercase()),
            _ => None,
        })
        .collect()
}

/// `path` is `base` or lies under it. Compared by components, case-insensitively, so it also
/// holds on a case-insensitive disk and across the `\\?\` forms.
fn within(path: &Path, base: &Path) -> bool {
    let (p, b) = (lower(path), lower(base));
    !b.is_empty() && p.starts_with(&b)
}

fn same(a: &Path, b: &Path) -> bool {
    lower(a) == lower(b)
}

/// Both spellings of a folder: as written and with symlinks resolved.
fn spellings(path: &Path) -> Vec<PathBuf> {
    let mut out = vec![path.to_path_buf()];
    if let Ok(real) = path.canonicalize() {
        let real = plain(real);
        if real != path {
            out.push(real);
        }
    }
    out
}

fn windows_trees() -> Vec<PathBuf> {
    if !cfg!(windows) {
        return Vec::new();
    }
    let mut out: Vec<PathBuf> = [
        "SystemRoot",
        "windir",
        "ProgramFiles",
        "ProgramFiles(x86)",
        "ProgramData",
    ]
    .iter()
    .filter_map(std::env::var_os)
    .map(PathBuf::from)
    .collect();
    out.push(PathBuf::from(r"C:\Windows"));
    out.push(PathBuf::from(r"C:\Program Files"));
    out.push(PathBuf::from(r"C:\Program Files (x86)"));
    out.push(PathBuf::from(r"C:\ProgramData"));
    out
}

/// Why `canonical` (an existing, resolved folder) is refused, or `None`.
fn refusal(canonical: &Path, home: Option<&Path>) -> Option<String> {
    if canonical.parent().is_none() || lower(canonical).is_empty() {
        return Some("it is the root of a disk".into());
    }
    for tree in SYSTEM_TREES.iter().map(Path::new) {
        if spellings(tree).iter().any(|t| within(canonical, t)) {
            return Some("it is a system folder".into());
        }
    }
    for tree in windows_trees() {
        if spellings(&tree).iter().any(|t| within(canonical, t)) {
            return Some("it is a system folder".into());
        }
    }
    for folder in SYSTEM_CONTAINERS.iter().map(Path::new) {
        if spellings(folder).iter().any(|f| same(canonical, f)) {
            return Some("it is a system folder".into());
        }
    }
    if let Some(home) = home {
        for home in spellings(home) {
            if within(&home, canonical) {
                return Some("it is the home folder or a folder that holds it".into());
            }
            for tail in HOME_TREES {
                if within(canonical, &home.join(tail)) {
                    return Some(format!("the {tail} folder of the home folder is private"));
                }
            }
            let guarded = credential_guard::INDEXED_HOME_PATHS
                .iter()
                .chain(credential_guard::MORE_HOME_CREDENTIALS.iter())
                .chain(credential_guard::SECRET_STORE_DIRS.iter())
                .map(|tail| home.join(tail))
                .chain(credential_guard::all_app_data_dirs(&home));
            for dir in guarded {
                for dir in spellings(&dir) {
                    if within(canonical, &dir) {
                        return Some("it holds credentials or the app's own data".into());
                    }
                    // A folder that holds one of them, inside the home folder (`~/.config`).
                    if within(&dir, canonical) && within(canonical, &home) {
                        return Some("it holds credentials or the app's own data".into());
                    }
                }
            }
        }
    }
    let deny = DenyList::new(&[]);
    let path = canonical.to_string_lossy().replace('\\', "/");
    if deny.is_denied_rel(&path) || deny.is_denied_rel(&format!("{path}/x")) {
        return Some("its location is on the sensitive-path deny-list".into());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn home() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    fn err(raw: &str, home: &Path) -> String {
        resolve(raw, Some(home)).unwrap_err()
    }

    #[test]
    fn a_relative_path_is_refused() {
        let h = home();
        assert_eq!(
            err("some/where", h.path()),
            "path must be absolute (or start with ~/)"
        );
        assert_eq!(err("  ", h.path()), "Provide path");
    }

    #[test]
    fn a_missing_path_is_refused() {
        let h = home();
        let missing = h.path().join("nope");
        assert!(err(missing.to_str().unwrap(), h.path()).starts_with("Cannot open"));
    }

    #[test]
    fn a_file_is_not_a_directory() {
        let h = home();
        let file = h.path().join("a.txt");
        std::fs::write(&file, "x").unwrap();
        assert!(err(file.to_str().unwrap(), h.path()).ends_with("is not a directory"));
    }

    #[test]
    fn a_folder_is_resolved_and_a_tilde_expands() {
        let h = home();
        let project = h.path().join("work").join("app");
        std::fs::create_dir_all(&project).unwrap();
        let expected = project.canonicalize().unwrap();
        assert_eq!(
            resolve(project.to_str().unwrap(), Some(h.path())).unwrap(),
            expected
        );
        assert_eq!(resolve("~/work/app", Some(h.path())).unwrap(), expected);
        assert!(resolve("~/work/app", None).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_resolves_to_its_target() {
        let h = home();
        let real = h.path().join("real");
        std::fs::create_dir(&real).unwrap();
        let link = h.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        assert_eq!(
            resolve(link.to_str().unwrap(), Some(h.path())).unwrap(),
            real.canonicalize().unwrap()
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_to_a_sensitive_folder_is_refused() {
        let h = home();
        let ssh = h.path().join(".ssh");
        std::fs::create_dir(&ssh).unwrap();
        let link = h.path().join("innocent");
        std::os::unix::fs::symlink(&ssh, &link).unwrap();
        assert!(err(link.to_str().unwrap(), h.path()).contains("credentials"));
    }

    #[test]
    fn the_home_folder_and_what_holds_it_are_refused() {
        let h = home();
        let inner = h.path().join("nested").join("home");
        std::fs::create_dir_all(&inner).unwrap();
        assert!(err(inner.to_str().unwrap(), &inner).contains("home folder"));
        assert!(err("~", &inner).contains("home folder"));
        // The folder above home holds it too.
        assert!(err(h.path().to_str().unwrap(), &inner).contains("home folder"));
    }

    #[test]
    fn credential_and_private_folders_under_home_are_refused() {
        let h = home();
        for tail in [
            ".ssh",
            ".aws/sso",
            ".config/gh",
            ".gnupg",
            "Library/Mail",
            ".config",
        ] {
            let dir = h.path().join(tail);
            std::fs::create_dir_all(&dir).unwrap();
            assert!(
                resolve(dir.to_str().unwrap(), Some(h.path())).is_err(),
                "{tail} was accepted"
            );
        }
        // A normal folder next to them is fine.
        let ok = h.path().join("code");
        std::fs::create_dir(&ok).unwrap();
        assert!(resolve(ok.to_str().unwrap(), Some(h.path())).is_ok());
    }

    #[test]
    fn the_deny_list_applies_outside_home_too() {
        let h = home();
        let other = tempfile::tempdir().unwrap();
        let ssh = other.path().join(".ssh");
        std::fs::create_dir(&ssh).unwrap();
        assert!(err(ssh.to_str().unwrap(), h.path()).contains("deny-list"));
    }

    #[cfg(unix)]
    #[test]
    fn root_and_system_folders_are_refused() {
        let h = home();
        for path in [
            "/", "/etc", "/usr", "/usr/bin", "/bin", "/sbin", "/var", "/private",
        ] {
            if Path::new(path).exists() {
                assert!(
                    resolve(path, Some(h.path())).is_err(),
                    "{path} was accepted"
                );
            }
        }
    }

    #[test]
    fn names_are_checked() {
        assert_eq!(check_name("  App  ").unwrap(), "App");
        assert!(check_name("").is_err());
        assert!(check_name("   ").is_err());
        assert!(check_name(&"a".repeat(NAME_MAX + 1)).is_err());
        assert!(check_name("a\nb").is_err());
    }
}
