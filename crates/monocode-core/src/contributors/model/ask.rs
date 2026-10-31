//! Ask Agent: the snapshot and prompt the user's chips and checkboxes ask for.
//! Port of `askAgent.ts`. The app side creates a chat session in the project,
//! writes `snapshot_json` to a file called `file_name` (an attachment, see
//! `crate::attachment`), attaches it and sends `prompt` as the first message.

use super::js_trim;
use super::pipeline::ContributorsModel;
use super::snapshot::{
    build_agent_prompt, build_snapshot, snapshot_file_name, AgentPrompt, Include, Snapshot,
    SnapshotInput, SnapshotPerson, SnapshotScope,
};
use crate::provider::Kind;
use chrono::{DateTime, Local};
use regex::Regex;
use std::collections::HashMap;
use std::sync::LazyLock;

/// Who an Ask Agent question is about: the whole team, one person, or the user.
pub type AskScope = SnapshotScope;

/// What the Contributors page hands to the app when the user presses "Start
/// session". Everything is already final: the snapshot reflects the chips and
/// checkboxes the user chose, and `prompt` already contains `question` and the
/// git/gh permission wording.
#[derive(Clone, Debug, PartialEq)]
pub struct AskAgentRequest {
    /// The user's question, trimmed, as typed. Useful for a session title.
    pub question: String,
    /// Full first message: context lines, tool permission and the question.
    pub prompt: String,
    /// Pretty-printed JSON text of the snapshot, to write into the attachment.
    pub snapshot_json: String,
    /// Attachment name, `contributors-<period id>.json`.
    pub file_name: String,
    /// Provider chosen in the dialog. It is set whenever the dialog knew any
    /// provider (the default one is preselected), so its presence does not
    /// mean the user changed it.
    pub harness: Option<Kind>,
    /// The real scope. Inside the snapshot and prompt it is aliased when the
    /// people chip was removed.
    pub scope: AskScope,
}

/// Parts of the snapshot the user can leave out with a chip.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AskContextPart {
    People,
    Scores,
    Formula,
    Ai,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AskOptions {
    /// Include the per-PR breakdown (`prs`).
    pub per_pr: bool,
    /// Include the raw commit list (`commits`).
    pub raw_commits: bool,
    /// Word the prompt so the agent may run read-only git / gh.
    pub allow_tools: bool,
    /// Chips the user removed.
    pub omit: Vec<AskContextPart>,
}

