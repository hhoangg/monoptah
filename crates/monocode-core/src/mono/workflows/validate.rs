//! Full validation of a workflow definition. Issue messages are copied verbatim from the React
//! app: the Mono and the workflow guide read them.
use super::condition::condition_syntax_problem;
use super::model::{
    is_node_id, Checkout, Cwd, Node, NodeId, NodeKind, Workflow, MAX_PARALLEL_LIMIT,
    MAX_ROUNDS_LIMIT, MAX_TIMEOUT_MINUTES,
};
use super::parse::{is_map_key, is_name};
use super::template::{js_space, js_trim, shell_template_problems, template_vars};
use crate::mono::habits::Schedule;
use serde_json::json;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkflowIssue {
    pub node_id: Option<NodeId>,
    pub message: String,
}

const RUN_VARS: [&str; 3] = ["round", "id", "startedAt"];
/// Template variables a node output must not shadow.
const RESERVED_NODE_IDS: [&str; 3] = ["run", "inputs", "results"];
/// Extra `run` fields only a report template gets (see `compose_report`).
const REPORT_RUN_VARS: [&str; 4] = ["name", "status", "statusLabel", "endedAt"];
const COMMAND_FIELDS: [&str; 3] = ["exitCode", "stdout", "stderr"];
const ASK_FIELDS: [&str; 2] = ["answer", "reason"];
const FOR_EACH_FIELDS: [&str; 1] = ["results"];
const PATH_ROOTS: [&str; 14] = [
    "Users",
    "home",
    "Volumes",
    "private",
    "tmp",
    "var",
    "opt",
    "etc",
    "usr",
    "mnt",
    "root",
    "Library",
    "Applications",
    "System",
];

// --- absolute path matchers ----------------------------------------------------------------
// Hand written: the React app uses lookaround regexes.

/// `[\s"'`=(\[,<>|;&:]`: what may come right before a path.
fn path_prefix(c: char) -> bool {
    js_space(c)
        || matches!(
            c,
            '"' | '\'' | '`' | '=' | '(' | '[' | ',' | '<' | '>' | '|' | ';' | '&' | ':'
        )
}

fn starts_at(c: &[char], i: usize, pat: &str) -> bool {
    pat.chars()
        .enumerate()
        .all(|(k, p)| c.get(i + k) == Some(&p))
}

fn drive_at(c: &[char], i: usize) -> bool {
    c.get(i).is_some_and(char::is_ascii_alphabetic)
        && c.get(i + 1) == Some(&':')
        && matches!(c.get(i + 2), Some('\\' | '/'))
}

/// `~\/|\/(?:Users|home|...)\/|[A-Za-z]:[\\/]` at `i`.
fn home_path_at(c: &[char], i: usize) -> bool {
    starts_at(c, i, "~/")
        || (c.get(i) == Some(&'/')
            && PATH_ROOTS
                .iter()
                .any(|root| starts_at(c, i + 1, root) && c.get(i + 1 + root.len()) == Some(&'/')))
        || drive_at(c, i)
}

fn shell_word_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '/' | '-')
}

/// `~\/|\/(?!\/|dev\/(?:null|stdin|stdout|stderr)(?![\w/.-]))\S|[A-Za-z]:[\\/]` at `i`.
fn command_path_at(c: &[char], i: usize) -> bool {
    if starts_at(c, i, "~/") || drive_at(c, i) {
        return true;
    }
    if c.get(i) != Some(&'/') {
        return false;
    }
    if c.get(i + 1) == Some(&'/') {
        return false;
    }
    if starts_at(c, i + 1, "dev/") {
        let device = ["null", "stdin", "stdout", "stderr"].iter().any(|d| {
            starts_at(c, i + 5, d) && c.get(i + 5 + d.len()).is_none_or(|n| !shell_word_char(*n))
        });
        if device {
            return false;
        }
    }
    c.get(i + 1).is_some_and(|n| !js_space(*n))
}

