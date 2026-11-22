//! Replays a recipe. Monoptah code does it, with no model call per step: each step becomes an
//! ordinary `browser_*` call that goes through the same policy as an agent call (trust level,
//! blocked and finance origins, approval cards), via the [`Host`].
//!
//! Elements are found with one scoped probe per locator candidate (`browser_snapshot` with a
//! Playwright `target` and a small `depth`, 20 to 140 tokens, 4 to 25 ms measured), never by
//! parsing a full-page snapshot; the first candidate that matches exactly one element wins. A
//! locator is polled for a few seconds first (auto-wait), because a page may still be loading
//! right after `browser_navigate`. Only the `extract` step reads a snapshot, and only of its
//! scope; Rust parses it and returns the declared fields, never the page.
//!
//! Steps that change things (Phase 3) are enforced here, not left to the recording. Before one
//! runs, the runner works out its risk again from the element it found on the live page (a
//! button that now says "Send" is a send, whatever the file says), reads the trust level again
//! (a level changed since the recording applies now), and asks the user through the gate's own
//! approval tool exactly where `gate::approval_required` says so. A typed value is always a
//! parameter, never goes to a password, code or card field, and is scrubbed from every failure
//! text. An action that already ran is never repeated by a repair.
use super::model::{
    bind_params, governing, substitute, ApproveKind, Candidate, Encode, Expect, FieldKind, Recipe,
    Step, Target, NAVIGATION_ROLES,
};
use super::risk::{self, Class};
use super::snapshot;
use crate::mono::browser::gate::{approval_required, is_finance_url, APPROVAL_TOOL};
use crate::mono::browser::BrowserTrust;
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

/// The most characters of a card's details the gate keeps (`MAX_FIELD_CHARS` there is 2000).
const CARD_DETAILS_MAX: usize = 1_900;

/// What a tool call returned.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolOutput {
    pub text: String,
    pub is_error: bool,
}

/// How a run reaches the browser. The gate implements it (policy, approval cards, one upstream
/// session); tests implement it with a scripted page.
pub trait Host {
    /// Runs one `browser_*` tool as an agent call would run: through the policy and, when it
    /// asks, the user. `Err` is the sentence of a refusal, and it ends the run.
    fn call(&self, tool: &str, args: Value) -> Result<ToolOutput, String>;
    /// The page the browser is on, as the last result reported it.
    fn page_url(&self) -> String;
    fn trust(&self) -> BrowserTrust;
    fn blocked_origins(&self) -> Vec<String>;
    /// Waits between polls. Tests do not sleep.
    fn pause(&self, duration: Duration) {
        std::thread::sleep(duration);
    }
    /// One line for the audit log, tagged with the recipe.
    fn audit(&self, tool: &str, url: &str, summary: &str);
}

