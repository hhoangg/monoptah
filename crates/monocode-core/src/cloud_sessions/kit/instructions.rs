//! The user's instructions: `CLAUDE.md` of the source config root, the files it imports and
//! `rules/**/*.md` (feature F3-6).
//!
//! The source is the account's own config dir when it has a `CLAUDE.md`, else `~/.claude`.
//! Every file that travels lies inside the canonical source root, passes the credential
//! rules the skill scan uses ([`credential_file`]) and is read without following a link.
//! An import that resolves outside the root, does not exist or is refused is listed in
//! [`KitInstructions::not_carried`] and never sent. When a cap is exceeded nothing is carried.
//!
//! Import syntax follows Claude Code: `@path` at the start of the text or after whitespace,
//! not inside a fenced code block or an inline code span; a relative path is resolved
//! against the importing file's folder, `@~/x` against the home folder; imports nest up to
//! [`MAX_IMPORT_DEPTH`] levels.

use std::collections::{BTreeMap, HashSet};
use std::io::Read;
use std::path::{Component, Path, PathBuf};

use super::{
    credential_file, digest_files, file_id, Exclusion, Guard, KitFile, KitInstructions, KitRoots,
    KitSource, MAX_IMPORT_DEPTH, MAX_INSTRUCTION_DEPTH, MAX_INSTRUCTION_FILES,
    MAX_INSTRUCTION_FILE_BYTES, MAX_INSTRUCTION_TOTAL_BYTES, SENSITIVE_NAMES,
};

pub(super) const OUTSIDE: &str = "Outside the Claude config folder";
pub(super) const NOT_FOUND: &str = "Not found";
pub(super) const REFUSED: &str = "Not allowed to copy";
pub(super) const ABSOLUTE: &str = "Written as an absolute path; it will not resolve in the cloud";
pub(super) const TOO_MANY_RULES: &str = "Too many files in rules";
pub(super) const NOT_MARKDOWN: &str = "Only Markdown files are copied";
pub(super) const CLAUDE_OWN: &str = "Claude's own folder";
/// Top-level folders Claude keeps its own state in; the Host refuses them too.
const CLAUDE_OWNED: [&str; 18] = [
    "skills",
    "plugins",
    "projects",
    "agents",
    "commands",
    "output-styles",
    "hooks",
    "todos",
    "shell-snapshots",
    "session-env",
    "file-history",
    "ide",
    "statsig",
    "plans",
    "backups",
    "downloads",
    "sessions",
    "state",
];
pub(super) const TOO_DEEP: &str = "Nested too deeply";
const MAX_NOT_CARRIED: usize = 50;
const MAX_RULE_ENTRIES: usize = 2_000;

/// Finds and reads the instructions. `None` when neither source has a `CLAUDE.md`.
pub(super) fn collect(roots: &KitRoots, home: &Path, guard: &Guard) -> Option<KitInstructions> {
    let (source, raw_root) = match &roots.account_dir {
        Some(dir) if dir.join("CLAUDE.md").exists() => (KitSource::Account, dir.clone()),
        _ if roots.claude_home.join("CLAUDE.md").exists() => {
            (KitSource::User, roots.claude_home.clone())
        }
        _ => return None,
    };
    let root = std::fs::canonicalize(&raw_root).unwrap_or_else(|_| raw_root.clone());
    let mut scan = Scan {
        root,
        raw_root,
        home: home.to_path_buf(),
        raw_home: roots.home.clone(),
        guard,
        files: BTreeMap::new(),
        seen: HashSet::new(),
        size: 0,
        exclusion: None,
        not_carried: Vec::new(),
        notes: Vec::new(),
    };
    let top = scan.root.join("CLAUDE.md");
    scan.add(&top, "CLAUDE.md", 0, false);
    scan.rules();

    let exclusion = scan.exclusion;
    let mut files: Vec<KitFile> = scan.files.into_values().collect();
    let has_top = files.iter().any(|f| f.name == Path::new("CLAUDE.md"));
    let mut not_carried = scan.not_carried;
    // Without its `CLAUDE.md` the rest has nothing to hang from, and a capped set is not sent.
    if exclusion.is_some() || !has_top {
        files.clear();
        if exclusion.is_none() && not_carried.is_empty() {
            not_carried.push(("CLAUDE.md".into(), NOT_FOUND.into()));
        }
    }
    let files_kept = !files.is_empty();
    let size = if files.is_empty() { 0 } else { scan.size };
    let hash = if files.is_empty() {
        String::new()
    } else {
        digest_files(&scan.root, &files).0
    };
    Some(KitInstructions {
        source,
        root: scan.root,
        files,
        size,
        hash,
        exclusion,
        not_carried,
        notes: if files_kept { scan.notes } else { Vec::new() },
    })
}

