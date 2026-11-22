//! Recipe files and the approved-hash table.
//!
//! Files: `<dir>/<origin slug>/<intent>.json`, and the versions they replaced under
//! `<dir>/<origin slug>/history/<intent>/v<N>.json` (the last 20 are kept). Recipes are not
//! secrets (they never hold one), but they decide what the app does in the user's browser, so the
//! folders are private to the user (0700) and the files 0600. The file is the source of truth for
//! the content.
//!
//! Approval is NOT in the file: it is a row in the store database, `(mono, recipe id, hash)`. The
//! runner recomputes the hash of the file it loads and runs it only when it equals the approved
//! hash. The table is never rebuilt from the files (a rebuilt hash would approve whatever a file
//! says); losing it makes every recipe a draft again, which is the safe direction.
use super::model::{Recipe, Revision, Stats};
use crate::mono::browser::MonoBrowserStore;
use rusqlite::{params, OptionalExtension};
use std::path::{Path, PathBuf};

const UNAVAILABLE: &str = "Browser storage is unavailable.";
pub const RECIPE_TABLES: &str = "
CREATE TABLE IF NOT EXISTS mono_browser_recipe_approvals(
  mono_id TEXT NOT NULL, recipe_id TEXT NOT NULL, hash TEXT NOT NULL, approved_at INTEGER NOT NULL,
  PRIMARY KEY(mono_id, recipe_id));
CREATE TABLE IF NOT EXISTS mono_browser_recipe_hashes(
  mono_id TEXT NOT NULL, recipe_id TEXT NOT NULL, hash TEXT NOT NULL, approved_at INTEGER NOT NULL,
  PRIMARY KEY(mono_id, recipe_id, hash));
CREATE TABLE IF NOT EXISTS mono_browser_recipe_verified(
  mono_id TEXT NOT NULL, recipe_id TEXT NOT NULL, hash TEXT NOT NULL, at INTEGER NOT NULL,
  PRIMARY KEY(mono_id, recipe_id, hash));";
/// Past versions kept per recipe.
pub const KEEP_VERSIONS: usize = 20;

