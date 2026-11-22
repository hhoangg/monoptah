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
use super::model::{
    bind_params, substitute, Candidate, Encode, Expect, Recipe, Step, Target, NAVIGATION_ROLES,
};
use super::snapshot;
use crate::mono::browser::gate::is_finance_url;
use crate::mono::browser::BrowserTrust;
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

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
            Some(f) => json!({
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
            }),
        }
    }
}

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
    };
    let failure = runner.run_all(given);
    runner.outcome.failure = failure.map(|f| *f);
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
                    "This recipe clicks, which is not available at Level {} (read only). Raise the browser level or do the task by hand.",
                    trust.level()
                ),
            ));
        }
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
            // One audit row per step, however many probes and polls it took.
            let failure = self.step(index, number, step, &mut extracts).or_else(|| {
                self.outcome.steps_run = number;
                self.stay_inside(number, step.op())
            });
            let verdict = if failure.is_some() { "failed" } else { "ok" };
            self.host.audit(
                step.op(),
                &self.host.page_url(),
                &format!("{} step {number} {} {verdict}", self.tag, step.op()),
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
