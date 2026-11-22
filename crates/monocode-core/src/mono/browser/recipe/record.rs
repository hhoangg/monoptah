//! Learning: a per-session, in-memory trace of the Mono's browser calls, and the compile pass that
//! turns a successful read-only trace into a recipe draft.
//!
//! Nothing here is stored. The trace is in memory and dropped when a recipe is saved or the gate
//! goes away. It keeps the text of a typed, filled or selected value in memory only (to find the
//! parameter that value is an example of), never in `args`, and its `Debug` output hides it. The
//! compile pass is deterministic Rust, not a model call: the Mono (which is the model) names the
//! intent, the parameters and the data to extract, and this code derives the navigation steps and
//! the locators from what the browser really showed.
//!
//! Phase 3 records tasks that change things, under rules that refuse rather than guess:
//!
//! * every typed, filled or selected value must equal the example of a parameter (the whole
//!   value, not a part of it), and becomes exactly `{name}`: a literal is never stored;
//! * a field that takes a password, a code or card data, and a value that looks like a card, a
//!   key or a token, are never recorded, whatever parameter they match;
//! * a click on a button (or anything but a link or tab) becomes an `act` step, or a `send` step
//!   when its name says it sends or posts; its locator holds no parameter value;
//! * an `approve` step is inserted before every step that changes things, following
//!   `model::governing`; an approval the Mono made in the trace can only make a step ask more
//!   (a payment), never less;
//! * code, uploads, drag and drop, dialogs and coordinate clicks refuse the recording.
//!
//! * A link that was only used to navigate becomes an `open` step with its address (measured: 0.45 s
//!   against 1.4 to 6.3 s for a click through the extension relay).
//! * A link without a usable address, and a tab, become `click` steps with locator candidates
//!   built from the element that was clicked: role and name, the same scoped by its nearest named
//!   ancestor. Every candidate is later checked for uniqueness on the live page (see
//!   `tools::record`) and the ones that are not unique are dropped.
//! * Anything else that changed something (typing, a button, a menu item, code) refuses the
//!   recording: Phase 1 records read-only work.
use super::model::{
    generated_looking, percent_encode, ApproveKind, Candidate, Each, Expect, Field, FieldKind,
    FormField, Param, ParamKind, Recipe, Risk, Scope, SendAction, Stats, Step, Target,
    NAVIGATION_ROLES, PRESS_KEYS, SCHEMA,
};
use super::risk;
use super::snapshot::{self, Node};
use crate::mono::browser::gate::{is_finance_url, Category};
use crate::mono::browser::{is_scroll_key, PLAYWRIGHT_MCP_VERSION};
use regex::Regex;
use serde::Deserialize;
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};

/// Calls kept per session; older ones are dropped.
pub const MAX_ENTRIES: usize = 120;
const MAX_REFS: usize = 6000;

/// Tools that type, fill or select: recorded (Phase 3) when every value is a parameter.
const WRITING_TOOLS: [&str; 3] = ["browser_type", "browser_fill_form", "browser_select_option"];

/// Tools a recipe can never replay, with what they do (for the refusal). A failed call to one of
/// them blocks the recording too: it may still have changed something.
const REFUSED_TOOLS: [(&str, &str); 11] = [
    (
        "browser_file_upload",
        "uploads a file from the user's computer, which a recipe never does",
    ),
    ("browser_drag", "drags an element"),
    ("browser_drop", "drops something on the page"),
    ("browser_handle_dialog", "answers a dialog"),
    ("browser_evaluate", "runs code"),
    ("browser_run_code_unsafe", "runs code"),
    ("browser_mouse_click_xy", "clicks at a screen position"),
    ("browser_mouse_drag_xy", "drags at a screen position"),
    (
        "browser_mouse_down",
        "presses the mouse at a screen position",
    ),
    (
        "browser_mouse_up",
        "releases the mouse at a screen position",
    ),
    (
        "browser_mouse_move_xy",
        "moves the mouse to a screen position",
    ),
];

fn refused_reason(tool: &str) -> Option<&'static str> {
    REFUSED_TOOLS
        .iter()
        .find(|(name, _)| *name == tool)
        .map(|(_, why)| *why)
}

/// What the browser showed for an element the Mono may later click.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NodeInfo {
    pub role: String,
    pub name: String,
    pub url: Option<String>,
    /// Named ancestors, nearest first.
    pub ancestors: Vec<Scope>,
}

/// One field of a recorded `browser_fill_form`. Its value is memory only; `Debug` hides it.
#[derive(Clone)]
pub struct TypedField {
    pub node: Option<NodeInfo>,
    pub kind: String,
    pub value: String,
}
impl std::fmt::Debug for TypedField {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "TypedField({}, value hidden)", self.kind)
    }
}

