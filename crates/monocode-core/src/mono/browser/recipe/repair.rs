//! Self-healing: what a repair may change, and the rules that keep a repair from becoming an
//! attack.
//!
//! The repair "LLM call" is the Mono's own turn, not a model call of ours: when a step fails the
//! runner collects a small observation and hands it to the Mono together with a one-use token;
//! the Mono answers with ONE patch (`recipe_repair`). That works the same for the Claude and the
//! Codex harness, needs no model client or key in core, and every browser call of the re-run still
//! goes through the gate. The patch is data from a model that has read page text, so it is treated
//! as hostile:
//!
//! * a closed schema (`deny_unknown_fields`): it can name no step, origin, parameter or code;
//! * it changes the failing step only (an inserted step goes right before it) and everything else
//!   must stay byte-identical ([`others_unchanged`]);
//! * a new locator keeps the role and the kind of name of the old one ([`same_class`]);
//! * only a same-class locator fix of a read step and a wait run under the earlier approval; a new
//!   step or a changed assertion needs the user's card ([`Class::NeedsApproval`]);
//! * the patched recipe must pass the full [`Recipe::validate`], so no `evaluate`, no origin
//!   outside the recipe's, no click on a button.
use super::model::{
    placeholders, substitute, Candidate, Encode, Expect, Recipe, Stats, Step, Target, MAX_WAIT_MS,
    NAVIGATION_ROLES,
};
use super::runner::Host;
use super::store::RunEffect;
use serde::Deserialize;
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Repair attempts per failed run.
pub const MAX_ATTEMPTS: u32 = 2;
/// Repair attempts per recipe per day (UTC). A flapping site stops costing tokens after this.
pub const DAILY_CAP: u32 = 5;
/// Runs kept in the rolling window of a recipe.
pub const WINDOW: usize = 20;
/// A recipe needs this many runs in the window before its fail rate counts.
pub const FLAKY_MIN_RUNS: usize = 5;
/// Fail rate above this share of the window (percent) marks a recipe flaky.
pub const FLAKY_PERCENT: usize = 30;
/// Clean runs in a row that end `repaired` and a forced `flaky`.
pub const CLEAN_RUNS: u32 = 3;
/// Failed runs in a row after an accepted repair that undo it.
pub const ROLLBACK_AFTER: u32 = 2;
/// Characters of page observation handed to the Mono.
pub const OBSERVATION_CAP: usize = 6000;
/// How long a repair offer stays valid.
pub const PENDING_TTL: Duration = Duration::from_secs(15 * 60);
const MAX_PENDING: usize = 8;
const DAY_MS: u64 = 86_400_000;

// ---- Stats: flaky, repaired, caps ----

impl Stats {
    /// Counts one run. `blame_repair` is false for a failure that says nothing about a repair
    /// (the browser was on a sign-in page).
    pub fn note_run(&mut self, ok: bool, blame_repair: bool) -> RunEffect {
        self.runs += 1;
        self.window.push(ok);
        while self.window.len() > WINDOW {
            self.window.remove(0);
        }
        if ok {
            self.ok += 1;
            self.consecutive_ok += 1;
            self.last_ok_ms = crate::time::now_ms();
            self.verified = true;
            self.failed_since_repair = 0;
            if self.repaired {
                self.clean_since_repair += 1;
                if self.clean_since_repair >= CLEAN_RUNS {
                    self.repaired = false;
                    self.clean_since_repair = 0;
                }
            }
        } else {
            self.consecutive_ok = 0;
            if self.repaired && blame_repair {
                self.failed_since_repair += 1;
            }
        }
        if self.fail_rate_high() {
            self.flaky = true;
        } else if ok && self.consecutive_ok >= CLEAN_RUNS {
            self.flaky = false;
        }
        RunEffect {
            rollback_due: self.repaired && self.failed_since_repair >= ROLLBACK_AFTER,
        }
    }
    /// More than [`FLAKY_PERCENT`] of the last runs failed.
    pub fn fail_rate_high(&self) -> bool {
        let fails = self.window.iter().filter(|ok| !**ok).count();
        self.window.len() >= FLAKY_MIN_RUNS && fails * 100 > FLAKY_PERCENT * self.window.len()
    }
    /// Repair attempts still allowed on the day of `now_ms`.
    pub fn repairs_left(&self, now_ms: u64) -> u32 {
        let used = if self.repair_day == now_ms / DAY_MS {
            self.repairs_today
        } else {
            0
        };
        DAILY_CAP.saturating_sub(used)
    }
    /// Counts one repair attempt today.
    pub fn count_repair(&mut self, now_ms: u64) {
        let day = now_ms / DAY_MS;
        if self.repair_day != day {
            self.repair_day = day;
            self.repairs_today = 0;
        }
        self.repairs_today += 1;
    }
}