struct Scan<'a> {
    root: PathBuf,
    raw_root: PathBuf,
    home: PathBuf,
    raw_home: PathBuf,
    guard: &'a Guard,
    /// Keyed by the name below the root as written, not by the link target.
    files: BTreeMap<PathBuf, KitFile>,
    seen: HashSet<PathBuf>,
    size: u64,
    exclusion: Option<Exclusion>,
    not_carried: Vec<(String, String)>,
    notes: Vec<(String, String)>,
}

/// Whether an import that is not found looks like a path (so it is worth telling the user)
/// and not a mention such as `@alice`.
fn looks_like_path(token: &str) -> bool {
    token.contains('/') || token.contains('.')
}

impl Scan<'_> {
    fn skip(&mut self, label: &str, reason: &str) {
        let entry = (label.to_owned(), reason.to_owned());
        if self.not_carried.len() < MAX_NOT_CARRIED && !self.not_carried.contains(&entry) {
            self.not_carried.push(entry);
        }
    }

    fn not_found(&mut self, label: &str) {
        if looks_like_path(label) {
            self.skip(label, NOT_FOUND);
        }
    }

    /// Reads one file and follows its imports. `named` is the path as written, lexically
    /// inside the root; the bytes come from its canonical target, which must also lie inside
    /// the root. The file travels under `named`, so a link keeps its own name. `label` is what
    /// the user sees if it cannot be carried.
    fn add(&mut self, named: &Path, label: &str, depth: usize, absolute: bool) {
        if self.exclusion.is_some() {
            return;
        }
        let Ok(rel) = named.strip_prefix(&self.root).map(Path::to_path_buf) else {
            return self.skip(label, OUTSIDE);
        };
        let Ok(real) = std::fs::canonicalize(named) else {
            return self.not_found(label);
        };
        if !real.starts_with(&self.root) {
            return self.skip(label, OUTSIDE);
        }
        if self.seen.contains(&rel) {
            return;
        }
        let Ok(meta) = std::fs::symlink_metadata(&real) else {
            return self.not_found(label);
        };
        if !meta.is_file() {
            return self.not_found(label);
        }
        if !self.plain_rel(&rel) {
            return self.skip(label, REFUSED);
        }
        let first = rel.components().next().and_then(|c| c.as_os_str().to_str());
        if first.is_some_and(|first| CLAUDE_OWNED.contains(&first)) {
            return self.skip(label, CLAUDE_OWN);
        }
        let markdown = |path: &Path| {
            path.extension()
                .and_then(|ext| ext.to_str())
                .is_some_and(|ext| ext.eq_ignore_ascii_case("md"))
        };
        if !markdown(&rel) || !markdown(&real) {
            return self.skip(label, NOT_MARKDOWN);
        }
        if self.refused(&real, &meta) {
            return self.skip(label, REFUSED);
        }
        let data = match read_plain(&real, file_id(&meta)) {
            ReadOutcome::Data(data) => data,
            ReadOutcome::TooLarge => {
                self.exclusion = Some(Exclusion::TooLarge);
                return;
            }
            ReadOutcome::Failed => return self.skip(label, REFUSED),
        };
        if self.files.len() >= MAX_INSTRUCTION_FILES {
            self.exclusion = Some(Exclusion::TooManyFiles);
            return;
        }
        self.size += data.len() as u64;
        if self.size > MAX_INSTRUCTION_TOTAL_BYTES {
            self.exclusion = Some(Exclusion::TooLarge);
            return;
        }
        if absolute {
            let note = (label.to_owned(), ABSOLUTE.to_owned());
            if !self.notes.contains(&note) {
                self.notes.push(note);
            }
        }
        self.seen.insert(rel.clone());
        self.files.insert(
            rel.clone(),
            KitFile {
                path: real.clone(),
                id: file_id(&meta),
                name: rel,
            },
        );
        let text = String::from_utf8_lossy(&data);
        let dir = named.parent().unwrap_or(&self.root).to_path_buf();
        for (token, absolute) in import_tokens(&text) {
            if depth >= MAX_IMPORT_DEPTH {
                // Only a path that would have been a new file is worth a mention.
                if looks_like_path(&token) {
                    match self.resolve(&token, &dir) {
                        Some(target) => {
                            let seen = target
                                .strip_prefix(&self.root)
                                .is_ok_and(|rel| self.seen.contains(rel));
                            if !seen {
                                self.skip(&token, TOO_DEEP);
                            }
                        }
                        None => self.skip(&token, OUTSIDE),
                    }
                }
                continue;
            }
            match self.resolve(&token, &dir) {
                Some(target) => self.add(&target, &token, depth + 1, absolute),
                None => self.skip(&token, OUTSIDE),
            }
        }
    }

    /// The path an import names (lexically, links kept), or `None` when it lies outside the
    /// source root by name.
    fn resolve(&self, token: &str, dir: &Path) -> Option<PathBuf> {
        let joined = if let Some(rest) = token.strip_prefix("~/") {
            self.raw_home.join(rest)
        } else if Path::new(token).is_absolute() {
            PathBuf::from(token)
        } else {
            dir.join(token)
        };
        let named = normalize(&joined);
        // Bring the spellings of the home and the root (which may sit behind a link) to the
        // canonical root before comparing.
        let mut candidates = vec![named.clone()];
        for (raw, real) in [(&self.raw_root, &self.root), (&self.raw_home, &self.home)] {
            if let Ok(rest) = named.strip_prefix(raw) {
                candidates.push(real.join(rest));
            }
        }
        candidates
            .into_iter()
            .find(|candidate| candidate.starts_with(&self.root))
    }

    /// The name below the root as plain text, within the depth cap and free of credential
    /// folders and dot folders (the Host refuses the whole kit on a `.git` segment).
    fn plain_rel(&self, rel: &Path) -> bool {
        let count = rel.components().count();
        for (ix, component) in rel.components().enumerate() {
            let Component::Normal(part) = component else {
                return false;
            };
            let Some(part) = part.to_str() else {
                return false;
            };
            let dir = ix + 1 < count;
            if part.is_empty()
                || part.len() > 255
                || part.chars().any(|c| c.is_control() || c == '\\')
                || SENSITIVE_NAMES.contains(&part)
                || part == ".git"
                || (dir && part.starts_with('.'))
            {
                return false;
            }
        }
        count > 0 && count <= MAX_INSTRUCTION_DEPTH
    }

    /// The same credential rules as the skill scan, for one file.
    fn refused(&self, real: &Path, meta: &std::fs::Metadata) -> bool {
        if credential_file(real, meta, self.guard).is_some() {
            return true;
        }
        // A credentials folder met on the way, except the config dirs that hold the root.
        self.guard
            .forbidden
            .iter()
            .any(|path| real.starts_with(path) && !self.root.starts_with(path))
    }

    /// `rules/**/*.md`, in name order. The folder may itself be a link inside the root; its
    /// files keep the `rules/` name.
    fn rules(&mut self) {
        let dir = self.root.join("rules");
        let Ok(real) = std::fs::canonicalize(&dir) else {
            return;
        };
        if !real.starts_with(&self.root) {
            return self.skip("rules", OUTSIDE);
        }
        let mut budget = MAX_RULE_ENTRIES;
        self.walk_rules(&dir, &mut budget);
    }

    fn walk_rules(&mut self, dir: &Path, budget: &mut usize) {
        let Ok(reader) = std::fs::read_dir(dir) else {
            return;
        };
        let mut children: Vec<_> = reader.flatten().collect();
        children.sort_by_key(|entry| entry.file_name());
        for entry in children {
            if self.exclusion.is_some() {
                return;
            }
            if *budget == 0 {
                return self.skip("rules", TOO_MANY_RULES);
            }
            *budget -= 1;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with('.') {
                continue;
            }
            let path = entry.path();
            let label = path
                .strip_prefix(&self.root)
                .map(|rel| rel.to_string_lossy().into_owned())
                .unwrap_or_else(|_| "rules".into());
            // Follows links: a linked folder or file is checked against the root below.
            let Ok(meta) = std::fs::metadata(&path) else {
                continue;
            };
            if meta.is_dir() {
                let inside =
                    std::fs::canonicalize(&path).is_ok_and(|real| real.starts_with(&self.root));
                let depth = path
                    .strip_prefix(&self.root)
                    .map_or(usize::MAX, |rel| rel.components().count());
                if !inside {
                    self.skip(&label, OUTSIDE);
                } else if depth < MAX_INSTRUCTION_DEPTH {
                    self.walk_rules(&path, budget);
                }
            } else if path.extension().is_some_and(|ext| ext == "md") {
                self.add(&path, &label, 0, false);
            }
        }
    }
}