/// The values a call typed, filled or selected, and the category of an approval the Mono asked
/// for. Kept in memory so the compile pass can match each value with the parameter it is an
/// example of; never written anywhere, and never printed by `Debug`.
#[derive(Clone, Default)]
pub struct Typed {
    pub text: Option<String>,
    /// `browser_type` with `submit`: the text was followed by Enter.
    pub submit: bool,
    pub values: Vec<String>,
    pub fields: Vec<TypedField>,
    pub category: Option<String>,
}
impl std::fmt::Debug for Typed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Typed(values hidden)")
    }
}

#[derive(Clone, Debug)]
pub struct Entry {
    pub tool: String,
    /// Only what a recipe needs: an address, a key, the text of a wait. Never a typed value.
    pub args: Value,
    pub page_before: String,
    pub page_after: String,
    pub ok: bool,
    pub node: Option<NodeInfo>,
    pub typed: Typed,
}

#[derive(Clone, Debug, Default)]
pub struct Trace {
    pub entries: Vec<Entry>,
    /// Older calls were dropped (more than [`MAX_ENTRIES`]): the start of the task is lost.
    pub truncated: bool,
    refs: HashMap<String, NodeInfo>,
}

fn keep_args(tool: &str, args: &Value) -> Value {
    let pick = |keys: &[&str]| {
        let mut map = serde_json::Map::new();
        for key in keys {
            if let Some(value) = args.get(*key) {
                map.insert((*key).to_owned(), value.clone());
            }
        }
        Value::Object(map)
    };
    match tool {
        "browser_navigate" => pick(&["url"]),
        "browser_click" => pick(&["target", "element"]),
        "browser_wait_for" => pick(&["text", "textGone", "time"]),
        "browser_press_key" => pick(&["key"]),
        "browser_tabs" => pick(&["action"]),
        _ => Value::Null,
    }
}

const LANDMARKS: [&str; 12] = [
    "navigation",
    "region",
    "main",
    "complementary",
    "banner",
    "list",
    "tablist",
    "dialog",
    "group",
    "form",
    "search",
    "toolbar",
];

fn collect_refs(nodes: &[Node], ancestors: &[Scope], out: &mut HashMap<String, NodeInfo>) {
    for node in nodes {
        if let Some(reference) = node.reference() {
            if out.len() < MAX_REFS {
                out.insert(
                    reference.to_owned(),
                    NodeInfo {
                        role: node.role.clone(),
                        name: node.name.clone(),
                        url: node.url.clone(),
                        ancestors: ancestors.to_vec(),
                    },
                );
            }
        }
        let mut next = ancestors.to_vec();
        if !node.name.is_empty()
            && (LANDMARKS.contains(&node.role.as_str()) || node.role == "heading")
            && !generated_looking(&node.name)
        {
            next.insert(
                0,
                Scope {
                    role: node.role.clone(),
                    name: node.name.clone(),
                },
            );
            next.truncate(3);
        }
        collect_refs(&node.children, &next, out);
    }
}