/// The first match of the absolute-path pattern (`strict` for shell commands), as the whole
/// match including the delimiter before it.
fn path_match(text: &str, strict: bool) -> Option<String> {
    let c: Vec<char> = text.chars().collect();
    let body = |i: usize| {
        if strict {
            command_path_at(&c, i)
        } else {
            home_path_at(&c, i)
        }
    };
    let run_end = |from: usize| {
        c[from..]
            .iter()
            .position(|ch| js_space(*ch))
            .map_or(c.len(), |p| from + p)
    };
    for p in 0..=c.len() {
        if p == 0 && body(0) {
            return Some(c[..run_end(0)].iter().collect());
        }
        if p < c.len() && path_prefix(c[p]) && body(p + 1) {
            return Some(c[p..run_end(p + 1)].iter().collect());
        }
    }
    None
}

/// The first thing in `text` that reads as an absolute path on this machine, if any. Something
/// that reads as a path here is a home or well-known root directory, a Windows drive, or `~/`.
/// A bare `/api/users` is not matched, so prompts may talk about URL paths.
pub fn find_absolute_path(text: &str) -> Option<String> {
    path_match(text, false).map(|m| js_trim(&m).to_string())
}

/// For shell commands every word that starts with `/` or `~/` is a path on this machine, also
/// right after a redirect or operator. `//` is left alone so a URL is not read as a path, and so
/// are the harmless /dev/null-style device files.
fn find_command_path(text: &str) -> Option<String> {
    path_match(text, true).map(|m| js_trim(&m).to_string())
}

// --- graph ---------------------------------------------------------------------------------

struct Graph<'a> {
    by_id: HashMap<&'a str, &'a Node>,
    /// First-seen order of the ids in `by_id`.
    order: Vec<&'a str>,
    /// Neighbours over non-loop edges whose endpoints both exist.
    next: HashMap<&'a str, Vec<&'a str>>,
    prev: HashMap<&'a str, Vec<&'a str>>,
}

fn reach<'a>(start: &str, adjacency: &HashMap<&'a str, Vec<&'a str>>) -> HashSet<&'a str> {
    let mut seen = HashSet::new();
    let mut queue = vec![start];
    while let Some(id) = queue.pop() {
        for n in adjacency.get(id).into_iter().flatten() {
            if seen.insert(*n) {
                queue.push(n);
            }
        }
    }
    seen
}

impl<'a> Graph<'a> {
    fn new(w: &'a Workflow) -> Self {
        let mut by_id = HashMap::new();
        let mut order = Vec::new();
        for node in &w.nodes {
            // The first node with an id wins.
            if !by_id.contains_key(node.id.as_str()) {
                by_id.insert(node.id.as_str(), node);
                order.push(node.id.as_str());
            }
        }
        let mut next: HashMap<&str, Vec<&str>> = HashMap::new();
        let mut prev: HashMap<&str, Vec<&str>> = HashMap::new();
        for e in &w.edges {
            if e.loop_spec.is_some()
                || !by_id.contains_key(e.from.as_str())
                || !by_id.contains_key(e.to.as_str())
            {
                continue;
            }
            next.entry(e.from.as_str()).or_default().push(e.to.as_str());
            prev.entry(e.to.as_str()).or_default().push(e.from.as_str());
        }
        Self {
            by_id,
            order,
            next,
            prev,
        }
    }

    /// Nodes that sit on a cycle of non-loop edges (or are reachable only through one).
    fn cycle_nodes(&self) -> Vec<&'a str> {
        let mut indegree: HashMap<&str, usize> = self
            .order
            .iter()
            .map(|id| (*id, self.prev.get(id).map_or(0, Vec::len)))
            .collect();
        let mut queue: Vec<&str> = self
            .order
            .iter()
            .copied()
            .filter(|id| indegree[id] == 0)
            .collect();
        let mut done = HashSet::new();
        while let Some(id) = queue.pop() {
            done.insert(id);
            for n in self.next.get(id).into_iter().flatten() {
                let left = indegree.get(n).copied().unwrap_or(0).saturating_sub(1);
                indegree.insert(n, left);
                if left == 0 {
                    queue.push(n);
                }
            }
        }
        self.order
            .iter()
            .copied()
            .filter(|id| !done.contains(id))
            .collect()
    }
}

