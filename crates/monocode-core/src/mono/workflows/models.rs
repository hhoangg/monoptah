//! Which harness, model and effort a session node may name. `validate_workflow` stays
//! catalog-agnostic; `workflow_issues` is the runnable gate that adds these checks.
//!
//! The React app stores ids from its bundled list (`claude:sonnet-5`, `grok:grok-4.6`); the native
//! catalog lists the ids the CLIs use (`claude-sonnet-5`, `grok-4.6`). A stored id is
//! `<harness>:<react name>` and `store::native_model` turns it into the native id, so a workflow
//! reads the same in both apps and a model is judged by what the native catalog offers.
use super::model::{Node, NodeKind, Workflow};
use super::validate::{validate_workflow, WorkflowIssue};
use crate::provider::Kind;
use crate::store::native_model;

const PICK: &str = "Pick one in the step's model list.";

/// The message for a model id the app cannot start. Also what a failed start reports.
pub fn model_unavailable(value: &str) -> String {
    format!("Model \"{value}\" is not available. {PICK}")
}

/// The message for an effort the app cannot start. Also what a failed start reports.
pub fn effort_unavailable(value: &str) -> String {
    format!("Effort \"{value}\" is not available for this model. {PICK}")
}

/// A model a harness offers, with the effort values it accepts (none: no effort setting).
#[derive(Clone, Debug, PartialEq)]
pub struct CatalogModel {
    /// The id the CLI is started with.
    pub id: String,
    /// The concrete id behind a moving alias (`opus`), also accepted.
    pub alias_target: Option<String>,
    pub efforts: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CatalogHarness {
    /// A `Kind` id (`claude`, `codex`, ...).
    pub id: String,
    pub installed: bool,
    /// None when the list could not be read: the models of this harness are then not judged.
    pub models: Option<Vec<CatalogModel>>,
}

/// What `session_node_problem` judges against. Built once per check by `for_harnesses`, or by hand
/// in tests. A catalog built from `harnesses_used(w)` is only valid for that workflow: checking
/// another one reports the harnesses it lacks as "Provider ... is not available".
#[derive(Clone, Debug, PartialEq)]
pub struct ModelCatalog {
    /// An installed CLI is only known once the probe has run. Until then every harness reads as
    /// missing, which would flag every model, so availability is judged after the probe.
    pub probed: bool,
    pub harnesses: Vec<CatalogHarness>,
}

impl ModelCatalog {
    /// Nothing known yet: only an unknown harness id is wrong.
    pub fn unprobed() -> Self {
        Self {
            probed: false,
            harnesses: Vec::new(),
        }
    }

    pub fn harness(&self, id: &str) -> Option<&CatalogHarness> {
        self.harnesses.iter().find(|h| h.id == id)
    }

    /// The catalog for `kinds`, asking `probe` about each in its own thread. Valid only for the
    /// workflow whose `harnesses_used` produced `kinds`. The harnesses a
    /// workflow does not name are left out (use `harnesses_used`), so a check waits only for the
    /// CLIs it needs. A list that is missing or empty is unknown, not "no models".
    ///
    /// This blocks for as long as the slowest probe: call it off the UI thread. The probe is
    /// injected so that tests (and gpui tests) never reach real CLIs; `native_probe` is the real
    /// one.
    pub fn for_harnesses(kinds: &[Kind], probe: impl Fn(Kind) -> Probe + Sync) -> Self {
        let probe = &probe;
        let probed: Vec<Probe> = std::thread::scope(|scope| {
            let running: Vec<_> = kinds
                .iter()
                .map(|kind| scope.spawn(move || probe(*kind)))
                .collect();
            running
                .into_iter()
                .map(|h| h.join().unwrap_or_else(|e| std::panic::resume_unwind(e)))
                .collect()
        });
        let harnesses = kinds
            .iter()
            .zip(probed)
            .map(|(kind, probe)| CatalogHarness {
                id: kind.id().to_string(),
                installed: probe.installed,
                models: probe
                    .models
                    .filter(|rows| !rows.is_empty())
                    .map(|rows| rows.into_iter().map(catalog_model).collect()),
            })
            .collect();
        Self {
            probed: true,
            harnesses,
        }
    }