impl Trace {
    /// Learns the elements of a snapshot (or a find result): `ref` to role, name and address.
    pub fn learn(&mut self, text: &str) {
        let forest = snapshot::parse(snapshot::snapshot_body(text));
        collect_refs(&forest, &[], &mut self.refs);
    }
    /// Notes one call. `args` are cut down to what a recipe may keep.
    pub fn note(
        &mut self,
        tool: &str,
        args: &Value,
        page_before: &str,
        page_after: &str,
        ok: bool,
    ) {
        let lookup = |reference: &Value| reference.as_str().and_then(|t| self.refs.get(t)).cloned();
        let node = matches!(
            tool,
            "browser_click" | "browser_type" | "browser_select_option"
        )
        .then(|| lookup(&args["target"]))
        .flatten();
        let text_of = |value: &Value| match value {
            Value::String(text) => Some(text.clone()),
            Value::Bool(flag) => Some(flag.to_string()),
            Value::Number(number) => Some(number.to_string()),
            _ => None,
        };
        let typed = match tool {
            "browser_type" => Typed {
                text: text_of(&args["text"]),
                submit: args["submit"] == true,
                ..Typed::default()
            },
            "browser_select_option" => Typed {
                values: args["values"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(text_of)
                    .collect(),
                ..Typed::default()
            },
            "browser_fill_form" => Typed {
                fields: args["fields"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|field| TypedField {
                        node: lookup(&field["target"]).or_else(|| lookup(&field["ref"])),
                        kind: field["type"].as_str().unwrap_or("").to_owned(),
                        value: text_of(&field["value"]).unwrap_or_default(),
                    })
                    .collect(),
                ..Typed::default()
            },
            "browser_request_approval" => Typed {
                category: args["category"].as_str().map(str::to_owned),
                ..Typed::default()
            },
            _ => Typed::default(),
        };
        self.entries.push(Entry {
            tool: tool.to_owned(),
            args: keep_args(tool, args),
            page_before: page_before.to_owned(),
            page_after: page_after.to_owned(),
            ok,
            node,
            typed,
        });
        if self.entries.len() > MAX_ENTRIES {
            self.entries.remove(0);
            self.truncated = true;
        }
    }
    pub fn clear(&mut self) {
        self.truncated = false;
        self.entries.clear();
        self.refs.clear();
    }
}

// ---- The draft the Mono gives ----

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ParamSpec {
    pub name: String,
    #[serde(rename = "type", default)]
    pub kind: ParamKind,
    #[serde(default)]
    pub description: String,
    /// The literal the trace used for this parameter (a channel name, a search word). It is
    /// replaced by `{name}` in the steps and used for the dry run; it is not stored.
    #[serde(default)]
    pub example: Option<String>,
    #[serde(default)]
    pub default: Option<Value>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExtractSpec {
    #[serde(default)]
    pub scope: Option<Target>,
    #[serde(default)]
    pub each: Option<Each>,
    #[serde(default)]
    pub take_last: Option<String>,
    #[serde(default)]
    pub take_first: Option<String>,
    pub fields: BTreeMap<String, Field>,
    #[serde(default = "default_field_chars")]
    pub max_chars_per_field: u32,
    /// At least this many items (default 1).
    #[serde(default = "one")]
    pub min_items: u32,
}
fn default_field_chars() -> u32 {
    300
}
fn one() -> u32 {
    1
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Draft {
    pub intent: String,
    pub description: String,
    #[serde(default)]
    pub params: Vec<ParamSpec>,
    pub extract: ExtractSpec,
}

// ---- Compile ----

const SECRET_QUERY: [&str; 18] = [
    "token",
    "key",
    "secret",
    "password",
    "passwd",
    "code",
    "auth",
    "session",
    "sig",
    "signature",
    "access",
    "otp",
    "jwt",
    "bearer",
    "apikey",
    "credential",
    "csrf",
    "cookie",
];

/// A UUID, a long token or a long number in a path segment: a magic link, a reset link, a share
/// token. The examples a parameter replaces are not counted.
fn secret_path_segment(segment: &str, examples: &[&str]) -> bool {
    let mut text = segment.to_owned();
    for example in examples {
        text = text
            .replace(example, "x")
            .replace(&percent_encode(example), "x");
    }
    static UUID: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    let uuid = UUID.get_or_init(|| {
        Regex::new(r"(?i)[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}")
            .expect("uuid")
    });
    static TOKEN: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    let token = TOKEN.get_or_init(|| Regex::new(r"[A-Za-z0-9_]{20,}|\d{7,}").expect("token"));
    uuid.is_match(&text) || token.is_match(&text)
}

/// A page address a recipe may keep: no user name or password, no query value that looks like a
/// credential, no fragment that carries data, and no path segment that looks like a token (an
/// address like that must become a parameter, or the task can not be recorded).
pub fn clean_url(raw: &str) -> Result<url::Url, String> {
    clean_url_with(raw, &[])
}

pub fn clean_url_with(raw: &str, examples: &[&str]) -> Result<url::Url, String> {
    let mut url = url::Url::parse(raw.trim())
        .map_err(|_| "A page address in the trace is not valid.".to_owned())?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err("Only http and https addresses are kept.".into());
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("An address with a user name or password is never stored.".into());
    }
    if url
        .path_segments()
        .into_iter()
        .flatten()
        .any(|segment| secret_path_segment(segment, examples))
    {
        return Err("An address in the trace has a token, an id or a one-time link in its path (such as /s/<token> or /reset/<uuid>), which is never stored. Make that part a parameter with an example, or do not record this task.".into());
    }
    let pairs: Vec<(String, String)> = url
        .query_pairs()
        .filter(|(key, value)| {
            let key = key.to_lowercase();
            let secret_key = SECRET_QUERY.iter().any(|word| key.contains(word));
            let secret_value = value.len() >= 24
                && value
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "-_.=".contains(c));
            !secret_key && !secret_value
        })
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    if pairs.is_empty() {
        url.set_query(None);
    } else {
        url.query_pairs_mut().clear().extend_pairs(pairs);
    }
    if url.fragment().is_some_and(|f| f.contains('=')) {
        url.set_fragment(None);
    }
    Ok(url)
}

struct Params<'a> {
    specs: &'a [ParamSpec],
}
impl Params<'_> {
    /// The parameters whose example is looked for in the trace: text parameters only (a number
    /// would match any digit in an address).
    fn found(&self) -> impl Iterator<Item = (&ParamSpec, &str)> {
        self.specs
            .iter()
            .filter(|spec| spec.kind == ParamKind::String)
            .filter_map(|spec| {
                spec.example
                    .as_deref()
                    .filter(|e| !e.is_empty())
                    .map(|example| (spec, example))
            })
    }
    /// The parameter whose example is exactly this value (a typed value is all of an example,
    /// never a part of one).
    fn exact(&self, text: &str) -> Option<&ParamSpec> {
        self.specs
            .iter()
            .find(|spec| spec.example.as_deref() == Some(text) && !text.is_empty())
    }
    /// Whether a plain text holds the example of a parameter (so the text is not a stable name).
    fn mentions(&self, text: &str) -> bool {
        self.found().any(|(_, example)| text.contains(example))
    }
    /// Writes `{name}` where an example appears, in a plain string.
    fn plain(&self, text: &str) -> String {
        let mut out = text.to_owned();
        for (spec, example) in self.found() {
            out = out.replace(example, &format!("{{{}}}", spec.name));
        }
        out
    }
    fn url(&self, text: &str) -> String {
        let mut out = text.to_owned();
        for (spec, example) in self.found() {
            out = out
                .replace(&percent_encode(example), &format!("{{{}}}", spec.name))
                .replace(example, &format!("{{{}}}", spec.name));
        }
        out
    }
    fn regex(&self, text: &str) -> String {
        let mut out = text.to_owned();
        for (spec, example) in self.found() {
            out = out
                .replace(
                    &regex::escape(&percent_encode(example)),
                    &format!("{{{}}}", spec.name),
                )
                .replace(&regex::escape(example), &format!("{{{}}}", spec.name));
        }
        out
    }
}