/// Removes `.` and resolves `..` by name, without touching the disk.
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

enum ReadOutcome {
    Data(Vec<u8>),
    TooLarge,
    Failed,
}

/// Reads a file the scan just looked at, without following a link, and only if it is still
/// the same plain file.
fn read_plain(path: &Path, expect: Option<(u64, u64)>) -> ReadOutcome {
    let Ok(mut handle) = open_plain(path) else {
        return ReadOutcome::Failed;
    };
    let Ok(meta) = handle.metadata() else {
        return ReadOutcome::Failed;
    };
    if !meta.is_file() || (expect.is_some() && file_id(&meta) != expect) {
        return ReadOutcome::Failed;
    }
    if meta.len() > MAX_INSTRUCTION_FILE_BYTES {
        return ReadOutcome::TooLarge;
    }
    let mut data = Vec::new();
    if (&mut handle)
        .take(MAX_INSTRUCTION_FILE_BYTES + 1)
        .read_to_end(&mut data)
        .is_err()
    {
        return ReadOutcome::Failed;
    }
    if data.len() as u64 > MAX_INSTRUCTION_FILE_BYTES {
        return ReadOutcome::TooLarge;
    }
    ReadOutcome::Data(data)
}

#[cfg(unix)]
fn open_plain(path: &Path) -> std::io::Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
}
#[cfg(not(unix))]
fn open_plain(path: &Path) -> std::io::Result<std::fs::File> {
    std::fs::File::open(path)
}

