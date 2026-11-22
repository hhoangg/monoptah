//! The recipe file format (JSON, schema 1), its validation and its content hash.
//!
//! A recipe is data the app runs on the user's logged-in browser, so it is treated as code: a
//! closed vocabulary of steps, strict limits, and a hash that an approval is bound to. Phase 1
//! has read steps (`open`, `wait`, `scroll`, `click` on a link or tab, `extract`); Phase 3 adds
//! the steps that change things (`act`, `type`, `fill`, `select`, `press`, `send`) and the
//! `approve` step that the user's card belongs to. Every acting step is guarded by an `approve`
//! step (see [`governing`]), its typed values are always `{parameters}` (never literals), and
//! the risk of a recipe is computed from its steps, never read from the file.
use super::risk::{self, Class};
use crate::mono::browser::gate::{contains_card_number, is_finance_url, Category};
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

/// What a step or a recipe can do, from the least to the most. A recipe's risk is the highest of
/// its steps and is computed, not trusted (see [`Recipe::computed_risk`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Risk {
    /// Reads pages and follows navigation only.
    #[default]
    Read,
    /// Types, selects or clicks something that changes a page (a draft, a setting).
    Write,
    /// Sends, posts, comments or publishes: it leaves the user's hands.
    Send,
    /// Pays, buys, subscribes or transfers.
    Money,
    /// Runs code. No recipe step does (the format has no such step); it is here so the
    /// classification is complete and a Level 3 rule has a name for it.
    Code,
}
impl Risk {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Write => "write",
            Self::Send => "send",
            Self::Money => "money",
            Self::Code => "code",
        }
    }
}

/// What the user is asked about on an `approve` step: the agent-chosen categories of the gate's
/// `browser_request_approval`, with the same names.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApproveKind {
    Payment,
    SendMessage,
    PostPublish,
    Other,
}
impl ApproveKind {
    pub fn category(self) -> Category {
        match self {
            Self::Payment => Category::Payment,
            Self::SendMessage => Category::SendMessage,
            Self::PostPublish => Category::PostPublish,
            Self::Other => Category::Other,
        }
    }
    pub fn from_category(category: Category) -> Self {
        match category {
            Category::Payment => Self::Payment,
            Category::SendMessage => Self::SendMessage,
            Category::PostPublish => Self::PostPublish,
            _ => Self::Other,
        }
    }
    /// The name `browser_request_approval` takes.
    pub fn name(self) -> &'static str {
        match self {
            Self::Payment => "payment",
            Self::SendMessage => "send_message",
            Self::PostPublish => "post_publish",
            Self::Other => "other",
        }
    }
    /// Whether an approval of this kind satisfies one `needed`: the same kind, or a payment
    /// approval (the strictest).
    pub fn covers(self, needed: Category) -> bool {
        self == Self::Payment || self.category() == needed
    }
}

/// Which way a `send` step leaves the user's hands.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SendAction {
    /// A message, an email, a direct message.
    Send,
    /// A post, a comment, a review, a publication.
    Post,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FieldKind {
    #[default]
    Textbox,
    Checkbox,
    Radio,
    Combobox,
    Slider,
}
impl FieldKind {
    pub fn name(self) -> &'static str {
        match self {
            Self::Textbox => "textbox",
            Self::Checkbox => "checkbox",
            Self::Radio => "radio",
            Self::Combobox => "combobox",
            Self::Slider => "slider",
        }
    }
    /// A checkbox or a radio button holds a fixed `true` or `false`; the others a `{parameter}`.
    pub fn is_toggle(self) -> bool {
        matches!(self, Self::Checkbox | Self::Radio)
    }
}

/// One field of a `fill` step.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FormField {
    pub target: Target,
    #[serde(rename = "type", default)]
    pub kind: FieldKind,
    /// A single `{parameter}`, or `true` or `false` for a checkbox or a radio button.
    pub value: String,
}