fn candidates_for(node: &NodeInfo, params: &Params) -> Vec<Candidate> {
    let name = node.name.trim();
    if name.is_empty() || generated_looking(name) {
        return Vec::new();
    }
    let name = params.plain(name);
    let mut out = vec![Candidate {
        role: Some(node.role.clone()),
        name: Some(name.clone()),
        ..Candidate::default()
    }];
    if let Some(scope) = node.ancestors.first() {
        out.push(Candidate {
            role: Some(node.role.clone()),
            name: Some(name.clone()),
            scope: Some(Scope {
                role: scope.role.clone(),
                name: params.plain(&scope.name),
            }),
            ..Candidate::default()
        });
    }
    out
}

fn same_origin_pattern(url: &url::Url, params: &Params) -> String {
    let origin = url.origin().ascii_serialization();
    let path = url.path().trim_end_matches('/');
    params.regex(&format!(
        "^{}{}",
        regex::escape(&origin),
        regex::escape(path)
    ))
}

/// The recipe a trace and a draft make, not yet checked on the live page.
pub fn compile(
    trace: &Trace,
    draft: &Draft,
    existing_version: Option<u32>,
) -> Result<Recipe, String> {
    for spec in &draft.params {
        if spec.kind == ParamKind::String
            && spec
                .example
                .as_deref()
                .is_some_and(|e| e.chars().count() == 1)
        {
            return Err(format!(
                "The example of `{}` is a single character, too short to find in the trace.",
                spec.name
            ));
        }
        if let Some(example) = &spec.example {
            if crate::mono::browser::gate::contains_card_number(&serde_json::json!(example)) {
                return Err(format!(
                    "The example of `{}` looks like a card number; recipes never carry one.",
                    spec.name
                ));
            }
            if let Some(what) = risk::secret_value(example) {
                return Err(format!(
                    "The example of `{}` looks like {what}; recipes never carry credentials.",
                    spec.name
                ));
            }
        }
        if let Some(what) = spec
            .default
            .as_ref()
            .and_then(Value::as_str)
            .and_then(risk::secret_value)
        {
            return Err(format!(
                "The default of `{}` looks like {what}; recipes never carry credentials.",
                spec.name
            ));
        }
    }
    let params = Params {
        specs: &draft.params,
    };
    let mut steps: Vec<Step> = Vec::new();
    let mut origin: Option<String> = None;
    let mut saw_navigate = false;
    let set_origin = |url: &url::Url, origin: &mut Option<String>| -> Result<(), String> {
        let this = url.origin().ascii_serialization();
        match origin {
            None => *origin = Some(this),
            Some(first) if *first == this => {}
            Some(first) => {
                return Err(format!(
                    "The task used two sites ({first} and {this}). A recipe belongs to one site."
                ))
            }
        }
        Ok(())
    };
    if trace.truncated {
        return Err("The task is longer than the trace kept ({MAX_ENTRIES} calls), so its start is lost. Record a shorter task.".replace("{MAX_ENTRIES}", &MAX_ENTRIES.to_string()));
    }
    let examples: Vec<&str> = params.found().map(|(_, example)| example).collect();
    // The approval the Mono asked for, waiting for the next step that changes something.
    let mut declared: Option<ApproveKind> = None;
    let mut upgrades: BTreeMap<usize, ApproveKind> = BTreeMap::new();
    for entry in &trace.entries {
        let tool = entry.tool.as_str();
        let pressed_other =
            tool == "browser_press_key" && !entry.args["key"].as_str().is_some_and(is_scroll_key);
        let clicked_other = tool == "browser_click"
            && entry
                .node
                .as_ref()
                .is_none_or(|n| !NAVIGATION_ROLES.contains(&n.role.as_str()));
        let changes = WRITING_TOOLS.contains(&tool)
            || refused_reason(tool).is_some()
            || pressed_other
            || clicked_other;
        // A call that failed may still have changed something: it blocks the recording too.
        if !entry.ok && changes {
            return Err(format!(
                "The task used {tool} and the call failed. A failed call may still have changed something, so this task can not be recorded."
            ));
        }
        if !entry.ok {
            continue;
        }
        if matches!(tool, "browser_navigate_back")
            || (tool == "browser_tabs" && entry.args["action"].as_str() != Some("list"))
        {
            return Err(format!(
                "The task used {tool}, which moves between pages or tabs in a way a recipe can not replay. Record a task that only opens addresses and follows links."
            ));
        }
        if let Some(why) = refused_reason(tool) {
            return Err(format!(
                "The task used {tool}, which {why}. A recipe never does that."
            ));
        }
        if changes
            && (is_finance_url(&entry.page_before, &[]) || is_finance_url(&entry.page_after, &[]))
        {
            return Err("The task changed something on a bank, wallet or payment page. Recipes are not recorded there.".into());
        }
        // A step that changes something takes the approval the Mono asked for (as a floor).
        let mut push_acting = |step: Step, steps: &mut Vec<Step>| {
            // The approval belongs to the click or key that does the thing, not to the typing
            // before it.
            if matches!(
                step,
                Step::Act { .. } | Step::Send { .. } | Step::Press { .. }
            ) {
                if let Some(kind) = declared.take() {
                    upgrades.insert(steps.len(), kind);
                }
            }
            steps.push(step);
        };
        match tool {
            "browser_request_approval" => {
                declared = entry
                    .typed
                    .category
                    .as_deref()
                    .and_then(Category::parse)
                    .map(ApproveKind::from_category);
            }
            "browser_type" => {
                let node = entry.node.as_ref().ok_or(
                    "A field the task typed into could not be matched to an element of a snapshot, so it can not be recorded.",
                )?;
                let text = entry.typed.text.as_deref().unwrap_or("");
                let spec = typed_param(&params, text, node)?;
                check_typed(
                    node,
                    text,
                    &["textbox", "searchbox", "combobox", "spinbutton"],
                )?;
                let step = Step::Type {
                    target: acting_target(node, &params, true)?,
                    value: format!("{{{}}}", spec.name),
                    expect: field_expect(node, &params),
                };
                push_acting(step, &mut steps);
                if entry.typed.submit {
                    let origin = origin
                        .clone()
                        .ok_or("The task typed before it opened a page.")?;
                    let step = Step::Press {
                        key: "Enter".into(),
                        expect: vec![Expect::UrlMatches(format!("^{}", regex::escape(&origin)))],
                    };
                    push_acting(step, &mut steps);
                }
            }
            "browser_select_option" => {
                let node = entry.node.as_ref().ok_or(
                    "A menu the task chose from could not be matched to an element of a snapshot, so it can not be recorded.",
                )?;
                if entry.typed.values.is_empty() {
                    return Err("A selection in the trace has no values.".into());
                }
                let mut values = Vec::new();
                for value in &entry.typed.values {
                    let spec = typed_param(&params, value, node)?;
                    check_typed(node, value, &["combobox", "listbox"])?;
                    values.push(format!("{{{}}}", spec.name));
                }
                let step = Step::Select {
                    target: acting_target(node, &params, true)?,
                    values,
                    expect: field_expect(node, &params),
                };
                push_acting(step, &mut steps);
            }
            "browser_fill_form" => {
                if entry.typed.fields.is_empty() {
                    return Err("A form fill in the trace has no fields.".into());
                }
                let mut fields = Vec::new();
                let mut first: Option<&NodeInfo> = None;
                for field in &entry.typed.fields {
                    let node = field.node.as_ref().ok_or(
                        "A form field the task filled could not be matched to an element of a snapshot, so it can not be recorded.",
                    )?;
                    first.get_or_insert(node);
                    let kind = match field.kind.as_str() {
                        "textbox" => FieldKind::Textbox,
                        "checkbox" => FieldKind::Checkbox,
                        "radio" => FieldKind::Radio,
                        "combobox" => FieldKind::Combobox,
                        "slider" => FieldKind::Slider,
                        other => {
                            return Err(format!(
                                "A form field of type {other:?} can not be recorded."
                            ))
                        }
                    };
                    let value = if kind.is_toggle() {
                        if !matches!(field.value.as_str(), "true" | "false") {
                            return Err(
                                "A checkbox or radio field in the trace is neither true nor false."
                                    .into(),
                            );
                        }
                        field.value.clone()
                    } else {
                        let spec = typed_param(&params, &field.value, node)?;
                        let roles: &[&str] = match kind {
                            FieldKind::Combobox => &["combobox", "listbox"],
                            FieldKind::Slider => &["slider"],
                            _ => &["textbox", "searchbox", "spinbutton"],
                        };
                        check_typed(node, &field.value, roles)?;
                        format!("{{{}}}", spec.name)
                    };
                    fields.push(FormField {
                        target: acting_target(node, &params, true)?,
                        kind,
                        value,
                    });
                }
                let step = Step::Fill {
                    fields,
                    expect: first.map(|n| field_expect(n, &params)).unwrap_or_default(),
                };
                push_acting(step, &mut steps);
            }
            "browser_navigate" => {
                let url = clean_url_with(entry.args["url"].as_str().unwrap_or(""), &examples)?;
                set_origin(&url, &mut origin)?;
                saw_navigate = true;
                declared = None;
                steps.push(open_step(&url, &params));
            }
            "browser_click" => {
                let node = entry.node.as_ref().ok_or(
                    "A click in the trace could not be matched to an element of a snapshot, so it can not be recorded.",
                )?;
                let after = url::Url::parse(&entry.page_after)
                    .ok()
                    .filter(|_| entry.page_after != entry.page_before && origin.is_some());
                let expect = match &after {
                    Some(after) => vec![Expect::UrlMatches(same_origin_pattern(after, &params))],
                    None => vec![Expect::RolePresent {
                        role: node.role.clone(),
                        name: Some(node.name.clone())
                            .filter(|n| !params.mentions(n) && !n.is_empty()),
                    }],
                };
                if !NAVIGATION_ROLES.contains(&node.role.as_str()) {
                    // A button, a checkbox, a menu item: it changes something. A name that says
                    // it sends or posts makes it a `send` step.
                    let class = risk::label_class(&node.role, &node.name);
                    let target = acting_target(node, &params, false)?;
                    let step = if class.risk == Risk::Send {
                        Step::Send {
                            target,
                            action: if class.category == Category::PostPublish {
                                SendAction::Post
                            } else {
                                SendAction::Send
                            },
                            expect,
                        }
                    } else {
                        Step::Act { target, expect }
                    };
                    push_acting(step, &mut steps);
                    continue;
                }
                let link = node
                    .url
                    .as_deref()
                    .filter(|u| !u.starts_with('#') && !u.starts_with("javascript:"))
                    .and_then(|u| {
                        url::Url::parse(&entry.page_before)
                            .ok()
                            .and_then(|base| base.join(u).ok())
                    });
                if let (true, Some(link)) = (node.role == "link", link) {
                    let url = clean_url_with(link.as_str(), &examples)?;
                    set_origin(&url, &mut origin)?;
                    saw_navigate = true;
                    steps.push(open_step(&url, &params));
                } else {
                    let candidates = candidates_for(node, &params);
                    if candidates.is_empty() {
                        return Err("A clicked element has no stable name, so a locator can not be built for it.".into());
                    }
                    let expect = match url::Url::parse(&entry.page_after) {
                        Ok(after) if entry.page_after != entry.page_before && origin.is_some() => {
                            vec![Expect::UrlMatches(same_origin_pattern(&after, &params))]
                        }
                        _ => vec![Expect::RolePresent {
                            role: node.role.clone(),
                            name: Some(params.plain(&node.name)),
                        }],
                    };
                    steps.push(Step::Click {
                        target: Target { candidates },
                        expect,
                    });
                }
            }
            "browser_wait_for" => {
                let text = entry.args["text"].as_str().map(|t| params.plain(t));
                let text_gone = entry.args["textGone"].as_str().map(|t| params.plain(t));
                let time_ms = (entry.args["time"].as_f64().unwrap_or(0.0) * 1000.0)
                    .clamp(0.0, 15_000.0) as u32;
                let waits_for_text = text.is_some() || text_gone.is_some();
                if waits_for_text || time_ms > 0 {
                    steps.push(Step::Wait {
                        text,
                        text_gone,
                        time_ms: if waits_for_text { 0 } else { time_ms },
                    });
                }
            }
            "browser_press_key" => {
                let key = entry.args["key"].as_str().unwrap_or("");
                if is_scroll_key(key) {
                    match steps.last_mut() {
                        Some(Step::Scroll { key: last, times }) if last == key && *times < 20 => {
                            *times += 1
                        }
                        _ => steps.push(Step::Scroll {
                            key: key.to_owned(),
                            times: 1,
                        }),
                    }
                } else if PRESS_KEYS.contains(&key) {
                    let origin = origin
                        .clone()
                        .ok_or("The task pressed a key before it opened a page.")?;
                    let step = Step::Press {
                        key: key.to_owned(),
                        expect: vec![Expect::UrlMatches(format!("^{}", regex::escape(&origin)))],
                    };
                    push_acting(step, &mut steps);
                } else {
                    return Err(format!(
                        "The task pressed {key:?}. Only the scroll keys and {} can be recorded.",
                        PRESS_KEYS.join(", ")
                    ));
                }
            }
            _ => {}
        }
    }
    if !saw_navigate {
        return Err("The trace has no page that was opened, so there is nothing to start a recipe from. Open the page, read it, then record.".into());
    }
    let steps = insert_approvals(steps, &upgrades);
    // An address followed at once by another address is only a stop on the way.
    let mut collapsed: Vec<Step> = Vec::new();
    for step in steps {
        if matches!(step, Step::Open { .. }) && matches!(collapsed.last(), Some(Step::Open { .. }))
        {
            collapsed.pop();
        }
        collapsed.push(step);
    }
    let extract = &draft.extract;
    let mut expect = vec![Expect::CountAtLeast(extract.min_items.max(1))];
    if let Some(origin) = &origin {
        // The page the data is read from must still be on the site.
        expect.push(Expect::UrlMatches(format!("^{}", regex::escape(origin))));
    }
    collapsed.push(Step::Extract {
        scope: extract.scope.clone(),
        each: extract.each.clone(),
        take_last: extract.take_last.clone(),
        take_first: extract.take_first.clone(),
        fields: extract.fields.clone(),
        max_chars_per_field: extract.max_chars_per_field,
        expect,
    });
    let risk = collapsed.iter().map(Step::risk).max().unwrap_or(Risk::Read);
    let recipe = Recipe {
        schema: SCHEMA,
        id: Recipe::new_id(),
        origin: origin.ok_or("No site found in the trace.")?,
        also_origins: Vec::new(),
        intent: draft.intent.clone(),
        description: draft.description.trim().to_owned(),
        version: existing_version.map_or(1, |v| v + 1),
        params: draft
            .params
            .iter()
            .map(|p| Param {
                name: p.name.clone(),
                kind: p.kind,
                description: p.description.clone(),
                default: p.default.clone(),
            })
            .collect(),
        steps: collapsed,
        risk,
        playwright_mcp: PLAYWRIGHT_MCP_VERSION.to_owned(),
        stats: Stats::default(),
    };
    for spec in &draft.params {
        let placeholder = format!("{{{}}}", spec.name);
        let used = recipe.canonical().contains(&placeholder);
        if !used {
            return Err(format!(
                "Parameter `{}` is not used by any step: its example was not found in the trace and the extractor does not use it.",
                spec.name
            ));
        }
    }
    recipe.validate()?;
    Ok(recipe)
}