#[derive(Clone, Debug)]
pub struct Options {
    /// The cap of the returned data, in characters of JSON.
    pub max_chars: usize,
    /// How long a locator or an assertion is polled.
    pub wait: Duration,
    pub poll: Duration,
    /// Probe every candidate of every click and extract scope, and report which are unique (the
    /// recorder's check). A normal run stops at the first unique one.
    pub verify_candidates: bool,
    /// Start at this step (0 based): a repair re-runs from the failing step, not from the start.
    pub start_at: usize,
    /// What the steps before `start_at` extracted in the run that failed (step index and items).
    pub carried: Vec<(usize, Vec<Value>)>,
    /// The audit tag of every step row; none means `recipe:<id>@<version>`.
    pub audit_tag: Option<String>,
    /// Stop (successfully) before the first step that changes things or asks for an approval:
    /// the record-time check replays the reading part and never types or sends.
    pub stop_before_acting: bool,
}
impl Default for Options {
    fn default() -> Self {
        Self {
            max_chars: 4000,
            wait: Duration::from_secs(5),
            poll: Duration::from_millis(250),
            verify_candidates: false,
            start_at: 0,
            carried: Vec::new(),
            audit_tag: None,
            stop_before_acting: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Failure {
    /// 1 based; 0 for a refusal before the first step.
    pub step: usize,
    pub op: String,
    pub reason: String,
    pub url: String,
    pub expected: String,
    pub observed: String,
    pub needs_login: bool,
    /// The page or an assertion did not match (a locator, a check, a wait that timed out): the
    /// kind of failure a repair can try to fix. A refusal by the policy or a sign-in page is not.
    pub repairable: bool,
    /// The step had already acted (typed, clicked, pressed) when it failed, so repeating it could
    /// act twice: never repairable.
    pub acted: bool,
}

#[derive(Clone, Debug, Default)]
pub struct Outcome {
    pub steps_run: usize,
    pub data: Value,
    pub items: usize,
    pub notes: Vec<String>,
    pub failure: Option<Failure>,
    pub elapsed_ms: u64,
    /// With [`Options::verify_candidates`]: per step index, the candidate indices that matched
    /// exactly one element.
    pub unique: BTreeMap<usize, Vec<usize>>,
    /// What the steps that ran extracted, by step index (kept on a failure so a repair can carry
    /// it into the re-run).
    pub extracts: Vec<(usize, Vec<Value>)>,
    /// The 1 based steps that changed something on the site (the action was sent to the page).
    pub acted: Vec<usize>,
    /// With [`Options::stop_before_acting`]: the 1 based step the run stopped before.
    pub stopped_before: Option<usize>,
}
impl Outcome {
    pub fn ok(&self) -> bool {
        self.failure.is_none()
    }
    /// The compact result the Mono sees. `data` is page content: the caller fences it.
    pub fn result(&self, recipe: &Recipe) -> Value {
        match &self.failure {
            None => json!({
                "ok": true,
                "recipe": recipe.id,
                "intent": recipe.intent,
                "version": recipe.version,
                "steps_run": self.steps_run,
                "items": self.items,
                "ms": self.elapsed_ms,
                "data": self.data,
                "notes": self.notes,
            }),
            Some(f) => {
                let mut result = json!({
                "ok": false,
                "recipe": recipe.id,
                "intent": recipe.intent,
                "version": recipe.version,
                "step": f.step,
                "op": f.op,
                "reason": f.reason,
                "url": f.url,
                "expected": f.expected,
                "observed_summary": f.observed,
                "needs_login": f.needs_login,
                "steps_run": self.steps_run,
                "ms": self.elapsed_ms,
                "notes": self.notes,
                "next": "Do this task the normal way (snapshot and click) and tell the user the recipe failed.",
                });
                if !self.acted.is_empty() {
                    // The Mono must not redo what already happened (a message sent twice).
                    result["changed_things"] = json!(self.acted);
                    result["next"] = json!(format!(
                        "Steps {} already changed things on the site (typed, clicked or sent). Look at the page before you do anything: do NOT repeat a step that may already have gone through (never send or post twice). Tell the user what happened.",
                        self.acted.iter().map(usize::to_string).collect::<Vec<_>>().join(", ")
                    ));
                }
                result
            }
        }
    }
}

/// What the live page showed for the element a step acts on.
struct Resolved {
    selector: String,
    role: String,
    name: String,
}

enum Probe {
    Unique(Resolved),
    Multiple,
    Missing,
}

pub fn origin_of(url: &str) -> Option<String> {
    url::Url::parse(url)
        .ok()
        .map(|u| u.origin().ascii_serialization())
}

fn clip(text: &str, max: usize) -> String {
    let one_line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    one_line.chars().take(max).collect()
}

struct Runner<'a> {
    host: &'a dyn Host,
    recipe: &'a Recipe,
    values: BTreeMap<String, String>,
    opts: Options,
    outcome: Outcome,
    tag: String,
    /// The `approve` steps the user answered yes to in this run, with what they were for.
    granted: BTreeMap<usize, ApproveKind>,
    /// The values that get typed or selected: scrubbed from every text that leaves the runner.
    secrets: Vec<String>,
}

/// Runs `recipe` with `given` parameters. Never panics on page content.
pub fn run(host: &dyn Host, recipe: &Recipe, given: &Map<String, Value>, opts: Options) -> Outcome {
    let started = Instant::now();
    let tag = opts
        .audit_tag
        .clone()
        .unwrap_or_else(|| format!("recipe:{}@{}", recipe.id, recipe.version));
    let mut runner = Runner {
        host,
        recipe,
        values: BTreeMap::new(),
        opts,
        outcome: Outcome {
            data: Value::Null,
            ..Outcome::default()
        },
        tag,
        granted: BTreeMap::new(),
        secrets: Vec::new(),
    };
    let failure = runner.run_all(given);
    runner.outcome.failure = failure.map(|mut f| {
        // The browser echoes what it typed ("Ran Playwright code: fill('...')"): never pass a typed
        // value on in a failure.
        let secrets = &runner.secrets;
        f.reason = risk::scrub(&f.reason, secrets);
        f.expected = risk::scrub(&f.expected, secrets);
        f.observed = risk::scrub(&f.observed, secrets);
        f.url = risk::scrub(&f.url, secrets);
        *f
    });
    let secrets = runner.secrets.clone();
    runner.outcome.notes = runner
        .outcome
        .notes
        .iter()
        .map(|n| risk::scrub(n, &secrets))
        .collect();
    runner.outcome.elapsed_ms = started.elapsed().as_millis() as u64;
    runner.outcome
}

impl Runner<'_> {
    fn refuse(&self, step: usize, op: &str, reason: impl Into<String>) -> Box<Failure> {
        Box::new(Failure {
            step,
            op: op.to_owned(),
            reason: reason.into(),
            url: self.host.page_url(),
            expected: String::new(),
            observed: String::new(),
            needs_login: false,
            repairable: false,
            acted: false,
        })
    }

    fn run_all(&mut self, given: &Map<String, Value>) -> Option<Box<Failure>> {
        let recipe = self.recipe;
        match bind_params(&recipe.params, given) {
            Ok(values) => self.values = values,
            Err(reason) => return Some(self.refuse(0, "params", reason)),
        }
        let trust = self.host.trust();
        if recipe.min_level() > trust.level() {
            return Some(self.refuse(
                0,
                "level",
                format!(
                    "This recipe {}, which is not available at Level {} (read only). Raise the browser level or do the task by hand.",
                    if recipe.changes_things() {
                        "changes things on the site (types, clicks a button or sends)"
                    } else {
                        "clicks"
                    },
                    trust.level()
                ),
            ));
        }
        self.secrets = recipe
            .typed_params()
            .into_iter()
            .filter_map(|name| self.values.get(name).cloned())
            .collect();
        let blocked = self.host.blocked_origins();
        if recipe
            .origins()
            .iter()
            .any(|o| blocked.contains(o) || is_finance_url(&format!("{o}/"), &blocked))
        {
            return Some(self.refuse(
                0,
                "origin",
                "This recipe's site is on the Mono's blocked list or is a bank or payment site, so recipes do not run there.",
            ));
        }
        let mut extracts = self.opts.carried.clone();
        let start = self.opts.start_at.min(recipe.steps.len());
        for (index, step) in recipe.steps.iter().enumerate().skip(start) {
            let number = index + 1;
            if self.opts.stop_before_acting
                && (step.is_acting() || matches!(step, Step::Approve { .. }))
            {
                self.outcome.stopped_before = Some(number);
                break;
            }
            // One audit row per step, however many probes and polls it took.
            let failure = self.step(index, number, step, &mut extracts).or_else(|| {
                self.outcome.steps_run = number;
                self.stay_inside(number, step.op())
            });
            let verdict = if failure.is_some() { "failed" } else { "ok" };
            // Typed and selected values are never written to the log.
            let masked = if matches!(
                step,
                Step::Type { .. } | Step::Fill { .. } | Step::Select { .. }
            ) {
                " (values masked)"
            } else {
                ""
            };
            self.host.audit(
                step.op(),
                &self.host.page_url(),
                &format!("{} step {number} {} {verdict}{masked}", self.tag, step.op()),
            );
            if failure.is_some() {
                self.outcome.extracts = extracts;
                return failure;
            }
        }
        self.outcome.extracts = extracts.clone();
        self.finish(extracts);
        None
    }

    /// The page must stay on the recipe's origins after every step, and never be on a blocked or
    /// finance page. An address that does not parse fails: it can not be shown to be safe.
    fn stay_inside(&self, number: usize, op: &str) -> Option<Box<Failure>> {
        let url = self.host.page_url();
        let Some(origin) = origin_of(&url) else {
            return Some(self.refuse(
                number,
                op,
                "The page address could not be read, so the run stops.",
            ));
        };
        let blocked = self.host.blocked_origins();
        let mut reason = None;
        if !self.recipe.origins().contains(&origin) {
            reason = Some(format!(
                "The page left the recipe's site (now on {origin})."
            ));
        } else if blocked.contains(&origin) || is_finance_url(&url, &blocked) {
            reason = Some(
                "The page is on a blocked, bank or payment site, so the run stops.".to_owned(),
            );
        }
        reason.map(|reason| {
            let mut failure = self.refuse(number, op, reason);
            failure.needs_login = looks_like_login(&url);
            failure
        })
    }

    fn finish(&mut self, extracts: Vec<(usize, Vec<Value>)>) {
        let total: usize = extracts.iter().map(|(_, items)| items.len()).sum();
        self.outcome.items = total;
        let mut parts: Vec<(usize, Vec<Value>)> = extracts;
        let cap = self.opts.max_chars;
        let size = |parts: &[(usize, Vec<Value>)]| {
            parts
                .iter()
                .map(|(_, items)| Value::Array(items.clone()).to_string().len())
                .sum::<usize>()
        };
        let mut dropped = 0;
        while size(&parts) > cap {
            // Drop from the longest list's end until the data fits.
            let Some(longest) = parts
                .iter_mut()
                .filter(|(_, items)| items.len() > 1)
                .max_by_key(|(_, items)| items.len())
            else {
                break;
            };
            longest.1.pop();
            dropped += 1;
        }
        if dropped > 0 {
            self.outcome.notes.push(format!(
                "Result cut to fit {cap} characters: {dropped} item(s) left out."
            ));
        }
        self.outcome.data = match parts.len() {
            0 => Value::Null,
            1 => Value::Array(parts.remove(0).1),
            _ => Value::Object(
                parts
                    .into_iter()
                    .enumerate()
                    .map(|(i, (_, items))| (format!("extract_{}", i + 1), Value::Array(items)))
                    .collect(),
            ),
        };
    }

    // ---- Calling the browser ----

    fn call(
        &self,
        number: usize,
        op: &str,
        tool: &str,
        args: Value,
    ) -> Result<ToolOutput, Box<Failure>> {
        self.host
            .call(tool, args)
            .map_err(|reason| self.refuse(number, op, reason))
    }

    fn observed(&self) -> String {
        let url = self.host.page_url();
        clip(&url, 200)
    }

    // ---- Steps ----

    fn step(
        &mut self,
        index: usize,
        number: usize,
        step: &Step,
        extracts: &mut Vec<(usize, Vec<Value>)>,
    ) -> Option<Box<Failure>> {
        let op = step.op();
        let result = match step {
            Step::Open { url, expect } => self.open(number, url, expect),
            Step::Click { target, expect } => self.click(index, number, target, expect),
            Step::Wait {
                text,
                text_gone,
                time_ms,
            } => self.wait(number, text, text_gone, *time_ms),
            Step::Scroll { key, times } => self.scroll(number, key, *times),
            Step::Approve { category } => self.approve(index, number, *category),
            Step::Act { target, expect } => self.click_acting(index, number, step, target, expect),
            Step::Send { target, expect, .. } => {
                self.click_acting(index, number, step, target, expect)
            }
            Step::Type {
                target,
                value,
                expect,
            } => self.type_text(index, number, step, target, value, expect),
            Step::Select {
                target,
                values,
                expect,
            } => self.select(index, number, step, target, values, expect),
            Step::Fill { fields, expect } => self.fill(index, number, step, fields, expect),
            Step::Press { key, expect } => self.press(index, number, step, key, expect),
            Step::Extract { .. } => match self.extract(index, number, step) {
                Ok(items) => {
                    extracts.push((index, items));
                    Ok(())
                }
                Err(failure) => Err(failure),
            },
        };
        result.err().map(|mut failure| {
            failure.op = op.to_owned();
            failure
        })
    }

    fn open(&mut self, number: usize, url: &str, expect: &[Expect]) -> Result<(), Box<Failure>> {
        let url = substitute(url, &self.values, Encode::Url);
        let inside = origin_of(&url).is_some_and(|o| self.recipe.origins().contains(&o));
        if !inside {
            return Err(self.refuse(number, "open", "The address is outside the recipe's site."));
        }
        let output = self.call(number, "open", "browser_navigate", json!({"url": url}))?;
        if output.is_error {
            return Err(self.refuse(number, "open", clip(&output.text, 300)));
        }
        self.assert_all(number, "open", expect, None)
    }

    fn click(
        &mut self,
        index: usize,
        number: usize,
        target: &Target,
        expect: &[Expect],
    ) -> Result<(), Box<Failure>> {
        let resolved = self.resolve(index, number, "click", target, true)?;
        // A link or tab whose live name says it sends, pays or removes something is not a plain
        // read: it takes the approval a step of that kind takes.
        if let Some(live) = risk::name_class(&resolved.name) {
            let recipe = self.recipe;
            self.guard_action(index, number, &recipe.steps[index], Some(live), &resolved)?;
        }
        let label = format!("{} {}", resolved.role, clip(&resolved.name, 60));
        let output = self.call(
            number,
            "click",
            "browser_click",
            json!({"target": resolved.selector, "element": label}),
        )?;
        if output.is_error {
            let mut failure = self.refuse(number, "click", clip(&output.text, 300));
            failure.needs_login = looks_like_login(&failure.url);
            failure.repairable = !failure.needs_login;
            return Err(failure);
        }
        self.assert_all(number, "click", expect, None)
    }

    fn wait(
        &mut self,
        number: usize,
        text: &Option<String>,
        text_gone: &Option<String>,
        time_ms: u32,
    ) -> Result<(), Box<Failure>> {
        let mut args = Map::new();
        if let Some(text) = text {
            args.insert(
                "text".into(),
                substitute(text, &self.values, Encode::Raw).into(),
            );
        }
        if let Some(text) = text_gone {
            args.insert(
                "textGone".into(),
                substitute(text, &self.values, Encode::Raw).into(),
            );
        }
        if time_ms > 0 {
            args.insert("time".into(), json!(f64::from(time_ms) / 1000.0));
        }
        let output = self.call(number, "wait", "browser_wait_for", Value::Object(args))?;
        if output.is_error {
            let mut failure = self.refuse(number, "wait", clip(&output.text, 300));
            failure.expected = text
                .clone()
                .or_else(|| text_gone.clone())
                .unwrap_or_default();
            failure.needs_login = looks_like_login(&failure.url);
            failure.repairable = !failure.needs_login;
            return Err(failure);
        }
        Ok(())
    }

    // ---- Steps that change things ----

    /// Asks the user through the gate's own approval tool: the same card, the same timeout and
    /// the same fail-closed rule as when the Mono asks. `Err` ends the run.
    fn ask_user(
        &self,
        number: usize,
        op: &str,
        kind: ApproveKind,
        steps: &[usize],
        target: Option<&str>,
    ) -> Result<(), Box<Failure>> {
        let mut lines: Vec<String> = steps
            .iter()
            .map(|i| {
                format!(
                    "Step {}: {}",
                    i + 1,
                    self.recipe.steps[*i].describe_with(&self.values)
                )
            })
            .collect();
        // A step that sends or pays sends everything typed on this page with it: list those
        // values too, also when a repair restarted the run after the steps that typed them.
        let mut also: Vec<usize> = Vec::new();
        for i in steps {
            // A card that sends or pays (by its kind, or by what the live page showed) lists what
            // was typed before it.
            if kind == ApproveKind::Other && self.recipe.steps[*i].risk() < super::model::Risk::Send
            {
                continue;
            }
            for j in (0..*i).rev() {
                let before = &self.recipe.steps[j];
                if matches!(before, Step::Open { .. } | Step::Click { .. }) {
                    break;
                }
                if matches!(
                    before,
                    Step::Type { .. } | Step::Fill { .. } | Step::Select { .. }
                ) && !steps.contains(&j)
                    && !also.contains(&j)
                {
                    also.push(j);
                }
            }
        }
        also.sort_unstable();
        for j in also {
            lines.push(format!(
                "Also typed on this page, sent with it: step {}: {}",
                j + 1,
                self.recipe.steps[j].describe_with(&self.values)
            ));
        }
        if let Some(target) = target {
            lines.push(format!("On the page now: {target}"));
        }
        let details = format!(
            "Recipe `{}` v{} will do this on {}:\n{}",
            self.recipe.intent,
            self.recipe.version,
            self.recipe.origin,
            lines.join("\n")
        );
        if details.chars().count() > CARD_DETAILS_MAX {
            return Err(self.refuse(
                number,
                op,
                "There is too much to show on one approval card, so it is not run. Do this task by hand.",
            ));
        }
        let first = steps.first().map_or(0, |i| *i);
        let action = format!(
            "Run the saved recipe `{}`: {}",
            self.recipe.intent,
            clip(&self.recipe.steps[first].describe_with(&self.values), 160)
        );
        let output = self
            .host
            .call(
                APPROVAL_TOOL,
                json!({
                    "category": kind.name(),
                    "action": action,
                    "url": self.host.page_url(),
                    "details": details,
                }),
            )
            .map_err(|reason| self.refuse(number, op, reason))?;
        if output.is_error {
            return Err(self.refuse(number, op, clip(&output.text, 300)));
        }
        Ok(())
    }

    /// An `approve` step: the card for the acting steps it guards, with their real values. A
    /// level that does not ask for this kind drops it (Level 3 for a send, a post or a plain
    /// write); money always asks.
    fn approve(
        &mut self,
        index: usize,
        number: usize,
        kind: ApproveKind,
    ) -> Result<(), Box<Failure>> {
        let trust = self.host.trust();
        if trust.level() < 2 {
            return Err(self.refuse(
                number,
                "approve",
                "Level 1 (read only) can not run a step that changes things.",
            ));
        }
        if !approval_required(trust, kind.category()) {
            self.outcome.notes.push(format!(
                "Step {number}: no approval asked, Level {} does not ask for it.",
                trust.level()
            ));
            return Ok(());
        }
        let guarded: Vec<usize> = governing(&self.recipe.steps)
            .iter()
            .enumerate()
            .filter(|(_, g)| **g == Some(index))
            .map(|(i, _)| i)
            // A repair restarts in the middle of a block: the steps before are done.
            .filter(|i| *i >= self.opts.start_at)
            .collect();
        if guarded.is_empty() {
            return Ok(());
        }
        self.ask_user(number, "approve", kind, &guarded, None)?;
        self.granted.insert(index, kind);
        Ok(())
    }

    /// The last check before a step acts: the trust level now, and the class the step has on the
    /// live page (never lower than the one recorded). Asks the user when the level asks for this
    /// class and no approve step of this run covers it.
    fn guard_action(
        &mut self,
        index: usize,
        number: usize,
        step: &Step,
        live: Option<Class>,
        resolved: &Resolved,
    ) -> Result<(), Box<Failure>> {
        let op = step.op();
        let trust = self.host.trust();
        if trust.level() < 2 {
            return Err(self.refuse(
                number,
                op,
                "Level 1 (read only) can not run a step that changes things.",
            ));
        }
        let class = step.class().max(live.unwrap_or(Class::READ));
        if !approval_required(trust, class.category) {
            return Ok(());
        }
        let covered = governing(&self.recipe.steps)
            .get(index)
            .copied()
            .flatten()
            .and_then(|approve| self.granted.get(&approve))
            .is_some_and(|kind| kind.covers(class.category));
        if covered {
            return Ok(());
        }
        // Not covered by an approval of this run (a repair restarted after it, or the page now
        // shows a stronger kind of action than was recorded): ask here, for this step alone.
        let target = format!("the {} \"{}\"", resolved.role, clip(&resolved.name, 80));
        self.ask_user(
            number,
            op,
            ApproveKind::from_category(class.category),
            &[index],
            Some(&target),
        )
    }

    /// Marks that the action was sent to the page, so a failure from here on is never repaired by
    /// running the step again.
    fn acted(&mut self, number: usize) {
        self.outcome.acted.push(number);
    }
    fn after_action(failure: Box<Failure>) -> Box<Failure> {
        let mut failure = failure;
        failure.acted = true;
        failure.repairable = false;
        failure
    }
    /// The failure of an action call that the browser reported as an error: never the raw text
    /// (it echoes what was typed).
    fn action_error(&self, number: usize, op: &str, text: &str) -> Box<Failure> {
        let mut failure = self.refuse(
            number,
            op,
            format!(
                "The browser reported an error: {}",
                clip(&risk::scrub(text, &self.secrets), 200)
            ),
        );
        failure.needs_login = looks_like_login(&failure.url);
        Self::after_action(failure)
    }

    /// `act` and `send`: a click that changes something.
    fn click_acting(
        &mut self,
        index: usize,
        number: usize,
        step: &Step,
        target: &Target,
        expect: &[Expect],
    ) -> Result<(), Box<Failure>> {
        let op = step.op();
        let resolved = self.resolve(index, number, op, target, false)?;
        let live = risk::label_class(&resolved.role, &resolved.name);
        self.guard_action(index, number, step, Some(live), &resolved)?;
        let label = format!("{} {}", resolved.role, clip(&resolved.name, 60));
        let output = self.call(
            number,
            op,
            "browser_click",
            json!({"target": resolved.selector, "element": label}),
        )?;
        self.acted(number);
        if output.is_error {
            return Err(self.action_error(number, op, &output.text));
        }
        self.assert_all(number, op, expect, None)
            .map_err(Self::after_action)
    }

    /// The live element must be the kind of field the step types into, and not a place for a
    /// credential: the page may have changed since the recording.
    fn check_field(
        &self,
        number: usize,
        op: &str,
        resolved: &Resolved,
        roles: &[&str],
    ) -> Result<(), Box<Failure>> {
        if !roles.contains(&resolved.role.as_str()) {
            return Err(self.refuse(
                number,
                op,
                format!(
                    "The element found is a {}, not the {} the recipe types into, so nothing was typed.",
                    resolved.role,
                    roles.join(" or ")
                ),
            ));
        }
        if risk::secret_field(&resolved.name) {
            return Err(self.refuse(
                number,
                op,
                "This field takes a password, a code or card data. A recipe never types those: do this step by hand.",
            ));
        }
        Ok(())
    }

    /// The value of a typed or selected parameter, refused if it is a credential: bound values
    /// were already checked, this is the last look before the page gets it.
    fn typed_value(&self, number: usize, op: &str, template: &str) -> Result<String, Box<Failure>> {
        let value = substitute(template, &self.values, Encode::Raw);
        if let Some(what) = risk::secret_value(&value) {
            return Err(self.refuse(
                number,
                op,
                format!("The value for this field looks like {what}, which a recipe never types."),
            ));
        }
        Ok(value)
    }

    /// The values typed, filled or selected since the last page change, before step `index`.
    fn earlier_values(&self, index: usize) -> Vec<String> {
        let fill = |t: &str| substitute(t, &self.values, Encode::Raw);
        let mut values: Vec<String> = Vec::new();
        for before in self.recipe.steps[..index].iter().rev() {
            let mut here: Vec<String> = match before {
                Step::Open { .. } | Step::Click { .. } => break,
                Step::Type { value, .. } => vec![fill(value)],
                Step::Select { values, .. } => values.iter().map(|v| fill(v)).collect(),
                Step::Fill { fields, .. } => fields
                    .iter()
                    .filter(|f| !f.kind.is_toggle())
                    .map(|f| fill(&f.value))
                    .collect(),
                _ => continue,
            };
            here.reverse();
            values.extend(here);
        }
        values.reverse();
        values
    }

    /// A card number split over several fields is still a card number: the values typed since the
    /// last page change, this one included, are checked together.
    fn split_card_guard(
        &self,
        number: usize,
        op: &str,
        index: usize,
        text: &str,
    ) -> Result<(), Box<Failure>> {
        let mut values = self.earlier_values(index);
        values.push(text.to_owned());
        let refs: Vec<&str> = values.iter().map(String::as_str).collect();
        if risk::split_card(&refs) {
            return Err(self.refuse(
                number,
                op,
                "The values typed on this page add up to a payment card number, which a recipe never types.",
            ));
        }
        Ok(())
    }

    fn type_text(
        &mut self,
        index: usize,
        number: usize,
        step: &Step,
        target: &Target,
        value: &str,
        expect: &[Expect],
    ) -> Result<(), Box<Failure>> {
        let resolved = self.resolve(index, number, "type", target, false)?;
        self.check_field(
            number,
            "type",
            &resolved,
            &["textbox", "searchbox", "combobox", "spinbutton"],
        )?;
        let text = self.typed_value(number, "type", value)?;
        self.split_card_guard(number, "type", index, &text)?;
        self.guard_action(index, number, step, None, &resolved)?;
        let label = format!("{} {}", resolved.role, clip(&resolved.name, 60));
        // Never `submit`: sending is the `send` step or a `press`, each with its own approval.
        let output = self.call(
            number,
            "type",
            "browser_type",
            json!({"target": resolved.selector, "element": label, "text": text}),
        )?;
        self.acted(number);
        if output.is_error {
            return Err(self.action_error(number, "type", &output.text));
        }
        self.assert_all(number, "type", expect, None)
            .map_err(Self::after_action)
    }

    fn select(
        &mut self,
        index: usize,
        number: usize,
        step: &Step,
        target: &Target,
        values: &[String],
        expect: &[Expect],
    ) -> Result<(), Box<Failure>> {
        let resolved = self.resolve(index, number, "select", target, false)?;
        self.check_field(number, "select", &resolved, &["combobox", "listbox"])?;
        let mut chosen = Vec::new();
        for value in values {
            chosen.push(self.typed_value(number, "select", value)?);
        }
        let earlier = self.earlier_values(index);
        let mut all: Vec<&str> = earlier.iter().map(String::as_str).collect();
        all.extend(chosen.iter().map(String::as_str));
        if risk::split_card(&all) {
            return Err(self.refuse(
                number,
                "select",
                "The values on this page add up to a payment card number, which a recipe never types.",
            ));
        }
        let live = risk::label_class(&resolved.role, &resolved.name);
        self.guard_action(index, number, step, Some(live), &resolved)?;
        let label = format!("{} {}", resolved.role, clip(&resolved.name, 60));
        let output = self.call(
            number,
            "select",
            "browser_select_option",
            json!({"target": resolved.selector, "element": label, "values": chosen}),
        )?;
        self.acted(number);
        if output.is_error {
            return Err(self.action_error(number, "select", &output.text));
        }
        self.assert_all(number, "select", expect, None)
            .map_err(Self::after_action)
    }

    fn fill(
        &mut self,
        index: usize,
        number: usize,
        step: &Step,
        fields: &[super::model::FormField],
        expect: &[Expect],
    ) -> Result<(), Box<Failure>> {
        let mut prepared = Vec::new();
        let mut last: Option<Resolved> = None;
        let mut live = Class::WRITE;
        for field in fields {
            let resolved = self.resolve(index, number, "fill", &field.target, false)?;
            let roles: &[&str] = match field.kind {
                FieldKind::Textbox => &["textbox", "searchbox", "spinbutton"],
                FieldKind::Checkbox => &["checkbox", "switch"],
                FieldKind::Radio => &["radio"],
                FieldKind::Combobox => &["combobox", "listbox"],
                FieldKind::Slider => &["slider"],
            };
            self.check_field(number, "fill", &resolved, roles)?;
            let value = if field.kind.is_toggle() {
                json!(field.value == "true")
            } else {
                json!(self.typed_value(number, "fill", &field.value)?)
            };
            prepared.push(json!({
                "name": clip(&resolved.name, 60),
                "type": field.kind.name(),
                "target": resolved.selector,
                "value": value,
            }));
            live = live.max(risk::label_class(&resolved.role, &resolved.name));
            last = Some(resolved);
        }
        let Some(last) = last else {
            return Err(self.refuse(number, "fill", "A fill step needs fields."));
        };
        let earlier = self.earlier_values(index);
        let mut texts: Vec<&str> = earlier.iter().map(String::as_str).collect();
        texts.extend(prepared.iter().filter_map(|f| f["value"].as_str()));
        if risk::split_card(&texts) {
            return Err(self.refuse(
                number,
                "fill",
                "The values of this form add up to a payment card number, which a recipe never types.",
            ));
        }
        self.guard_action(index, number, step, Some(live), &last)?;
        let output = self.call(
            number,
            "fill",
            "browser_fill_form",
            json!({"fields": prepared}),
        )?;
        self.acted(number);
        if output.is_error {
            return Err(self.action_error(number, "fill", &output.text));
        }
        self.assert_all(number, "fill", expect, None)
            .map_err(Self::after_action)
    }

    fn press(
        &mut self,
        index: usize,
        number: usize,
        step: &Step,
        key: &str,
        expect: &[Expect],
    ) -> Result<(), Box<Failure>> {
        let none = Resolved {
            selector: String::new(),
            role: "page".into(),
            name: format!("key {key}"),
        };
        self.guard_action(index, number, step, None, &none)?;
        let output = self.call(number, "press", "browser_press_key", json!({"key": key}))?;
        self.acted(number);
        if output.is_error {
            return Err(self.action_error(number, "press", &output.text));
        }
        self.assert_all(number, "press", expect, None)
            .map_err(Self::after_action)
    }

    fn scroll(&mut self, number: usize, key: &str, times: u32) -> Result<(), Box<Failure>> {
        for _ in 0..times {
            let output = self.call(number, "scroll", "browser_press_key", json!({"key": key}))?;
            if output.is_error {
                return Err(self.refuse(number, "scroll", clip(&output.text, 300)));
            }
        }
        Ok(())
    }

    // ---- Locators ----

    fn probe(
        &self,
        number: usize,
        op: &str,
        candidate: &Candidate,
        click: bool,
    ) -> Result<Probe, Box<Failure>> {
        let selector = candidate.selector(&self.values);
        let output = self.call(
            number,
            op,
            "browser_snapshot",
            json!({"target": selector, "depth": 1}),
        )?;
        if output.is_error {
            let multiple = output.text.contains("strict mode violation");
            return Ok(if multiple {
                Probe::Multiple
            } else {
                Probe::Missing
            });
        }
        let forest = snapshot::parse(snapshot::snapshot_body(&output.text));
        let Some(node) = forest.first() else {
            return Ok(Probe::Missing);
        };
        if click && !NAVIGATION_ROLES.contains(&node.role.as_str()) {
            // Whatever the locator found, it is not the link or tab the recipe means.
            return Ok(Probe::Missing);
        }
        Ok(Probe::Unique(Resolved {
            selector,
            role: node.role.clone(),
            name: node.name.clone(),
        }))
    }

    /// The first candidate that matches exactly one element, polling until `wait` has passed.
    fn resolve(
        &mut self,
        index: usize,
        number: usize,
        op: &str,
        target: &Target,
        click: bool,
    ) -> Result<Resolved, Box<Failure>> {
        let deadline = Instant::now() + self.opts.wait;
        loop {
            let mut multiple = 0;
            let mut found: Option<Resolved> = None;
            let mut unique = Vec::new();
            for (ci, candidate) in target.candidates.iter().enumerate() {
                match self.probe(number, op, candidate, click)? {
                    Probe::Unique(resolved) => {
                        unique.push(ci);
                        if found.is_none() {
                            found = Some(resolved);
                        }
                        if !self.opts.verify_candidates {
                            break;
                        }
                    }
                    Probe::Multiple => multiple += 1,
                    Probe::Missing => {}
                }
            }
            if let Some(found) = found {
                self.outcome.unique.insert(index, unique);
                return Ok(found);
            }
            if Instant::now() >= deadline {
                let mut failure = self.refuse(
                    number,
                    op,
                    if multiple > 0 {
                        "No locator candidate matched exactly one element (some matched several)."
                    } else {
                        "No locator candidate matched an element."
                    },
                );
                failure.expected = target
                    .candidates
                    .first()
                    .map(|c| clip(&c.selector(&self.values), 200))
                    .unwrap_or_default();
                failure.observed = self.observed();
                failure.needs_login = looks_like_login(&failure.url);
                failure.repairable = !failure.needs_login;
                return Err(failure);
            }
            self.host.pause(self.opts.poll);
        }
    }

    // ---- Assertions ----

    fn assert_all(
        &mut self,
        number: usize,
        op: &str,
        expect: &[Expect],
        items: Option<usize>,
    ) -> Result<(), Box<Failure>> {
        let deadline = Instant::now() + self.opts.wait;
        loop {
            let mut failed: Option<String> = None;
            for assertion in expect {
                if let Err(what) = self.check(number, op, assertion, items)? {
                    failed = Some(what);
                    break;
                }
            }
            let Some(what) = failed else {
                return Ok(());
            };
            if Instant::now() >= deadline || items.is_some() {
                let url = self.host.page_url();
                return Err(Box::new(Failure {
                    step: number,
                    op: op.to_owned(),
                    reason: "A check after the step failed.".into(),
                    needs_login: looks_like_login(&url),
                    repairable: !looks_like_login(&url),
                    url,
                    expected: clip(&what, 300),
                    observed: self.observed(),
                    acted: false,
                }));
            }
            self.host.pause(self.opts.poll);
        }
    }

    /// `Ok(Ok(()))` holds, `Ok(Err(text))` does not hold (say what was expected), `Err` ends the
    /// run (a refusal).
    fn check(
        &self,
        number: usize,
        op: &str,
        assertion: &Expect,
        items: Option<usize>,
    ) -> Result<Result<(), String>, Box<Failure>> {
        Ok(match assertion {
            Expect::UrlMatches(pattern) => {
                let filled = substitute(pattern, &self.values, Encode::UrlRegex);
                let re = regex::RegexBuilder::new(&filled)
                    .size_limit(1 << 20)
                    .build()
                    .map_err(|_| self.refuse(number, op, "A URL pattern is not valid."))?;
                let mut url = self.host.page_url();
                if !re.is_match(&url) {
                    // The page may have moved on its own since the last result.
                    let output = self.call(number, op, "browser_snapshot", json!({"depth": 1}))?;
                    url = snapshot::page_header(&output.text).0;
                    if url.is_empty() {
                        url = self.host.page_url();
                    }
                }
                if re.is_match(&url) {
                    Ok(())
                } else {
                    Err(format!("the address to match {filled}"))
                }
            }
            Expect::TextVisible(text) | Expect::TextGone(text) => {
                let filled = substitute(text, &self.values, Encode::Raw);
                let output = self.call(
                    number,
                    op,
                    "browser_find",
                    json!({"text": filled, "maxResults": 1}),
                )?;
                let present = !output.is_error && !output.text.contains("No matches found");
                match (assertion, present) {
                    (Expect::TextVisible(_), true) | (Expect::TextGone(_), false) => Ok(()),
                    (Expect::TextVisible(_), false) => {
                        Err(format!("the text \"{filled}\" on the page"))
                    }
                    _ => Err(format!("the text \"{filled}\" gone from the page")),
                }
            }
            Expect::RolePresent { role, name } => {
                let candidate = Candidate {
                    role: Some(role.clone()),
                    name: name.clone(),
                    ..Candidate::default()
                };
                let selector = candidate.selector(&self.values);
                let output = self.call(
                    number,
                    op,
                    "browser_snapshot",
                    json!({"target": selector, "depth": 1}),
                )?;
                // Several matches (a strict-mode error) still mean the role is present.
                if !output.is_error || output.text.contains("strict mode violation") {
                    Ok(())
                } else {
                    Err(format!("a {role} on the page"))
                }
            }
            Expect::CountAtLeast(n) => match items {
                Some(count) if count >= *n as usize => Ok(()),
                Some(count) => Err(format!("at least {n} items (found {count})")),
                None => Ok(()),
            },
        })
    }

    // ---- Extract ----

    fn extract(
        &mut self,
        index: usize,
        number: usize,
        step: &Step,
    ) -> Result<Vec<Value>, Box<Failure>> {
        let Step::Extract {
            scope,
            each,
            take_last,
            take_first,
            fields,
            max_chars_per_field,
            expect,
        } = step
        else {
            return Ok(Vec::new());
        };
        let minimum = expect
            .iter()
            .filter_map(|e| match e {
                Expect::CountAtLeast(n) => Some(*n as usize),
                _ => None,
            })
            .max()
            .unwrap_or(1);
        let limit = |value: &Option<String>| -> Option<usize> {
            value.as_ref().and_then(|v| {
                substitute(v, &self.values, Encode::Raw)
                    .parse::<usize>()
                    .ok()
                    .map(|n| n.min(500))
            })
        };
        let (last, first) = (limit(take_last), limit(take_first));
        let deadline = Instant::now() + self.opts.wait;
        loop {
            let selector = match scope {
                Some(target) => Some(
                    self.resolve(index, number, "extract", target, false)?
                        .selector,
                ),
                None => None,
            };
            let args = match &selector {
                Some(selector) => json!({"target": selector}),
                None => json!({}),
            };
            let output = self.call(number, "extract", "browser_snapshot", args)?;
            let mut items = Vec::new();
            if !output.is_error {
                let forest = snapshot::parse(snapshot::snapshot_body(&output.text));
                let picked = snapshot::items(&forest, each.as_ref(), &self.values)
                    .map_err(|reason| self.refuse(number, "extract", reason))?;
                let mut rows: Vec<Value> = Vec::new();
                for node in picked {
                    let mut row = Map::new();
                    for (name, field) in fields {
                        let value =
                            snapshot::field_value(node, field, *max_chars_per_field as usize)
                                .map_err(|reason| self.refuse(number, "extract", reason))?;
                        row.insert(name.clone(), value.map_or(Value::Null, Value::String));
                    }
                    // An item with no value at all is layout, not data.
                    if row
                        .values()
                        .any(|v| v.as_str().is_some_and(|s| !s.is_empty()))
                    {
                        rows.push(Value::Object(row));
                    }
                }
                items = rows;
                if let Some(n) = last {
                    let skip = items.len().saturating_sub(n);
                    items.drain(..skip);
                } else if let Some(n) = first {
                    items.truncate(n);
                }
            }
            if items.len() >= minimum || Instant::now() >= deadline {
                self.assert_all(number, "extract", expect, Some(items.len()))?;
                return Ok(items);
            }
            self.host.pause(self.opts.poll);
        }
    }
}

/// A page that asks the user to sign in: reported as `needs_login` so the Mono tells the user
/// rather than exploring a login form.
pub fn looks_like_login(url: &str) -> bool {
    let lower = url.to_lowercase();
    [
        "/login",
        "/signin",
        "/sign-in",
        "/sign_in",
        "/sso",
        "/auth",
        "accounts.",
        "login.",
    ]
    .iter()
    .any(|part| lower.contains(part))
}
