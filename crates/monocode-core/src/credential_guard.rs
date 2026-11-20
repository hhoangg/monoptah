//! What is shared by the guards that keep credentials and the app's own data out of anything
//! that serves or copies files: the cloud kit scan (`cloud_sessions::kit`) and the remote
//! `chat.mentionedFile` read (`mobile::mentioned_file`).
//!
//! Three pieces live here so the two never drift apart: the credential locations under the home
//! folder, the folders that hold the app's own data (both flavors, every platform layout), and
//! the (device, inode) index of the files under the credential folders, which spots a hard link
//! to one of them.
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

/// Entries read when collecting credential file identities.
pub(crate) const MAX_INDEX_ENTRIES: usize = 20_000;

/// Credential locations under `$HOME`, in the order they are indexed: the small and
/// important ones first, the folders that may hold logs and caches last.
pub(crate) const INDEXED_HOME_PATHS: [&str; 9] = [
    ".claude/.credentials.json",
    ".ssh",
    ".aws",
    ".config/gh",
    ".gnupg",
    ".codex/auth.json",
    ".kube",
    ".docker",
    ".config/gcloud",
];

/// More credential locations under `$HOME` that a remote read refuses (and indexes for hard
/// links), beyond [`INDEXED_HOME_PATHS`]. A folder is refused whole, a file by itself.
pub(crate) const MORE_HOME_CREDENTIALS: [&str; 15] = [
    ".azure",
    ".config/github-copilot",
    ".config/monoptah",
    ".git-credentials",
    ".gitconfig",
    ".config/git/credentials",
    ".cargo/credentials",
    ".cargo/credentials.toml",
    ".vault-token",
    ".pgpass",
    ".my.cnf",
    ".netrc",
    ".claude.json",
    ".zsh_history",
    ".bash_history",
];

/// Secret stores of other programs under `$HOME` that a remote read refuses whole, for every device and on every
/// platform layout (macOS `Library`, Linux dot folders, snap and flatpak, Windows `AppData`): browser profiles
/// (saved logins, cookies, session tokens), password managers and keyrings, GnuPG, and the chat apps whose
/// local storage holds login tokens. Not part of the hard link index: the folders are large.
pub(crate) const SECRET_STORE_DIRS: &[&str] = &[
    // Browsers.
    "Library/Application Support/Firefox",
    "Library/Application Support/Google/Chrome",
    "Library/Application Support/Chromium",
    "Library/Application Support/BraveSoftware",
    "Library/Application Support/Microsoft Edge",
    "Library/Application Support/Arc",
    "Library/Application Support/Vivaldi",
    "Library/Application Support/com.operasoftware.Opera",
    "Library/Safari",
    "Library/Containers/com.apple.Safari",
    ".mozilla",
    ".librewolf",
    ".config/google-chrome",
    ".config/google-chrome-beta",
    ".config/chromium",
    ".config/BraveSoftware",
    ".config/microsoft-edge",
    ".config/vivaldi",
    ".config/opera",
    "snap/firefox",
    "snap/chromium",
    ".var/app/org.mozilla.firefox",
    ".var/app/com.google.Chrome",
    ".var/app/org.chromium.Chromium",
    ".var/app/com.brave.Browser",
    "AppData/Roaming/Mozilla",
    "AppData/Local/Google/Chrome",
    "AppData/Local/Chromium",
    "AppData/Local/BraveSoftware",
    "AppData/Local/Microsoft/Edge",
    "AppData/Local/Vivaldi",
    // Keyrings and password managers.
    ".local/share/keyrings",
    ".password-store",
    ".gnupg",
    ".config/op",
    ".op",
    ".config/1Password",
    "Library/Application Support/1Password",
    "Library/Group Containers/2BUA8C4S2C.com.1password",
    "Library/Containers/com.1password.1password",
    "AppData/Local/1Password",
    "Library/Application Support/Bitwarden",
    ".config/Bitwarden",
    "AppData/Roaming/Bitwarden",
    "snap/bitwarden",
    ".var/app/com.bitwarden.desktop",
    // Chat apps (leveldb and sqlite token stores).
    "Library/Application Support/Slack",
    "Library/Application Support/discord",
    "Library/Application Support/Discord",
    "Library/Application Support/Signal",
    ".config/Slack",
    ".config/discord",
    ".config/Signal",
    "AppData/Roaming/Slack",
    "AppData/Roaming/discord",
    "AppData/Roaming/Signal",
    "snap/slack",
    "snap/discord",
    ".var/app/com.slack.Slack",
    ".var/app/com.discordapp.Discord",
];

/// Names of the app's data folders under the platform data dirs (`Application Support`,
/// `~/.local/share`, `~/.config`, `AppData`), both flavors: the Tauri app, the native app, its
/// development build and the upstream names it can still read.
pub(crate) const APP_DATA_NAMES: [&str; 8] = [
    "Monoptah",
    "Monoptah Native",
    "Monoptah Development",
    "MonoCode",
    "monoptah",
    "monocode",
    crate::brand::NATIVE_APP_ID,
    "com.monocode.desktop",
];

/// Folders that hold the app's own data, as the app finds them plus the per-OS layouts
/// under the given home (the home may differ from the process one).
pub(crate) fn app_data_dirs(home: &Path) -> Vec<PathBuf> {
    let id = crate::brand::DESKTOP_APP_ID;
    let mut out = vec![
        home.join("Library/Application Support").join(id),
        home.join(".local/share").join(id),
    ];
    if let Some(dir) = crate::store::monocode_database().parent() {
        out.push(dir.to_path_buf());
    }
    if let Some(xdg) = std::env::var_os("XDG_DATA_HOME").filter(|v| !v.is_empty()) {
        out.push(PathBuf::from(xdg).join(id));
    }
    out
}

