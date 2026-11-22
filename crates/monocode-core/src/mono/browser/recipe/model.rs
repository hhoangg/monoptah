//! The recipe file format (JSON, schema 1), its validation and its content hash.
//!
//! A recipe is data the app runs on the user's logged-in browser, so it is treated as code: a
//! closed vocabulary of steps, strict limits, and a hash that an approval is bound to. Phase 1
//! has only read steps (`open`, `wait`, `scroll`, `click` on a link or tab, `extract`).
use crate::mono::browser::gate::{contains_card_number, is_finance_url};
use crate::mono::browser::{is_scroll_key, normalize_origin};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

pub const SCHEMA: u32 = 1;
pub const MAX_STEPS: usize = 30;
pub const MAX_PARAMS: usize = 10;
pub const MAX_CANDIDATES: usize = 8;
pub const MAX_FIELDS: usize = 12;
pub const MAX_TEXT: usize = 500;
pub const MAX_SCROLLS: u32 = 20;
pub const MAX_WAIT_MS: u32 = 15_000;
/// Roles a `click` step may target: they navigate. A button can send, delete or buy, and the gate
/// cannot tell from a click, so Phase 1 records and runs no click on one.
pub const NAVIGATION_ROLES: [&str; 2] = ["link", "tab"];

fn is_false(value: &bool) -> bool {
    !*value
}
fn is_zero(value: &u32) -> bool {
    *value == 0
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Risk {
    /// Reads pages and follows navigation only. The only risk class of Phase 1.
    #[default]
    Read,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ParamKind {
    #[default]
    String,
    Integer,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Param {
    pub name: String,
    #[serde(rename = "type", default)]
    pub kind: ParamKind,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<Value>,
}

/// An ancestor that narrows a candidate: a named landmark or heading.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Scope {
    pub role: String,
    pub name: String,
}

/// One way to find an element. Exactly one of `role`, `label`, `testid`, `text` and `css` is set.
/// `css` is only for the scope of an `extract` step. The snapshot `ref` is never stored: refs
/// change between snapshots.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Candidate {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub testid: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub css: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<Scope>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Target {
    /// Tried in order: the first that matches exactly one element wins.
    pub candidates: Vec<Candidate>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Expect {
    /// The page address matches this regular expression.
    UrlMatches(String),
    /// This text is on the page.
    TextVisible(String),
    /// This text is not on the page.
    TextGone(String),
    /// An element of this role (and name) is on the page.
    RolePresent {
        role: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
    },
    /// An `extract` step found at least this many items.
    CountAtLeast(u32),
}

/// An element inside an item that must be present, by role and a text pattern.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Has {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    /// Regular expression the element's name or text must match.
    pub matches: String,
}

/// Which nodes of the scope are the repeated items.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Each {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub has: Option<Has>,
    /// A regular expression the item's name must not match.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub not_name_matches: Option<String>,
    /// Keep only matches that hold no other match (rows inside a layout table).
    #[serde(default, skip_serializing_if = "is_false")]
    pub innermost: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    /// The accessible name.
    #[default]
    Name,
    /// The text inside the element.
    Text,
    /// The link address (`/url`).
    Url,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Field {
    /// Role of the element inside the item; none means the item itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(default)]
    pub from: Source,
    /// Which match (0 based) when several elements of that role are in the item.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub nth: u32,
    /// The first capture group (or the whole match) of this regular expression is kept.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub regex: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum Step {
    /// `browser_navigate` to a URL inside the recipe's origin.
    Open {
        url: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        expect: Vec<Expect>,
    },
    /// `browser_click` on a link or tab found by its locator.
    Click {
        target: Target,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        expect: Vec<Expect>,
    },
    /// `browser_wait_for` text to appear or disappear, or a short pause.
    Wait {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text_gone: Option<String>,
        #[serde(default, skip_serializing_if = "is_zero")]
        time_ms: u32,
    },
    /// `browser_press_key` with a scroll key, a bounded number of times.
    Scroll { key: String, times: u32 },
    /// Reads the declared fields of a scoped snapshot. Never returns a page dump.
    Extract {
        /// The area to read; none reads the whole page.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        scope: Option<Target>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        each: Option<Each>,
        /// Keep this many items from the end (an integer or `{param}`).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        take_last: Option<String>,
        /// Keep this many items from the start (an integer or `{param}`).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        take_first: Option<String>,
        fields: BTreeMap<String, Field>,
        max_chars_per_field: u32,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        expect: Vec<Expect>,
    },
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Stats {
    pub runs: u32,
    pub ok: u32,
    pub consecutive_ok: u32,
    pub last_ok_ms: u64,
    /// A full run succeeded on the live page (the dry run at record time counts).
    pub verified: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Recipe {
    pub schema: u32,
    pub id: String,
    pub origin: String,
    /// Other origins a step may land on (sign-in redirects). Empty in Phase 1.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub also_origins: Vec<String>,
    pub intent: String,
    pub description: String,
    pub version: u32,
    #[serde(default)]
    pub params: Vec<Param>,
    pub steps: Vec<Step>,
    #[serde(default)]
    pub risk: Risk,
    /// The `@playwright/mcp` version the recipe was recorded on.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub playwright_mcp: String,
    /// Not part of the hash: it changes on every run.
    #[serde(default)]
    pub stats: Stats,
}

// ---- Templates ----

/// How a parameter value is written into a template.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Encode {
    /// As is (names, text).
    Raw,
    /// Percent-encoded (a URL path or query).
    Url,
    /// Escaped for a regular expression.
    Regex,
    /// Escaped for a regular expression that is matched against a page address: the value as
    /// written or percent-encoded.
    UrlRegex,
}

fn placeholder() -> &'static Regex {
    static RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\{([a-z][a-z0-9_]*)\}").expect("placeholder pattern"))
}

/// The parameter names a template refers to.
pub fn placeholders(template: &str) -> Vec<String> {
    placeholder()
        .captures_iter(template)
        .map(|c| c[1].to_owned())
        .collect()
}

pub fn percent_encode(value: &str) -> String {
    let mut out = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// Fills `{name}` placeholders from `values`. An unknown placeholder is left as written.
pub fn substitute(template: &str, values: &BTreeMap<String, String>, encode: Encode) -> String {
    placeholder()
        .replace_all(template, |caps: &regex::Captures| {
            match values.get(&caps[1]) {
                Some(value) => match encode {
                    Encode::Raw => value.clone(),
                    Encode::Url => percent_encode(value),
                    Encode::Regex => regex::escape(value),
                    Encode::UrlRegex => format!(
                        "(?:{}|{})",
                        regex::escape(value),
                        regex::escape(&percent_encode(value))
                    ),
                },
                None => caps[0].to_owned(),
            }
        })
        .into_owned()
}

/// The values for a run: what the caller gave, else each parameter's default. Strings are plain
/// text without control characters; a value that looks like a payment card number is refused (a
/// recipe never carries one).
pub fn bind_params(
    params: &[Param],
    given: &serde_json::Map<String, Value>,
) -> Result<BTreeMap<String, String>, String> {
    for key in given.keys() {
        if !params.iter().any(|p| &p.name == key) {
            return Err(format!("`{key}` is not a parameter of this recipe."));
        }
    }
    let mut out = BTreeMap::new();
    for param in params {
        let value = given
            .get(&param.name)
            .filter(|v| !v.is_null())
            .or(param.default.as_ref())
            .ok_or_else(|| format!("Missing parameter `{}`.", param.name))?;
        let text = match (param.kind, value) {
            (ParamKind::Integer, Value::Number(n)) => n
                .as_i64()
                .filter(|n| (0..=1_000_000).contains(n))
                .map(|n| n.to_string()),
            (ParamKind::Integer, Value::String(s)) => s
                .trim()
                .parse::<i64>()
                .ok()
                .filter(|n| (0..=1_000_000).contains(n))
                .map(|n| n.to_string()),
            (ParamKind::String, Value::String(s)) => Some(s.trim().to_owned()),
            (ParamKind::String, Value::Number(n)) => Some(n.to_string()),
            _ => None,
        }
        .ok_or_else(|| format!("Parameter `{}` has the wrong type or range.", param.name))?;
        if text.chars().count() > 200 || text.chars().any(char::is_control) {
            return Err(format!(
                "Parameter `{}` is too long or has control characters.",
                param.name
            ));
        }
        if contains_card_number(&json!(text)) {
            return Err(format!(
                "Parameter `{}` looks like a card number; recipes never carry one.",
                param.name
            ));
        }
        out.insert(param.name.clone(), text);
    }
    Ok(out)
}

// ---- Validation ----

fn slug_ok(text: &str) -> bool {
    static RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^[a-z][a-z0-9_.-]{0,59}$").expect("slug pattern"))
        .is_match(text)
}
fn param_name_ok(text: &str) -> bool {
    static RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^[a-z][a-z0-9_]{0,29}$").expect("param pattern"))
        .is_match(text)
}
/// A parameter that is a credential by name: a recipe never carries one (Phase 1 has no
/// sensitive parameters at all).
pub fn secret_name(text: &str) -> bool {
    let lower = text.to_lowercase();
    [
        "password",
        "passwd",
        "token",
        "secret",
        "otp",
        "cvv",
        "cvc",
        "pin",
        "apikey",
        "api_key",
        "cookie",
        "session",
        "credential",
        "card",
    ]
    .iter()
    .any(|word| lower.contains(word))
}
/// A Playwright role name: letters only.
fn role_ok(text: &str) -> bool {
    !text.is_empty() && text.len() <= 40 && text.bytes().all(|b| b.is_ascii_lowercase())
}
fn text_ok(text: &str) -> bool {
    !text.is_empty() && text.chars().count() <= MAX_TEXT && !text.chars().any(|c| c.is_control())
}
fn regex_ok(pattern: &str) -> Result<(), String> {
    if !text_ok(pattern) {
        return Err("A pattern is empty, too long or has control characters.".into());
    }
    // Placeholders are replaced before the pattern is compiled; check the shape with a stand-in.
    let probe = placeholder().replace_all(pattern, "x");
    regex::RegexBuilder::new(&probe)
        .size_limit(1 << 20)
        .build()
        .map(|_| ())
        .map_err(|_| format!("`{pattern}` is not a valid pattern."))
}

impl Candidate {
    pub fn kind(&self) -> &'static str {
        if self.role.is_some() {
            "role"
        } else if self.label.is_some() {
            "label"
        } else if self.testid.is_some() {
            "testid"
        } else if self.text.is_some() {
            "text"
        } else if self.css.is_some() {
            "css"
        } else {
            "empty"
        }
    }
    fn validate(&self, allow_css: bool) -> Result<(), String> {
        let kinds = [
            self.role.is_some(),
            self.label.is_some(),
            self.testid.is_some(),
            self.text.is_some(),
            self.css.is_some(),
        ];
        if kinds.iter().filter(|k| **k).count() != 1 {
            return Err(
                "A locator candidate needs exactly one of role, label, testid, text or css.".into(),
            );
        }
        if self.name.is_some() && self.role.is_none() {
            return Err("`name` belongs to a `role` candidate.".into());
        }
        if self.css.is_some() && (!allow_css || self.scope.is_some()) {
            return Err("A `css` candidate is only for the scope of an extract step, without a scope of its own.".into());
        }
        for text in [&self.name, &self.label, &self.testid, &self.text, &self.css]
            .into_iter()
            .flatten()
        {
            if !text_ok(text) {
                return Err("A locator text is empty, too long or has control characters.".into());
            }
        }
        if self.role.as_deref().is_some_and(|r| !role_ok(r)) {
            return Err("A locator role is not a role name.".into());
        }
        if let Some(scope) = &self.scope {
            if !role_ok(&scope.role) || !text_ok(&scope.name) {
                return Err("A locator scope needs a role and a name.".into());
            }
        }
        Ok(())
    }
}

impl Target {
    fn validate(&self, allow_css: bool) -> Result<(), String> {
        if self.candidates.is_empty() || self.candidates.len() > MAX_CANDIDATES {
            return Err(format!(
                "A target needs 1 to {MAX_CANDIDATES} locator candidates."
            ));
        }
        self.candidates
            .iter()
            .try_for_each(|c| c.validate(allow_css))
    }
}

impl Expect {
    fn validate(&self) -> Result<(), String> {
        match self {
            Self::UrlMatches(p) => regex_ok(p),
            Self::TextVisible(t) | Self::TextGone(t) if text_ok(t) => Ok(()),
            Self::RolePresent { role, name }
                if role_ok(role) && name.as_deref().is_none_or(text_ok) =>
            {
                Ok(())
            }
            Self::CountAtLeast(n) if (1..=1000).contains(n) => Ok(()),
            _ => Err("An assertion is malformed.".into()),
        }
    }
}

impl Step {
    pub fn op(&self) -> &'static str {
        match self {
            Self::Open { .. } => "open",
            Self::Click { .. } => "click",
            Self::Wait { .. } => "wait",
            Self::Scroll { .. } => "scroll",
            Self::Extract { .. } => "extract",
        }
    }
    pub fn expects(&self) -> &[Expect] {
        match self {
            Self::Open { expect, .. }
            | Self::Click { expect, .. }
            | Self::Extract { expect, .. } => expect,
            _ => &[],
        }
    }
    /// The strings of this step that may hold `{param}` placeholders.
    fn templates(&self) -> Vec<&str> {
        let mut out: Vec<&str> = Vec::new();
        fn candidates<'a>(t: &'a Target, out: &mut Vec<&'a str>) {
            for c in &t.candidates {
                out.extend(
                    [&c.name, &c.label, &c.testid, &c.text]
                        .into_iter()
                        .flatten()
                        .map(String::as_str),
                );
                if let Some(scope) = &c.scope {
                    out.push(&scope.name);
                }
            }
        }
        match self {
            Self::Open { url, .. } => out.push(url),
            Self::Click { target, .. } => candidates(target, &mut out),
            Self::Wait {
                text, text_gone, ..
            } => {
                out.extend([text, text_gone].into_iter().flatten().map(String::as_str));
            }
            Self::Scroll { .. } => {}
            Self::Extract {
                scope,
                take_last,
                take_first,
                ..
            } => {
                if let Some(scope) = scope {
                    candidates(scope, &mut out);
                }
                out.extend(
                    [take_last, take_first]
                        .into_iter()
                        .flatten()
                        .map(String::as_str),
                );
            }
        }
        for expect in self.expects() {
            match expect {
                Expect::UrlMatches(t) | Expect::TextVisible(t) | Expect::TextGone(t) => out.push(t),
                Expect::RolePresent { name: Some(n), .. } => out.push(n),
                _ => {}
            }
        }
        out
    }
    /// The step in plain words, for the confirm card and `recipe_show`.
    pub fn describe(&self) -> String {
        let locator = |t: &Target| {
            t.candidates
                .first()
                .map(|c| {
                    let what = c
                        .name
                        .as_ref()
                        .or(c.label.as_ref())
                        .or(c.text.as_ref())
                        .or(c.testid.as_ref())
                        .or(c.css.as_ref())
                        .cloned()
                        .unwrap_or_default();
                    match &c.role {
                        Some(role) => format!("the {role} \"{what}\""),
                        None => format!("\"{what}\""),
                    }
                })
                .unwrap_or_default()
        };
        match self {
            Self::Open { url, .. } => format!("Open {url}"),
            Self::Click { target, .. } => format!("Click {}", locator(target)),
            Self::Wait {
                text,
                text_gone,
                time_ms,
            } => match (text, text_gone) {
                (Some(t), _) => format!("Wait for the text \"{t}\""),
                (_, Some(t)) => format!("Wait for the text \"{t}\" to go"),
                _ => format!("Wait {time_ms} ms"),
            },
            Self::Scroll { key, times } => format!("Press {key} {times} time(s) to scroll"),
            Self::Extract {
                scope,
                each,
                take_last,
                take_first,
                fields,
                ..
            } => {
                let names = fields.keys().cloned().collect::<Vec<_>>().join(", ");
                let from = scope
                    .as_ref()
                    .map_or("the page".to_owned(), |t| format!("in {}", locator(t)));
                let items = each
                    .as_ref()
                    .and_then(|e| e.role.clone())
                    .unwrap_or_else(|| "item".into());
                let limit = take_last
                    .as_ref()
                    .map(|n| format!("the last {n} "))
                    .or_else(|| take_first.as_ref().map(|n| format!("the first {n} ")))
                    .unwrap_or_default();
                format!("Read {names} of {limit}{items}s from {from}")
            }
        }
    }
}

