//! Sharing recipes (Phase 4): the export file, importing it, and the shared library.
//!
//! A recipe that comes from outside this Mono is untrusted input, whoever wrote it, so every
//! road in goes through [`import`]:
//!
//! * the file is size limited, its format and version checked, every field of the schema known
//!   (an unknown field or step kind is refused, never skipped) and the recipe passes the same
//!   `Recipe::validate` as a recipe the app wrote;
//! * the content hash in the file must match the recipe (a file edited after the export is
//!   refused); the hash catches accidents and naive edits, it is not a signature, so it never
//!   replaces the approval below;
//! * nothing that looks like a secret (a card, a key, a token) may be in it, and no site of it
//!   may be on this Mono's blocked list or be a bank or payment site;
//! * it gets a new id and version 1, no stats, and NO approval. It is saved as a draft and
//!   marked as coming from outside in the store database (`MonoBrowserStore::mark_outside`),
//!   so after the user approved it, it still runs only once a dry run of exactly that content
//!   passed in this Mono (`MonoBrowserStore::needs_dry_run`).
//!
//! The shared library is a folder of the same files (plus who shared them). Using one in another
//! Mono is [`Library::adopt`]: a copy that goes through [`import`] like a file, so that Mono's
//! blocked list, trust level, approval and dry run all apply. Sharing never widens anything.
use super::model::{Recipe, Revision, Stats, SCHEMA};
use super::risk;
use super::store::{private_dir, write_private, RecipeStore};
use crate::mono::browser::gate::is_finance_url;
use crate::mono::browser::MonoBrowserStore;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::Digest;
use std::path::{Path, PathBuf};

/// The `format` field of every recipe file.
pub const FORMAT: &str = "monoptah-recipe";
/// The file format version this build reads and writes. A newer file is refused.
pub const FORMAT_VERSION: u32 = 1;
/// A recipe file is small; anything bigger is not one.
pub const MAX_FILE_BYTES: usize = 256 * 1024;

/// Where a recipe that came from outside came from (the value of `mark_outside`).
pub const SOURCE_IMPORT: &str = "import";
pub const SOURCE_SHARED: &str = "shared";

/// The export file: a recipe with its origin, version and content hash, and nothing from its
/// runs (no stats, no parameter values).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecipeFile {
    pub format: String,
    pub format_version: u32,
    /// The recipe's origin, repeated so a file can be read without opening it.
    pub origin: String,
    /// The recipe's version when it was written.
    pub version: u32,
    /// `sha256:<hex>` of the recipe's canonical content ([`Recipe::content_hash`]).
    pub hash: String,
    #[serde(default)]
    pub exported_at_ms: u64,
    /// The Mono that promoted it (library files only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shared_by: Option<String>,
    pub recipe: Recipe,
}