struct Checker<'a> {
    w: &'a Workflow,
    g: Graph<'a>,
    issues: Vec<WorkflowIssue>,
    ancestors_cache: RefCell<HashMap<String, HashSet<&'a str>>>,
    descendants_cache: RefCell<HashMap<String, HashSet<&'a str>>>,
}

fn output_fields(node: &Node) -> Vec<String> {
    match &node.kind {
        NodeKind::Session { output, .. } => output.keys().cloned().collect(),
        NodeKind::Command { .. } => COMMAND_FIELDS.iter().map(|s| s.to_string()).collect(),
        NodeKind::AskMono { .. } => ASK_FIELDS.iter().map(|s| s.to_string()).collect(),
        NodeKind::ForEach { .. } => FOR_EACH_FIELDS.iter().map(|s| s.to_string()).collect(),
        _ => Vec::new(),
    }
}

/// The loop variable name of a For each node.
fn item_name(node: &Node) -> Option<String> {
    match &node.kind {
        NodeKind::ForEach { item, .. } => Some(item.clone().unwrap_or_else(|| "item".into())),
        _ => None,
    }
}

fn is_whole(n: f64) -> bool {
    n.fract() == 0.0 && n.is_finite()
}

impl<'a> Checker<'a> {
    fn add(&mut self, message: impl Into<String>, node_id: Option<&str>) {
        self.issues.push(WorkflowIssue {
            node_id: node_id.map(str::to_string),
            message: message.into(),
        });
    }