// ---- The patch ----

/// The one change the Mono may propose for the failing step. Anything else does not parse.
#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum Patch {
    /// New locator candidates for the failing click or extract scope.
    ReplaceCandidates { candidates: Vec<Candidate> },
    /// Wait for text (or a short time) before the failing step.
    AddWait {
        #[serde(default)]
        text: Option<String>,
        #[serde(default)]
        text_gone: Option<String>,
        #[serde(default)]
        time_ms: u32,
    },
    /// A wait, a scroll or a click on a link or tab, right before the failing step (dismiss a
    /// banner). Needs the user's approval: it is a new step.
    InsertStep { step: Step },
    /// New assertions for the failing step. Needs the user's approval: it changes what is checked.
    ChangeAssertion { expect: Vec<Expect> },
    /// Stop repairing: the Mono does the task by hand.
    GiveUp {
        #[serde(default)]
        reason: String,
    },
}

impl Patch {
    pub fn name(&self) -> &'static str {
        match self {
            Self::ReplaceCandidates { .. } => "replace_candidates",
            Self::AddWait { .. } => "add_wait",
            Self::InsertStep { .. } => "insert_step",
            Self::ChangeAssertion { .. } => "change_assertion",
            Self::GiveUp { .. } => "give_up",
        }
    }
}

/// Whether a repaired version still runs under the approval the user gave the old one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Class {
    /// A same-class locator fix of a read-only step, or a wait: shown in the history only.
    SameClass,
    /// The user must approve the new content first (the reason is shown on the card).
    NeedsApproval(&'static str),
}

pub struct Applied {
    /// The recipe with the patch (the caller sets version, revision and stats).
    pub recipe: Recipe,
    /// The step to re-run from (0 based): the first one the patch touched.
    pub restart: usize,
    /// The failing step in `recipe` (0 based): the re-run has to get past it.
    pub target: usize,
    pub class: Class,
    /// What changed, in words and without any parameter value.
    pub summary: String,
}

/// A step that may change something on the site: it is a write or a send, not a read. Phase 1
/// has only link and tab clicks as acting steps; the file format can not hold another kind yet,
/// but a click on anything else counts as one so a repair of it is never silent.
pub fn changes_things(step: &Step) -> bool {
    match step {
        Step::Click { target, .. } => target.candidates.iter().any(|c| {
            c.role
                .as_deref()
                .is_none_or(|r| !NAVIGATION_ROLES.contains(&r))
        }),
        _ => false,
    }
}

/// How a patch of step `index` is approved. A step that changes things, or that sits next to an
/// `approve` step (Phase 3 adds them), never runs repaired under the earlier approval.
pub fn classify(recipe: &Recipe, index: usize, patch: &Patch) -> Class {
    let step = &recipe.steps[index];
    match patch {
        Patch::InsertStep { .. } => Class::NeedsApproval("it adds a step"),
        Patch::ChangeAssertion { .. } => Class::NeedsApproval("it changes what the run checks"),
        Patch::ReplaceCandidates { .. } | Patch::AddWait { .. } => {
            if changes_things(step) {
                Class::NeedsApproval("the step changes things on the site")
            } else {
                Class::SameClass
            }
        }
        Patch::GiveUp { .. } => Class::SameClass,
    }
}