impl MonoBrowserStore {
    /// Records that the user approved exactly this content of the recipe.
    pub fn approve_recipe(&self, mono_id: &str, recipe_id: &str, hash: &str) -> Result<(), String> {
        Self::valid_id(mono_id)?;
        let conn = self.open()?.ok_or(UNAVAILABLE)?;
        conn.execute_batch(RECIPE_TABLES).map_err(|_| UNAVAILABLE)?;
        let now = crate::time::now_ms() as i64;
        conn.execute(
            "INSERT INTO mono_browser_recipe_approvals(mono_id,recipe_id,hash,approved_at) VALUES(?1,?2,?3,?4)
             ON CONFLICT(mono_id,recipe_id) DO UPDATE SET hash=excluded.hash, approved_at=excluded.approved_at",
            params![mono_id, recipe_id, hash, now],
        )
        .map_err(|_| UNAVAILABLE)?;
        // Every content the user approved is remembered, so a rollback to it needs no new card.
        conn.execute(
            "INSERT INTO mono_browser_recipe_hashes(mono_id,recipe_id,hash,approved_at) VALUES(?1,?2,?3,?4)
             ON CONFLICT(mono_id,recipe_id,hash) DO UPDATE SET approved_at=excluded.approved_at",
            params![mono_id, recipe_id, hash, now],
        )
        .map(|_| ())
        .map_err(|_| UNAVAILABLE.into())
    }
    /// Whether the user once approved exactly this content (the current approval or an earlier
    /// one). Never rebuilt from files, like the approvals themselves.
    pub fn hash_was_approved(&self, mono_id: &str, recipe_id: &str, hash: &str) -> bool {
        let Ok(Some(conn)) = self.open() else {
            return false;
        };
        conn.query_row(
            "SELECT 1 FROM mono_browser_recipe_hashes WHERE mono_id=?1 AND recipe_id=?2 AND hash=?3",
            params![mono_id, recipe_id, hash],
            |_| Ok(()),
        )
        .optional()
        .ok()
        .flatten()
        .is_some()
    }
    /// Records that a full run of exactly this content passed on the live page. Kept here, not in
    /// the recipe file: a recipe that changes things asks the user on its first run, and a file
    /// edit (or its stats) must not be able to skip that. Never rebuilt from files.
    pub fn mark_verified(&self, mono_id: &str, recipe_id: &str, hash: &str) -> Result<(), String> {
        Self::valid_id(mono_id)?;
        let conn = self.open()?.ok_or(UNAVAILABLE)?;
        conn.execute_batch(RECIPE_TABLES).map_err(|_| UNAVAILABLE)?;
        conn.execute(
            "INSERT OR REPLACE INTO mono_browser_recipe_verified(mono_id,recipe_id,hash,at) VALUES(?1,?2,?3,?4)",
            params![mono_id, recipe_id, hash, crate::time::now_ms() as i64],
        )
        .map(|_| ())
        .map_err(|_| UNAVAILABLE.into())
    }
    /// Whether a full run of exactly this content passed.
    pub fn is_verified(&self, mono_id: &str, recipe_id: &str, hash: &str) -> bool {
        let Ok(Some(conn)) = self.open() else {
            return false;
        };
        conn.query_row(
            "SELECT 1 FROM mono_browser_recipe_verified WHERE mono_id=?1 AND recipe_id=?2 AND hash=?3",
            params![mono_id, recipe_id, hash],
            |_| Ok(()),
        )
        .optional()
        .ok()
        .flatten()
        .is_some()
    }
    /// The hash the user approved for a recipe, if any.
    pub fn approved_hash(&self, mono_id: &str, recipe_id: &str) -> Option<String> {
        let Ok(Some(conn)) = self.open() else {
            return None;
        };
        conn.query_row(
            "SELECT hash FROM mono_browser_recipe_approvals WHERE mono_id=?1 AND recipe_id=?2",
            params![mono_id, recipe_id],
            |row| row.get(0),
        )
        .optional()
        .ok()
        .flatten()
    }
    /// Whether this exact content is the approved one.
    pub fn recipe_approved(&self, mono_id: &str, recipe: &Recipe) -> bool {
        self.approved_hash(mono_id, &recipe.id)
            .is_some_and(|hash| hash == recipe.content_hash())
    }
    pub fn revoke_recipe(&self, mono_id: &str, recipe_id: &str) -> Result<(), String> {
        Self::valid_id(mono_id)?;
        let Some(conn) = self.open()? else {
            return Ok(());
        };
        for sql in [
            "DELETE FROM mono_browser_recipe_approvals WHERE mono_id=?1 AND recipe_id=?2",
            "DELETE FROM mono_browser_recipe_hashes WHERE mono_id=?1 AND recipe_id=?2",
            "DELETE FROM mono_browser_recipe_verified WHERE mono_id=?1 AND recipe_id=?2",
        ] {
            match conn.execute(sql, params![mono_id, recipe_id]) {
                Ok(_) => {}
                Err(e) if e.to_string().contains("no such table") => {}
                Err(_) => return Err(UNAVAILABLE.into()),
            }
        }
        Ok(())
    }
}

/// The recipe files of one Mono.
#[derive(Clone, Debug)]
pub struct RecipeStore {
    dir: PathBuf,
}

fn origin_slug(origin: &str) -> String {
    origin
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect()
}

#[cfg(unix)]
fn private_dir(dir: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)?;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
}
#[cfg(not(unix))]
fn private_dir(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)
}

