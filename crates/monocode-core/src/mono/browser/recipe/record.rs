//! Learning: a per-session, in-memory trace of the Mono's browser calls, and the compile pass that
//! turns a successful read-only trace into a recipe draft.
//!
//! Nothing here is stored. The trace keeps no typed value (a `browser_type`, `browser_fill_form`
//! or `browser_select_option` call is only noted as "changed something") and it is dropped when a
//! recipe is saved or the gate goes away. The compile pass is deterministic Rust, not a model call:
//! the Mono (which is the model) names the intent, the parameters and the data to extract, and
//! this code derives the navigation steps and the locators from what the browser really showed.
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
    generated_looking, percent_encode, Candidate, Each, Expect, Field, Param, ParamKind, Recipe,
    Risk, Scope, Stats, Step, Target, NAVIGATION_ROLES, SCHEMA,
};
use super::snapshot::{self, Node};
use crate::mono::browser::{is_scroll_key, PLAYWRIGHT_MCP_VERSION};
use regex::Regex;
use serde::Deserialize;
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};

/// Calls kept per session; older ones are dropped.
pub const MAX_ENTRIES: usize = 120;
const MAX_REFS: usize = 6000;

/// Tools whose successful call means the page or the user's data was changed or code ran.
const CHANGING_TOOLS: [&str; 14] = [
    "browser_type",
    "browser_fill_form",
    "browser_select_option",
    "browser_file_upload",
    "browser_drag",
    "browser_drop",
    "browser_handle_dialog",
    "browser_evaluate",
    "browser_run_code_unsafe",
    "browser_mouse_click_xy",
    "browser_mouse_drag_xy",
    "browser_mouse_down",
    "browser_mouse_up",
    "browser_request_approval",
];

/// What the browser showed for an element the Mono may later click.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NodeInfo {
    pub role: String,
    pub name: String,
    pub url: Option<String>,
    /// Named ancestors, nearest first.
    pub ancestors: Vec<Scope>,
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
        let node = (tool == "browser_click")
            .then(|| {
                args["target"]
                    .as_str()
                    .and_then(|t| self.refs.get(t))
                    .cloned()
            })
            .flatten();
        self.entries.push(Entry {
            tool: tool.to_owned(),
            args: keep_args(tool, args),
            page_before: page_before.to_owned(),
            page_after: page_after.to_owned(),
            ok,
            node,
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
    for entry in &trace.entries {
        let tool = entry.tool.as_str();
        let pressed_other =
            tool == "browser_press_key" && !entry.args["key"].as_str().is_some_and(is_scroll_key);
        let clicked_other = tool == "browser_click"
            && entry
                .node
                .as_ref()
                .is_none_or(|n| !NAVIGATION_ROLES.contains(&n.role.as_str()));
        // A call that failed may still have changed something: it blocks the recording too.
        if !entry.ok && !(CHANGING_TOOLS.contains(&tool) || pressed_other || clicked_other) {
            continue;
        }
        if matches!(tool, "browser_navigate_back")
            || (tool == "browser_tabs" && entry.args["action"].as_str() != Some("list"))
        {
            return Err(format!(
                "The task used {tool}, which moves between pages or tabs in a way a recipe can not replay. Record a task that only opens addresses and follows links."
            ));
        }
        if CHANGING_TOOLS.contains(&tool) {
            return Err(format!(
                "The task used {tool}, which changes something or runs code. Phase 1 records read-only tasks only (opening pages, following links, reading)."
            ));
        }
        match tool {
            "browser_navigate" => {
                let url = clean_url_with(entry.args["url"].as_str().unwrap_or(""), &examples)?;
                set_origin(&url, &mut origin)?;
                saw_navigate = true;
                steps.push(open_step(&url, &params));
            }
            "browser_click" => {
                let node = entry.node.as_ref().ok_or(
                    "A click in the trace could not be matched to an element of a snapshot, so it can not be recorded.",
                )?;
                if !NAVIGATION_ROLES.contains(&node.role.as_str()) {
                    return Err(format!(
                        "The task clicked a {} (\"{}\"). Phase 1 only records links and tabs, because a click on anything else can change something.",
                        node.role,
                        node.name.chars().take(60).collect::<String>()
                    ));
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
                if !is_scroll_key(key) {
                    return Err(format!(
                        "The task pressed {key:?}. Only the scroll keys can be recorded."
                    ));
                }
                match steps.last_mut() {
                    Some(Step::Scroll { key: last, times }) if last == key && *times < 20 => {
                        *times += 1
                    }
                    _ => steps.push(Step::Scroll {
                        key: key.to_owned(),
                        times: 1,
                    }),
                }
            }
            _ => {}
        }
    }
    if !saw_navigate {
        return Err("The trace has no page that was opened, so there is nothing to start a recipe from. Open the page, read it, then record.".into());
    }
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
        risk: Risk::Read,
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

fn open_step(url: &url::Url, params: &Params) -> Step {
    Step::Open {
        url: params.url(url.as_str()),
        expect: vec![Expect::UrlMatches(same_origin_pattern(url, params))],
    }
}

/// Keeps only the candidates the dry run found unique. A step with none left cannot be recorded.
pub fn prune(recipe: &mut Recipe, unique: &BTreeMap<usize, Vec<usize>>) -> Result<(), String> {
    for (index, step) in recipe.steps.iter_mut().enumerate() {
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