/// Applies `patch` to step `index` of `base`. Never touches `base`.
pub fn apply(base: &Recipe, index: usize, patch: &Patch, level: u8) -> Result<Applied, String> {
    let step = base.steps.get(index).ok_or("The failing step is gone.")?;
    let class = classify(base, index, patch);
    let mut recipe = base.clone();
    let (restart, target, summary) = match patch {
        Patch::GiveUp { .. } => return Err("give_up is not a change.".into()),
        Patch::ReplaceCandidates { candidates } => {
            let old = match step {
                Step::Click { target, .. } => &target.candidates,
                Step::Extract {
                    scope: Some(target),
                    ..
                } => &target.candidates,
                _ => return Err("This step has no locator to replace.".into()),
            };
            if candidates.is_empty() {
                return Err("A locator needs at least one candidate.".into());
            }
            for new in candidates {
                if new.looks_generated() {
                    return Err("A generated-looking name or id is not a stable locator.".into());
                }
                if !old.iter().any(|o| same_class(o, new)) {
                    return Err("The new locator must keep the role and the kind of name of the old one (for example a link named like the old link). A different role or a different name is not a repair.".into());
                }
            }
            match &mut recipe.steps[index] {
                Step::Click { target, .. }
                | Step::Extract {
                    scope: Some(target),
                    ..
                } => {
                    *target = Target {
                        candidates: candidates.clone(),
                    }
                }
                _ => unreachable!("checked above"),
            }
            (
                index,
                index,
                format!(
                    "step {}: {} new locator candidate(s), same role and kind of name",
                    index + 1,
                    candidates.len()
                ),
            )
        }
        Patch::AddWait {
            text,
            text_gone,
            time_ms,
        } => {
            if text.is_none() && text_gone.is_none() && *time_ms == 0 {
                return Err("A wait needs text, text_gone or time_ms.".into());
            }
            if *time_ms > MAX_WAIT_MS {
                return Err(format!("A wait is at most {MAX_WAIT_MS} ms."));
            }
            recipe.steps.insert(
                index,
                Step::Wait {
                    text: text.clone(),
                    text_gone: text_gone.clone(),
                    time_ms: *time_ms,
                },
            );
            (
                index,
                index + 1,
                format!("a wait added before step {}", index + 1),
            )
        }
        Patch::InsertStep { step: new } => {
            match new {
                Step::Wait { .. } | Step::Scroll { .. } => {}
                Step::Click { target, .. } => {
                    if level < 2 {
                        return Err("A click is not available at Level 1.".into());
                    }
                    if !target.candidates.iter().all(|c| {
                        c.role
                            .as_deref()
                            .is_some_and(|r| NAVIGATION_ROLES.contains(&r))
                    }) {
                        return Err(
                            "An inserted click may only target a link or a tab, found by role."
                                .into(),
                        );
                    }
                }
                _ => {
                    return Err("insert_step may add a wait, a scroll or a click on a link or tab. It can not add an open (no navigation), an extract or anything that types or runs code.".into());
                }
            }
            let what = new.op();
            recipe.steps.insert(index, new.clone());
            (
                index,
                index + 1,
                format!("a {what} step added before step {}", index + 1),
            )
        }
        Patch::ChangeAssertion { expect } => {
            match &mut recipe.steps[index] {
                Step::Open { expect: e, .. }
                | Step::Click { expect: e, .. }
                | Step::Extract { expect: e, .. } => *e = expect.clone(),
                _ => return Err("This step has no assertion.".into()),
            }
            (
                index,
                index,
                format!("step {}: assertions changed", index + 1),
            )
        }
    };
    others_unchanged(
        base,
        &recipe,
        index,
        matches!(patch, Patch::AddWait { .. } | Patch::InsertStep { .. }),
    )?;
    recipe.validate().map_err(|reason| {
        format!("The patched recipe is not valid, so it was not run: {reason}")
    })?;
    Ok(Applied {
        recipe,
        restart,
        target,
        class,
        summary,
    })
}

/// A repair changes one step. Identity, origins, parameters, risk and every other step must be
/// exactly what the user approved: a patch can not smuggle in an origin, a parameter use, a new
/// step elsewhere or a removed `approve` step.
pub fn others_unchanged(
    base: &Recipe,
    next: &Recipe,
    index: usize,
    inserted: bool,
) -> Result<(), String> {
    let same_frame = base.id == next.id
        && base.origin == next.origin
        && base.also_origins == next.also_origins
        && base.intent == next.intent
        && base.description == next.description
        && base.params == next.params
        && base.risk == next.risk
        && base.playwright_mcp == next.playwright_mcp;
    let mut kept: Vec<&Step> = next.steps.iter().collect();
    if inserted {
        if kept.len() != base.steps.len() + 1 {
            return Err("A repair adds one step at most.".into());
        }
        kept.remove(index);
    } else if kept.len() != base.steps.len() {
        return Err("A repair keeps the number of steps.".into());
    }
    let same_steps = kept.iter().zip(&base.steps).enumerate().all(|(i, (a, b))| {
        // Without an insertion the patched step itself is the one allowed to differ.
        (!inserted && i == index) || *a == b
    });
    if same_frame && same_steps {
        Ok(())
    } else {
        Err("A repair may change the failing step only.".into())
    }
}