/// The keys a `press` step may send: the keys that scroll have their own `scroll` step.
pub const PRESS_KEYS: [&str; 8] = [
    "Enter",
    "Escape",
    "Tab",
    "Backspace",
    "Delete",
    "ArrowLeft",
    "ArrowRight",
    "Space",
];
pub const MAX_FORM_FIELDS: usize = 10;
pub const MAX_SELECT_VALUES: usize = 5;

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
    /// `browser_click` on any element that changes something (a button, a checkbox, a menu
    /// item). The target never holds a `{parameter}`: a value must not choose what is clicked.
    Act {
        target: Target,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        expect: Vec<Expect>,
    },
    /// `browser_type` a `{parameter}` into a field. Never a literal, never Enter (`press`).
    Type {
        target: Target,
        value: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        expect: Vec<Expect>,
    },
    /// `browser_fill_form`: several fields at once.
    Fill {
        fields: Vec<FormField>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        expect: Vec<Expect>,
    },
    /// `browser_select_option` with `{parameter}` values.
    Select {
        target: Target,
        values: Vec<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        expect: Vec<Expect>,
    },
    /// `browser_press_key` with a key that does not scroll. Enter and Space count as sending.
    Press {
        key: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        expect: Vec<Expect>,
    },
    /// The click that sends or posts (`browser_click`): explicit, so the card and the rules can
    /// name it. It always needs the user's approval at Level 2.
    Send {
        target: Target,
        action: SendAction,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        expect: Vec<Expect>,
    },
    /// The user's approval (`browser_request_approval`) for the acting steps it guards: see
    /// [`governing`]. At Level 3 an approval that Level 3 does not ask for is dropped at run time.
    Approve { category: ApproveKind },
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

/// Where a version came from. Bookkeeping like [`Stats`]: not part of the hash.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Revision {
    /// The version this one was made from.
    pub parent: u32,
    /// `record`, `repair` or `rollback`.
    pub reason: String,
    /// What changed, in words (never a parameter value).
    pub summary: String,
    pub at_ms: u64,
}