    fn usable(&self, harness: &str) -> bool {
        !self.probed || self.harness(harness).is_some_and(|h| h.installed)
    }
}

/// What a probe found for one harness.
pub struct Probe {
    pub installed: bool,
    /// None when the list could not be read.
    pub models: Option<Vec<crate::catalog::Model>>,
}

/// The setting ids React treats as reasoning effort (`EFFORT_SETTING_IDS`); OpenCode's `variant`
/// and Pi's `thinking` are such selects.
const EFFORT_SETTING_IDS: [&str; 5] = [
    "effort",
    "reasoning",
    "reasoningEffort",
    "thinking",
    "variant",
];

/// The values a model accepts as effort: its efforts, else the choices of its control that React
/// treats as effort (OpenCode lists them as a `variant` control).
fn catalog_model(m: crate::catalog::Model) -> CatalogModel {
    let efforts = if m.efforts.is_empty() {
        m.controls
            .iter()
            // A thinking toggle is not a select of levels.
            .find(|c| {
                EFFORT_SETTING_IDS.contains(&c.id.as_str())
                    && !c.choices.iter().all(|(v, _)| v == "true" || v == "false")
            })
            .map(|c| c.choices.iter().map(|(value, _)| value.clone()).collect())
            .unwrap_or_default()
    } else {
        m.efforts
    };
    CatalogModel {
        id: m.id,
        alias_target: m.alias_target,
        efforts,
    }
}

/// The harnesses `session_model_issues` looks up for `w`: the one a session names, else the one
/// its model id is prefixed with. Unknown ids are judged without a catalog.
pub fn harnesses_used(w: &Workflow) -> Vec<Kind> {
    let mut kinds = Vec::new();
    for node in &w.nodes {
        let NodeKind::Session { harness, model, .. } = &node.kind else {
            continue;
        };
        let named = harness.as_deref().filter(|h| !h.is_empty());
        let owner = match (named, model.as_deref().filter(|m| !m.is_empty())) {
            (Some(h), _) => Kind::parse(h),
            (None, Some(m)) => id_harness(m).and_then(Kind::parse),
            (None, None) => None,
        };
        if let Some(kind) = owner {
            if !kinds.contains(&kind) {
                kinds.push(kind);
            }
        }
    }
    kinds
}

/// How long a harness whose model list failed is not asked again.
const FAILURE_MEMORY: std::time::Duration = std::time::Duration::from_secs(60);

/// Remembers when a harness's listing last failed, so that validating again does not wait for the
/// same CLI timeout again.
#[derive(Default)]
struct FailureMemo(std::collections::HashMap<Kind, std::time::Instant>);

impl FailureMemo {
    fn failed_recently(&self, kind: Kind, now: std::time::Instant) -> bool {
        self.0
            .get(&kind)
            .is_some_and(|at| now.saturating_duration_since(*at) < FAILURE_MEMORY)
    }
    fn record(&mut self, kind: Kind, now: std::time::Instant) {
        self.0.insert(kind, now);
    }
    fn forget(&mut self, kind: Kind) {
        self.0.remove(&kind);
    }
}

fn failures() -> &'static std::sync::Mutex<FailureMemo> {
    static MEMO: std::sync::OnceLock<std::sync::Mutex<FailureMemo>> = std::sync::OnceLock::new();
    MEMO.get_or_init(Default::default)
}

/// The probe the model picker's data comes from: a CLI is installed when `provider::resolve`
/// finds it, and its models come from `catalog::models_for` for the default account (cached for
/// five minutes). A listing that failed or came back empty is not retried for 60 seconds.
/// It scans the disk and may start a provider: it blocks, so it must run off the UI thread.
pub fn native_probe(kind: Kind) -> Probe {
    let installed = crate::provider::resolve(kind).is_some();
    if !installed {
        return Probe {
            installed,
            models: None,
        };
    }
    let lock = || failures().lock().unwrap_or_else(|e| e.into_inner());
    if lock().failed_recently(kind, std::time::Instant::now()) {
        return Probe {
            installed,
            models: None,
        };
    }
    let models = crate::catalog::models_for(kind, &crate::accounts::Profile::default())
        .ok()
        .filter(|rows| !rows.is_empty());
    match &models {
        Some(_) => lock().forget(kind),
        None => lock().record(kind, std::time::Instant::now()),
    }
    Probe { installed, models }
}