// ---- Same role, same kind of name ----

fn words(text: &str) -> BTreeSet<String> {
    let mut plain = String::new();
    let mut rest = text;
    // Placeholders are not words.
    while let Some(start) = rest.find('{') {
        plain.push_str(&rest[..start]);
        rest = match rest[start..].find('}') {
            Some(end) => &rest[start + end + 1..],
            None => "",
        };
    }
    plain.push_str(rest);
    plain
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty() && !w.chars().all(|c| c.is_ascii_digit()))
        .map(str::to_owned)
        .collect()
}

/// The text a candidate looks for.
fn needle(candidate: &Candidate) -> Option<&String> {
    candidate
        .name
        .as_ref()
        .or(candidate.label.as_ref())
        .or(candidate.testid.as_ref())
        .or(candidate.text.as_ref())
}

/// Whether `new` may stand in for `old`: the same kind of locator and the same role, and a name
/// of the same kind: the same parameters and no word the old name did not have (case, spacing and
/// punctuation do not count; words may be dropped). No list of "dangerous" words is relied on.
pub fn same_class(old: &Candidate, new: &Candidate) -> bool {
    if old.kind() != new.kind() || old.role != new.role {
        return false;
    }
    match (needle(old), needle(new)) {
        (None, None) => true,
        (Some(o), Some(n)) => name_class_ok(o, n),
        _ => false,
    }
}

fn name_class_ok(old: &str, new: &str) -> bool {
    let old_params: BTreeSet<String> = placeholders(old).into_iter().collect();
    let new_params: BTreeSet<String> = placeholders(new).into_iter().collect();
    if old_params != new_params {
        return false;
    }
    let (old_words, new_words) = (words(old), words(new));
    new_words.is_subset(&old_words) && old_words.is_empty() == new_words.is_empty()
}

// ---- Pending repairs (server side) ----

/// A failed run waiting for the Mono's patch. Lives in memory in the gate: the Mono holds a token
/// only, so it can not choose another recipe, version or step.
pub struct Pending {
    pub token: String,
    pub recipe_id: String,
    pub base_version: u32,
    pub base_hash: String,
    /// The recipe with the patches accepted so far in this repair (the saved one is untouched).
    pub working: Recipe,
    /// The failing step in `working` (0 based).
    pub step: usize,
    /// The page the failure left the browser on: the re-run starts from it.
    pub page: String,
    /// The run's parameters (memory only).
    pub given: Map<String, Value>,
    pub carried: Vec<(usize, Vec<Value>)>,
    pub attempts_left: u32,
    pub attempts_used: u32,
    pub changes: Vec<String>,
    /// A patch that needed the user's card was approved in this repair.
    pub user_approved: bool,
    pub max_chars: usize,
    pub created: Instant,
}

pub fn new_token() -> String {
    format!("rpr_{:032x}", rand::random::<u128>())
}