impl Default for AskOptions {
    fn default() -> Self {
        Self {
            per_pr: true,
            raw_commits: false,
            allow_tools: true,
            omit: vec![],
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AskChipId {
    Project,
    Branch,
    Period,
    Focus,
    People,
    Scores,
    Formula,
    Ai,
}

impl AskChipId {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Project => "project",
            Self::Branch => "branch",
            Self::Period => "period",
            Self::Focus => "focus",
            Self::People => "people",
            Self::Scores => "scores",
            Self::Formula => "formula",
            Self::Ai => "ai",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AskChip {
    pub id: AskChipId,
    pub label: String,
    pub removable: bool,
    /// Tooltip saying what removing the chip leaves out.
    pub hint: Option<&'static str>,
}

pub fn person_name_of(model: &ContributorsModel, scope: &AskScope) -> Option<String> {
    scope.person_id().map(|id| model.resolver.name_of(id))
}

pub fn ask_subtitle(scope: &AskScope, person_name: Option<&str>) -> String {
    match scope {
        AskScope::Team => "about the whole team".into(),
        AskScope::Me { .. } => "about your score".into(),
        AskScope::Person { .. } => format!("about {}", person_name.unwrap_or("this person")),
    }
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// Everyone the snapshot lists: the period's ranking plus the stats rows.
fn listed_person_ids(model: &ContributorsModel) -> Vec<String> {
    let mut rows: Vec<_> = model.leaderboard.rows.iter().collect();
    rows.sort_by_key(|row| row.rank);
    let mut ids: Vec<String> = rows.iter().map(|row| row.person_id.clone()).collect();
    for row in &model.stats.rows {
        if !ids.contains(&row.person_id) {
            ids.push(row.person_id.clone());
        }
    }
    ids
}

pub fn ask_context_chips(
    model: &ContributorsModel,
    project: &str,
    scope: &AskScope,
) -> Vec<AskChip> {
    let name = person_name_of(model, scope);
    let ids = listed_person_ids(model);
    let people = model.resolver.people();
    let identities: usize = ids
        .iter()
        .map(|id| {
            people
                .iter()
                .find(|p| p.id == *id)
                .map_or(1, |p| p.identities.len())
        })
        .sum();
    let count = format!(
        "{} · {}",
        plural(ids.len(), "person", "people"),
        plural(identities, "identity", "identities")
    );
    let fixed = |id, label: String| AskChip {
        id,
        label,
        removable: false,
        hint: None,
    };
    let mut chips = vec![
        fixed(
            AskChipId::Project,
            crate::telegram::forward::project_name(project),
        ),
        fixed(AskChipId::Branch, model.raw.branch.clone()),
        fixed(AskChipId::Period, model.period.label.clone()),
    ];
    if let Some(id) = scope.person_id() {
        let who = name.as_deref().unwrap_or(id);
        let role = if matches!(scope, AskScope::Me { .. }) {
            "you"
        } else {
            "focus"
        };
        chips.push(fixed(AskChipId::Focus, format!("{who} ({role})")));
    }
    let removable = |id, label: String, hint| AskChip {
        id,
        label,
        removable: true,
        hint: Some(hint),
    };
    chips.extend([
        removable(
            AskChipId::People,
            if matches!(scope, AskScope::Team) {
                count
            } else {
                format!("Whole team: {count}")
            },
            "Remove to leave out e-mail addresses and logins: people are then sent under their display name and an alias (person-1, person-2, ...)",
        ),
        removable(
            AskChipId::Scores,
            "Scores + breakdown".into(),
            "Remove to send only scores and ranks, without the per-dimension breakdown",
        ),
        removable(
            AskChipId::Formula,
            "Scoring formula".into(),
            "Remove to leave out the explanation of how the score is calculated",
        ),
        removable(
            AskChipId::Ai,
            "AI-assisted stats".into(),
            "Remove to leave out the share of AI-assisted work",
        ),
    ]);
    chips
}

fn omit_text(part: AskContextPart) -> &'static str {
    match part {
        AskContextPart::People => "identity lists (e-mails and logins)",
        AskContextPart::Scores => "score breakdowns",
        AskContextPart::Formula => "the scoring formula",
        AskContextPart::Ai => "AI-assisted stats",
    }
}

fn is_email(text: &str) -> bool {
    text.contains('@')
}

static AUTHOR_ADDRESS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\s*<[^>]*>\s*$").expect("author address pattern"));

/// Stable aliases (person-1, person-2, ...) in rank order, so no e-mail address
/// reaches the agent through an id, a display name or a commit author.
struct Anonymiser<'a> {
    model: &'a ContributorsModel,
    /// Ids in the order they got an alias.
    order: Vec<String>,
    aliases: HashMap<String, String>,
}

impl<'a> Anonymiser<'a> {
    fn new(model: &'a ContributorsModel) -> Self {
        let order = listed_person_ids(model);
        let aliases = order
            .iter()
            .enumerate()
            .map(|(i, id)| (id.clone(), format!("person-{}", i + 1)))
            .collect();
        Self {
            model,
            order,
            aliases,
        }
    }

    fn alias_of(&mut self, id: &str) -> String {
        if let Some(alias) = self.aliases.get(id) {
            return alias.clone();
        }
        let alias = format!("person-{}", self.aliases.len() + 1);
        self.aliases.insert(id.to_string(), alias.clone());
        self.order.push(id.to_string());
        alias
    }

    /// The display name, or the alias when the name is an address.
    fn name_of(&mut self, id: &str, name: &str) -> String {
        if is_email(name) {
            self.alias_of(id)
        } else {
            name.to_string()
        }
    }

    fn id_of_name(&self, name: &str) -> Option<String> {
        self.order
            .iter()
            .find(|id| self.model.resolver.name_of(id) == name)
            .cloned()
    }

    fn scope(&mut self, scope: &AskScope) -> AskScope {
        match scope {
            AskScope::Team => AskScope::Team,
            AskScope::Person { person_id } => AskScope::Person {
                person_id: self.alias_of(person_id),
            },
            AskScope::Me { person_id } => AskScope::Me {
                person_id: self.alias_of(person_id),
            },
        }
    }

    /// Replaces an id and a name in place.
    fn person(&mut self, person_id: &mut String, name: &mut String) {
        let alias = self.alias_of(person_id);
        *name = self.name_of(person_id, name);
        *person_id = alias;
    }

    fn commit(&mut self, person: &mut Option<String>, author: &mut String) {
        let alias = match person.as_deref() {
            None => None,
            Some(p) if is_email(p) => Some(match self.id_of_name(p) {
                Some(id) => self.alias_of(&id),
                None => "unknown".to_string(),
            }),
            Some(p) => Some(p.to_string()),
        };
        let bare = AUTHOR_ADDRESS.replace(author, "").into_owned();
        *author = if is_email(&bare) || js_trim(&bare).is_empty() {
            alias.clone().unwrap_or_else(|| "unknown".into())
        } else {
            bare
        };
        *person = alias;
    }
}

struct Built {
    snapshot: Snapshot,
    scope: AskScope,
    person_name: Option<String>,
}

pub struct AskSnapshotInput<'a> {
    pub model: &'a ContributorsModel,
    pub project: &'a str,
    pub scope: AskScope,
    pub options: &'a AskOptions,
    pub now: &'a DateTime<Local>,
}

fn build_ask_snapshot_parts(input: &AskSnapshotInput) -> Built {
    let (model, scope, options) = (input.model, &input.scope, input.options);
    let mut snapshot = build_snapshot(&SnapshotInput {
        project: &crate::telegram::forward::project_name(input.project),
        branch: &model.raw.branch,
        period: &model.period,
        scores: &model.current,
        leaderboard: &model.leaderboard,
        stats: Some(&model.stats),
        config: &model.config,
        scope: scope.clone(),
        include: Include {
            per_pr: options.per_pr,
            raw_commits: options.raw_commits,
        },
        raw: Some(&model.raw),
        gh_available: model.raw.gh_available,
        resolver: &model.resolver,
        now: input.now,
    });
    let omit = |part: AskContextPart| options.omit.contains(&part);
    let (no_scores, no_ai, no_people) = (
        omit(AskContextPart::Scores),
        omit(AskContextPart::Ai),
        omit(AskContextPart::People),
    );
    let mut anon = no_people.then(|| Anonymiser::new(model));

    if omit(AskContextPart::Formula) {
        snapshot.formula = None;
    }
    // The order the aliases are handed out in: scope, people, focus, stats,
    // PRs, commits.
    let mut scope_out = scope.clone();
    if let Some(anon) = anon.as_mut() {
        scope_out = anon.scope(scope);
        snapshot.scope = scope_out.clone();
    }
    let mut strip_person = |p: &mut SnapshotPerson| {
        if let Some(anon) = anon.as_mut() {
            anon.person(&mut p.person_id, &mut p.name);
        }
        if no_scores {
            p.parts = None;
            p.breakdown = None;
        }
        if no_ai {
            p.ai_assisted_pct = None;
        }
        if no_people {
            p.identities = None;
        }
    };
    for p in &mut snapshot.people {
        strip_person(p);
    }
    if let Some(Some(focus)) = &mut snapshot.focus {
        strip_person(focus);
    }
    if let Some(stats) = &mut snapshot.stats {
        for row in &mut stats.rows {
            if let Some(anon) = anon.as_mut() {
                anon.person(&mut row.person_id, &mut row.name);
            }
            if no_ai {
                row.ai_assisted_pct = None;
            }
            if no_people {
                row.identities = None;
            }
        }
    }
    if let Some(prs) = &mut snapshot.prs {
        for pr in prs {
            if let Some(anon) = anon.as_mut() {
                anon.person(&mut pr.person_id, &mut pr.person_name);
            }
            if no_ai {
                pr.ai_assisted = None;
            }
        }
    }
    if let (Some(commits), Some(anon)) = (&mut snapshot.commits, anon.as_mut()) {
        for c in commits {
            anon.commit(&mut c.person, &mut c.author);
        }
    }

    if !options.omit.is_empty() {
        // a JS `Set`: each part once, in the order first given
        let mut seen: Vec<AskContextPart> = Vec::new();
        for part in &options.omit {
            if !seen.contains(part) {
                seen.push(*part);
            }
        }
        let left_out: Vec<&str> = seen.iter().map(|p| omit_text(*p)).collect();
        snapshot.notes.push(format!(
            "Left out at the user's request: {}.",
            left_out.join(", ")
        ));
    }
    if anon.is_some() {
        snapshot.notes.push(
            "People are identified by aliases (person-1, person-2, ... in rank order) in every personId field; e-mail addresses and logins were removed."
                .into(),
        );
    }

    let name = person_name_of(model, scope);
    let person_name = match (anon.as_mut(), scope.person_id()) {
        (Some(anon), Some(id)) => name.as_deref().map(|n| anon.name_of(id, n)),
        _ => name,
    };
    Built {
        snapshot,
        scope: scope_out,
        person_name,
    }
}

/// The snapshot as the user configured it: the model's snapshot minus the removed chips.
pub fn build_ask_snapshot(input: &AskSnapshotInput) -> Snapshot {
    build_ask_snapshot_parts(input).snapshot
}

/// What the attached file holds, for the prompt; mirrors the snapshot built above.
fn contents_of(options: &AskOptions) -> Vec<String> {
    let omit = |part: AskContextPart| options.omit.contains(&part);
    let mut contents = vec!["scores and ranks"];
    if !omit(AskContextPart::Scores) {
        contents.push("per-dimension breakdowns");
    }
    if !omit(AskContextPart::People) {
        contents.push("identity mapping");
    }
    if !omit(AskContextPart::Formula) {
        contents.push("the scoring formula");
    }
    if options.per_pr {
        contents.push("per-PR details");
    }
    if options.raw_commits {
        contents.push("the raw commit list");
    }
    if !omit(AskContextPart::Ai) {
        contents.push("AI-assisted shares");
    }
    contents.into_iter().map(String::from).collect()
}

pub struct AskRequestInput<'a> {
    pub model: &'a ContributorsModel,
    pub project: &'a str,
    pub scope: AskScope,
    pub question: &'a str,
    pub options: &'a AskOptions,
    pub harness: Option<Kind>,
    pub now: &'a DateTime<Local>,
}

pub fn build_ask_request(input: &AskRequestInput) -> AskAgentRequest {
    let model = input.model;
    let question = js_trim(input.question).to_string();
    let file_name = snapshot_file_name(&model.period);
    let built = build_ask_snapshot_parts(&AskSnapshotInput {
        model,
        project: input.project,
        scope: input.scope.clone(),
        options: input.options,
        now: input.now,
    });
    let contains = contents_of(input.options);
    let prompt = build_agent_prompt(&AgentPrompt {
        question: &question,
        scope: &built.scope,
        file_name: &file_name,
        allow_tools: input.options.allow_tools,
        project_name: &crate::telegram::forward::project_name(input.project),
        period_label: &model.period.label,
        person_name: built.person_name.as_deref(),
        contains: Some(&contains),
    });
    AskAgentRequest {
        question,
        prompt,
        snapshot_json: serde_json::to_string_pretty(&built.snapshot)
            .expect("a snapshot always serializes"),
        file_name,
        harness: input.harness,
        scope: input.scope.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contributors::model::config::default_config;
    use crate::contributors::model::period::{current_period, PeriodKind};
    use crate::contributors::model::pipeline::{build_contributors_model, ModelInput};
    use crate::contributors::model::test_fixtures::*;
    use crate::contributors::{GitIdentity, GitUser};
    use serde_json::{json, Value};
    use std::sync::Arc;

    fn model() -> ContributorsModel {
        leaderboard_model()
    }

    fn row_id(model: &ContributorsModel, name: &str) -> String {
        model
            .leaderboard
            .rows
            .iter()
            .find(|r| r.name == name)
            .unwrap()
            .person_id
            .clone()
    }

    fn options(patch: impl FnOnce(&mut AskOptions)) -> AskOptions {
        let mut o = AskOptions::default();
        patch(&mut o);
        o
    }

    fn snapshot_of(model: &ContributorsModel, scope: AskScope, options: &AskOptions) -> Value {
        serde_json::to_value(build_ask_snapshot(&AskSnapshotInput {
            model,
            project: "/work/monoptah",
            scope,
            options,
            now: &leaderboard_now(),
        }))
        .unwrap()
    }

    fn snap(patch: impl FnOnce(&mut AskOptions)) -> Value {
        snapshot_of(&model(), AskScope::Team, &options(patch))
    }

    fn array<'a>(value: &'a Value, key: &str) -> &'a Vec<Value> {
        value[key]
            .as_array()
            .unwrap_or_else(|| panic!("{key} is not an array"))
    }