/// Bookkeeping that changes after the user approved a recipe, so it is NOT part of the hash (see
/// [`Recipe::canonical`]). It decides nothing about what a recipe does.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Stats {
    pub runs: u32,
    pub ok: u32,
    pub consecutive_ok: u32,
    pub last_ok_ms: u64,
    /// When the last run ended, passed or failed (the Browser tab shows it).
    pub last_run_ms: u64,
    /// A full run succeeded on the live page (the dry run at record time counts). Bookkeeping in
    /// the file: a recipe that changes things is trusted only by the store database
    /// (`MonoBrowserStore::is_verified`), which a file edit can not touch.
    pub verified: bool,
    /// The result of the last runs, oldest first (at most `repair::WINDOW`): true is a pass.
    pub window: Vec<bool>,
    /// Failed too often lately (or was rolled back after a bad repair): ranked last, verify it.
    pub flaky: bool,
    /// The current version came from a repair and has not had enough clean runs yet.
    pub repaired: bool,
    pub clean_since_repair: u32,
    /// Failed runs in a row since the repair.
    pub failed_since_repair: u32,
    /// Days since the epoch of the repair counter below.
    pub repair_day: u64,
    /// Repair attempts today: the per-day cap that stops a flapping site from burning tokens.
    pub repairs_today: u32,
    pub revision: Option<Revision>,
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
        if let Some(what) = risk::secret_value(&text) {
            return Err(format!(
                "Parameter `{}` looks like {what}; recipes never carry credentials.",
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
        "passcode",
        "passphrase",
        "2fa",
        "mfa",
        "totp",
        "verification",
        "security_code",
        "login_code",
        "one_time",
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

/// The locator of a step that changes things: found by role, label or test id (never by a bare
/// text or a CSS selector), and never with a `{parameter}` in it. A value from the Mono or from
/// a page must not be able to choose what gets clicked or typed into.
fn acting_target(target: &Target) -> Result<(), String> {
    target.validate(false)?;
    for candidate in &target.candidates {
        if candidate.text.is_some() && candidate.role.is_none() {
            return Err(
                "A step that changes things is found by role, label or test id, not by text alone."
                    .into(),
            );
        }
        let strings = [
            &candidate.name,
            &candidate.label,
            &candidate.testid,
            &candidate.text,
        ]
        .into_iter()
        .flatten()
        .chain(candidate.scope.as_ref().map(|s| &s.name));
        for text in strings {
            if !placeholders(text).is_empty() {
                return Err("The locator of a step that changes things holds no {parameter}: a value must not choose the target.".into());
            }
        }
    }
    Ok(())
}

/// A field that takes typed text is not a password, code or card field.
fn typed_target(target: &Target) -> Result<(), String> {
    let secret = target
        .candidates
        .iter()
        .flat_map(|c| [&c.name, &c.label, &c.testid, &c.text])
        .flatten()
        .any(|text| risk::secret_field(text));
    if secret {
        return Err("This field takes a password, a code or card data. A recipe never types those: do this step by hand.".into());
    }
    Ok(())
}

/// A typed value is exactly one `{parameter}`: never a literal, never text around it.
fn single_param(value: &str) -> Result<(), String> {
    let names = placeholders(value);
    if names.len() == 1 && value == format!("{{{}}}", names[0]) {
        Ok(())
    } else {
        Err("A typed or selected value is exactly one {parameter}, never a literal.".into())
    }
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

impl Target {
    /// The first candidate in words: `the link "Archive"`.
    pub fn describe(&self) -> String {
        self.candidates
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
                    Some(role) if what.is_empty() => format!("the {role}"),
                    Some(role) => format!("the {role} \"{what}\""),
                    None => format!("\"{what}\""),
                }
            })
            .unwrap_or_default()
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
            Self::Act { .. } => "act",
            Self::Type { .. } => "type",
            Self::Fill { .. } => "fill",
            Self::Select { .. } => "select",
            Self::Press { .. } => "press",
            Self::Send { .. } => "send",
            Self::Approve { .. } => "approve",
        }
    }
    pub fn expects(&self) -> &[Expect] {
        match self {
            Self::Open { expect, .. }
            | Self::Click { expect, .. }
            | Self::Extract { expect, .. }
            | Self::Act { expect, .. }
            | Self::Type { expect, .. }
            | Self::Fill { expect, .. }
            | Self::Select { expect, .. }
            | Self::Press { expect, .. }
            | Self::Send { expect, .. } => expect,
            _ => &[],
        }
    }
    /// The locators of an acting step (not of `click`, which only follows a link or a tab).
    pub fn acting_targets(&self) -> Vec<&Target> {
        match self {
            Self::Act { target, .. }
            | Self::Type { target, .. }
            | Self::Select { target, .. }
            | Self::Send { target, .. } => vec![target],
            Self::Fill { fields, .. } => fields.iter().map(|f| &f.target).collect(),
            _ => Vec::new(),
        }
    }
    /// The text a name-based class is read from: every name, label and text of the locators.
    fn target_names(&self) -> Vec<&str> {
        self.acting_targets()
            .into_iter()
            .flat_map(|t| &t.candidates)
            .flat_map(|c| [&c.name, &c.label, &c.text, &c.testid])
            .flatten()
            .map(String::as_str)
            .collect()
    }
    /// What this step can do, from what is stored: a floor, because the runner works it out again
    /// from the element it finds on the live page and takes the higher of the two.
    pub fn class(&self) -> Class {
        let by_names = || {
            self.target_names()
                .into_iter()
                .filter_map(risk::name_class)
                .fold(Class::WRITE, Class::max)
        };
        match self {
            Self::Act { .. } => by_names(),
            Self::Send { action, .. } => {
                let base = match action {
                    SendAction::Send => Class::send(),
                    SendAction::Post => Class::post(),
                };
                // A send button that is really a payment is a payment.
                if by_names().risk == Risk::Money {
                    Class::money()
                } else {
                    base
                }
            }
            Self::Type { .. } => Class::WRITE,
            Self::Select { .. } => by_names(),
            // Only the toggles and menus of a form can be a payment or a send ("Pay by card");
            // a text field is where text goes.
            Self::Fill { fields, .. } => fields
                .iter()
                .filter(|f| f.kind != FieldKind::Textbox)
                .flat_map(|f| &f.target.candidates)
                .flat_map(|c| [&c.name, &c.label, &c.text, &c.testid])
                .flatten()
                .filter_map(|n| risk::name_class(n))
                .fold(Class::WRITE, Class::max),
            Self::Press { key, .. } => {
                if matches!(key.as_str(), "Enter" | "Space") {
                    Class::send()
                } else {
                    Class::WRITE
                }
            }
            _ => Class::READ,
        }
    }
    pub fn risk(&self) -> Risk {
        self.class().risk
    }
    /// Whether the step changes something (everything but the read steps and `approve`).
    pub fn is_acting(&self) -> bool {
        self.risk() > Risk::Read
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
            Self::Scroll { .. } | Self::Press { .. } | Self::Approve { .. } => {}
            Self::Act { target, .. } | Self::Send { target, .. } => candidates(target, &mut out),
            Self::Type { target, value, .. } => {
                candidates(target, &mut out);
                out.push(value);
            }
            Self::Select { target, values, .. } => {
                candidates(target, &mut out);
                out.extend(values.iter().map(String::as_str));
            }
            Self::Fill { fields, .. } => {
                for field in fields {
                    candidates(&field.target, &mut out);
                    out.push(&field.value);
                }
            }
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
    /// The step in plain words with the run's values filled in, for an approval card.
    pub fn describe_with(&self, values: &BTreeMap<String, String>) -> String {
        substitute(&self.describe(), values, Encode::Raw)
    }
    /// The step in plain words, for the confirm card and `recipe_show`.
    pub fn describe(&self) -> String {
        let locator = |t: &Target| t.describe();
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
            Self::Act { target, .. } => format!("Click {} (changes something)", locator(target)),
            Self::Send { target, action, .. } => format!(
                "{} by clicking {}",
                match action {
                    SendAction::Send => "SEND",
                    SendAction::Post => "POST",
                },
                locator(target)
            ),
            Self::Type { target, value, .. } => {
                format!("Type \"{value}\" into {}", locator(target))
            }
            Self::Select { target, values, .. } => {
                format!(
                    "Select {} in {}",
                    values
                        .iter()
                        .map(|v| format!("\"{v}\""))
                        .collect::<Vec<_>>()
                        .join(", "),
                    locator(target)
                )
            }
            Self::Fill { fields, .. } => format!(
                "Fill {}",
                fields
                    .iter()
                    .map(|f| {
                        if f.kind.is_toggle() {
                            format!("set {} to {}", locator(&f.target), f.value)
                        } else {
                            format!("\"{}\" into {}", f.value, locator(&f.target))
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("; ")
            ),
            Self::Press { key, .. } => format!("Press {key}"),
            Self::Approve { category } => {
                format!("Ask the user first ({})", category.category().label())
            }
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
    /// The lowest trust level that can run every step: a click, or any step that changes
    /// something, is not available at Level 1.
    pub fn min_level(&self) -> u8 {
        if self
            .steps
            .iter()
            .any(|s| matches!(s, Step::Click { .. }) || s.is_acting())
        {
            2
        } else {
            1
        }
    }
    /// The risk of the recipe: the highest of its steps. Computed from the steps every time; the
    /// `risk` field of a file is only checked against it, never believed.
    pub fn computed_risk(&self) -> Risk {
        self.steps
            .iter()
            .map(Step::risk)
            .max()
            .unwrap_or(Risk::Read)
    }
    /// Whether any step changes something.
    pub fn changes_things(&self) -> bool {
        self.computed_risk() > Risk::Read
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
        self.validate_writes()
    }

    /// The rules of the steps that change things: the risk field is the computed one, every
    /// acting step is guarded by an `approve` step of a strong enough kind, and every `approve`
    /// step guards something.
    fn validate_writes(&self) -> Result<(), String> {
        let computed = self.computed_risk();
        if self.risk != computed {
            return Err(format!(
                "The recipe says its risk is {} but its steps are {}: the risk is computed from the steps.",
                self.risk.as_str(),
                computed.as_str()
            ));
        }
        let guard = governing(&self.steps);
        let mut guarding = BTreeSet::new();
        for (index, step) in self.steps.iter().enumerate() {
            if !step.is_acting() {
                continue;
            }
            let fail = |reason: &str| format!("Step {} ({}): {reason}", index + 1, step.op());
            let Some(approve) = guard[index] else {
                return Err(fail(
                    "it changes things and no approve step guards it (put an approve step right before it).",
                ));
            };
            let Step::Approve { category } = &self.steps[approve] else {
                return Err(fail("its guard is not an approve step."));
            };
            if !category.covers(step.class().category) {
                return Err(fail(
                    "its approve step asks about something weaker than this step does (a send or a payment needs its own approval).",
                ));
            }
            guarding.insert(approve);
        }
        for (index, step) in self.steps.iter().enumerate() {
            if matches!(step, Step::Approve { .. }) && !guarding.contains(&index) {
                return Err(format!(
                    "Step {} (approve): it guards no step that changes things.",
                    index + 1
                ));
            }
        }
        // A default may not be a credential: not a card, a key or a token (checked by
        // `bind_params`) and, for a value that gets typed, not a short numeric code.
        let typed = self.typed_params();
        for param in &self.params {
            if typed.contains(param.name.as_str()) && param.default.is_some() {
                return Err(format!(
                    "`{}` is typed into a page, so it has no default: the caller gives every typed value.",
                    param.name
                ));
            }
            let default = param.default.as_ref().and_then(Value::as_str);
            if typed.contains(param.name.as_str()) && default.is_some_and(risk::otp_like) {
                return Err(format!(
                    "The default of `{}` looks like a one-time code; recipes never carry one.",
                    param.name
                ));
            }
        }
        Ok(())
    }

    /// The parameters whose value is typed or selected into a page.
    pub fn typed_params(&self) -> BTreeSet<&str> {
        let mut out = BTreeSet::new();
        let mut take = |value: &'_ str| {
            for name in placeholders(value) {
                if let Some(param) = self.params.iter().find(|p| p.name == name) {
                    out.insert(param.name.as_str());
                }
            }
        };
        for step in &self.steps {
            match step {
                Step::Type { value, .. } => take(value),
                Step::Select { values, .. } => values.iter().for_each(|v| take(v)),
                Step::Fill { fields, .. } => fields.iter().for_each(|f| take(&f.value)),
                _ => {}
            }
        }
        out
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
            Step::Act { target, expect } | Step::Send { target, expect, .. } => {
                acting_target(target)?;
                if expect.is_empty() {
                    return Err("A step that changes things needs an assertion.".into());
                }
            }
            Step::Type {
                target,
                value,
                expect,
            } => {
                acting_target(target)?;
                typed_target(target)?;
                single_param(value)?;
                if expect.is_empty() {
                    return Err("A step that changes things needs an assertion.".into());
                }
            }
            Step::Select {
                target,
                values,
                expect,
            } => {
                acting_target(target)?;
                typed_target(target)?;
                if values.is_empty() || values.len() > MAX_SELECT_VALUES {
                    return Err(format!("Select 1 to {MAX_SELECT_VALUES} values."));
                }
                values.iter().try_for_each(|v| single_param(v))?;
                if expect.is_empty() {
                    return Err("A step that changes things needs an assertion.".into());
                }
            }
            Step::Fill { fields, expect } => {
                if fields.is_empty() || fields.len() > MAX_FORM_FIELDS {
                    return Err(format!("A fill step has 1 to {MAX_FORM_FIELDS} fields."));
                }
                for field in fields {
                    acting_target(&field.target)?;
                    typed_target(&field.target)?;
                    if field.kind.is_toggle() {
                        if !matches!(field.value.as_str(), "true" | "false") {
                            return Err("A checkbox or radio field is `true` or `false`.".into());
                        }
                    } else {
                        single_param(&field.value)?;
                    }
                }
                if expect.is_empty() {
                    return Err("A step that changes things needs an assertion.".into());
                }
            }
            Step::Press { key, expect } => {
                if !PRESS_KEYS.contains(&key.as_str()) {
                    return Err(format!(
                        "A press step uses one of {} (the scroll keys have their own step).",
                        PRESS_KEYS.join(", ")
                    ));
                }
                if expect.is_empty() {
                    return Err("A step that changes things needs an assertion.".into());
                }
            }
            Step::Approve { .. } => {}
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

/// For every step, the `approve` step that guards it (acting steps only).
///
/// * An `approve` of kind `other` guards the plain writes after it (types, fills, selects, clicks
///   that only write) until something else comes: a step that sends or pays, another `approve`,
///   or a page change (`open`, `click`), because the card showed those writes on one page.
/// * An `approve` of any other kind guards exactly the next acting step, on the same page.
///
/// The recorder writes recipes to this rule, `validate` refuses a recipe that breaks it, and the
/// runner follows it, so a removed or moved `approve` can not go unnoticed.
pub fn governing(steps: &[Step]) -> Vec<Option<usize>> {
    let mut out = vec![None; steps.len()];
    let mut current: Option<(usize, ApproveKind)> = None;
    for (index, step) in steps.iter().enumerate() {
        match step {
            Step::Approve { category } => current = Some((index, *category)),
            // A page change: the card showed the steps for the page they were written for.
            Step::Open { .. } | Step::Click { .. } => current = None,
            step if step.is_acting() => match current {
                Some((approve, ApproveKind::Other)) if step.risk() == Risk::Write => {
                    out[index] = Some(approve);
                }
                Some((approve, kind)) if kind != ApproveKind::Other => {
                    out[index] = Some(approve);
                    current = None;
                }
                _ => current = None,
            },
            _ => {}
        }
    }
    out
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