// ---- Steps that change things ----

/// The parameter a typed value is the example of. A value that is not (a literal, or only a part
/// of an example) refuses the recording: a recipe never stores a typed literal.
fn typed_param<'a>(
    params: &'a Params,
    value: &str,
    node: &NodeInfo,
) -> Result<&'a ParamSpec, String> {
    params.exact(value).ok_or_else(|| {
        format!(
            "A value the task typed or chose in the {} \"{}\" is not a parameter. A recipe never stores a typed literal: give the whole value as the `example` of a parameter (and nothing around it), or do not record this task.",
            node.role,
            node.name.chars().take(60).collect::<String>()
        )
    })
}

/// A typed value goes only into the kind of field the call says, never into a place for a
/// credential, and is never a credential.
fn check_typed(node: &NodeInfo, value: &str, roles: &[&str]) -> Result<(), String> {
    if !roles.contains(&node.role.as_str()) {
        return Err(format!(
            "The task typed into a {} (\"{}\"), which is not a text field of the kind the recipe types into.",
            node.role,
            node.name.chars().take(60).collect::<String>()
        ));
    }
    if risk::secret_field(&node.name) {
        return Err(format!(
            "The task typed into \"{}\", a field for a password, a code or card data. A recipe never types those.",
            node.name.chars().take(60).collect::<String>()
        ));
    }
    if let Some(what) = risk::secret_value(value) {
        return Err(format!(
            "The task typed {what}. A recipe never types a credential."
        ));
    }
    Ok(())
}