impl Recipe {
    /// The lowest trust level that can run every step: a click is not available at Level 1.
    pub fn min_level(&self) -> u8 {
        if self.steps.iter().any(|s| matches!(s, Step::Click { .. })) {
            2
        } else {
            1
        }
    }
    /// Every origin a step may land on.
    pub fn origins(&self) -> Vec<String> {
        std::iter::once(self.origin.clone())
            .chain(self.also_origins.iter().cloned())
            .collect()
    }

    /// Checks the recipe against the schema and the Phase 1 rules. A recipe that fails never runs,
    /// whether the app wrote it or a file was edited by hand.
    pub fn validate(&self) -> Result<(), String> {
        if self.schema != SCHEMA {
            return Err(format!("Recipe schema {} is not supported.", self.schema));
        }
        static ID: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
        if !ID
            .get_or_init(|| Regex::new(r"^rcp_[0-9a-f]{8}$").expect("id pattern"))
            .is_match(&self.id)
        {
            return Err("The recipe id is malformed.".into());
        }
        if !slug_ok(&self.intent) {
            return Err(
                "The intent must be a short lowercase slug such as `news.top_stories`.".into(),
            );
        }
        if self.version == 0 || self.version > 10_000 {
            return Err("The recipe version is out of range.".into());
        }
        if self.description.is_empty() || self.description.chars().count() > 200 {
            return Err("The description must be one short line.".into());
        }
        if self.description.chars().any(char::is_control) {
            return Err("The description must be one short line.".into());
        }
        let origin = normalize_origin(&self.origin).map_err(|_| "The origin is not valid.")?;
        if origin != self.origin {
            return Err("The origin must be written as scheme://host[:port].".into());
        }
        if is_finance_url(&format!("{origin}/"), &[]) {
            return Err("Recipes are not created for bank, wallet or payment sites.".into());
        }
        for other in &self.also_origins {
            if normalize_origin(other).ok().as_deref() != Some(other.as_str())
                || is_finance_url(&format!("{other}/"), &[])
            {
                return Err("An extra origin is not valid.".into());
            }
        }
        if self.params.len() > MAX_PARAMS {
            return Err(format!("At most {MAX_PARAMS} parameters."));
        }
        let mut names = BTreeSet::new();
        for param in &self.params {
            if !param_name_ok(&param.name) || !names.insert(param.name.as_str()) {
                return Err(format!(
                    "Parameter name `{}` is not valid or repeated.",
                    param.name
                ));
            }
            if secret_name(&param.name) {
                return Err(format!(
                    "`{}` looks like a credential. Recipes never carry passwords, tokens, codes or card data.",
                    param.name
                ));
            }
            if param.description.chars().count() > 200 {
                return Err("A parameter description is too long.".into());
            }
            if let Some(default) = &param.default {
                let given = serde_json::Map::from_iter([(param.name.clone(), default.clone())]);
                bind_params(std::slice::from_ref(param), &given)?;
            }
        }
        if self.steps.is_empty() || self.steps.len() > MAX_STEPS {
            return Err(format!("A recipe has 1 to {MAX_STEPS} steps."));
        }
        let mut extracts = 0;
        for (index, step) in self.steps.iter().enumerate() {
            let fail = |reason: String| format!("Step {} ({}): {reason}", index + 1, step.op());
            self.validate_step(step, &names).map_err(fail)?;
            if matches!(step, Step::Extract { .. }) {
                extracts += 1;
            }
        }
        if extracts == 0 {
            return Err("A recipe needs an extract step: it returns the data.".into());
        }
        Ok(())
    }

