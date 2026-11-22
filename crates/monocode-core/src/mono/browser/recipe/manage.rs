//! What the Browser tab does with recipes (Phase 4): the list, the steps, approve, dry run,
//! rollback, delete, and promoting a recipe to the shared library. These are the user's own
//! actions in the app, not tools of the Mono, so they are not offered to the gate's agents; the
//! approval and the dry run here are the only way an imported or shared recipe becomes usable.
use super::model::{bind_params, ParamKind, Recipe, Risk};
use super::record;
use super::runner::{self, Host, Options};
use super::share::Library;
use super::store::RecipeStore;
use crate::mono::browser::MonoBrowserStore;
use serde_json::{json, Map, Value};

/// Whether a recipe can run now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    /// Not approved (new, imported, shared, or its file changed after the approval).
    Draft,
    /// Approved, but it came from outside and no dry run of this content passed in this Mono.
    NeedsDryRun,
    Approved,
}
impl Status {
    pub fn label(self) -> &'static str {
        match self {
            Self::Draft => "Draft",
            Self::NeedsDryRun => "Needs a dry run",
            Self::Approved => "Approved",
        }
    }
}

/// One line of the Browser tab's list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub id: String,
    pub intent: String,
    pub origin: String,
    pub description: String,
    pub version: u32,
    pub risk: Risk,
    pub status: Status,
    pub runs: u32,
    pub ok: u32,
    /// 0 when it never ran.
    pub last_run_ms: u64,
    pub flaky: bool,
    pub repaired: bool,
    pub min_level: u8,
    /// `import` or `shared` when it came from outside this Mono.
    pub outside: Option<String>,
    /// The newest older version in the history (what "roll back" goes to).
    pub previous: Option<u32>,
    /// The hash of the content the row shows: what an approval card is bound to.
    pub hash: String,
}
impl Row {
    /// Percent of runs that passed, or `None` before the first run.
    pub fn success_percent(&self) -> Option<u32> {
        (self.runs > 0).then(|| (u64::from(self.ok) * 100 / u64::from(self.runs)) as u32)
    }
}

pub fn status_of(approvals: &MonoBrowserStore, mono_id: &str, recipe: &Recipe) -> Status {
    if !approvals.recipe_approved(mono_id, recipe) {
        Status::Draft
    } else if approvals.needs_dry_run(mono_id, recipe) {
        Status::NeedsDryRun
    } else {
        Status::Approved
    }
}

/// Every recipe of the Mono, by site then intent.
pub fn rows(store: &RecipeStore, approvals: &MonoBrowserStore, mono_id: &str) -> Vec<Row> {
    store
        .list()
        .into_iter()
        .map(|recipe| {
            let previous = store
                .history(&recipe.origin, &recipe.intent)
                .iter()
                .map(|r| r.version)
                .filter(|v| *v < recipe.version)
                .max();
            Row {
                id: recipe.id.clone(),
                intent: recipe.intent.clone(),
                origin: recipe.origin.clone(),
                description: recipe.description.clone(),
                version: recipe.version,
                risk: recipe.computed_risk(),
                status: status_of(approvals, mono_id, &recipe),
                runs: recipe.stats.runs,
                ok: recipe.stats.ok,
                last_run_ms: recipe.stats.last_run_ms.max(recipe.stats.last_ok_ms),
                flaky: recipe.stats.flaky,
                repaired: recipe.stats.repaired,
                min_level: recipe.min_level(),
                outside: approvals.outside_source(mono_id, &recipe.id),
                previous,
                hash: recipe.content_hash(),
            }
        })
        .collect()
}

/// The steps in plain words. Values are never in a recipe, only `{parameter}` placeholders, so
/// this is safe to show; parameters are listed by name and description.
pub fn steps_text(recipe: &Recipe) -> String {
    let params = if recipe.params.is_empty() {
        "Parameters: none".to_owned()
    } else {
        format!(
            "Parameters: {}",
            recipe
                .params
                .iter()
                .map(|p| format!("{} ({})", p.name, p.description))
                .collect::<Vec<_>>()
                .join(", ")
        )
    };
    format!(
        "{}\nRisk: {}\n{params}\n{}",
        recipe.description,
        recipe.computed_risk().as_str(),
        record::plain_summary(recipe)
    )
}

/// The user approves the recipe whose content they were shown. `shown_hash` is the hash of that
/// content: if the file changed since (edited, synced, replaced), nothing is approved.
pub fn approve(
    store: &RecipeStore,
    approvals: &MonoBrowserStore,
    mono_id: &str,
    recipe_id: &str,
    shown_hash: &str,
) -> Result<(), String> {
    let recipe = store
        .get(recipe_id)
        .ok_or("That recipe no longer exists.")?;
    recipe.validate()?;
    if recipe.content_hash() != shown_hash {
        return Err(
            "The recipe changed since it was shown, so nothing was approved. Review it again."
                .into(),
        );
    }
    approvals.approve_recipe(mono_id, &recipe.id, shown_hash)
}