    fn request(
        model: &ContributorsModel,
        scope: AskScope,
        question: &str,
        options: &AskOptions,
        harness: Option<Kind>,
    ) -> AskAgentRequest {
        build_ask_request(&AskRequestInput {
            model,
            project: "/work/monoptah",
            scope,
            question,
            options,
            harness,
            now: &leaderboard_now(),
        })
    }

    fn person(id: &str) -> AskScope {
        AskScope::Person {
            person_id: id.into(),
        }
    }

    fn me(id: &str) -> AskScope {
        AskScope::Me {
            person_id: id.into(),
        }
    }

    // askSubtitle

    #[test]
    fn ask_subtitle_describes_each_scope() {
        assert_eq!(ask_subtitle(&AskScope::Team, None), "about the whole team");
        assert_eq!(ask_subtitle(&me("x"), Some("Ana")), "about your score");
        assert_eq!(ask_subtitle(&person("x"), Some("Bob")), "about Bob");
        assert_eq!(ask_subtitle(&person("x"), None), "about this person");
    }

    // personNameOf

    #[test]
    fn person_name_of_resolves_the_focused_person_and_is_undefined_for_a_team() {
        let m = model();
        assert_eq!(person_name_of(&m, &AskScope::Team), None);
        assert_eq!(
            person_name_of(&m, &person(&row_id(&m, "Bob"))).as_deref(),
            Some("Bob")
        );
    }