/// [`app_data_dirs`] plus every other flavor and name the app keeps data under, including the
/// `.dev` bundle of the development build and the WebKit storage of the desktop shell.
pub(crate) fn all_app_data_dirs(home: &Path) -> Vec<PathBuf> {
    let mut out = app_data_dirs(home);
    let bases = [
        home.join("Library/Application Support"),
        home.join(".local/share"),
        home.join(".config"),
        home.join("AppData/Roaming"),
        home.join("AppData/Local"),
    ];
    let mut xdg_bases: Vec<PathBuf> = Vec::new();
    for var in ["XDG_DATA_HOME", "XDG_CONFIG_HOME"] {
        if let Some(dir) = std::env::var_os(var).filter(|v| !v.is_empty()) {
            xdg_bases.push(PathBuf::from(dir));
        }
    }
    for base in bases.iter().chain(xdg_bases.iter()) {
        for name in APP_DATA_NAMES {
            out.push(base.join(name));
        }
        out.push(base.join(format!("{}.dev", crate::brand::DESKTOP_APP_ID)));
    }
    for id in [
        crate::brand::DESKTOP_APP_ID,
        crate::brand::NATIVE_APP_ID,
        "com.monocode.desktop",
    ] {
        out.push(home.join("Library/WebKit").join(id));
        out.push(home.join("Library/WebKit").join(format!("{id}.dev")));
        out.push(home.join("Library/Caches").join(id));
        out.push(home.join("Library/HTTPStorages").join(id));
    }
    out.sort();
    out.dedup();
    out
}

/// The (device, inode) of a file, where the platform has them.
#[cfg(unix)]
pub(crate) fn file_id(meta: &std::fs::Metadata) -> Option<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    Some((meta.dev(), meta.ino()))
}
#[cfg(not(unix))]
pub(crate) fn file_id(_meta: &std::fs::Metadata) -> Option<(u64, u64)> {
    None
}

/// How many names a file has.
#[cfg(unix)]
pub(crate) fn link_count(meta: &std::fs::Metadata) -> u64 {
    std::os::unix::fs::MetadataExt::nlink(meta)
}
#[cfg(not(unix))]
pub(crate) fn link_count(_meta: &std::fs::Metadata) -> u64 {
    1
}

/// (device, inode) of the regular files under the credential locations of a home folder, to
/// spot a hard link to one of them.
pub(crate) struct SecretIndex {
    pub ids: HashSet<(u64, u64)>,
    /// False when the scan ran out of time or entries, so a hard link to a credential file may
    /// go unnoticed.
    pub complete: bool,
}

impl SecretIndex {
    /// Indexes [`INDEXED_HOME_PATHS`] under `home` within `budget` of `started`.
    pub fn build(home: &Path, started: Instant, budget: Duration) -> Self {
        Self::build_for(home, &INDEXED_HOME_PATHS, started, budget)
    }

    /// Indexes the given locations (relative to `home`), in order, until the budget ends.
    pub fn build_for(home: &Path, paths: &[&str], started: Instant, budget: Duration) -> Self {
        let mut ids = HashSet::new();
        let mut left = MAX_INDEX_ENTRIES;
        let mut complete = true;
        for rel in paths {
            let path = home.join(rel);
            // A credentials folder may itself be a link (stow-style dotfiles).
            let path = std::fs::canonicalize(&path).unwrap_or(path);
            if !collect_file_ids(&path, &mut ids, &mut left, started, budget) {
                complete = false;
                break;
            }
        }
        Self { ids, complete }
    }
}

/// Add the (device, inode) of every regular file under `path`, without following links.
/// Errors are ignored. Returns false when the entry cap or the time budget ran out.
fn collect_file_ids(
    path: &Path,
    out: &mut HashSet<(u64, u64)>,
    left: &mut usize,
    started: Instant,
    budget: Duration,
) -> bool {
    if *left == 0 || started.elapsed() >= budget {
        return false;
    }
    *left -= 1;
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return true;
    };
    if meta.is_file() {
        if let Some(id) = file_id(&meta) {
            out.insert(id);
        }
    } else if meta.is_dir() {
        if let Ok(reader) = std::fs::read_dir(path) {
            for entry in reader.flatten() {
                if !collect_file_ids(&entry.path(), out, left, started, budget) {
                    return false;
                }
            }
        }
    }
    true
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn a_hard_link_to_a_credential_file_is_in_the_index_and_other_files_are_not() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        std::fs::create_dir_all(home.join(".ssh")).unwrap();
        std::fs::write(home.join(".ssh/id_x"), "k").unwrap();
        std::fs::write(home.join("other"), "o").unwrap();
        std::fs::hard_link(home.join(".ssh/id_x"), home.join("alias")).unwrap();
        let index = SecretIndex::build(home, Instant::now(), Duration::from_secs(5));
        assert!(index.complete);
        let id = |p: &str| file_id(&std::fs::metadata(home.join(p)).unwrap()).unwrap();
        assert!(index.ids.contains(&id("alias")));
        assert!(!index.ids.contains(&id("other")));
    }

    #[test]
    fn every_flavor_of_the_app_data_dir_is_listed() {
        let dirs = all_app_data_dirs(Path::new("/h"));
        for expected in [
            "/h/Library/Application Support/com.monoptah.desktop",
            "/h/Library/Application Support/com.monoptah.desktop.dev",
            "/h/Library/Application Support/Monoptah Development",
            "/h/Library/Application Support/Monoptah Native",
            "/h/.local/share/com.monoptah.native",
            "/h/Library/WebKit/com.monoptah.desktop.dev",
        ] {
            assert!(dirs.contains(&PathBuf::from(expected)), "{expected}");
        }
    }
}