enum Resolved<'a> {
    /// The harness has no list to check against.
    Unjudged,
    /// The bundled default model of Pi, OMP or Hermes: it has no effort setting.
    NoEfforts,
    Found(&'a CatalogModel),
}

/// The harness part of a stored model id (`claude` of `claude:sonnet-5`), when it is a harness.
fn id_harness(value: &str) -> Option<&str> {
    let (harness, rest) = value.split_once(':')?;
    (Kind::parse(harness).is_some() && !rest.is_empty()).then_some(harness)
}

/// The model a stored id names in `harness`, or None when the catalog does not offer it. The id
/// must carry the harness prefix, exactly as the React app stores it.
fn resolve<'a>(catalog: &'a ModelCatalog, harness: &str, value: &str) -> Option<Resolved<'a>> {
    if id_harness(value) != Some(harness) || !catalog.usable(harness) {
        return None;
    }
    let rest = &value[harness.len() + 1..];
    // The bundled default models; they have no effort setting.
    if rest == "default" && ["pi", "omp", "hermes"].contains(&harness) {
        return Some(Resolved::NoEfforts);
    }
    // A variant suffix (`[1m]`) is not part of a catalog id.
    if rest.contains('[') {
        return None;
    }
    let Some(list) = catalog.harness(harness).and_then(|h| h.models.as_ref()) else {
        return Some(Resolved::Unjudged);
    };
    let native = native_model(value, harness)?;
    list.iter()
        .find(|m| m.id == native || m.alias_target.as_deref() == Some(native.as_str()))
        .map(Resolved::Found)
}

/// The message for what is wrong with the harness, model and effort a session node names, if
/// anything. It applies the rules `sessions.start` applies when the node runs, so what is flagged
/// is what would fail there. A node that sets nothing runs with the Mono's own model and is fine.
/// None for a node that is not a session.
pub fn session_node_problem(node: &Node, catalog: &ModelCatalog) -> Option<String> {
    let NodeKind::Session {
        harness,
        model,
        effort,
        ..
    } = &node.kind
    else {
        return None;
    };
    let nonempty = |v: &Option<String>| v.clone().filter(|v| !v.is_empty());
    let harness = nonempty(harness);
    if let Some(name) = &harness {
        if Kind::parse(name).is_none() || !catalog.usable(name) {
            return Some(format!("Provider \"{name}\" is not available. {PICK}"));
        }
    }
    let mut found = None;
    if let Some(value) = nonempty(model) {
        // Without a harness the runner starts the model's own, so the model must exist by itself.
        let owner = harness.as_deref().or_else(|| id_harness(&value));
        match owner.and_then(|owner| resolve(catalog, owner, &value)) {
            Some(resolved) => found = Some(resolved),
            None => return Some(model_unavailable(&value)),
        }
    }
    // Without a model the effort belongs to the Mono's model, which is not known here.
    if let Some(effort) = nonempty(effort) {
        let offered = match found {
            Some(Resolved::Found(model)) => Some(model.efforts.contains(&effort)),
            Some(Resolved::NoEfforts) => Some(false),
            Some(Resolved::Unjudged) | None => None,
        };
        if offered == Some(false) {
            return Some(effort_unavailable(&effort));
        }
    }
    None
}

/// Session nodes whose harness, model or effort the app cannot start.
///
/// `validate_workflow` stays catalog-agnostic; use `workflow_issues` wherever a workflow is
/// gated on being runnable.
pub fn session_model_issues(w: &Workflow, catalog: &ModelCatalog) -> Vec<WorkflowIssue> {
    w.nodes
        .iter()
        .filter_map(|node| {
            session_node_problem(node, catalog).map(|message| WorkflowIssue {
                node_id: Some(node.id.clone()),
                message,
            })
        })
        .collect()
}