/// The locator of a step that changes things: role and name, the same scoped by its nearest
/// named ancestor. It never holds a parameter value. A field (`nameless_ok`) whose name has a
/// parameter value in it (a box named after the channel) is found by role inside its ancestor
/// instead, and a locator that matches several elements fails at run time rather than guess.
fn acting_target(node: &NodeInfo, params: &Params, nameless_ok: bool) -> Result<Target, String> {
    let name = node.name.trim();
    let scope = node
        .ancestors
        .first()
        .filter(|s| !params.mentions(&s.name) && !params.mentions(&s.role));
    let stable = !name.is_empty() && !generated_looking(name);
    let literal = stable && !params.mentions(name);
    let mut candidates = Vec::new();
    if literal {
        candidates.push(Candidate {
            role: Some(node.role.clone()),
            name: Some(name.to_owned()),
            ..Candidate::default()
        });
        if let Some(scope) = scope {
            candidates.push(Candidate {
                role: Some(node.role.clone()),
                name: Some(name.to_owned()),
                scope: Some(scope.clone()),
                ..Candidate::default()
            });
        }
    } else if nameless_ok && stable {
        let Some(scope) = scope else {
            return Err(format!(
                "The {} \"{}\" has a parameter value in its name and no named area around it, so a locator for it can not be built safely.",
                node.role,
                name.chars().take(60).collect::<String>()
            ));
        };
        candidates.push(Candidate {
            role: Some(node.role.clone()),
            scope: Some(scope.clone()),
            ..Candidate::default()
        });
    } else if stable {
        return Err(format!(
            "The {} \"{}\" has a parameter value in its name. A value must not choose what gets clicked, so this task can not be recorded.",
            node.role,
            name.chars().take(60).collect::<String>()
        ));
    } else {
        return Err(format!(
            "The {} that was changed has no stable name, so a locator can not be built for it.",
            node.role
        ));
    }
    Ok(Target { candidates })
}