/// Writes `bytes` to `path` through a temporary file in the same folder (so a crash never leaves
/// half a recipe), readable by the user only.
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let parent = path.parent().unwrap_or(Path::new("."));
    let tmp = parent.join(format!(".tmp-{:08x}", rand::random::<u32>()));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let result = (|| {
        let mut file = options.open(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        std::fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

impl RecipeStore {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }
    pub fn dir(&self) -> &Path {
        &self.dir
    }
    fn folder(&self, origin: &str) -> PathBuf {
        self.dir.join(origin_slug(origin))
    }
    pub fn path_of(&self, recipe: &Recipe) -> PathBuf {
        self.folder(&recipe.origin)
            .join(format!("{}.json", recipe.intent))
    }

    /// Saves a valid recipe. The file it replaces moves to the history.
    pub fn save(&self, recipe: &Recipe) -> Result<(), String> {
        recipe.validate()?;
        let path = self.path_of(recipe);
        let folder = path.parent().ok_or("Bad recipe path.")?.to_path_buf();
        private_dir(&self.dir)
            .and_then(|()| private_dir(&folder))
            .map_err(|_| "Could not create the recipe folder.")?;
        if let Some(old) = read(&path) {
            if old.id != recipe.id || old.version != recipe.version {
                self.archive(&old)?;
            }
        }
        let text = serde_json::to_vec_pretty(recipe).map_err(|_| "Could not write the recipe.")?;
        write_private(&path, &text).map_err(|_| "Could not save the recipe.".to_owned())
    }
    fn history_dir(&self, origin: &str, intent: &str) -> PathBuf {
        self.folder(origin).join("history").join(intent)
    }
    fn archive(&self, old: &Recipe) -> Result<(), String> {
        let dir = self.history_dir(&old.origin, &old.intent);
        private_dir(&dir).map_err(|_| "Could not create the recipe history.")?;
        let text = serde_json::to_vec_pretty(old).map_err(|_| "Could not write the recipe.")?;
        write_private(&dir.join(format!("v{}.json", old.version)), &text)
            .map_err(|_| "Could not archive the recipe.".to_owned())?;
        let mut versions = self.versions_in(&dir);
        versions.sort_unstable();
        while versions.len() > KEEP_VERSIONS {
            let oldest = versions.remove(0);
            let _ = std::fs::remove_file(dir.join(format!("v{oldest}.json")));
        }
        Ok(())
    }
    fn versions_in(&self, dir: &Path) -> Vec<u32> {
        std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| {
                e.file_name()
                    .to_str()?
                    .strip_prefix('v')?
                    .strip_suffix(".json")?
                    .parse()
                    .ok()
            })
            .collect()
    }

    /// Every valid current recipe, by origin then intent. A file that fails validation (hand
    /// edited, newer schema) is skipped: it can not run.
    pub fn list(&self) -> Vec<Recipe> {
        let mut out: Vec<Recipe> = Vec::new();
        for folder in std::fs::read_dir(&self.dir).into_iter().flatten().flatten() {
            if !folder.path().is_dir() {
                continue;
            }
            for file in std::fs::read_dir(folder.path())
                .into_iter()
                .flatten()
                .flatten()
            {
                let path = file.path();
                if path.extension().is_some_and(|e| e == "json") && path.is_file() {
                    if let Some(recipe) = read(&path).filter(|r| r.validate().is_ok()) {
                        out.push(recipe);
                    }
                }
            }
        }
        out.sort_by(|a, b| (&a.origin, &a.intent).cmp(&(&b.origin, &b.intent)));
        out
    }
    pub fn get(&self, id: &str) -> Option<Recipe> {
        self.list().into_iter().find(|r| r.id == id)
    }
    pub fn find_intent(&self, origin: &str, intent: &str) -> Option<Recipe> {
        self.list()
            .into_iter()
            .find(|r| r.origin == origin && r.intent == intent)
    }
    /// The versions of a recipe, newest first: the current one and the archived ones.
    pub fn history(&self, origin: &str, intent: &str) -> Vec<Recipe> {
        let dir = self.history_dir(origin, intent);
        let mut out: Vec<Recipe> = self
            .versions_in(&dir)
            .into_iter()
            .filter_map(|v| read(&dir.join(format!("v{v}.json"))))
            .collect();
        // After a rollback the current version is also in the archive: list it once.
        if let Some(current) = self.find_intent(origin, intent) {
            out.retain(|r| r.version != current.version);
            out.push(current);
        }
        out.sort_by_key(|r| std::cmp::Reverse(r.version));
        out
    }
    /// The number a new version of this recipe gets: above every version kept, so a repair after
    /// a rollback never overwrites the version that was rolled back from.
    pub fn next_version(&self, recipe: &Recipe) -> u32 {
        self.history(&recipe.origin, &recipe.intent)
            .iter()
            .map(|r| r.version)
            .chain([recipe.version])
            .max()
            .unwrap_or(0)
            + 1
    }
    /// Makes an archived version the current one. The version it replaces moves to the history.
    /// The caller decides whether the restored content is still approved (see
    /// `MonoBrowserStore::hash_was_approved`).
    pub fn rollback(
        &self,
        current: &Recipe,
        version: u32,
        summary: &str,
        mark_flaky: bool,
    ) -> Result<Recipe, String> {
        let on_disk = self
            .find_intent(&current.origin, &current.intent)
            .filter(|r| r.id == current.id)
            .ok_or("The recipe is no longer saved.")?;
        if version == on_disk.version {
            return Err(format!("Version {version} is already the current one."));
        }
        let dir = self.history_dir(&current.origin, &current.intent);
        let mut restored = read(&dir.join(format!("v{version}.json")))
            .filter(|r| r.id == current.id && r.version == version && r.validate().is_ok())
            .ok_or_else(|| format!("Version {version} is not in the history."))?;
        restored.stats = on_disk.stats.clone();
        restored.stats.repaired = false;
        restored.stats.clean_since_repair = 0;
        restored.stats.failed_since_repair = 0;
        restored.stats.consecutive_ok = 0;
        restored.stats.flaky |= mark_flaky;
        restored.stats.revision = Some(Revision {
            parent: on_disk.version,
            reason: "rollback".into(),
            summary: summary.chars().take(200).collect(),
            at_ms: crate::time::now_ms(),
        });
        self.save(&restored)?;
        Ok(restored)
    }
    /// Changes the stats in the current file of this recipe (not part of the hash).
    pub fn update_stats<R>(
        &self,
        recipe: &Recipe,
        change: impl FnOnce(&mut Stats) -> R,
    ) -> Option<R> {
        let path = self.path_of(recipe);
        let mut current = read(&path).filter(|r| r.id == recipe.id)?;
        let result = change(&mut current.stats);
        let text = serde_json::to_vec_pretty(&current).ok()?;
        write_private(&path, &text).ok()?;
        Some(result)
    }
    pub fn delete(&self, recipe: &Recipe) -> Result<(), String> {
        let _ = std::fs::remove_file(self.path_of(recipe));
        let _ = std::fs::remove_dir_all(self.history_dir(&recipe.origin, &recipe.intent));
        Ok(())
    }
    /// Counts one run in the recipe's stats (not part of the hash). `blame_repair` is false for a
    /// failure that says nothing about the recipe (a sign-in page).
    pub fn record_run(&self, recipe: &Recipe, ok: bool) -> RunEffect {
        self.record_run_with(recipe, ok, true)
    }
    pub fn record_run_with(&self, recipe: &Recipe, ok: bool, blame_repair: bool) -> RunEffect {
        self.update_stats(recipe, |stats| stats.note_run(ok, blame_repair))
            .unwrap_or_default()
    }
}

/// What a counted run changed beyond the counters.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RunEffect {
    /// The repaired version failed twice in a row: go back to the version before it.
    pub rollback_due: bool,
}

fn read(path: &Path) -> Option<Recipe> {
    // A recipe file is small; anything bigger is not one.
    let meta = std::fs::metadata(path).ok()?;
    if meta.len() > 256 * 1024 {
        return None;
    }
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}