/// Everything wrong with a workflow's graph and with the models its sessions name.
pub fn workflow_issues(w: &Workflow, catalog: &ModelCatalog) -> Vec<WorkflowIssue> {
    let mut issues = validate_workflow(w);
    issues.extend(session_model_issues(w, catalog));
    issues
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mono::workflows::fixtures::workflow_from;
    use crate::provider::Kind;

    const NODES_TAIL: &str = r#"{"id":"report","type":"report"}"#;

    fn session(extra: &str) -> Workflow {
        let extra = if extra.is_empty() {
            String::new()
        } else {
            format!(",{extra}")
        };
        workflow_from(&format!(
            r#"{{"id":"wf-abcd1234","name":"Models","inputs":{{"project":{{"type":"project"}}}},
                "inputValues":{{"project":"/work/api"}},
                "nodes":[{{"id":"trigger","type":"manual"}},
                  {{"id":"hi","type":"session","project":"{{{{inputs.project}}}}","prompt":"hi","checkout":"root","output":{{}}{extra}}},
                  {NODES_TAIL}],
                "edges":[{{"from":"trigger","to":"hi"}},{{"from":"hi","to":"report"}}],
                "enabled":false,"createdAt":0}}"#
        ))
    }

    fn plain() -> Workflow {
        workflow_from(&format!(
            r#"{{"id":"wf-abcd1234","name":"Plain","inputs":{{}},"inputValues":{{}},
                "nodes":[{{"id":"trigger","type":"manual"}},{NODES_TAIL}],
                "edges":[{{"from":"trigger","to":"report"}}],"enabled":false,"createdAt":0}}"#
        ))
    }

    fn unavailable(value: &str) -> String {
        format!("Model \"{value}\" is not available. Pick one in the step's model list.")
    }

    fn model(id: &str, efforts: &[&str]) -> CatalogModel {
        CatalogModel {
            id: id.into(),
            alias_target: None,
            efforts: efforts.iter().map(|e| e.to_string()).collect(),
        }
    }

    /// Claude and Grok installed, every other harness missing: the TS tests' `NONE` plus claude
    /// and grok. Models are the native ids behind the bundled React ids.
    fn catalog(probed: bool) -> ModelCatalog {
        let entry = |kind: Kind| {
            let (installed, models): (bool, Vec<CatalogModel>) = match kind {
                Kind::Claude => (
                    true,
                    vec![
                        model("claude-sonnet-5", &[]),
                        model("claude-sonnet-4-6", &[]),
                        model("claude-opus-5", &["low", "high"]),
                    ],
                ),
                Kind::Grok => (
                    true,
                    vec![model("grok-4.6", &["low", "medium", "high", "xhigh"])],
                ),
                Kind::Cursor => (false, vec![model("grok-4.6", &["low"])]),
                _ => (false, vec![]),
            };
            CatalogHarness {
                id: kind.id().into(),
                installed,
                models: Some(models),
            }
        };
        ModelCatalog {
            probed,
            harnesses: Kind::ALL.into_iter().map(entry).collect(),
        }
    }

    fn issues(w: &Workflow) -> Vec<WorkflowIssue> {
        session_model_issues(w, &catalog(true))
    }

    fn issue(message: String) -> WorkflowIssue {
        WorkflowIssue {
            node_id: Some("hi".into()),
            message,
        }
    }

    mod session_model_issues_tests {
        use super::*;

        #[test]
        fn flags_the_free_text_model_id_a_session_was_saved_with() {
            assert_eq!(
                issues(&session(r#""model":"claude-sonnet-5-5""#)),
                [issue(unavailable("claude-sonnet-5-5"))]
            );
        }

        #[test]
        fn flags_a_model_name_that_is_not_an_app_id() {
            assert_eq!(
                issues(&session(r#""model":"sonnet""#)),
                [issue(unavailable("sonnet"))]
            );
        }

        #[test]
        fn flags_an_unknown_harness() {
            let found = issues(&session(r#""harness":"gpt""#));
            assert_eq!(found.len(), 1);
            assert_eq!(found[0].node_id.as_deref(), Some("hi"));
            assert!(found[0].message.contains("\"gpt\""));
            assert_eq!(
                found[0].message,
                "Provider \"gpt\" is not available. Pick one in the step's model list."
            );
        }

        #[test]
        fn flags_a_harness_that_is_not_installed() {
            let found = issues(&session(r#""harness":"codex""#));
            assert_eq!(found.len(), 1);
            assert!(found[0].message.contains("\"codex\""));
        }

        #[test]
        fn flags_a_model_that_belongs_to_another_harness_than_the_one_set() {
            assert_eq!(
                issues(&session(r#""harness":"grok","model":"claude:sonnet-5""#)),
                [issue(unavailable("claude:sonnet-5"))]
            );
        }

        #[test]
        fn flags_a_model_whose_own_harness_is_not_installed() {
            assert_eq!(issues(&session(r#""model":"cursor:grok-4.6""#)).len(), 1);
        }

        #[test]
        fn flags_an_effort_the_model_does_not_offer() {
            let found = issues(&session(
                r#""harness":"grok","model":"grok:grok-4.6","effort":"turbo""#,
            ));
            assert_eq!(found.len(), 1);
            assert!(found[0].message.contains("\"turbo\""));
            assert_eq!(
                found[0].message,
                "Effort \"turbo\" is not available for this model. Pick one in the step's model list."
            );
        }

        #[test]
        fn flags_an_effort_on_a_model_with_no_effort_setting() {
            assert_eq!(
                issues(&session(
                    r#""harness":"claude","model":"claude:sonnet-4.6","effort":"high""#
                ))
                .len(),
                1
            );
        }

        #[test]
        fn accepts_exact_ids_and_a_valid_effort() {
            assert_eq!(
                issues(&session(
                    r#""harness":"grok","model":"grok:grok-4.6","effort":"xhigh""#
                )),
                []
            );
            assert_eq!(issues(&session(r#""model":"claude:sonnet-5""#)), []);
            assert_eq!(issues(&session(r#""harness":"claude""#)), []);
        }

        #[test]
        fn raises_nothing_for_a_session_that_sets_nothing() {
            assert_eq!(issues(&session("")), []);
        }

        #[test]
        fn does_not_judge_availability_before_the_first_probe() {
            let before = catalog(false);
            assert_eq!(
                session_model_issues(
                    &session(r#""harness":"cursor","model":"cursor:grok-4.6""#),
                    &before
                ),
                []
            );
            // A harness the app does not know is still wrong before the probe.
            assert_eq!(
                session_model_issues(&session(r#""harness":"gpt""#), &before).len(),
                1
            );
        }

        #[test]
        fn ignores_nodes_that_are_not_sessions() {
            assert_eq!(issues(&plain()), []);
        }
    }

    mod workflow_issues_tests {
        use super::*;

        #[test]
        fn adds_the_model_issues_to_the_graph_issues() {
            let w = session(r#""model":"sonnet""#);
            assert_eq!(
                workflow_issues(&w, &catalog(true)),
                [issue(unavailable("sonnet"))]
            );
            let mut empty = w.clone();
            empty.nodes.clear();
            assert!(!workflow_issues(&empty, &catalog(true)).is_empty());
        }
    }

    mod native_catalog {
        use super::*;
        use crate::catalog::Model;

        fn native(id: &str, alias: Option<&str>, efforts: &[&str]) -> Model {
            Model {
                id: id.into(),
                name: id.into(),
                alias_target: alias.map(str::to_owned),
                efforts: efforts.iter().map(|e| e.to_string()).collect(),
                default_effort: None,
                fast: false,
                thinking: false,
                default: false,
                tiers: vec![],
                controls: vec![],
            }
        }

        use crate::catalog::ModelControl;
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::time::{Duration, Instant};

        fn with_control(mut model: Model, id: &str, choices: &[&str]) -> Model {
            model.controls.push(ModelControl {
                id: id.into(),
                name: id.into(),
                current: choices[0].into(),
                choices: choices
                    .iter()
                    .map(|c| (c.to_string(), c.to_string()))
                    .collect(),
            });
            model
        }

        fn probe(
            installed: impl Fn(Kind) -> bool + Sync,
            models: impl Fn(Kind) -> Option<Vec<Model>> + Sync,
        ) -> impl Fn(Kind) -> Probe + Sync {
            move |kind| {
                let installed = installed(kind);
                Probe {
                    installed,
                    models: if installed { models(kind) } else { None },
                }
            }
        }

        #[test]
        fn is_built_from_the_installed_cli_flag_and_the_provider_catalog() {
            let built = ModelCatalog::for_harnesses(
                &Kind::ALL,
                probe(
                    |kind| kind == Kind::Claude,
                    |kind| match kind {
                        Kind::Claude => Some(vec![
                            native("opus", Some("claude-opus-4-8"), &["low", "high"]),
                            native("claude-sonnet-5", None, &[]),
                        ]),
                        _ => None,
                    },
                ),
            );
            assert!(built.probed);
            assert_eq!(built.harnesses.len(), Kind::ALL.len());
            let claude = built.harness("claude").unwrap();
            assert!(claude.installed);
            assert_eq!(claude.models.as_ref().unwrap().len(), 2);
            assert!(!built.harness("codex").unwrap().installed);
            assert_eq!(built.harness("codex").unwrap().models, None);
            // The React id `claude:opus-4.8` reaches the alias target, `claude:sonnet-5` the id.
            let w = session(r#""model":"claude:opus-4.8","effort":"high""#);
            assert_eq!(session_model_issues(&w, &built), []);
            let w = session(r#""model":"claude:sonnet-5""#);
            assert_eq!(session_model_issues(&w, &built), []);
            let w = session(r#""model":"claude:opus-4.8","effort":"max""#);
            assert_eq!(session_model_issues(&w, &built).len(), 1);
        }

        #[test]
        fn an_empty_catalog_is_unknown_not_empty() {
            let built =
                ModelCatalog::for_harnesses(&[Kind::Claude], probe(|_| true, |_| Some(vec![])));
            assert_eq!(built.harness("claude").unwrap().models, None);
        }

        #[test]
        fn a_catalog_that_could_not_be_read_does_not_flag_the_models() {
            let built = ModelCatalog::for_harnesses(&[Kind::Claude], probe(|_| true, |_| None));
            let w = session(r#""model":"claude:whatever","effort":"turbo""#);
            assert_eq!(session_model_issues(&w, &built), []);
        }

        #[test]
        fn an_unprobed_catalog_judges_nothing_but_unknown_harnesses() {
            let w = session(r#""harness":"codex","model":"codex:x","effort":"turbo""#);
            assert_eq!(session_model_issues(&w, &ModelCatalog::unprobed()), []);
        }

        #[test]
        fn opencode_variant_is_its_effort() {
            let opencode = with_control(
                native("anthropic/claude-sonnet-4-5", None, &[]),
                "variant",
                &["low", "high"],
            );
            let built = ModelCatalog::for_harnesses(
                &[Kind::OpenCode],
                probe(|_| true, move |_| Some(vec![opencode.clone()])),
            );
            let node = |effort: &str| {
                session(&format!(
                    r#""harness":"opencode","model":"opencode:anthropic/claude-sonnet-4-5","effort":"{effort}""#
                ))
            };
            assert_eq!(session_model_issues(&node("high"), &built), []);
            assert_eq!(session_model_issues(&node("turbo"), &built).len(), 1);
        }

        #[test]
        fn only_effort_controls_count_and_a_toggle_is_not_a_level_list() {
            let model = with_control(
                with_control(native("m", None, &[]), "agent", &["build", "plan"]),
                "thinking",
                &["false", "true"],
            );
            let got = catalog_model(model);
            assert!(got.efforts.is_empty());
            let listed =
                catalog_model(with_control(native("m", None, &["low"]), "variant", &["x"]));
            assert_eq!(listed.efforts, ["low"]);
            let pi = catalog_model(with_control(
                native("m", None, &[]),
                "thinking",
                &["off", "low"],
            ));
            assert_eq!(pi.efforts, ["off", "low"]);
        }

        #[test]
        fn only_pi_omp_and_hermes_have_a_default_model_and_it_has_no_effort() {
            let built = ModelCatalog::for_harnesses(
                &Kind::ALL,
                probe(|_| true, |_| Some(vec![native("x", None, &["high"])])),
            );
            for harness in ["pi", "omp", "hermes"] {
                let ok = session(&format!(
                    r#""harness":"{harness}","model":"{harness}:default""#
                ));
                assert_eq!(session_model_issues(&ok, &built), [], "{harness}");
                let effort = session(&format!(
                    r#""harness":"{harness}","model":"{harness}:default","effort":"high""#
                ));
                assert_eq!(session_model_issues(&effort, &built).len(), 1, "{harness}");
            }
            let other = session(r#""harness":"claude","model":"claude:default""#);
            assert_eq!(session_model_issues(&other, &built).len(), 1);
            let missing = ModelCatalog::for_harnesses(&[Kind::Pi], probe(|_| false, |_| None));
            let w = session(r#""harness":"pi","model":"pi:default""#);
            assert_eq!(session_model_issues(&w, &missing).len(), 1);
        }

        #[test]
        fn only_the_harnesses_a_workflow_names_are_looked_up() {
            let w = session(r#""model":"claude:sonnet-5""#);
            assert_eq!(harnesses_used(&w), [Kind::Claude]);
            let w = session(r#""harness":"grok","model":"claude:sonnet-5""#);
            assert_eq!(harnesses_used(&w), [Kind::Grok]);
            assert_eq!(harnesses_used(&session("")), []);
            assert_eq!(harnesses_used(&session(r#""harness":"gpt""#)), []);
            assert_eq!(harnesses_used(&plain()), []);
            // A catalog for just those harnesses judges the node the same way.
            let w = session(r#""model":"claude:sonnet-5","effort":"turbo""#);
            let asked = AtomicUsize::new(0);
            let built = ModelCatalog::for_harnesses(&harnesses_used(&w), |_| {
                asked.fetch_add(1, Ordering::SeqCst);
                Probe {
                    installed: true,
                    models: Some(vec![native("claude-sonnet-5", None, &["low"])]),
                }
            });
            assert_eq!(asked.load(Ordering::SeqCst), 1);
            assert_eq!(session_model_issues(&w, &built).len(), 1);
        }

        #[test]
        fn the_probes_run_in_parallel() {
            // Each probe waits (bounded) until both have started: a serial run times out and
            // fails instead of hanging.
            let started = AtomicUsize::new(0);
            let saw_both = AtomicUsize::new(0);
            let built = ModelCatalog::for_harnesses(&[Kind::Claude, Kind::Grok], |_| {
                started.fetch_add(1, Ordering::SeqCst);
                let deadline = Instant::now() + Duration::from_secs(5);
                while started.load(Ordering::SeqCst) < 2 && Instant::now() < deadline {
                    std::thread::sleep(Duration::from_millis(5));
                }
                if started.load(Ordering::SeqCst) == 2 {
                    saw_both.fetch_add(1, Ordering::SeqCst);
                }
                Probe {
                    installed: true,
                    models: None,
                }
            });
            assert_eq!(built.harnesses.len(), 2);
            assert_eq!(saw_both.load(Ordering::SeqCst), 2);
        }

        #[test]
        fn a_failed_listing_is_remembered_for_a_minute() {
            let start = Instant::now();
            let mut memo = FailureMemo::default();
            assert!(!memo.failed_recently(Kind::Claude, start));
            memo.record(Kind::Claude, start);
            assert!(memo.failed_recently(Kind::Claude, start + Duration::from_secs(59)));
            assert!(!memo.failed_recently(Kind::Claude, start + Duration::from_secs(60)));
            assert!(!memo.failed_recently(Kind::Codex, start));
            memo.forget(Kind::Claude);
            assert!(!memo.failed_recently(Kind::Claude, start));
        }

        #[test]
        fn a_bare_native_id_is_not_a_stored_id() {
            // The React app would flag it, so the same file reads the same in both apps.
            let w = session(r#""harness":"claude","model":"claude-sonnet-5""#);
            assert_eq!(issues(&w), [issue(unavailable("claude-sonnet-5"))]);
        }

        #[test]
        fn a_variant_suffix_is_not_a_catalog_id() {
            let w = session(r#""model":"claude:sonnet-5[1m]""#);
            assert_eq!(issues(&w), [issue(unavailable("claude:sonnet-5[1m]"))]);
        }
    }
}