    fn validate_step(&self, step: &Step, names: &BTreeSet<&str>) -> Result<(), String> {
        for template in step.templates() {
            if !text_ok(template) {
                return Err("A string is empty, too long or has control characters.".into());
            }
            for name in placeholders(template) {
                if !names.contains(name.as_str()) {
                    return Err(format!("`{{{name}}}` is not a declared parameter."));
                }
            }
        }
        for expect in step.expects() {
            expect.validate()?;
        }
        match step {
            Step::Open { url, expect } => {
                if expect.is_empty() {
                    return Err("An open step needs an assertion (for example url_matches).".into());
                }
                let allowed = self.origins().iter().any(|o| {
                    url.strip_prefix(o)
                        .is_some_and(|rest| rest.is_empty() || rest.starts_with(['/', '?', '#']))
                });
                if !allowed {
                    return Err("The address must start with the recipe's origin.".into());
                }
                let probe = substitute(
                    url,
                    &self
                        .params
                        .iter()
                        .map(|p| (p.name.clone(), "x".to_owned()))
                        .collect(),
                    Encode::Url,
                );
                let parsed =
                    url::Url::parse(&probe).map_err(|_| "The address is not a valid URL.")?;
                if !parsed.username().is_empty() || parsed.password().is_some() {
                    return Err("The address must not carry a user name or password.".into());
                }
                if !self
                    .origins()
                    .contains(&parsed.origin().ascii_serialization())
                {
                    return Err("The address leaves the recipe's origin.".into());
                }
            }
            Step::Click { target, expect } => {
                target.validate(false)?;
                if expect.is_empty() {
                    return Err("A click step needs an assertion.".into());
                }
                if !target.candidates.iter().all(|c| {
                    c.role
                        .as_deref()
                        .is_some_and(|r| NAVIGATION_ROLES.contains(&r))
                }) {
                    return Err("Phase 1 clicks only a link or a tab, found by role.".into());
                }
            }
            Step::Wait {
                text,
                text_gone,
                time_ms,
            } => {
                if text.is_none() && text_gone.is_none() && *time_ms == 0 {
                    return Err("A wait step needs text, text_gone or time_ms.".into());
                }
                if *time_ms > MAX_WAIT_MS {
                    return Err(format!("A wait is at most {MAX_WAIT_MS} ms."));
                }
            }
            Step::Scroll { key, times } => {
                if !is_scroll_key(key) {
                    return Err(
                        "A scroll step uses PageDown, PageUp, Home, End, ArrowDown or ArrowUp."
                            .into(),
                    );
                }
                if *times == 0 || *times > MAX_SCROLLS {
                    return Err(format!("Scroll 1 to {MAX_SCROLLS} times."));
                }
            }
            Step::Extract {
                scope,
                each,
                take_last,
                take_first,
                fields,
                max_chars_per_field,
                expect,
            } => {
                if let Some(scope) = scope {
                    scope.validate(true)?;
                }
                if fields.is_empty() || fields.len() > MAX_FIELDS {
                    return Err(format!("An extract step reads 1 to {MAX_FIELDS} fields."));
                }
                if !(1..=2000).contains(max_chars_per_field) {
                    return Err("max_chars_per_field is 1 to 2000.".into());
                }
                if take_last.is_some() && take_first.is_some() {
                    return Err("Use take_last or take_first, not both.".into());
                }
                for count in [take_last, take_first].into_iter().flatten() {
                    let names_ok = placeholders(count).len() == 1
                        && count.starts_with('{')
                        && count.ends_with('}');
                    if !names_ok && count.parse::<u32>().is_err() {
                        return Err(
                            "take_last and take_first are an integer or one {param}.".into()
                        );
                    }
                }
                if !expect.iter().any(|e| matches!(e, Expect::CountAtLeast(_))) {
                    return Err("An extract step needs a count_at_least assertion.".into());
                }
                for name in fields.keys() {
                    if !param_name_ok(name) {
                        return Err(format!("Field name `{name}` is not valid."));
                    }
                }
                for field in fields.values() {
                    if field.role.as_deref().is_some_and(|r| !role_ok(r)) || field.nth > 50 {
                        return Err("A field has a bad role or index.".into());
                    }
                    if let Some(pattern) = &field.regex {
                        regex_ok(pattern)?;
                    }
                }
                if let Some(each) = each {
                    if each.role.as_deref().is_some_and(|r| !role_ok(r)) {
                        return Err("`each` has a bad role.".into());
                    }
                    if let Some(has) = &each.has {
                        regex_ok(&has.matches)?;
                    }
                    if let Some(pattern) = &each.not_name_matches {
                        regex_ok(pattern)?;
                    }
                }
            }
        }
        Ok(())
    }