    fn ancestors(&self, id: &str) -> HashSet<&'a str> {
        let mut cache = self.ancestors_cache.borrow_mut();
        cache
            .entry(id.to_string())
            .or_insert_with(|| reach(id, &self.g.prev))
            .clone()
    }

    fn descendants(&self, id: &str) -> HashSet<&'a str> {
        let mut cache = self.descendants_cache.borrow_mut();
        cache
            .entry(id.to_string())
            .or_insert_with(|| reach(id, &self.g.next))
            .clone()
    }

    /// The For each nodes `node` runs inside, innermost first.
    fn parent_chain(&self, node: &Node) -> Vec<&'a Node> {
        let mut chain = Vec::new();
        let mut seen: HashSet<&str> = HashSet::from([node.id.as_str()]);
        let mut parent = node
            .parent
            .as_deref()
            .and_then(|p| self.g.by_id.get(p).copied());
        while let Some(p) = parent {
            if !matches!(p.kind, NodeKind::ForEach { .. }) || seen.contains(p.id.as_str()) {
                break;
            }
            chain.push(p);
            seen.insert(p.id.as_str());
            parent = p
                .parent
                .as_deref()
                .and_then(|id| self.g.by_id.get(id).copied());
        }
        chain
    }

    fn item_names(&self, node: &Node) -> HashSet<String> {
        self.parent_chain(node)
            .iter()
            .filter_map(|p| item_name(p))
            .collect()
    }

    /// Nodes whose output is in scope for `id`: upstream over non-loop edges, the loop body when
    /// inside a loop (it holds the previous round's output), and whatever is upstream of the
    /// enclosing For each nodes.
    fn scope_of(&self, id: &str) -> HashSet<String> {
        let mut scope: HashSet<String> = HashSet::new();
        self.visit(id, &mut HashSet::new(), &mut scope);
        scope.remove(id);
        scope
    }

    fn visit(&self, node_id: &str, seen: &mut HashSet<String>, scope: &mut HashSet<String>) {
        if !seen.insert(node_id.to_string()) {
            return;
        }
        scope.extend(self.ancestors(node_id).into_iter().map(str::to_string));
        for e in &self.w.edges {
            if e.loop_spec.is_none()
                || !self.g.by_id.contains_key(e.from.as_str())
                || !self.g.by_id.contains_key(e.to.as_str())
            {
                continue;
            }
            let from_ancestors = self.ancestors(&e.from);
            let mut candidates: Vec<&str> = self.descendants(&e.to).into_iter().collect();
            candidates.push(e.to.as_str());
            let body: Vec<&str> = candidates
                .into_iter()
                .filter(|n| *n == e.from || from_ancestors.contains(n))
                .collect();
            if !body.contains(&node_id) {
                continue;
            }
            scope.extend(
                body.into_iter()
                    .filter(|n| *n != node_id)
                    .map(str::to_string),
            );
        }
        if let Some(parent) = self.g.by_id.get(node_id).and_then(|n| n.parent.as_deref()) {
            if self
                .g
                .by_id
                .get(parent)
                .is_some_and(|p| matches!(p.kind, NodeKind::ForEach { .. }))
            {
                self.visit(parent, seen, scope);
            }
        }
    }

    fn check_var_list(
        &mut self,
        vars: &[String],
        node_id: Option<&str>,
        scope: &HashSet<String>,
        item_names: &HashSet<String>,
        run_vars: &[&str],
    ) {
        for v in vars {
            let path: Vec<&str> = v.split('.').collect();
            let head = path[0];
            let field = path.get(1).copied();
            let label = format!("{{{{{v}}}}}");
            if head == "inputs" {
                if path.len() < 2 || !self.w.inputs.contains_key(field.unwrap_or("")) {
                    self.add(format!("{label} does not name a declared input"), node_id);
                }
            } else if head == "run" {
                if path.len() != 2 || !run_vars.contains(&field.unwrap_or("")) {
                    let list: Vec<String> = run_vars.iter().map(|f| format!("run.{f}")).collect();
                    self.add(
                        format!("{label} is not a run variable (use {})", list.join(", ")),
                        node_id,
                    );
                }
            } else if item_names.contains(head) {
                // Any path under the loop variable is allowed; the items' shape is not declared.
            } else if let Some(node) = self.g.by_id.get(head).copied() {
                if !scope.contains(head) {
                    self.add(
                        format!("{label} refers to \"{head}\", which is not upstream of this node"),
                        node_id,
                    );
                } else if path.len() > 1
                    && !output_fields(node)
                        .iter()
                        .any(|f| Some(f.as_str()) == field)
                {
                    self.add(
                        format!(
                            "{label}: \"{head}\" has no output field \"{}\"",
                            field.unwrap_or("")
                        ),
                        node_id,
                    );
                }
            } else {
                self.add(
                    format!(
                        "{label} is unknown: it names no input, upstream node or loop variable"
                    ),
                    node_id,
                );
            }
        }
    }

    fn check_vars(
        &mut self,
        text: &str,
        node_id: Option<&str>,
        scope: &HashSet<String>,
        item_names: &HashSet<String>,
    ) {
        self.check_var_list(&template_vars(text), node_id, scope, item_names, &RUN_VARS);
    }

    fn check_text(&mut self, text: &str, node: &Node) {
        let scope = self.scope_of(&node.id);
        let item_names = self.item_names(node);
        self.check_vars(text, Some(&node.id), &scope, &item_names);
    }

    fn check_text_in(&mut self, node: &Node, label: &str, text: &str, strict: bool) {
        let found = if strict {
            find_command_path(text)
        } else {
            find_absolute_path(text)
        };
        if let Some(path) = found {
            self.add(
                format!("{label} contains an absolute path (\"{path}\"); use a project input or a relative path"),
                Some(&node.id),
            );
        }
    }

    fn check_paths(&mut self, node: &Node) {
        let mut fields: Vec<(String, &str, bool)> = Vec::new();
        if let Some(title) = &node.title {
            fields.push(("title".into(), title, false));
        }
        match &node.kind {
            NodeKind::Schedule { schedule } => {
                fields.push((
                    "schedule.scheduleKind".into(),
                    &schedule.schedule_kind,
                    false,
                ));
                fields.push(("schedule.time".into(), &schedule.time, false));
                if let Some(from) = &schedule.interval_from {
                    fields.push(("schedule.intervalFrom".into(), from, false));
                }
            }
            NodeKind::Manual => {}
            NodeKind::Session {
                prompt,
                checkout,
                output,
                harness,
                model,
                effort,
                nest_under,
                ..
            } => {
                fields.push(("prompt".into(), prompt, false));
                if let Checkout::WorktreeOf(id) = checkout {
                    fields.push(("checkout.worktreeOf".into(), id, false));
                }
                for (name, field) in output.iter() {
                    if let Some(d) = &field.description {
                        fields.push((format!("output.{name}.description"), d, false));
                    }
                }
                for (label, value) in [
                    ("harness", harness),
                    ("model", model),
                    ("effort", effort),
                    ("nestUnder", nest_under),
                ] {
                    if let Some(v) = value {
                        fields.push((label.into(), v, false));
                    }
                }
            }
            NodeKind::Command { command, cwd, .. } => {
                fields.push(("command".into(), command, true));
                if let Some(Cwd::WorktreeOf(id)) = cwd {
                    fields.push(("cwd.worktreeOf".into(), id, false));
                }
            }
            NodeKind::AskMono { question, answers } => {
                fields.push(("question".into(), question, false));
                for a in answers {
                    fields.push(("answers".into(), a, false));
                }
            }
            NodeKind::If {
                condition,
                question,
                ..
            } => {
                for (label, value) in [("condition", condition), ("question", question)] {
                    if let Some(v) = value {
                        fields.push((label.into(), v, false));
                    }
                }
            }
            NodeKind::ForEach { list, item, .. } => {
                fields.push(("in".into(), list, false));
                if let Some(item) = item {
                    fields.push(("as".into(), item, false));
                }
            }
            NodeKind::Report { template } => {
                if let Some(t) = template {
                    fields.push(("template".into(), t, false));
                }
            }
        }
        for (label, text, strict) in fields {
            self.check_text_in(node, &label, text, strict);
        }
    }

    fn check_project(&mut self, node: &Node, project: &str) {
        match project_ref(project) {
            None => {
                let message = if find_absolute_path(project).is_some() || project.starts_with('/') {
                    "project is an absolute path; it must be {{inputs.<name>}} naming a project input"
                } else {
                    "project must be exactly {{inputs.<name>}} naming a project input"
                };
                self.add(message, Some(&node.id));
            }
            Some(name) => {
                let declared = self
                    .w
                    .inputs
                    .get(name)
                    .is_some_and(|i| i.ty == super::model::InputType::Project);
                if !declared {
                    self.add(
                        format!("{project} is not a declared project input"),
                        Some(&node.id),
                    );
                }
            }
        }
    }

    fn check_session_ref(&mut self, target: &str, what: &str, node: &Node) {
        match self.g.by_id.get(target) {
            None => self.add(
                format!("{what} \"{target}\" does not exist"),
                Some(&node.id),
            ),
            Some(t) if !matches!(t.kind, NodeKind::Session { .. }) => self.add(
                format!("{what} \"{target}\" must be a session node"),
                Some(&node.id),
            ),
            Some(_) => {}
        }
    }

    fn check_timeout(&mut self, node: &Node, timeout: Option<f64>) {
        let Some(t) = timeout else { return };
        // Parsed numbers are finite, so `<=` is the negation of `>` here.
        if t <= 0.0 {
            self.add("timeoutMinutes must be greater than 0", Some(&node.id));
        } else if t > MAX_TIMEOUT_MINUTES as f64 {
            self.add(
                format!("timeoutMinutes must be at most {MAX_TIMEOUT_MINUTES} (24 hours)"),
                Some(&node.id),
            );
        }
    }

    fn check_node(&mut self, node: &Node) {
        if let Some(parent) = &node.parent {
            match self.g.by_id.get(parent.as_str()) {
                None => self.add(
                    format!("parent \"{parent}\" does not exist"),
                    Some(&node.id),
                ),
                Some(p) if !matches!(p.kind, NodeKind::ForEach { .. }) => self.add(
                    format!("parent \"{parent}\" must be a For each node"),
                    Some(&node.id),
                ),
                Some(_) => {}
            }
        }
        self.check_paths(node);
        match &node.kind {
            NodeKind::Schedule { schedule } => {
                if let Some(problem) = schedule_problem(schedule) {
                    self.add(format!("Invalid schedule: {problem}"), Some(&node.id));
                }
            }
            NodeKind::Manual => {}
            NodeKind::Session {
                project,
                prompt,
                checkout,
                output,
                nest_under,
                timeout_minutes,
                ..
            } => {
                self.check_project(node, project);
                self.check_text(prompt, node);
                for name in output.keys() {
                    if !is_map_key(name) {
                        self.add(
                            format!("Invalid output field name \"{name}\": use letters, digits and underscores"),
                            Some(&node.id),
                        );
                    }
                }
                if let Checkout::WorktreeOf(target) = checkout {
                    self.check_session_ref(target, "checkout worktreeOf", node);
                }
                if let Some(target) = nest_under {
                    self.check_session_ref(target, "nestUnder", node);
                }
                self.check_timeout(node, *timeout_minutes);
            }
            NodeKind::Command {
                project,
                command,
                cwd,
                timeout_minutes,
            } => {
                self.check_project(node, project);
                self.check_text(command, node);
                // Commands are shell text, so quoting problems are security problems.
                for problem in shell_template_problems(command) {
                    self.add(problem, Some(&node.id));
                }
                if let Some(Cwd::WorktreeOf(target)) = cwd {
                    self.check_session_ref(target, "cwd worktreeOf", node);
                }
                self.check_timeout(node, *timeout_minutes);
            }
            NodeKind::AskMono { question, answers } => {
                self.check_text(question, node);
                if answers.is_empty() {
                    self.add("An Ask Mono node needs at least one answer", Some(&node.id));
                }
            }
            NodeKind::If {
                condition,
                decide,
                question,
            } => {
                let has_condition = condition.as_deref().is_some_and(|c| !js_trim(c).is_empty());
                if has_condition == decide.is_some() {
                    self.add(
                        "An If node needs either a condition or decide \"mono\", not both or neither",
                        Some(&node.id),
                    );
                }
                if has_condition {
                    if let Some(problem) = condition.as_deref().and_then(condition_syntax_problem) {
                        self.add(problem, Some(&node.id));
                    }
                }
                if let Some(c) = condition.as_deref().filter(|c| !c.is_empty()) {
                    self.check_text(c, node);
                }
                if let Some(q) = question.as_deref().filter(|q| !q.is_empty()) {
                    self.check_text(q, node);
                }
            }
            NodeKind::ForEach {
                list,
                item,
                max_parallel,
            } => {
                if js_trim(list).is_empty() {
                    self.add("A For each node needs an input list", Some(&node.id));
                } else {
                    self.check_text(list, node);
                }
                let name = item.clone().unwrap_or_else(|| "item".into());
                if !is_name(&name)
                    || RESERVED_NODE_IDS.contains(&name.as_str())
                    || self.g.by_id.contains_key(name.as_str())
                {
                    self.add(
                        format!("Invalid loop variable name \"{name}\""),
                        Some(&node.id),
                    );
                }
                if let Some(p) = max_parallel {
                    if !(is_whole(*p) && *p >= 1.0 && *p <= MAX_PARALLEL_LIMIT as f64) {
                        self.add(
                            format!(
                                "maxParallel must be a whole number from 1 to {MAX_PARALLEL_LIMIT}"
                            ),
                            Some(&node.id),
                        );
                    }
                }
            }
            NodeKind::Report { template } => {
                if node.parent.is_some() {
                    self.add(
                        "The Report node cannot be inside a For each",
                        Some(&node.id),
                    );
                }
                if let Some(template) = template.as_deref().filter(|t| !t.is_empty()) {
                    self.check_report_template(node, template);
                }
            }
        }
    }

    /// The report is rendered with `inputs`, `run`, `results` and the outputs of top-level nodes
    /// (see `compose_report`). Steps inside a For each, and its loop variable, are not in that
    /// scope: their outputs reach the report through `results`.
    fn check_report_template(&mut self, node: &Node, template: &str) {
        let loop_names: HashSet<String> = self.w.nodes.iter().filter_map(item_name).collect();
        let mut readable: Vec<String> = Vec::new();
        for v in template_vars(template) {
            let head = v.split('.').next().unwrap_or("");
            if head == "results" {
                continue;
            }
            let target = self.g.by_id.get(head).copied();
            if let Some(parent) = target.and_then(|t| t.parent.as_deref()) {
                self.add(
                    format!(
                        "The report cannot read \"{v}\": steps inside a For each are listed in {{{{results}}}} (or use {{{{{parent}.results}}}})."
                    ),
                    Some(&node.id),
                );
            } else if target.is_none()
                && head != "inputs"
                && head != "run"
                && loop_names.contains(head)
            {
                self.add(
                    format!(
                        "The report cannot read \"{v}\": the loop variable only exists inside a For each; use {{{{results}}}}."
                    ),
                    Some(&node.id),
                );
            } else {
                readable.push(v);
            }
        }
        let run_vars: Vec<&str> = RUN_VARS
            .iter()
            .chain(REPORT_RUN_VARS.iter())
            .copied()
            .collect();
        let scope = self.scope_of(&node.id);
        self.check_var_list(
            &readable,
            Some(&node.id),
            &scope,
            &HashSet::new(),
            &run_vars,
        );
    }
}