fn clip(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

/// What of a recipe looks like a secret, if anything: a card number, a private key, a token
/// shape or one long unbroken token in any text a person wrote (description, parameters, steps).
/// Identity fields (id, intent, hash) are not looked at.
pub fn find_secret(recipe: &Recipe) -> Option<String> {
    fn walk(value: &Value, found: &mut Option<String>) {
        match value {
            Value::String(text) => {
                if found.is_none() {
                    *found = risk::secret_value(text).map(str::to_owned);
                }
            }
            Value::Array(items) => items.iter().for_each(|v| walk(v, found)),
            Value::Object(map) => map.values().for_each(|v| walk(v, found)),
            _ => {}
        }
    }
    let value = serde_json::to_value(recipe).unwrap_or(Value::Null);
    let mut found = None;
    for key in ["description", "params", "steps", "also_origins"] {
        if let Some(part) = value.get(key) {
            walk(part, &mut found);
        }
    }
    found
}

fn build(recipe: &Recipe, shared_by: Option<&str>) -> Result<RecipeFile, String> {
    recipe.validate()?;
    if let Some(what) = find_secret(recipe) {
        return Err(format!(
            "This recipe holds {what}. A recipe never carries one, so it is not exported or shared."
        ));
    }
    let mut clean = recipe.clone();
    clean.stats = Stats::default();
    // A default is a value somebody once chose (an address, a name): it is not part of what a
    // recipe does, so it never leaves this Mono. The parameter stays, and the next user gives it.
    clean.params.iter_mut().for_each(|p| p.default = None);
    Ok(RecipeFile {
        format: FORMAT.into(),
        format_version: FORMAT_VERSION,
        origin: clean.origin.clone(),
        version: clean.version,
        hash: clean.content_hash(),
        exported_at_ms: crate::time::now_ms(),
        shared_by: shared_by.map(str::to_owned),
        recipe: clean,
    })
}

/// The text of the export file of a recipe: the recipe as it is (parameters are placeholders,
/// never values), without its stats. A recipe that holds anything secret-looking is refused.
pub fn export(recipe: &Recipe) -> Result<String, String> {
    let file = build(recipe, None)?;
    serde_json::to_string_pretty(&file).map_err(|_| "Could not write the recipe file.".into())
}

/// Reads and checks a recipe file. `blocked` is the blocked list of the Mono that will own the
/// recipe. Nothing is saved.
pub fn parse(text: &str, blocked: &[String]) -> Result<RecipeFile, String> {
    if text.len() > MAX_FILE_BYTES {
        return Err("This file is too big to be a recipe.".into());
    }
    let value: Value =
        serde_json::from_str(text).map_err(|_| "This file is not a recipe (not valid JSON).")?;
    if value["format"] != FORMAT {
        return Err("This file is not a Monoptah recipe.".into());
    }
    match value["format_version"].as_u64() {
        Some(v) if v == u64::from(FORMAT_VERSION) => {}
        Some(v) if v > u64::from(FORMAT_VERSION) => {
            return Err(
                "This recipe file is from a newer Monoptah. Update the app to use it.".into(),
            )
        }
        _ => return Err("This recipe file has an unknown format version.".into()),
    }
    if value["recipe"]["schema"].as_u64() != Some(u64::from(SCHEMA)) {
        return Err("This recipe uses a schema this version does not support.".into());
    }
    let file: RecipeFile = serde_json::from_value(value).map_err(|e| {
        let reason = e.to_string();
        if reason.contains("unknown variant") {
            format!(
                "The recipe has a step kind this version does not know, so it is refused: {}",
                clip(&reason, 160)
            )
        } else {
            format!("The recipe file is malformed: {}", clip(&reason, 160))
        }
    })?;
    if file.recipe.content_hash() != file.hash {
        return Err(
            "The recipe does not match its hash: the file was changed after it was exported, so it is refused."
                .into(),
        );
    }
    if file.origin != file.recipe.origin || file.version != file.recipe.version {
        return Err("The recipe file disagrees with itself (origin or version).".into());
    }
    file.recipe.validate()?;
    if let Some(what) = find_secret(&file.recipe) {
        return Err(format!(
            "This recipe holds {what}. A recipe never carries one, so it is refused."
        ));
    }
    if blocked_for(&file.recipe, blocked) {
        return Err(
            "This recipe is for a site that is on this Mono's blocked list, or a bank or payment site, so it is refused."
                .into(),
        );
    }
    Ok(file)
}

/// Whether any site of the recipe is blocked for this Mono (its list, or a bank or payment site).
pub fn blocked_for(recipe: &Recipe, blocked: &[String]) -> bool {
    recipe
        .origins()
        .iter()
        .any(|o| blocked.contains(o) || is_finance_url(&format!("{o}/"), blocked))
}

/// Imports the text of a recipe file into a Mono as a DRAFT: a new id, version 1, no stats, no
/// approval, and marked as coming from outside (so it needs the user's approval and then a
/// passing dry run before it runs). `source` is [`SOURCE_IMPORT`] or [`SOURCE_SHARED`].
pub fn import(
    store: &RecipeStore,
    approvals: &MonoBrowserStore,
    mono_id: &str,
    text: &str,
    blocked: &[String],
    source: &str,
) -> Result<Recipe, String> {
    let file = parse(text, blocked)?;
    let mut recipe = file.recipe;
    // Defaults are values, not steps: a file that carries some (made by hand) loses them.
    recipe.params.iter_mut().for_each(|p| p.default = None);
    if store.find_intent(&recipe.origin, &recipe.intent).is_some() {
        return Err(format!(
            "This Mono already has a recipe `{}` for {}. Delete it first to bring this one in.",
            recipe.intent, recipe.origin
        ));
    }
    recipe.id = loop {
        let id = Recipe::new_id();
        if store.get(&id).is_none() {
            break id;
        }
    };
    recipe.version = 1;
    recipe.stats = Stats {
        revision: Some(Revision {
            parent: 0,
            reason: source.to_owned(),
            summary: format!("from version {} ({})", file.version, clip(&file.hash, 19)),
            at_ms: crate::time::now_ms(),
        }),
        ..Stats::default()
    };
    recipe.validate()?;
    // The marker first: a crash between the two steps leaves a marker and no file, never a file
    // that could be approved and run without the dry run.
    approvals.mark_outside(mono_id, &recipe.id, source)?;
    if let Err(reason) = store.save(&recipe) {
        let _ = approvals.revoke_recipe(mono_id, &recipe.id);
        return Err(reason);
    }
    Ok(recipe)
}

/// The recipes users promoted, one file per site and intent (the export format, plus who shared).
#[derive(Clone, Debug)]
pub struct Library {
    dir: PathBuf,
}
impl Library {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }
    pub fn dir(&self) -> &Path {
        &self.dir
    }
    fn path(&self, origin: &str, intent: &str) -> PathBuf {
        // An intent is a slug (the recipe validates it); anything else must not reach the disk.
        let intent = if !intent.is_empty()
            && !intent.contains("..")
            && intent
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'))
        {
            intent
        } else {
            "invalid"
        };
        // Lossless: the readable slug alone would map `my-site.com` and `my.site.com` together,
        // so a short hash of the exact origin is part of the folder name.
        let readable: String = origin
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
            .take(60)
            .collect();
        let digest = sha2::Sha256::digest(origin.as_bytes());
        let tag: String = digest.iter().take(6).map(|b| format!("{b:02x}")).collect();
        let slug = format!("{readable}-{tag}");
        self.dir.join(slug).join(format!("{intent}.json"))
    }
    /// Shares a copy of `recipe`. The caller has checked the user approved it. Another Mono's
    /// entry with the same site and intent is not replaced.
    pub fn promote(&self, mono_id: &str, recipe: &Recipe) -> Result<(), String> {
        let file = build(recipe, Some(mono_id))?;
        let path = self.path(&recipe.origin, &recipe.intent);
        if let Some(existing) = read(&path) {
            if existing.shared_by.as_deref() != Some(mono_id) {
                return Err(
                    "Another Mono already shares a recipe with this name for this site. Unshare it first.".into(),
                );
            }
        }
        let folder = path.parent().ok_or("Bad library path.")?.to_path_buf();
        private_dir(&self.dir)
            .and_then(|()| private_dir(&folder))
            .map_err(|_| "Could not create the shared library.")?;
        let text = serde_json::to_vec_pretty(&file).map_err(|_| "Could not write the recipe.")?;
        write_private(&path, &text).map_err(|_| "Could not share the recipe.".to_owned())
    }
    /// Every readable entry that passes the file checks (a hand edited one is skipped).
    pub fn list(&self) -> Vec<RecipeFile> {
        let mut out = Vec::new();
        for folder in std::fs::read_dir(&self.dir).into_iter().flatten().flatten() {
            if !folder.path().is_dir() {
                continue;
            }
            for entry in std::fs::read_dir(folder.path())
                .into_iter()
                .flatten()
                .flatten()
            {
                let path = entry.path();
                if path.extension().is_some_and(|e| e == "json") {
                    if let Some(file) = read(&path) {
                        out.push(file);
                    }
                }
            }
        }
        out.sort_by(|a, b| (&a.origin, &a.recipe.intent).cmp(&(&b.origin, &b.recipe.intent)));
        out
    }
    /// Takes an entry out of the library. Copies already made are not touched.
    pub fn unshare(&self, origin: &str, intent: &str) -> Result<(), String> {
        let path = self.path(origin, intent);
        if !path.is_file() {
            return Err("That recipe is not in the shared library.".into());
        }
        // Whatever is in the file must be the entry that was asked for.
        if read(&path).is_some_and(|f| f.origin != origin || f.recipe.intent != intent) {
            return Err("That recipe is not in the shared library.".into());
        }
        std::fs::remove_file(path).map_err(|_| "Could not unshare the recipe.".to_owned())
    }
    /// Copies an entry into a Mono as a draft, like importing its file: the Mono's blocked
    /// list, trust level, approval and dry run all still apply.
    pub fn adopt(
        &self,
        origin: &str,
        intent: &str,
        store: &RecipeStore,
        approvals: &MonoBrowserStore,
        mono_id: &str,
        blocked: &[String],
    ) -> Result<Recipe, String> {
        let path = self.path(origin, intent);
        let meta = std::fs::metadata(&path).map_err(|_| "That recipe is not shared.")?;
        if meta.len() > MAX_FILE_BYTES as u64 {
            return Err("This file is too big to be a recipe.".into());
        }
        let text = std::fs::read_to_string(&path).map_err(|_| "That recipe could not be read.")?;
        let file = parse(&text, blocked)?;
        if file.origin != origin || file.recipe.intent != intent {
            return Err("That shared recipe does not match its place in the library.".into());
        }
        import(store, approvals, mono_id, &text, blocked, SOURCE_SHARED)
    }
}

fn read(path: &Path) -> Option<RecipeFile> {
    let meta = std::fs::metadata(path).ok()?;
    if meta.len() > MAX_FILE_BYTES as u64 {
        return None;
    }
    // A library file passes the same checks as an imported one (the blocked list is the
    // adopting Mono's, checked at adopt time).
    parse(&std::fs::read_to_string(path).ok()?, &[]).ok()
}