    // ---- Hash ----

    /// The canonical content: every field but `stats`, keys sorted. What an approval is bound to.
    pub fn canonical(&self) -> String {
        let mut value = serde_json::to_value(self).unwrap_or(Value::Null);
        if let Some(map) = value.as_object_mut() {
            map.remove("stats");
        }
        let mut out = String::new();
        write_canonical(&value, &mut out);
        out
    }
    /// `sha256:<hex>` of [`Self::canonical`].
    pub fn content_hash(&self) -> String {
        let digest = Sha256::digest(self.canonical().as_bytes());
        let mut hex = String::from("sha256:");
        for byte in digest {
            hex.push_str(&format!("{byte:02x}"));
        }
        hex
    }
    pub fn new_id() -> String {
        format!("rcp_{:08x}", rand::random::<u32>())
    }
}

fn write_canonical(value: &Value, out: &mut String) {
    match value {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            out.push('{');
            for (i, key) in keys.into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&Value::String(key.clone()).to_string());
                out.push(':');
                write_canonical(&map[key], out);
            }
            out.push('}');
        }
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_canonical(item, out);
            }
            out.push(']');
        }
        other => out.push_str(&other.to_string()),
    }
}

// ---- Locator selectors ----

/// A Playwright string literal for an exact match: JSON quoting plus the `s` (case sensitive)
/// flag, as the `internal:` selector engines read it.
fn exact(text: &str) -> String {
    format!("{}s", Value::String(text.to_owned()))
}