pub struct Book {
    ttl: Duration,
    items: Mutex<Vec<Pending>>,
}
impl Default for Book {
    fn default() -> Self {
        Self::with_ttl(PENDING_TTL)
    }
}
impl Book {
    pub fn with_ttl(ttl: Duration) -> Self {
        Self {
            ttl,
            items: Mutex::new(Vec::new()),
        }
    }
    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<Pending>> {
        self.items.lock().unwrap_or_else(|e| e.into_inner())
    }
    /// One offer per recipe: a new failure replaces the old offer.
    pub fn put(&self, pending: Pending) {
        let mut items = self.lock();
        items.retain(|p| p.recipe_id != pending.recipe_id);
        items.push(pending);
        while items.len() > MAX_PENDING {
            items.remove(0);
        }
    }
    /// Takes the offer (it is used up) unless it expired.
    pub fn take(&self, token: &str) -> Option<Pending> {
        let mut items = self.lock();
        let at = items.iter().position(|p| p.token == token)?;
        let pending = items.remove(at);
        (pending.created.elapsed() < self.ttl).then_some(pending)
    }
    pub fn forget(&self, recipe_id: &str) {
        self.lock().retain(|p| p.recipe_id != recipe_id);
    }
    pub fn len(&self) -> usize {
        self.lock().len()
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

// ---- The partial observation ----

const FENCE: &str = "untrusted_page_observation";

/// Wraps page text so it reads as data: the fence name inside the text is defused.
pub fn fence(text: &str) -> String {
    static TAG: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let tag = TAG.get_or_init(|| {
        regex::Regex::new(r"(?i)untrusted[\s_-]*page[\s_-]*observation").expect("fence pattern")
    });
    let safe = tag.replace_all(text, "untrusted-page-observation");
    format!("<{FENCE}>\n{safe}\n</{FENCE}>")
}

/// The text the failing step looks for (a name, a label, some text), with parameters filled in.
pub fn expected_name(step: &Step, values: &BTreeMap<String, String>) -> Option<String> {
    let fill = |t: &str| substitute(t, values, Encode::Raw);
    let from_target = |t: &Target| t.candidates.iter().find_map(needle).map(|n| fill(n));
    let from_expect = |expect: &[Expect]| {
        expect.iter().find_map(|e| match e {
            Expect::TextVisible(t) => Some(fill(t)),
            Expect::RolePresent { name: Some(n), .. } => Some(fill(n)),
            _ => None,
        })
    };
    match step {
        Step::Click { target, expect } => from_target(target).or_else(|| from_expect(expect)),
        Step::Extract { scope, expect, .. } => scope
            .as_ref()
            .and_then(from_target)
            .or_else(|| from_expect(expect)),
        Step::Open { expect, .. } => from_expect(expect),
        Step::Wait { text, .. } => text.as_deref().map(fill),
        Step::Scroll { .. } => None,
    }
    .filter(|n| !n.is_empty())
}

/// A small look at the page around the failing step, never the whole page: `browser_find` around
/// the expected name, else a depth-limited snapshot of the nearest named ancestor, else only the
/// top-level outline (depth 1). Capped at [`OBSERVATION_CAP`] characters. Every call goes through
/// the host, so the gate's rules apply to it as to any browser call. The result is page data:
/// the caller fences it.
pub fn observe(
    host: &dyn Host,
    recipe: &Recipe,
    step: usize,
    values: &BTreeMap<String, String>,
) -> String {
    let Some(failing) = recipe.steps.get(step) else {
        return String::new();
    };
    let mut parts: Vec<String> = Vec::new();
    if let Some(name) = expected_name(failing, values) {
        if let Ok(out) = host.call("browser_find", json!({"text": name, "maxResults": 6})) {
            if !out.is_error
                && !out.text.contains("No matches found")
                && !out.text.trim().is_empty()
            {
                parts.push(format!("Matches for the text \"{name}\":\n{}", out.text));
            }
        }
    }
    if parts.is_empty() {
        let scope = match failing {
            Step::Click { target, .. } => Some(target),
            Step::Extract { scope, .. } => scope.as_ref(),
            _ => None,
        }
        .and_then(|t| t.candidates.iter().find_map(|c| c.scope.clone()));
        if let Some(scope) = scope {
            let ancestor = Candidate {
                role: Some(scope.role),
                name: Some(scope.name),
                ..Candidate::default()
            };
            let selector = ancestor.selector(values);
            if let Ok(out) = host.call("browser_snapshot", json!({"target": selector, "depth": 3}))
            {
                if !out.is_error && !out.text.trim().is_empty() {
                    parts.push(format!("Nearest named area (3 levels):\n{}", out.text));
                }
            }
        }
    }
    if parts.is_empty() {
        if let Ok(out) = host.call("browser_snapshot", json!({"depth": 1})) {
            if !out.is_error {
                parts.push(format!("Top-level outline of the page:\n{}", out.text));
            }
        }
    }
    parts.join("\n\n").chars().take(OBSERVATION_CAP).collect()
}