    // askContextChips

    fn chip_ids(chips: &[AskChip]) -> Vec<&'static str> {
        chips.iter().map(|c| c.id.as_str()).collect()
    }

    fn chip<'a>(chips: &'a [AskChip], id: &str) -> &'a AskChip {
        chips.iter().find(|c| c.id.as_str() == id).unwrap()
    }

    fn people_label_matches(label: &str, prefix: &str) -> bool {
        // /^<prefix>4 people · \d+ identities$/
        let Some(rest) = label.strip_prefix(prefix) else {
            return false;
        };
        let Some(rest) = rest.strip_prefix("4 people · ") else {
            return false;
        };
        let Some(count) = rest.strip_suffix(" identities") else {
            return false;
        };
        !count.is_empty() && count.chars().all(|c| c.is_ascii_digit())
    }

    #[test]
    fn ask_context_chips_lists_project_branch_period_people_and_the_three_data_parts_for_a_team() {
        let m = model();
        let chips = ask_context_chips(&m, "/work/monoptah", &AskScope::Team);
        assert_eq!(
            chip_ids(&chips),
            ["project", "branch", "period", "people", "scores", "formula", "ai"]
        );
        assert_eq!(chips[0].label, "monoptah");
        assert_eq!(chips[1].label, m.raw.branch);
        assert_eq!(chips[2].label, m.period.label);
        assert!(
            people_label_matches(&chips[3].label, ""),
            "{}",
            chips[3].label
        );
        assert_eq!(chips[4].label, "Scores + breakdown");
        assert_eq!(chips[5].label, "Scoring formula");
        assert_eq!(chips[6].label, "AI-assisted stats");
    }

    #[test]
    fn ask_context_chips_only_the_data_chips_are_removable() {
        let chips = ask_context_chips(&model(), "/work/monoptah", &AskScope::Team);
        let got: Vec<(&str, bool)> = chips.iter().map(|c| (c.id.as_str(), c.removable)).collect();
        assert_eq!(
            got,
            [
                ("project", false),
                ("branch", false),
                ("period", false),
                ("people", true),
                ("scores", true),
                ("formula", true),
                ("ai", true),
            ]
        );
    }

    #[test]
    fn ask_context_chips_swaps_the_people_chip_for_the_focused_person() {
        let m = model();
        let bob = row_id(&m, "Bob");
        let chips = ask_context_chips(&m, "/work/monoptah", &person(&bob));
        assert_eq!(
            chip_ids(&chips),
            ["project", "branch", "period", "focus", "people", "scores", "formula", "ai"]
        );
        let focus = chip(&chips, "focus");
        assert_eq!(focus.label, "Bob (focus)");
        assert!(!focus.removable);
        let people = chip(&chips, "people");
        assert!(
            people_label_matches(&people.label, "Whole team: "),
            "{}",
            people.label
        );
        assert!(people.removable);
        let ana = row_id(&m, "Ana");
        let mine = ask_context_chips(&m, "/work/monoptah", &me(&ana));
        assert_eq!(chip(&mine, "focus").label, "Ana (you)");
    }

    // buildAskSnapshot

    #[test]
    fn build_ask_snapshot_keeps_everything_by_default_with_per_pr_on_and_raw_commits_off() {
        let m = model();
        let s = snap(|_| {});
        assert!(s.get("formula").is_some());
        let first = &array(&s, "people")[0];
        assert!(first.get("parts").is_some());
        assert!(first.get("breakdown").is_some());
        assert!(first.get("aiAssistedPct").is_some());
        assert!(first.get("identities").is_some());
        assert!(!array(&s, "prs").is_empty());
        assert!(s.get("commits").is_none());
        assert_eq!(s["project"], "monoptah");
        assert_eq!(s["branch"], m.raw.branch);
    }

    #[test]
    fn build_ask_snapshot_drops_the_formula_when_that_chip_is_removed() {
        let s = snap(|o| o.omit = vec![AskContextPart::Formula]);
        assert!(s.get("formula").is_none());
        assert!(array(&s, "people")[0].get("parts").is_some());
    }

    #[test]
    fn build_ask_snapshot_drops_parts_and_breakdown_when_scores_are_removed_also_on_the_focus() {
        let m = model();
        let s = snapshot_of(
            &m,
            person(&row_id(&m, "Bob")),
            &options(|o| o.omit = vec![AskContextPart::Scores]),
        );
        let mut everyone: Vec<&Value> = array(&s, "people").iter().collect();
        everyone.push(&s["focus"]);
        for p in everyone {
            assert!(p.get("parts").is_none());
            assert!(p.get("breakdown").is_none());
            assert!(p["score"].is_number());
            assert!(p["rank"].is_number());
        }
    }

    #[test]
    fn build_ask_snapshot_drops_the_ai_assisted_fields_everywhere() {
        let s = snap(|o| o.omit = vec![AskContextPart::Ai]);
        assert!(array(&s, "people")
            .iter()
            .all(|p| p.get("aiAssistedPct").is_none()));
        assert!(array(&s["stats"], "rows")
            .iter()
            .all(|r| r.get("aiAssistedPct").is_none()));
        assert!(array(&s, "prs")
            .iter()
            .all(|p| p.get("aiAssisted").is_none()));
    }

    #[test]
    fn build_ask_snapshot_drops_identities_when_the_people_chip_is_removed() {
        let s = snap(|o| o.omit = vec![AskContextPart::People]);
        assert!(array(&s, "people")
            .iter()
            .all(|p| p.get("identities").is_none()));
        assert!(array(&s["stats"], "rows")
            .iter()
            .all(|r| r.get("identities").is_none()));
    }

    #[test]
    fn build_ask_snapshot_per_pr_off_removes_prs_raw_commits_on_adds_commits() {
        assert!(snap(|o| o.per_pr = false).get("prs").is_none());
        let s = snap(|o| o.raw_commits = true);
        assert!(!array(&s, "commits").is_empty());
    }

    #[test]
    fn build_ask_snapshot_tells_the_agent_what_was_left_out() {
        let s = snap(|o| o.omit = vec![AskContextPart::Formula, AskContextPart::Ai]);
        let notes: Vec<&str> = array(&s, "notes")
            .iter()
            .map(|n| n.as_str().unwrap())
            .collect();
        let note = notes.iter().find(|n| n.starts_with("Left out")).unwrap();
        assert!(note.contains("scoring formula"));
        assert!(note.contains("AI-assisted"));
        let plain = snap(|_| {});
        assert!(!array(&plain, "notes")
            .iter()
            .any(|n| n.as_str().unwrap().starts_with("Left out")));
    }

    #[test]
    fn build_ask_snapshot_describes_the_period_once_for_the_scores_and_the_stats_alike() {
        let s = snap(|_| {});
        assert!(!array(&s, "notes")
            .iter()
            .any(|n| n.as_str().unwrap().contains("page range")));
        assert_eq!(s["period"]["kind"], "month");
        assert_eq!(s["period"]["label"], "Oct 2026");
    }

    #[test]
    fn build_ask_snapshot_limits_prs_to_the_focused_person() {
        let m = model();
        let ana = row_id(&m, "Ana");
        let s = snapshot_of(&m, person(&ana), &AskOptions::default());
        let prs = array(&s, "prs");
        assert!(!prs.is_empty());
        assert!(prs.iter().all(|p| p["personId"] == ana));
    }

    // buildAskRequest

    #[test]
    fn build_ask_request_builds_the_file_the_prompt_and_the_snapshot_json() {
        let m = model();
        let req = request(
            &m,
            AskScope::Team,
            "  Who drove the work?  ",
            &AskOptions::default(),
            Some(Kind::Claude),
        );
        assert_eq!(req.question, "Who drove the work?");
        assert_eq!(req.file_name, format!("contributors-{}.json", m.period.id));
        assert_eq!(req.harness, Some(Kind::Claude));
        assert_eq!(req.scope, AskScope::Team);
        assert!(req.prompt.contains("Question: Who drove the work?"));
        assert!(req.prompt.contains(&req.file_name));
        assert!(req.prompt.contains("\"monoptah\""));
        let parsed: Value = serde_json::from_str(&req.snapshot_json).unwrap();
        assert_eq!(parsed["period"]["id"], m.period.id);
    }

    #[test]
    fn build_ask_request_snapshot_json_is_pretty_printed_with_2_spaces() {
        let req = request(&model(), AskScope::Team, "q", &AskOptions::default(), None);
        assert!(req.snapshot_json.starts_with("{\n  \"generatedAt\": "));
        assert!(!req.snapshot_json.ends_with('\n'));
    }

    #[test]
    fn build_ask_request_expresses_allow_tools_in_the_prompt() {
        let m = model();
        let on = request(&m, AskScope::Team, "q", &AskOptions::default(), None);
        let off = request(
            &m,
            AskScope::Team,
            "q",
            &options(|o| o.allow_tools = false),
            None,
        );
        assert!(on.prompt.contains("read-only git and gh"));
        assert!(off.prompt.contains("Do not run commands"));
        assert_eq!(on.harness, None);
    }

    #[test]
    fn build_ask_request_names_the_focused_person_in_the_prompt() {
        let m = model();
        let req = request(
            &m,
            person(&row_id(&m, "Bob")),
            "q",
            &AskOptions::default(),
            None,
        );
        assert!(req.prompt.contains("Bob"));
    }

    // anonymised snapshot (people chip removed)

    mod anonymised {
        use super::*;

        // Eve has no display name, so her name falls back to her e-mail address.
        fn eve() -> GitIdentity {
            ident("", "eve@example.com")
        }

        fn with_eve() -> ContributorsModel {
            let raw = leaderboard_raw(|r| {
                r.commits
                    .push(commit_with("eve-1", &eve(), &oct(3, 12), |c| {
                        c.files = vec![file("src/features/a.ts", 5)]
                    }));
            });
            build_contributors_model(ModelInput {
                raw: Arc::new(raw),
                config: Arc::new(default_config()),
                mailmap: vec![],
                merge: true,
                period: current_period(PeriodKind::Month, &leaderboard_now(), None),
                now: leaderboard_now(),
                user: Some(GitUser {
                    name: "Ana".into(),
                    email: "ana@example.com".into(),
                }),
                color_overrides: Default::default(),
            })
        }

        fn omit_people(raw_commits: bool) -> AskOptions {
            options(|o| {
                o.omit = vec![AskContextPart::People];
                o.raw_commits = raw_commits;
            })
        }

        fn build(m: &ContributorsModel, scope: AskScope) -> Value {
            snapshot_of(m, scope, &omit_people(true))
        }

        #[test]
        fn contains_no_e_mail_address_anywhere_for_every_scope() {
            let m = with_eve();
            let bob = row_id(&m, "Bob");
            for scope in [AskScope::Team, person(&bob), me(&row_id(&m, "Ana"))] {
                let text = serde_json::to_string(&build(&m, scope)).unwrap();
                assert!(!text.contains('@'), "{text}");
            }
            // Sanity: the same data does contain addresses when the chip is kept.
            let kept = snapshot_of(&m, AskScope::Team, &options(|o| o.raw_commits = true));
            assert!(serde_json::to_string(&kept).unwrap().contains('@'));
        }

        #[test]
        fn numbers_people_person_1_person_2_in_rank_order_and_keeps_real_names() {
            let m = with_eve();
            let s = build(&m, AskScope::Team);
            let rows = &m.leaderboard.rows;
            let ids: Vec<&str> = array(&s, "people")
                .iter()
                .map(|p| p["personId"].as_str().unwrap())
                .collect();
            let expected: Vec<String> = (0..rows.len())
                .map(|i| format!("person-{}", i + 1))
                .collect();
            assert_eq!(ids, expected);
            let names: Vec<&str> = array(&s, "people")
                .iter()
                .map(|p| p["name"].as_str().unwrap())
                .collect();
            for row in rows {
                if !row.name.contains('@') {
                    assert!(names.contains(&row.name.as_str()), "{}", row.name);
                }
            }
        }

        #[test]
        fn replaces_a_display_name_that_is_an_e_mail_address_by_the_alias() {
            let m = with_eve();
            let s = build(&m, AskScope::Team);
            let index = m
                .leaderboard
                .rows
                .iter()
                .position(|r| r.person_id == "eve@example.com")
                .unwrap();
            let alias = format!("person-{}", index + 1);
            assert_eq!(array(&s, "people")[index]["personId"], alias);
            assert_eq!(array(&s, "people")[index]["name"], alias);
        }

        #[test]
        fn keeps_scope_focus_prs_stats_and_commits_consistent() {
            let m = with_eve();
            let bob_row = m.leaderboard.rows.iter().find(|r| r.name == "Bob").unwrap();
            let bob_index = m
                .leaderboard
                .rows
                .iter()
                .position(|r| r.person_id == bob_row.person_id)
                .unwrap();
            let bob_alias = format!("person-{}", bob_index + 1);
            let s = build(&m, person(&bob_row.person_id));
            assert_eq!(s["scope"], json!({"kind": "person", "personId": bob_alias}));
            assert_eq!(s["focus"]["personId"], bob_alias);
            assert_eq!(s["focus"]["name"], "Bob");
            let listed = array(&s, "people")
                .iter()
                .find(|p| p["personId"] == bob_alias)
                .unwrap();
            assert_eq!(listed["name"], "Bob");
            assert!(array(&s["stats"], "rows")
                .iter()
                .any(|r| r["personId"] == bob_alias));
            assert!(array(&s["stats"], "rows").iter().all(|r| {
                let id = r["personId"].as_str().unwrap();
                id.strip_prefix("person-")
                    .is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
            }));
            assert!(array(&s, "prs").iter().all(|p| p["personId"] == bob_alias));
            let commits = array(&s, "commits");
            assert!(!commits.is_empty());
            for c in commits {
                let author = c["author"].as_str().unwrap();
                assert!(!author.contains('<') && !author.contains('>'), "{author}");
            }
        }

        #[test]
        fn gives_a_commit_by_an_unnamed_author_the_alias_instead_of_an_empty_name() {
            let m = with_eve();
            let s = build(&m, AskScope::Team);
            let index = m
                .leaderboard
                .rows
                .iter()
                .position(|r| r.person_id == "eve@example.com")
                .unwrap();
            let alias = format!("person-{}", index + 1);
            let eve = array(&s, "commits")
                .iter()
                .find(|c| c["person"] == alias)
                .expect("eve's commit");
            assert_eq!(eve["author"], alias);
        }

        #[test]
        fn anonymises_the_prompt_too() {
            let m = with_eve();
            let bob = row_id(&m, "Bob");
            let req = request(
                &m,
                person(&bob),
                "q",
                &options(|o| o.omit = vec![AskContextPart::People]),
                None,
            );
            assert!(!req.prompt.contains('@'));
            assert!(req.prompt.contains("Bob"));
            assert_eq!(req.scope, person(&bob));
            let parsed: Value = serde_json::from_str(&req.snapshot_json).unwrap();
            let id = parsed["scope"]["personId"].as_str().unwrap();
            assert!(id.starts_with("person-"));
        }

        #[test]
        fn explains_the_aliases_to_the_agent() {
            let s = build(&with_eve(), AskScope::Team);
            let notes: Vec<&str> = array(&s, "notes")
                .iter()
                .map(|n| n.as_str().unwrap())
                .collect();
            assert!(notes.join("\n").contains("person-1"));
        }
    }

    // people chip counts

    #[test]
    fn people_chip_counts_counts_people_seen_only_in_the_stats_rows_too() {
        let m = model();
        let rows = m.leaderboard.rows.len();
        let chips = ask_context_chips(&m, "/work/monoptah", &AskScope::Team);
        let people = chip(&chips, "people");
        let union: std::collections::BTreeSet<&str> = m
            .leaderboard
            .rows
            .iter()
            .map(|r| r.person_id.as_str())
            .chain(m.stats.rows.iter().map(|r| r.person_id.as_str()))
            .collect();
        assert!(union.len() >= rows);
        assert!(people.label.starts_with(&format!("{} ", union.len())));
    }

    #[test]
    fn people_chip_counts_says_what_removing_it_does() {
        let chips = ask_context_chips(&model(), "/work/monoptah", &AskScope::Team);
        let hint = chip(&chips, "people").hint.unwrap();
        assert!(hint.contains("person-1"));
        assert!(hint.contains("e-mail"));
    }

    // prompt wording follows the chips

    fn prompt(patch: impl FnOnce(&mut AskOptions)) -> String {
        request(&model(), AskScope::Team, "q", &options(patch), None).prompt
    }

    #[test]
    fn prompt_wording_lists_everything_by_default() {
        let text = prompt(|_| {});
        assert!(text.contains("scores and ranks"));
        assert!(text.contains("per-dimension breakdowns"));
        assert!(text.contains("identity mapping"));
        assert!(text.contains("the scoring formula"));
        assert!(text.contains("per-PR details"));
        assert!(text.contains("AI-assisted shares"));
        assert!(!text.contains("raw commit list"));
    }

    #[test]
    fn a_part_omitted_twice_is_named_once_like_a_set() {
        let s = snap(|o| {
            o.omit = vec![
                AskContextPart::Formula,
                AskContextPart::Ai,
                AskContextPart::Formula,
            ]
        });
        let notes: Vec<&str> = array(&s, "notes")
            .iter()
            .filter_map(Value::as_str)
            .collect();
        assert!(
            notes.contains(
                &"Left out at the user's request: the scoring formula, AI-assisted stats."
            ),
            "{notes:?}"
        );
    }

    #[test]
    fn prompt_wording_drops_what_was_removed_and_adds_what_was_switched_on() {
        let text = prompt(|o| {
            o.per_pr = false;
            o.raw_commits = true;
            o.omit = vec![
                AskContextPart::Scores,
                AskContextPart::Formula,
                AskContextPart::People,
                AskContextPart::Ai,
            ];
        });
        assert!(text.contains("scores and ranks"));
        assert!(text.contains("the raw commit list"));
        for gone in [
            "per-dimension breakdowns",
            "identity mapping",
            "scoring formula",
            "per-PR details",
            "AI-assisted",
        ] {
            assert!(!text.contains(gone), "{gone}");
        }
    }
}