impl Candidate {
    /// The selector `browser_snapshot` and `browser_click` take as `target`, with `values`
    /// filled in. Role, label and text are exact matches (strict, case sensitive).
    pub fn selector(&self, values: &BTreeMap<String, String>) -> String {
        let fill = |t: &str| substitute(t, values, Encode::Raw);
        let own = if let Some(role) = &self.role {
            match &self.name {
                Some(name) => format!("internal:role={role}[name={}]", exact(&fill(name))),
                None => format!("internal:role={role}"),
            }
        } else if let Some(label) = &self.label {
            format!("internal:label={}", exact(&fill(label)))
        } else if let Some(testid) = &self.testid {
            format!("internal:testid=[data-testid={}]", exact(&fill(testid)))
        } else if let Some(text) = &self.text {
            format!("internal:text={}", exact(&fill(text)))
        } else {
            return self.css.clone().unwrap_or_default();
        };
        match &self.scope {
            Some(scope) => format!(
                "internal:role={}[name={}] >> {own}",
                scope.role,
                exact(&fill(&scope.name))
            ),
            None => own,
        }
    }
    /// Whether the candidate rests on something that changes between visits: a long number or a
    /// generated-looking token in a name, label, test id or text.
    pub fn looks_generated(&self) -> bool {
        [&self.name, &self.label, &self.testid, &self.text]
            .into_iter()
            .flatten()
            .any(|text| generated_looking(text))
    }
}

/// A string with 5 or more digits in a row, or a long hex or mixed token, is not a stable name.
pub fn generated_looking(text: &str) -> bool {
    static RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"\d{5,}|\b[a-z]+-[0-9a-f]{6,}\b|\b[A-Za-z0-9_-]{24,}\b|\b[0-9a-f]{8,}\b")
            .expect("generated pattern")
    })
    .is_match(text)
}