/// Replaces inline code spans with a space.
fn strip_inline_code(line: &str) -> String {
    let chars: Vec<char> = line.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] != '`' {
            out.push(chars[i]);
            i += 1;
            continue;
        }
        let mut n = 0;
        while i + n < chars.len() && chars[i + n] == '`' {
            n += 1;
        }
        // The next run of exactly `n` backticks closes the span.
        let mut j = i + n;
        let mut close = None;
        while j < chars.len() {
            if chars[j] == '`' {
                let mut m = 0;
                while j + m < chars.len() && chars[j + m] == '`' {
                    m += 1;
                }
                if m == n {
                    close = Some(j + m);
                    break;
                }
                j += m;
            } else {
                j += 1;
            }
        }
        match close {
            Some(end) => {
                out.push(' ');
                i = end;
            }
            None => {
                out.extend(std::iter::repeat_n('`', n));
                i += n;
            }
        }
    }
    out
}

/// Whether a line opens a list item (`-`, `*`, `+` or `1.`).
fn is_list_item(line: &str) -> bool {
    let rest = line.trim_start();
    let digits = rest.chars().take_while(char::is_ascii_digit).count();
    let after = &rest[digits..];
    (digits == 0
        && matches!(rest.chars().next(), Some('-' | '*' | '+'))
        && rest[1..].starts_with(' '))
        || (digits > 0 && (after.starts_with(". ") || after.starts_with(") ")))
}

/// The `@path` imports of a markdown text, in order, without the `@`. The flag is true for
/// an absolute path (`/...`). Fenced blocks, indented code blocks and inline code are not
/// scanned.
fn import_tokens(text: &str) -> Vec<(String, bool)> {
    let mut out = Vec::new();
    let mut fence: Option<(char, usize)> = None;
    let mut prev_blank = true;
    let mut prev_code = false;
    let mut in_list = false;
    for line in text.lines() {
        let trimmed = line.trim_start();
        let blank = trimmed.is_empty();
        let indented = line.starts_with('\t') || line.starts_with("    ");
        if let Some(mark) = trimmed.chars().next().filter(|c| matches!(c, '`' | '~')) {
            let run = trimmed.chars().take_while(|c| *c == mark).count();
            if run >= 3 && !(indented && fence.is_none() && ((prev_blank && !in_list) || prev_code))
            {
                match fence {
                    None => {
                        fence = Some((mark, run));
                        prev_blank = false;
                        prev_code = false;
                        continue;
                    }
                    Some((open, len))
                        if open == mark && run >= len && trimmed[run..].trim().is_empty() =>
                    {
                        fence = None;
                        prev_blank = false;
                        prev_code = false;
                        continue;
                    }
                    _ => {}
                }
            }
        }
        if fence.is_some() {
            continue;
        }
        if blank {
            prev_blank = true;
            continue;
        }
        // An indented block after a blank line (or more of the same block) is code, unless
        // it continues a list item.
        let code = indented && ((prev_blank && !in_list) || prev_code);
        if !indented {
            in_list = is_list_item(line);
        }
        prev_blank = false;
        prev_code = code;
        if code {
            continue;
        }
        for word in strip_inline_code(line).split_whitespace() {
            let Some(path) = word.strip_prefix('@') else {
                continue;
            };
            let path =
                path.trim_end_matches(['.', ',', ';', ':', '!', '?', ')', ']', '}', '"', '\'']);
            if !path.is_empty() {
                out.push((path.to_owned(), path.starts_with('/')));
            }
        }
    }
    out
}