/// What must hold after a typed, filled or selected step: the field is still on the page.
fn field_expect(node: &NodeInfo, params: &Params) -> Vec<Expect> {
    vec![Expect::RolePresent {
        role: node.role.clone(),
        name: Some(node.name.trim().to_owned())
            .filter(|n| !n.is_empty() && !params.mentions(n) && !generated_looking(n)),
    }]
}

/// Writes the `approve` steps (see `model::governing`): one for a run of plain writes, one for each
/// step that sends, posts or pays. An approval the Mono asked for can make a step ask for a
/// payment approval, never for less.
fn insert_approvals(steps: Vec<Step>, upgrades: &BTreeMap<usize, ApproveKind>) -> Vec<Step> {
    let mut out = Vec::with_capacity(steps.len() + 4);
    let mut block_open = false;
    for (index, step) in steps.into_iter().enumerate() {
        if matches!(step, Step::Open { .. } | Step::Click { .. }) {
            block_open = false;
        }
        if step.is_acting() {
            let mut kind = ApproveKind::from_category(step.class().category);
            if upgrades.get(&index) == Some(&ApproveKind::Payment) {
                kind = ApproveKind::Payment;
            }
            if kind == ApproveKind::Other && step.risk() == Risk::Write {
                if !block_open {
                    out.push(Step::Approve {
                        category: ApproveKind::Other,
                    });
                    block_open = true;
                }
            } else {
                out.push(Step::Approve { category: kind });
                block_open = false;
            }
        }
        out.push(step);
    }
    out
}