/// Deletes the recipe, its history and every approval, dry run and mark kept for it.
pub fn delete(
    store: &RecipeStore,
    approvals: &MonoBrowserStore,
    mono_id: &str,
    recipe_id: &str,
) -> Result<(), String> {
    let recipe = store
        .get(recipe_id)
        .ok_or("That recipe no longer exists.")?;
    approvals.revoke_recipe(mono_id, &recipe.id)?;
    store.delete(&recipe)
}

/// Makes an earlier version the current one. It is approved again only if the user approved
/// exactly that content before (the same rule as the Mono's `recipe_rollback`), and a recipe
/// from outside still needs a dry run of that content.
pub fn rollback(
    store: &RecipeStore,
    approvals: &MonoBrowserStore,
    mono_id: &str,
    recipe_id: &str,
    version: u32,
) -> Result<Recipe, String> {
    let current = store
        .get(recipe_id)
        .ok_or("That recipe no longer exists.")?;
    let restored = store.rollback(&current, version, "rolled back by the user", false)?;
    let hash = restored.content_hash();
    if approvals.hash_was_approved(mono_id, &restored.id, &hash) {
        approvals.approve_recipe(mono_id, &restored.id, &hash)?;
    }
    Ok(restored)
}

/// Shares a copy of an approved recipe. A recipe that is not approved, or came from outside
/// and has not passed a dry run, is not shared: the library only holds what a user vetted.
pub fn promote(
    store: &RecipeStore,
    approvals: &MonoBrowserStore,
    library: &Library,
    mono_id: &str,
    recipe_id: &str,
) -> Result<(), String> {
    let recipe = store
        .get(recipe_id)
        .ok_or("That recipe no longer exists.")?;
    match status_of(approvals, mono_id, &recipe) {
        Status::Approved => library.promote(mono_id, &recipe),
        _ => Err("Only an approved recipe that has passed its dry run can be shared.".into()),
    }
}

/// What a dry run found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DryRunReport {
    pub ok: bool,
    pub steps_run: usize,
    /// The 1 based step it stopped before (the first step that changes things).
    pub stopped_before: Option<usize>,
    /// Why it failed, for the user (never a value).
    pub failure: Option<String>,
}

/// Values for a dry run: each parameter's default, else a placeholder of its kind (exports and
/// shared recipes carry no defaults). The dry run never types, so a placeholder only shapes the
/// pages it opens and reads; a locator or URL that depends on a real value may fail it, and the
/// user then runs the recipe once with real values.
fn dry_values(recipe: &Recipe) -> Result<Map<String, Value>, String> {
    let mut given = Map::new();
    for param in recipe.params.iter().filter(|p| p.default.is_none()) {
        given.insert(
            param.name.clone(),
            match param.kind {
                ParamKind::Integer => json!(1),
                _ => json!("dry run"),
            },
        );
    }
    bind_params(&recipe.params, &given)?;
    Ok(given)
}

/// Replays the recipe on the live page through `host` (the Mono's gate: trust level, blocked
/// sites, finance pages and approval cards all apply), stopping before the first step that
/// changes anything, so it never types, clicks a button or sends. A pass is remembered for
/// exactly this content. The recipe must be approved first: the user looked at the steps before
/// anything of it runs in their browser.
pub fn dry_run(
    host: &dyn Host,
    store: &RecipeStore,
    approvals: &MonoBrowserStore,
    mono_id: &str,
    recipe_id: &str,
    options: Options,
) -> Result<DryRunReport, String> {
    let recipe = store
        .get(recipe_id)
        .ok_or("That recipe no longer exists.")?;
    recipe.validate()?;
    if !approvals.recipe_approved(mono_id, &recipe) {
        return Err("Approve the recipe first: a dry run opens its pages in the browser.".into());
    }
    let given = dry_values(&recipe)?;
    let mut options = options;
    options.stop_before_acting = true;
    options.verify_candidates = false;
    let outcome = runner::run(host, &recipe, &given, options);
    let hash = recipe.content_hash();
    host.audit(
        "recipe_dry_run",
        &host.page_url(),
        &format!(
            "recipe:{}@{} dry run {}, {} steps",
            recipe.id,
            recipe.version,
            if outcome.ok() { "ok" } else { "failed" },
            outcome.steps_run
        ),
    );
    if let Some(failure) = &outcome.failure {
        return Ok(DryRunReport {
            ok: false,
            steps_run: outcome.steps_run,
            stopped_before: None,
            failure: Some(format!(
                "Step {} ({}): {}",
                failure.step, failure.op, failure.reason
            )),
        });
    }
    approvals.mark_dry_run(mono_id, &recipe.id, &hash)?;
    Ok(DryRunReport {
        ok: true,
        steps_run: outcome.steps_run,
        stopped_before: outcome.stopped_before,
        failure: None,
    })
}