/// The input name of `{{inputs.<name>}}` when `project` is exactly that.
fn project_ref(project: &str) -> Option<&str> {
    let name = project.strip_prefix("{{inputs.")?.strip_suffix("}}")?;
    (!name.is_empty()
        && !name
            .chars()
            .any(|c| matches!(c, '{' | '}' | '.') || js_space(c)))
    .then_some(name)
}

/// What `habitSchedule()` would throw for this schedule, if anything.
fn schedule_problem(s: &Schedule) -> Option<String> {
    let mut input = json!({
        "kind": s.schedule_kind,
        "time": s.time,
        "minute": s.minute,
        "dayOfWeek": s.day_of_week,
    });
    if let Some(hours) = s.interval_hours {
        input["hours"] = json!(hours);
    }
    Schedule::from_cli(&input).err()
}

pub fn validate_workflow(w: &Workflow) -> Vec<WorkflowIssue> {
    let mut c = Checker {
        w,
        g: Graph::new(w),
        issues: Vec::new(),
        ancestors_cache: RefCell::default(),
        descendants_cache: RefCell::default(),
    };

    // --- structure ---------------------------------------------------------------------------
    let triggers: Vec<&Node> = w.nodes.iter().filter(|n| n.is_trigger()).collect();
    if triggers.len() != 1 {
        c.add(
            format!(
                "A workflow needs exactly one trigger (found {})",
                triggers.len()
            ),
            None,
        );
    }
    let reports: Vec<&Node> = w
        .nodes
        .iter()
        .filter(|n| matches!(n.kind, NodeKind::Report { .. }))
        .collect();
    if reports.len() != 1 {
        c.add(
            format!(
                "A workflow needs exactly one Report node (found {})",
                reports.len()
            ),
            None,
        );
    } else if triggers.len() == 1 {
        let reachable = reach(&triggers[0].id, &c.g.next);
        if !reachable.contains(reports[0].id.as_str()) {
            c.add(
                "The Report node must be reachable from the trigger",
                Some(&reports[0].id),
            );
        }
    }

    let mut ids: HashSet<&str> = HashSet::new();
    for node in &w.nodes {
        if !is_node_id(&node.id) {
            c.add(
                format!(
                    "Invalid node id \"{}\": use lowercase letters, digits and underscores, starting with a letter (max 40)",
                    node.id
                ),
                Some(&node.id),
            );
        }
        if RESERVED_NODE_IDS.contains(&node.id.as_str()) {
            c.add(
                format!(
                    "Node id \"{}\" is reserved (it is a template variable)",
                    node.id
                ),
                Some(&node.id),
            );
        }
        if ids.contains(node.id.as_str()) {
            c.add(format!("Duplicate node id \"{}\"", node.id), Some(&node.id));
        }
        ids.insert(&node.id);
    }

    for name in w.inputs.keys() {
        if !is_map_key(name) {
            c.add(
                format!("Invalid input name \"{name}\": use letters, digits and underscores"),
                None,
            );
        }
    }

    for e in &w.edges {
        for end in [&e.from, &e.to] {
            if !c.g.by_id.contains_key(end.as_str()) {
                c.add(
                    format!(
                        "Edge {} -> {} points at \"{end}\", which does not exist",
                        e.from, e.to
                    ),
                    None,
                );
            }
        }
    }

    let cyclic = c.g.cycle_nodes();
    if !cyclic.is_empty() {
        c.add(
            format!(
                "The workflow has a cycle without a loop edge ({}); declare a loop edge to repeat steps",
                cyclic.join(", ")
            ),
            None,
        );
    }

    // --- per node, in the order the user reads them ------------------------------------------
    for node in &w.nodes {
        c.check_node(node);
    }

    // --- edges -------------------------------------------------------------------------------
    let mut loop_sources: HashSet<&str> = HashSet::new();
    for e in &w.edges {
        let from = c.g.by_id.get(e.from.as_str()).copied();
        let to = c.g.by_id.get(e.to.as_str()).copied();
        let is_if = |n: Option<&Node>| n.is_some_and(|n| matches!(n.kind, NodeKind::If { .. }));
        if e.handle.is_some() && from.is_some() && !is_if(from) {
            c.add(
                format!(
                    "Edge {} -> {} has a handle, but only If nodes have true/false handles",
                    e.from, e.to
                ),
                Some(&e.from),
            );
        }
        if is_if(from) && e.loop_spec.is_none() && e.handle.is_none() {
            c.add(
                format!(
                    "Edge {} -> {} leaves an If node without a \"true\" or \"false\" handle",
                    e.from, e.to
                ),
                Some(&e.from),
            );
        }
        if from.is_some_and(|n| matches!(n.kind, NodeKind::Report { .. })) && e.loop_spec.is_none()
        {
            c.add(
                format!(
                    "Edge {} -> {} leaves the Report node; the Report node must be the end",
                    e.from, e.to
                ),
                Some(&e.from),
            );
        }
        let (Some(spec), Some(from), Some(to)) = (&e.loop_spec, from, to) else {
            continue;
        };

        if !matches!(
            from.kind,
            NodeKind::Session { .. } | NodeKind::Command { .. } | NodeKind::AskMono { .. }
        ) {
            c.add(
                format!(
                    "Loop edge {} -> {}: a loop can only start from a session, command or Ask Mono node",
                    e.from, e.to
                ),
                Some(&e.from),
            );
        }
        let sources = w
            .edges
            .iter()
            .filter(|o| o.loop_spec.is_some() && o.from == e.from)
            .count();
        if sources > 1 && loop_sources.insert(e.from.as_str()) {
            c.add(
                format!(
                    "Node \"{}\" is the source of more than one loop edge; only one is allowed",
                    e.from
                ),
                Some(&e.from),
            );
        }
        if !c.ancestors(&e.from).contains(e.to.as_str()) {
            c.add(
                format!(
                    "Loop edge {} -> {}: \"{}\" must be an ancestor of \"{}\"",
                    e.from, e.to, e.to, e.from
                ),
                Some(&e.from),
            );
        }
        if from.parent != to.parent {
            c.add(
                format!(
                    "Loop edge {} -> {} must stay inside the same For each",
                    e.from, e.to
                ),
                Some(&e.from),
            );
        }
        let rounds = spec.max_rounds;
        if !(is_whole(rounds) && rounds >= 1.0 && rounds <= MAX_ROUNDS_LIMIT as f64) {
            c.add(
                format!(
                    "Loop edge {} -> {}: maxRounds must be a whole number from 1 to {MAX_ROUNDS_LIMIT}",
                    e.from, e.to
                ),
                Some(&e.from),
            );
        }
        if spec.decide != "mono" {
            if js_trim(&spec.decide).is_empty() {
                c.add(
                    format!(
                        "Loop edge {} -> {}: decide must be \"mono\" or a condition",
                        e.from, e.to
                    ),
                    Some(&e.from),
                );
            } else {
                if let Some(problem) = condition_syntax_problem(&spec.decide) {
                    c.add(
                        format!("Loop edge {} -> {}: {problem}", e.from, e.to),
                        Some(&e.from),
                    );
                }
                let mut scope = c.scope_of(&e.from);
                scope.insert(e.from.clone());
                let item_names = c.item_names(from);
                c.check_vars(&spec.decide, Some(&e.from), &scope, &item_names);
            }
        }
    }

    c.issues
}