fn open_step(url: &url::Url, params: &Params) -> Step {
    Step::Open {
        url: params.url(url.as_str()),
        expect: vec![Expect::UrlMatches(same_origin_pattern(url, params))],
    }
}

/// Keeps only the candidates the dry run found unique. A step with none left cannot be recorded.
pub fn prune(recipe: &mut Recipe, unique: &BTreeMap<usize, Vec<usize>>) -> Result<(), String> {
    prune_until(recipe, unique, usize::MAX)
}

/// Like [`prune`], for a replay that stopped early: only the steps before index `until` were
/// probed, the others keep their candidates (the run time check is strict single match anyway).
pub fn prune_until(
    recipe: &mut Recipe,
    unique: &BTreeMap<usize, Vec<usize>>,
    until: usize,
) -> Result<(), String> {
    for (index, step) in recipe.steps.iter_mut().enumerate() {
        if index >= until {
            break;
        }
        let target = match step {
            Step::Click { target, .. } => Some(target),
            Step::Extract {
                scope: Some(target),
                ..
            } => Some(target),
            _ => None,
        };
        let Some(target) = target else { continue };
        let keep = unique.get(&index).cloned().unwrap_or_default();
        let kept: Vec<Candidate> = target
            .candidates
            .iter()
            .enumerate()
            .filter(|(i, _)| keep.contains(i))
            .map(|(_, c)| c.clone())
            .collect();
        if kept.is_empty() {
            return Err(format!(
                "Step {}: no locator candidate matched exactly one element on the live page.",
                index + 1
            ));
        }
        target.candidates = kept;
    }
    Ok(())
}

/// A readable one-line check that a name does not hold a regex special that would confuse the
/// summary card. Kept here so the card builder and tests share it.
pub fn plain_summary(recipe: &Recipe) -> String {
    let re = Regex::new(r"\s+").expect("whitespace");
    recipe
        .steps
        .iter()
        .enumerate()
        .map(|(i, s)| format!("{}. {}", i + 1, re.replace_all(&s.describe(), " ")))
        .collect::<Vec<_>>()
        .join("\n")
}
