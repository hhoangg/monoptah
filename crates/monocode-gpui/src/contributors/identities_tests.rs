//! IdentitiesTab.test.ts and useIdentityDrag.test.ts: the edit rules behind the
//! Identities section (what each action writes to the config) and the drag
//! rules as tests of pure functions, and the interactions (rename, menus,
//! drag, exclude, lock) in a real window over the fake backend.
use super::identities::{
    NEW_PERSON_NAME, Suggestion, add_person, can_remove, dismiss, exclude_identity_of,
    exclude_person_of, excluded_text, include_again, merge_into, move_identity, remove, rename,
    suggestions,
};
use chrono::{DateTime, Local, TimeZone};
use monocode_core::contributors::{
    GitIdentity, RawCommit, RawData,
    model::{
        config::default_config,
        identity::{MailmapPerson, MergeReason},
        period::{PeriodKind, current_period},
        pipeline::{ContributorsModel, ModelInput, build_contributors_model},
        types::{ContributorsConfig, Person},
        unmapped::{PersonRow, excluded_identities, person_rows},
    },
};
use std::sync::Arc;

fn local(y: i32, m: u32, d: u32, h: u32) -> DateTime<Local> {
    Local.with_ymd_and_hms(y, m, d, h, 0, 0).earliest().unwrap()
}

fn now() -> DateTime<Local> {
    local(2026, 10, 20, 10)
}

fn who(name: &str, email: &str) -> GitIdentity {
    GitIdentity {
        name: name.into(),
        email: email.into(),
        login: None,
    }
}
fn with_login(name: &str, email: &str, login: &str) -> GitIdentity {
    GitIdentity {
        login: Some(login.into()),
        ..who(name, email)
    }
}

fn ana() -> GitIdentity {
    with_login("Ana", "ana@example.com", "ana")
}
fn cat() -> GitIdentity {
    with_login("Cat", "cat@example.com", "cat")
}
fn bob() -> GitIdentity {
    who("Bob Stone", "bob@example.com")
}
fn bob_home() -> GitIdentity {
    who("Bob Stone", "bob@home.net")
}
fn claude() -> GitIdentity {
    who("Claude", "noreply@anthropic.com")
}
fn dependabot() -> GitIdentity {
    with_login(
        "dependabot[bot]",
        "49699333+dependabot[bot]@users.noreply.github.com",
        "dependabot[bot]",
    )
}
fn runner() -> GitIdentity {
    who("CI Runner", "ci@build-box")
}

fn commit(hash: &str, author: &GitIdentity, day: u32) -> RawCommit {
    let at = local(2026, 10, day, 9).to_rfc3339();
    RawCommit {
        hash: hash.into(),
        subject: format!("commit {hash}"),
        author: author.clone(),
        committer: author.clone(),
        authored_at: at.clone(),
        committed_at: at,
        co_authors: vec![],
        files: vec![],
        pr_id: None,
    }
}

/// The raw data of IdentitiesTab.test.ts.
pub(super) fn raw() -> RawData {
    RawData {
        head: "h1".into(),
        branch: "main".into(),
        since: String::new(),
        until: String::new(),
        gh_available: true,
        gh_note: None,
        commits: vec![
            commit("a1", &ana(), 1),
            commit("a2", &who("Ana", "ana@laptop.local"), 2),
            commit("b1", &bob(), 3),
            commit("b2", &bob(), 4),
            commit("b3", &bob_home(), 5),
            commit("c1", &cat(), 6),
            commit("d1", &dependabot(), 7),
            commit("x1", &claude(), 8),
            commit("r1", &runner(), 9),
            commit("r2", &runner(), 10),
        ],
        prs: vec![],
        reviews: vec![],
        survival: vec![],
        reverted_pr_ids: vec![],
        truncated: false,
    }
}

/// The config of IdentitiesTab.test.ts: Ana with two identities.
pub(super) fn config() -> ContributorsConfig {
    let mut config = default_config();
    config.people = vec![Person {
        id: "ana".into(),
        name: "Ana Sol".into(),
        identities: vec!["ana@example.com".into(), "ana@laptop.local".into()],
    }];
    config
}

pub(super) fn model_of(
    config: &ContributorsConfig,
    merge: bool,
    mailmap: Vec<MailmapPerson>,
) -> ContributorsModel {
    build_contributors_model(ModelInput {
        raw: Arc::new(raw()),
        config: Arc::new(config.clone()),
        mailmap,
        merge,
        period: current_period(PeriodKind::Month, &now(), None),
        now: now(),
        user: None,
        color_overrides: Default::default(),
    })
}

fn model() -> ContributorsModel {
    model_of(&config(), true, vec![])
}

fn rows_of(model: &ContributorsModel) -> Vec<PersonRow> {
    person_rows(&model.raw, &model.config, &model.resolver)
}

fn row(rows: &[PersonRow], id: &str) -> PersonRow {
    rows.iter()
        .find(|r| r.id == id)
        .unwrap_or_else(|| panic!("no row {id}"))
        .clone()
}

fn person<'a>(config: &'a ContributorsConfig, id: &str) -> Option<&'a Person> {
    config.people.iter().find(|p| p.id == id)
}

fn keys(row: &PersonRow) -> Vec<&str> {
    row.identities.iter().map(|i| i.key.as_str()).collect()
}

fn mailmap_bob() -> Vec<MailmapPerson> {
    vec![MailmapPerson {
        name: "Bob Stone".into(),
        identities: vec!["bob@example.com".into(), "bob@home.net".into()],
    }]
}

// ---- People ------------------------------------------------------------------

#[test]
fn lists_people_with_identity_and_commit_counts_and_marks_agent_identities() {
    let mut config = config();
    config.people[0]
        .identities
        .push("noreply@anthropic.com".into());
    let model = model_of(&config, true, vec![]);
    let rows = rows_of(&model);
    let ana = row(&rows, "ana");
    assert_eq!(ana.name, "Ana Sol");
    assert_eq!(
        keys(&ana),
        [
            "ana@example.com",
            "ana@laptop.local",
            "noreply@anthropic.com"
        ]
    );
    assert_eq!(ana.commits, 3);
    assert!(ana.identities.iter().any(|i| i.agent));
    let bob = row(&rows, "bob@example.com");
    assert_eq!((bob.identities.len(), bob.commits), (1, 2));
}

#[test]
fn a_rename_creates_a_person_for_an_unmapped_one() {
    let model = model();
    let rows = rows_of(&model);
    let bob = row(&rows, "bob@example.com");
    let next = rename(&model.config, &bob, "Robert Stone").expect("a change");
    let created = person(&next, "bob@example.com").expect("a person");
    assert_eq!(created.name, "Robert Stone");
    assert_eq!(created.identities, ["bob@example.com"]);
}

#[test]
fn a_rename_renames_a_config_person() {
    let model = model();
    let ana = row(&rows_of(&model), "ana");
    let next = rename(&model.config, &ana, "Ana Maria").unwrap();
    assert_eq!(person(&next, "ana").unwrap().name, "Ana Maria");
}

#[test]
fn a_blank_or_unchanged_name_writes_nothing() {
    let model = model();
    let ana = row(&rows_of(&model), "ana");
    assert_eq!(rename(&model.config, &ana, "   "), None);
    assert_eq!(rename(&model.config, &ana, ""), None);
    assert_eq!(rename(&model.config, &ana, "Ana Sol"), None);
    assert_eq!(rename(&model.config, &ana, "  Ana Sol  "), None);
}

#[test]
fn moving_an_identity_to_another_person_takes_it_from_its_source() {
    let model = model();
    let rows = rows_of(&model);
    let bob = row(&rows, "bob@example.com");
    let next = move_identity(&model.config, &rows, "ana@laptop.local", &bob).unwrap();
    assert_eq!(
        person(&next, "ana").unwrap().identities,
        ["ana@example.com"]
    );
    assert_eq!(
        person(&next, "bob@example.com").unwrap().identities,
        ["bob@example.com", "ana@laptop.local"]
    );
}

#[test]
fn moving_an_identity_onto_the_person_that_has_it_is_a_no_op() {
    let model = model();
    let rows = rows_of(&model);
    let ana = row(&rows, "ana");
    assert_eq!(
        move_identity(&model.config, &rows, "ana@example.com", &ana),
        None
    );
}

#[test]
fn the_rest_of_a_mailmap_group_stays_with_its_person_when_one_identity_moves_out() {
    let model = model_of(&config(), true, mailmap_bob());
    let rows = rows_of(&model);
    assert_eq!(row(&rows, "bob@example.com").identities.len(), 2);
    let ana = row(&rows, "ana");
    let next = move_identity(&model.config, &rows, "bob@home.net", &ana).unwrap();
    assert!(
        person(&next, "ana")
            .unwrap()
            .identities
            .contains(&"bob@home.net".to_string())
    );
    // Rebuilt from the new config, Bob keeps the other identity.
    let after = model_of(&next, true, mailmap_bob());
    let after_rows = rows_of(&after);
    let bob = row(&after_rows, "bob@example.com");
    assert_eq!(keys(&bob), ["bob@example.com"]);
    assert_eq!(bob.commits, 2);
    assert!(keys(&row(&after_rows, "ana")).contains(&"bob@home.net"));
}

#[test]
fn identities_a_mailmap_group_adds_to_a_config_person_are_listed() {
    let mut config = default_config();
    config.people = vec![Person {
        id: "ana".into(),
        name: "Ana Sol".into(),
        identities: vec!["ana@example.com".into()],
    }];
    let mailmap = vec![MailmapPerson {
        name: "Ana".into(),
        identities: vec!["ana@example.com".into(), "ana@laptop.local".into()],
    }];
    let model = model_of(&config, true, mailmap);
    let ana = row(&rows_of(&model), "ana");
    assert_eq!(ana.identities.len(), 2);
    assert!(keys(&ana).contains(&"ana@laptop.local"));
}

#[test]
fn an_unmapped_identity_is_added_to_a_person() {
    let model = model();
    let rows = rows_of(&model);
    let ana = row(&rows, "ana");
    let next = move_identity(&model.config, &rows, "ci@build-box", &ana).unwrap();
    assert!(
        person(&next, "ana")
            .unwrap()
            .identities
            .contains(&"ci@build-box".to_string())
    );
}

#[test]
fn merging_a_person_into_another_keeps_the_target_and_its_name() {
    let model = model();
    let rows = rows_of(&model);
    let (ana, cat) = (row(&rows, "ana"), row(&rows, "cat@example.com"));
    let next = merge_into(&model.config, &ana, &[cat]);
    let merged = person(&next, "ana").unwrap();
    assert_eq!(
        merged.identities,
        ["ana@example.com", "ana@laptop.local", "cat@example.com"]
    );
    assert_eq!(merged.name, "Ana Sol");
}

#[test]
fn removing_a_person_ungroups_its_identities() {
    let model = model();
    let ana = row(&rows_of(&model), "ana");
    assert!(can_remove(&ana));
    let next = remove(&model.config, &ana).unwrap();
    assert!(next.people.is_empty());
}

#[test]
fn a_person_that_only_exists_through_observation_cannot_be_removed() {
    let model = model();
    let cat = row(&rows_of(&model), "cat@example.com");
    assert!(!can_remove(&cat));
    assert_eq!(remove(&model.config, &cat), None);
}

#[test]
fn a_new_person_is_named_new_person_and_its_id_is_known_up_front() {
    let model = model();
    let (next, id) = add_person(&model.config);
    let created = next
        .people
        .iter()
        .find(|p| p.name == NEW_PERSON_NAME)
        .unwrap();
    assert_eq!(created.id, id);
    assert!(created.identities.is_empty());
    // A second one gets another id.
    let (again, second) = add_person(&next);
    assert_ne!(second, id);
    assert_eq!(again.people.len(), next.people.len() + 1);
}

// ---- suggested merges -------------------------------------------------------------

#[test]
fn suggests_merging_two_people_with_the_same_name_into_the_busiest() {
    let model = model();
    let found = suggestions(&model, &rows_of(&model));
    assert_eq!(found.len(), 1);
    let Suggestion { suggestion, people } = &found[0];
    assert_eq!(suggestion.reason, MergeReason::SameName);
    let ids: Vec<&str> = people.iter().map(|p| p.id.as_str()).collect();
    assert_eq!(ids, ["bob@example.com", "bob@home.net"]);
    assert_eq!(people[0].name, "Bob Stone");
}

#[test]
fn merging_a_suggestion_writes_one_person_with_both_identities() {
    let model = model();
    let found = suggestions(&model, &rows_of(&model));
    let (target, others) = found[0].people.split_first().unwrap();
    let next = merge_into(&model.config, target, others);
    let merged = person(&next, "bob@example.com").unwrap();
    assert_eq!(merged.identities, ["bob@example.com", "bob@home.net"]);
    assert_eq!(merged.name, "Bob Stone");
}

#[test]
fn a_dismissed_suggestion_stays_gone() {
    let model = model();
    let found = suggestions(&model, &rows_of(&model));
    let next = dismiss(&model.config, &found[0].suggestion.id).unwrap();
    assert_eq!(next.dismissed_merges.len(), 1);
    let again = model_of(&next, true, vec![]);
    assert!(suggestions(&again, &rows_of(&again)).is_empty());
    // Dismissing twice changes nothing.
    assert_eq!(dismiss(&next, &found[0].suggestion.id), None);
}

#[test]
fn nothing_is_suggested_while_merging_is_off() {
    let model = model_of(&config(), false, vec![]);
    assert!(suggestions(&model, &rows_of(&model)).is_empty());
}

#[test]
fn the_busiest_person_is_kept_then_a_config_person_then_the_name() {
    let model = model();
    let rows = rows_of(&model);
    let mut a = row(&rows, "bob@example.com");
    let mut b = row(&rows, "bob@home.net");
    // The same commits: a config person wins, then the name.
    a.commits = 3;
    b.commits = 3;
    a.in_config = false;
    b.in_config = true;
    assert_eq!(
        super::identities::keep_order(vec![a.clone(), b.clone()])[0].id,
        "bob@home.net"
    );
    a.in_config = true;
    a.name = "Zed".into();
    b.name = "abe".into();
    assert_eq!(
        super::identities::keep_order(vec![a.clone(), b.clone()])[0].id,
        "bob@home.net"
    );
    b.commits = 4;
    a.commits = 9;
    assert_eq!(
        super::identities::keep_order(vec![b, a])[0].id,
        "bob@example.com"
    );
}

// ---- unmapped and excluded --------------------------------------------------------

#[test]
fn excluding_an_identity_lists_it_as_excluded_and_including_it_again_lifts_it() {
    let model = model();
    let before = excluded_identities(&model.raw, &model.config);
    assert_eq!(before.len(), 1, "the dependabot wildcard");
    let rows = rows_of(&model);
    let ana = row(&rows, "ana");
    let next = super::identities::exclude_key(&model.config, "ci@build-box").unwrap();
    assert!(
        next.exclude_identities
            .contains(&"ci@build-box".to_string())
    );
    let after = model_of(&next, true, vec![]);
    let excluded = excluded_identities(&after.raw, &after.config);
    assert_eq!(excluded.len(), 2);
    let runner = excluded.iter().find(|e| e.key == "ci@build-box").unwrap();
    assert!(runner.removable);
    let lifted = include_again(&next, runner);
    assert!(
        !lifted
            .exclude_identities
            .contains(&"ci@build-box".to_string())
    );
    let _ = ana;
}

#[test]
fn an_exclusion_from_a_wildcard_is_explained_instead_of_offered_for_lifting() {
    let model = model();
    let excluded = excluded_identities(&model.raw, &model.config);
    let bot = &excluded[0];
    assert!(!bot.removable);
    assert!(bot.patterns.iter().any(|p| p == "*[bot]*"));
    assert_eq!(
        excluded_text(bot).as_deref(),
        Some(
            "Excluded by pattern *[bot]*, dependabot[bot]. Edit excludeIdentities in .monoptah/contributors.json to change this."
        )
    );
}

#[test]
fn a_removable_exclusion_has_no_explanation() {
    let mut config = config();
    config.exclude_identities.push("cat@example.com".into());
    let model = model_of(&config, true, vec![]);
    let excluded = excluded_identities(&model.raw, &model.config);
    let cat = excluded
        .iter()
        .find(|e| e.key == "cat@example.com")
        .unwrap();
    assert!(cat.removable);
    assert_eq!(excluded_text(cat), None);
}

#[test]
fn an_identity_is_excluded_from_its_own_menu() {
    let model = model();
    let rows = rows_of(&model);
    let cat = row(&rows, "cat@example.com");
    let next = exclude_identity_of(&model.config, &cat, "cat@example.com").unwrap();
    assert!(
        next.exclude_identities
            .contains(&"cat@example.com".to_string())
    );
    let after = model_of(&next, true, vec![]);
    assert!(rows_of(&after).iter().all(|r| r.id != "cat@example.com"));
}

#[test]
fn an_agent_identity_is_never_excluded() {
    let mut config = config();
    config.people[0]
        .identities
        .push("noreply@anthropic.com".into());
    let model = model_of(&config, true, vec![]);
    let ana = row(&rows_of(&model), "ana");
    assert!(
        ana.identities
            .iter()
            .any(|i| i.key == "noreply@anthropic.com" && i.agent)
    );
    assert_eq!(
        exclude_identity_of(&model.config, &ana, "noreply@anthropic.com"),
        None
    );
}

#[test]
fn excluding_a_person_excludes_every_identity_but_the_agents() {
    let mut config = config();
    config.people[0]
        .identities
        .push("noreply@anthropic.com".into());
    let model = model_of(&config, true, vec![]);
    let ana = row(&rows_of(&model), "ana");
    let next = exclude_person_of(&model.config, &ana).unwrap();
    for key in ["ana@example.com", "ana@laptop.local"] {
        assert!(next.exclude_identities.contains(&key.to_string()), "{key}");
    }
    assert!(
        !next
            .exclude_identities
            .contains(&"noreply@anthropic.com".to_string())
    );
}

#[test]
fn a_person_made_only_of_agents_cannot_be_excluded() {
    let mut config = default_config();
    config.people = vec![Person {
        id: "bot".into(),
        name: "Robot".into(),
        identities: vec!["noreply@anthropic.com".into()],
    }];
    let model = model_of(&config, true, vec![]);
    let robot = row(&rows_of(&model), "bot");
    assert_eq!(exclude_person_of(&model.config, &robot), None);
}

#[test]
fn the_menu_says_how_many_identities_a_person_exclusion_takes() {
    let model = model_of(&config(), true, mailmap_bob());
    let rows = rows_of(&model);
    let text = super::identities::exclude_person_text;
    assert_eq!(
        text(&row(&rows, "bob@example.com")),
        "Excludes all 2 identities from the scores."
    );
    assert_eq!(
        text(&row(&rows, "cat@example.com")),
        "Excludes all 1 identity from the scores."
    );
}

#[test]
fn the_person_menu_texts_for_agents() {
    let mut config = config();
    config.people[0]
        .identities
        .push("noreply@anthropic.com".into());
    let model = model_of(&config, true, vec![]);
    let ana = row(&rows_of(&model), "ana");
    assert_eq!(
        super::identities::exclude_person_text(&ana),
        "Excludes all 2 identities from the scores."
    );
    assert_eq!(
        super::identities::exclude_identity_text(true),
        "Agents cannot be excluded."
    );
    assert_eq!(
        super::identities::exclude_identity_text(false),
        "Leaves it out of the scores."
    );
}

// ---- the drag rules (useIdentityDrag.test.ts) -----------------------------------------

use super::identities::{
    AGENT_NOTE, DragState, MERGING_OFF, SAVED_PATH, SAVED_REST, SAVED_TO, UNMAPPED_INTRO, WIDE_AT,
    counts_text, person_color, plural, reason_text, unmapped_sub,
};

fn drag_of(key: &str, from: &str) -> DragState {
    let mut drag = DragState::default();
    drag.start(key, from);
    drag
}

#[test]
fn there_is_no_drag_until_one_starts() {
    let mut drag = DragState::default();
    assert!(!drag.is_active());
    assert_eq!(drag.over(), None);
    assert_eq!(drag.key(), None);
    // A release without a drag drops nothing.
    assert_eq!(drag.drop_on("bob"), None);
    drag.hover(Some("bob"));
    assert_eq!(drag.over(), None, "nothing to hover with");
}

#[test]
fn a_started_drag_names_its_identity_and_follows_the_card_under_the_pointer() {
    let mut drag = drag_of("ana@laptop.local", "ana");
    assert!(drag.is_active());
    assert_eq!(drag.key(), Some("ana@laptop.local"));
    assert_eq!(drag.over(), None);
    drag.hover(Some("bob"));
    assert_eq!(drag.over(), Some("bob"));
    drag.hover(None);
    assert_eq!(drag.over(), None);
}

#[test]
fn leaving_a_card_only_clears_it_when_it_is_the_one_under_the_pointer() {
    let mut drag = drag_of("k", "ana");
    drag.hover(Some("bob"));
    drag.leave("cat");
    assert_eq!(drag.over(), Some("bob"));
    drag.leave("bob");
    assert_eq!(drag.over(), None);
}

#[test]
fn drops_on_the_person_under_the_pointer_and_then_resets() {
    let mut drag = drag_of("ana@laptop.local", "ana");
    drag.hover(Some("bob"));
    assert_eq!(
        drag.drop_on("bob"),
        Some(("ana@laptop.local".to_string(), "bob".to_string()))
    );
    assert!(!drag.is_active());
    assert_eq!(drag.over(), None);
    assert_eq!(drag.drop_on("bob"), None, "the gesture is over");
}

#[test]
fn it_is_a_no_op_to_drop_on_the_person_the_identity_already_belongs_to() {
    let mut drag = drag_of("ana@laptop.local", "ana");
    drag.hover(Some("ana"));
    assert_eq!(drag.drop_on("ana"), None);
    assert!(!drag.is_active());
}

#[test]
fn cancelling_ends_the_drag_so_a_later_release_drops_nothing() {
    let mut drag = drag_of("ana@laptop.local", "ana");
    drag.hover(Some("bob"));
    drag.cancel();
    assert!(!drag.is_active());
    assert_eq!(drag.over(), None);
    assert_eq!(drag.drop_on("bob"), None);
}

#[test]
fn a_card_that_already_holds_the_identity_is_not_highlighted_as_a_target() {
    let model = model();
    let rows = rows_of(&model);
    let (ana, bob) = (row(&rows, "ana"), row(&rows, "bob@example.com"));
    let mut drag = drag_of("ana@laptop.local", "ana");
    assert!(!drag.highlights(&bob), "not under the pointer yet");
    drag.hover(Some("bob@example.com"));
    assert!(drag.highlights(&bob));
    drag.hover(Some("ana"));
    assert!(!drag.highlights(&ana), "it is Ana's own identity");
    drag.cancel();
    assert!(!drag.highlights(&bob));
}

#[test]
fn only_the_dragged_identity_row_is_dimmed() {
    let drag = drag_of("ana@laptop.local", "ana");
    assert!(drag.dims("ana@laptop.local"));
    assert!(!drag.dims("ana@example.com"));
    assert!(!DragState::default().dims("ana@laptop.local"));
}

// ---- copy -------------------------------------------------------------------------

#[test]
fn counts_read_like_the_cards() {
    let model = model();
    let rows = rows_of(&model);
    assert_eq!(counts_text(&row(&rows, "ana")), "2 identities · 2 commits");
    assert_eq!(
        counts_text(&row(&rows, "bob@example.com")),
        "1 identity · 2 commits"
    );
    assert_eq!(
        counts_text(&row(&rows, "cat@example.com")),
        "1 identity · 1 commit"
    );
    assert_eq!(plural(0, "commit", "commits"), "0 commits");
}

#[test]
fn an_unmapped_row_counts_commits_and_flags_machine_accounts() {
    let model = model();
    let runner = model
        .unmapped
        .iter()
        .find(|u| u.key == "ci@build-box")
        .unwrap();
    assert_eq!(unmapped_sub(runner), "2 commits · machine account");
    let claude = model
        .unmapped
        .iter()
        .find(|u| u.key == "noreply@anthropic.com")
        .unwrap();
    assert!(claude.agent);
    assert_eq!(unmapped_sub(claude), "1 commit");
}

#[test]
fn the_unmapped_card_lists_agents_and_machines_but_not_ordinary_authors() {
    let model = model();
    let keys: Vec<&str> = model.unmapped.iter().map(|u| u.key.as_str()).collect();
    assert!(keys.contains(&"ci@build-box"));
    assert!(keys.contains(&"noreply@anthropic.com"));
    for author in ["ana@example.com", "bob@example.com", "cat@example.com"] {
        assert!(!keys.contains(&author), "{author}");
    }
}

#[test]
fn the_fixed_copy_is_the_react_copy() {
    assert_eq!(
        MERGING_OFF,
        "Merging is off: showing raw git identities. Turn on Merge identities in the header to apply this mapping."
    );
    assert_eq!(
        format!("{SAVED_TO}{SAVED_PATH}{SAVED_REST}"),
        "Saved to .monoptah/contributors.json in the repo (shared with the team)."
    );
    assert!(UNMAPPED_INTRO.starts_with("Agents and machine accounts that no person lists yet."));
    assert!(UNMAPPED_INTRO.contains("Ordinary authors are not listed"));
    assert_eq!(
        AGENT_NOTE,
        "Agents are credited to the human (see How is the score calculated?)"
    );
    assert_eq!(WIDE_AT, 768.);
}

#[test]
fn a_suggestion_names_its_reason() {
    assert_eq!(reason_text(MergeReason::SameName), "Same name");
    assert_eq!(reason_text(MergeReason::SimilarEmail), "Similar email");
    assert_eq!(reason_text(MergeReason::GithubNoreply), "GitHub noreply");
}

#[test]
fn an_avatar_takes_the_color_the_other_tabs_use() {
    let mut model = model();
    // A color no hash would pick, so only the shared map can explain it.
    model.colors.insert("ana".into(), "#102030");
    assert_eq!(person_color(&model, "ana"), "#102030");
    assert_eq!(
        person_color(&model, "cat@example.com"),
        model.colors["cat@example.com"]
    );
}

// ---- in a window ------------------------------------------------------------------------

use super::{
    data_tests::{Call, Fake, start_of_tests},
    identities::IdentitiesView,
    page_tests::{Ui, click, github, settle, shown},
};
use gpui::{Entity, Focusable as _, MouseButton, TestAppContext, VisualTestContext};
use monocode_core::contributors::{
    jobs::LoadResponse,
    model::{config::serialize_config, period::sync_window},
};

fn window_fake(config_text: Option<String>) -> Arc<Fake> {
    window_fake_of(raw(), config_text)
}

fn window_fake_of(mut raw: RawData, config_text: Option<String>) -> Arc<Fake> {
    let fake = Fake::new();
    let october = current_period(PeriodKind::Month, &start_of_tests(), None);
    raw.since = sync_window(&october, &start_of_tests()).from.to_rfc3339();
    fake.set_load(LoadResponse {
        raw,
        covered: true,
        has_gaps: false,
        missing: vec![],
        missing_commits: 0,
        synced_at: Some(start_of_tests().timestamp() - 300),
        coverage_head: Some("h1".into()),
        head: "h1".into(),
        gh_state: Some("complete".into()),
    });
    fake.with(|s| {
        s.config.default = Ok(Some(
            config_text.unwrap_or_else(|| serialize_config(&config())),
        ));
        s.saved_range = Some(monocode_core::contributors::model::prefs::serialize_pick(
            &monocode_core::contributors::model::period::PeriodPick {
                kind: PeriodKind::Month,
                id: Some("2026-10".into()),
            },
        ));
    });
    fake
}

fn identities_window(cx: &mut TestAppContext) -> (Ui, VisualTestContext) {
    identities_window_of(cx, window_fake(None))
}

fn identities_window_of(cx: &mut TestAppContext, fake: Arc<Fake>) -> (Ui, VisualTestContext) {
    let (ui, mut v) = github(cx, fake);
    // Everything fits: the cards stack up to a few hundred pixels.
    v.simulate_resize(gpui::size(gpui::px(1600.), gpui::px(4000.)));
    // Blur listeners only see focus moves of an active window.
    ui.window
        .update(&mut v, |_, window, _| window.activate_window())
        .unwrap();
    settle(&mut v);
    click(&mut v, "contributors-section-identities");
    assert!(
        shown(&mut v, "contributors-body-identities"),
        "the Identities section is shown"
    );
    settle(&mut v);
    (ui, v)
}

fn writes(fake: &Fake) -> Vec<serde_json::Value> {
    fake.calls()
        .into_iter()
        .filter_map(|c| match c {
            Call::Write(text) => Some(serde_json::from_str(&text).expect("a JSON config")),
            _ => None,
        })
        .collect()
}

/// The people of a written config: (id, name, identities).
fn people_of(config: &serde_json::Value) -> Vec<(String, String, Vec<String>)> {
    config["people"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| {
            (
                p["id"].as_str().unwrap().to_string(),
                p["name"].as_str().unwrap().to_string(),
                p["identities"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|i| i.as_str().unwrap().to_string())
                    .collect(),
            )
        })
        .collect()
}

fn person_in(config: &serde_json::Value, id: &str) -> Option<(String, String, Vec<String>)> {
    people_of(config).into_iter().find(|p| p.0 == id)
}

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}

fn leak(text: String) -> &'static str {
    Box::leak(text.into_boxed_str())
}
fn sel(prefix: &str, id: &str) -> &'static str {
    leak(format!("contributors-id-{prefix}-{id}"))
}

fn ids(ui: &Ui, v: &mut VisualTestContext) -> Entity<IdentitiesView> {
    let view = ui.view(v);
    v.read(|cx| {
        view.read(cx)
            .identities_ui
            .clone()
            .expect("the section is drawn")
    })
}

fn name_field(ui: &Ui, v: &mut VisualTestContext, id: &str) -> Option<Entity<crate::input::Input>> {
    let view = ids(ui, v);
    v.read(|cx| view.read(cx).name_input(id))
}

fn row_handle(ui: &Ui, v: &mut VisualTestContext, key: &str) -> Option<gpui::FocusHandle> {
    let view = ids(ui, v);
    v.read(|cx| view.read(cx).row_handle(key))
}

fn focus_root(ui: &Ui, v: &mut VisualTestContext) {
    let root = ids(ui, v);
    let handle = v.read(|cx| root.read(cx).root_focus());
    ui.window
        .update(v, |_, window, cx| window.focus(&handle, cx))
        .unwrap();
    settle(v);
}

fn is_focused(ui: &Ui, v: &mut VisualTestContext, handle: &gpui::FocusHandle) -> bool {
    ui.window
        .update(v, |_, window, _| handle.is_focused(window))
        .unwrap()
}

fn type_name(ui: &Ui, v: &mut VisualTestContext, id: &str, text: &str) {
    let input = name_field(ui, v, id).expect("a name field");
    // The user clicks into the field before typing.
    let handle = v.read(|cx| input.read(cx).focus_handle(cx));
    ui.window
        .update(v, |_, window, cx| window.focus(&handle, cx))
        .unwrap();
    input.update(v, |input, cx| input.set_text(text.to_string(), cx));
    settle(v);
}

fn name_text(ui: &Ui, v: &mut VisualTestContext, id: &str) -> String {
    let input = name_field(ui, v, id).expect("a name field");
    v.read(|cx| input.read(cx).content.to_string())
}

fn center(v: &mut VisualTestContext, selector: &'static str) -> gpui::Point<gpui::Pixels> {
    v.debug_bounds(selector)
        .unwrap_or_else(|| panic!("nothing drawn as {selector}"))
        .center()
}

/// Presses an identity, moves it over `over` (or nowhere), without releasing.
fn drag_start(v: &mut VisualTestContext, grab: &'static str, over: Option<&'static str>) {
    let from = center(v, grab);
    v.simulate_mouse_down(from, MouseButton::Left, Default::default());
    v.simulate_mouse_move(
        from + gpui::point(gpui::px(8.), gpui::px(0.)),
        Some(MouseButton::Left),
        Default::default(),
    );
    settle(v);
    if let Some(over) = over {
        let to = center(v, over);
        v.simulate_mouse_move(to, Some(MouseButton::Left), Default::default());
        settle(v);
    }
}

fn release(v: &mut VisualTestContext, at: Option<&'static str>) {
    let to = at.map_or(gpui::point(gpui::px(2.), gpui::px(2.)), |selector| {
        center(v, selector)
    });
    v.simulate_mouse_up(to, MouseButton::Left, Default::default());
    settle(v);
}

#[gpui::test]
fn the_section_shows_the_people_the_suggestions_and_the_unmapped_card(cx: &mut TestAppContext) {
    let (_, mut v) = identities_window(cx);
    for selector in [
        "contributors-id-people",
        "contributors-id-new-person",
        "contributors-id-saved",
        "contributors-id-person-ana",
        "contributors-id-person-bob@example.com",
        "contributors-id-person-cat@example.com",
        "contributors-id-unmapped-card",
        "contributors-id-suggestion-bob@example.com|bob@home.net",
    ] {
        assert!(shown(&mut v, selector), "{selector}");
    }
    // The dependabot account is excluded outright, so it has no card.
    assert!(!shown(
        &mut v,
        "contributors-id-person-49699333+dependabot[bot]@users.noreply.github.com"
    ));
    assert!(!shown(&mut v, "contributors-id-merging-off"));
}

#[gpui::test]
fn the_cards_sit_in_two_columns_on_a_wide_page_and_stack_on_a_narrow_one(cx: &mut TestAppContext) {
    let (_, mut v) = identities_window(cx);
    let people = v.debug_bounds("contributors-id-people").unwrap();
    let side = v.debug_bounds("contributors-id-suggestions").unwrap();
    assert!(people.right() <= side.left(), "side by side");
    assert!(
        (people.top() - side.top()).abs() < gpui::px(1.),
        "from the same top"
    );
    v.simulate_resize(gpui::size(gpui::px(600.), gpui::px(4000.)));
    settle(&mut v);
    settle(&mut v);
    let people = v.debug_bounds("contributors-id-people").unwrap();
    let side = v.debug_bounds("contributors-id-suggestions").unwrap();
    assert!(
        side.top() >= people.bottom(),
        "stacked: {:?} {:?}",
        people,
        side
    );
}

#[gpui::test]
fn each_name_is_an_editable_field_with_the_saved_name(cx: &mut TestAppContext) {
    let (ui, mut v) = identities_window(cx);
    assert_eq!(name_text(&ui, &mut v, "ana"), "Ana Sol");
    assert_eq!(name_text(&ui, &mut v, "bob@example.com"), "Bob Stone");
    assert!(writes(&ui.fake).is_empty());
}

#[gpui::test]
fn commits_a_rename_on_blur(cx: &mut TestAppContext) {
    let (ui, mut v) = identities_window(cx);
    type_name(&ui, &mut v, "ana", "Ana Maria");
    assert!(writes(&ui.fake).is_empty());
    click(&mut v, "contributors-id-name-ana");
    focus_root(&ui, &mut v);
    let written = writes(&ui.fake);
    assert_eq!(written.len(), 1);
    assert_eq!(person_in(&written[0], "ana").unwrap().1, "Ana Maria");
}

#[gpui::test]
fn commits_a_rename_on_enter_and_creates_a_person_for_an_unmapped_one(cx: &mut TestAppContext) {
    let (ui, mut v) = identities_window(cx);
    type_name(&ui, &mut v, "bob@example.com", "Robert Stone");
    let input = name_field(&ui, &mut v, "bob@example.com").unwrap();
    input.update(&mut v, |_, cx| cx.emit(crate::input::Submitted));
    settle(&mut v);
    settle(&mut v);
    let written = writes(&ui.fake);
    assert_eq!(written.len(), 1);
    assert_eq!(
        person_in(&written[0], "bob@example.com"),
        Some((
            "bob@example.com".into(),
            "Robert Stone".into(),
            strings(&["bob@example.com"])
        ))
    );
}

#[gpui::test]
fn reverts_on_escape_without_saving_and_the_page_stays_open(cx: &mut TestAppContext) {
    let (ui, mut v) = identities_window(cx);
    click(&mut v, "contributors-id-name-ana");
    type_name(&ui, &mut v, "ana", "Nope");
    v.simulate_keystrokes("escape");
    settle(&mut v);
    assert_eq!(name_text(&ui, &mut v, "ana"), "Ana Sol");
    assert!(writes(&ui.fake).is_empty());
    assert_eq!(
        ui.closed_count(),
        0,
        "Escape in a field never closes the page"
    );
    // The next Escape belongs to the page.
    v.simulate_keystrokes("escape");
    settle(&mut v);
    assert_eq!(ui.closed_count(), 1);
}

#[gpui::test]
fn ignores_a_blank_name(cx: &mut TestAppContext) {
    let (ui, mut v) = identities_window(cx);
    type_name(&ui, &mut v, "ana", "   ");
    focus_root(&ui, &mut v);
    assert!(writes(&ui.fake).is_empty());
    assert_eq!(name_text(&ui, &mut v, "ana"), "Ana Sol");
}

#[gpui::test]
fn keeps_a_typed_name_while_the_saved_one_lands(cx: &mut TestAppContext) {
    let (ui, mut v) = identities_window(cx);
    type_name(&ui, &mut v, "ana", "Ana Maria");
    focus_root(&ui, &mut v);
    assert_eq!(name_text(&ui, &mut v, "ana"), "Ana Maria");
    settle(&mut v);
    let model = ui.snap(&mut v).model.expect("a model");
    assert_eq!(person_in_model(&model, "ana"), "Ana Maria");
    assert_eq!(name_text(&ui, &mut v, "ana"), "Ana Maria");
}

fn person_in_model(model: &ContributorsModel, id: &str) -> String {
    rows_of(model)
        .into_iter()
        .find(|r| r.id == id)
        .unwrap()
        .name
}

// ---- dragging

#[gpui::test]
fn moves_an_identity_when_it_is_dragged_onto_another_person(cx: &mut TestAppContext) {
    let (ui, mut v) = identities_window(cx);
    drag_start(
        &mut v,
        "contributors-id-grab-ana@laptop.local",
        Some("contributors-id-person-bob@example.com"),
    );
    let view = ids(&ui, &mut v);
    assert!(v.read(|cx| view.read(cx).dragging()));
    assert_eq!(
        v.read(|cx| view.read(cx).drag_over()).as_deref(),
        Some("bob@example.com"),
        "the card under the pointer takes the drop"
    );
    assert!(
        shown(&mut v, "contributors-id-ghost"),
        "the label follows the pointer"
    );
    assert!(writes(&ui.fake).is_empty());
    release(&mut v, Some("contributors-id-person-bob@example.com"));
    assert!(!shown(&mut v, "contributors-id-ghost"));
    assert!(!v.read(|cx| view.read(cx).dragging()));
    let written = writes(&ui.fake);
    assert_eq!(written.len(), 1);
    assert_eq!(
        person_in(&written[0], "ana").unwrap().2,
        strings(&["ana@example.com"])
    );
    assert_eq!(
        person_in(&written[0], "bob@example.com").unwrap().2,
        strings(&["bob@example.com", "ana@laptop.local"])
    );
}

#[gpui::test]
fn a_press_that_barely_moves_stays_a_click(cx: &mut TestAppContext) {
    let (ui, mut v) = identities_window(cx);
    let from = center(&mut v, "contributors-id-grab-ana@laptop.local");
    v.simulate_mouse_down(from, MouseButton::Left, Default::default());
    v.simulate_mouse_move(
        from + gpui::point(gpui::px(1.), gpui::px(0.)),
        Some(MouseButton::Left),
        Default::default(),
    );
    settle(&mut v);
    let view = ids(&ui, &mut v);
    assert!(!v.read(|cx| view.read(cx).dragging()));
    assert!(!shown(&mut v, "contributors-id-ghost"));
    let to = center(&mut v, "contributors-id-person-bob@example.com");
    v.simulate_mouse_up(to, MouseButton::Left, Default::default());
    settle(&mut v);
    assert!(writes(&ui.fake).is_empty());
}

#[gpui::test]
fn does_nothing_when_an_identity_is_dropped_on_its_own_person(cx: &mut TestAppContext) {
    let (ui, mut v) = identities_window(cx);
    drag_start(
        &mut v,
        "contributors-id-grab-ana@example.com",
        Some("contributors-id-person-ana"),
    );
    let view = ids(&ui, &mut v);
    assert!(
        v.read(|cx| view.read(cx).drag_over()).as_deref() == Some("ana"),
        "the pointer is over Ana's card"
    );
    release(&mut v, Some("contributors-id-person-ana"));
    assert!(writes(&ui.fake).is_empty());
    assert!(!v.read(|cx| view.read(cx).dragging()));
}

#[gpui::test]
fn does_nothing_when_released_outside_any_person(cx: &mut TestAppContext) {
    let (ui, mut v) = identities_window(cx);
    drag_start(&mut v, "contributors-id-grab-ana@laptop.local", None);
    release(&mut v, None);
    assert!(writes(&ui.fake).is_empty());
    let view = ids(&ui, &mut v);
    assert!(!v.read(|cx| view.read(cx).dragging()));
    assert!(!shown(&mut v, "contributors-id-ghost"));
}

#[gpui::test]
fn escape_cancels_the_drag_and_the_page_stays_open(cx: &mut TestAppContext) {
    let (ui, mut v) = identities_window(cx);
    drag_start(
        &mut v,
        "contributors-id-grab-ana@laptop.local",
        Some("contributors-id-person-bob@example.com"),
    );
    v.simulate_keystrokes("escape");
    settle(&mut v);
    let view = ids(&ui, &mut v);
    assert!(!v.read(|cx| view.read(cx).dragging()));
    assert!(!shown(&mut v, "contributors-id-ghost"));
    assert_eq!(ui.closed_count(), 0, "this Escape belonged to the drag");
    release(&mut v, Some("contributors-id-person-bob@example.com"));
    assert!(writes(&ui.fake).is_empty());
}

#[gpui::test]
fn only_the_primary_button_drags(cx: &mut TestAppContext) {
    let (ui, mut v) = identities_window(cx);
    let from = center(&mut v, "contributors-id-grab-ana@laptop.local");
    v.simulate_mouse_down(from, MouseButton::Right, Default::default());
    v.simulate_mouse_move(
        from + gpui::point(gpui::px(30.), gpui::px(0.)),
        Some(MouseButton::Right),
        Default::default(),
    );
    settle(&mut v);
    let view = ids(&ui, &mut v);
    assert!(!v.read(|cx| view.read(cx).dragging()));
    assert!(!shown(&mut v, "contributors-id-ghost"));
}

#[gpui::test]
fn does_not_drag_from_the_move_to_button_so_it_still_opens_its_menu(cx: &mut TestAppContext) {
    let (ui, mut v) = identities_window(cx);
    let from = center(&mut v, "contributors-id-move-ana@laptop.local");
    v.simulate_mouse_down(from, MouseButton::Left, Default::default());
    v.simulate_mouse_move(
        from + gpui::point(gpui::px(40.), gpui::px(0.)),
        Some(MouseButton::Left),
        Default::default(),
    );
    settle(&mut v);
    let view = ids(&ui, &mut v);
    assert!(!v.read(|cx| view.read(cx).dragging()));
    v.simulate_mouse_up(from, MouseButton::Left, Default::default());
    settle(&mut v);
    assert!(writes(&ui.fake).is_empty());
}

// ---- menus

#[gpui::test]
fn moves_an_identity_through_the_move_to_menu_without_a_drag(cx: &mut TestAppContext) {
    let (ui, mut v) = identities_window(cx);
    click(&mut v, "contributors-id-move-ana@laptop.local");
    assert!(shown(&mut v, "menu-action-move:bob@example.com"));
    assert!(
        !shown(&mut v, "menu-action-move:ana"),
        "its own person is not offered"
    );
    click(&mut v, "menu-action-move:bob@example.com");
    let written = writes(&ui.fake);
    assert_eq!(written.len(), 1);
    assert_eq!(
        person_in(&written[0], "ana").unwrap().2,
        strings(&["ana@example.com"])
    );
    assert!(
        person_in(&written[0], "bob@example.com")
            .unwrap()
            .2
            .contains(&"ana@laptop.local".to_string())
    );
    assert!(
        !shown(&mut v, "menu-action-move:bob@example.com"),
        "the menu closed"
    );
}

#[gpui::test]
fn keeps_focus_on_the_moved_identity_row_in_the_destination_card(cx: &mut TestAppContext) {
    let (ui, mut v) = identities_window(cx);
    click(&mut v, "contributors-id-move-ana@laptop.local");
    click(&mut v, "menu-action-move:bob@example.com");
    settle(&mut v);
    let handle = row_handle(&ui, &mut v, "ana@laptop.local").unwrap();
    assert!(
        is_focused(&ui, &mut v, &handle),
        "the moved row has the focus"
    );
    let model = ui.snap(&mut v).model.unwrap();
    let bob = rows_of(&model)
        .into_iter()
        .find(|r| r.id == "bob@example.com")
        .unwrap();
    assert!(
        keys(&bob).contains(&"ana@laptop.local"),
        "and it is in Bob's card"
    );
    let view = ids(&ui, &mut v);
    assert!(!v.read(|cx| view.read(cx).focus_pending()));
}

#[gpui::test]
fn a_drag_does_not_move_the_focus(cx: &mut TestAppContext) {
    let (ui, mut v) = identities_window(cx);
    drag_start(
        &mut v,
        "contributors-id-grab-ana@laptop.local",
        Some("contributors-id-person-bob@example.com"),
    );
    release(&mut v, Some("contributors-id-person-bob@example.com"));
    settle(&mut v);
    let view = ids(&ui, &mut v);
    assert!(!v.read(|cx| view.read(cx).focus_pending()));
}

#[gpui::test]
fn keeps_focus_on_the_identity_row_after_add_to_person_too(cx: &mut TestAppContext) {
    let (ui, mut v) = identities_window(cx);
    click(&mut v, "contributors-id-add-ci@build-box");
    click(&mut v, "menu-action-add:ana");
    settle(&mut v);
    let written = writes(&ui.fake);
    assert!(
        person_in(&written[0], "ana")
            .unwrap()
            .2
            .contains(&"ci@build-box".to_string())
    );
    let handle = row_handle(&ui, &mut v, "ci@build-box").unwrap();
    assert!(is_focused(&ui, &mut v, &handle));
}

#[gpui::test]
fn does_not_steal_focus_later_when_the_move_could_not_be_saved(cx: &mut TestAppContext) {
    let (ui, mut v) = identities_window(cx);
    ui.fake
        .with(|s| s.write.once.push_back(Err("disk is full".into())));
    click(&mut v, "contributors-id-move-ana@laptop.local");
    click(&mut v, "menu-action-move:bob@example.com");
    settle(&mut v);
    settle(&mut v);
    assert_eq!(writes(&ui.fake).len(), 1, "the write was tried");
    let view = ids(&ui, &mut v);
    assert!(
        !v.read(|cx| view.read(cx).focus_pending()),
        "the wish is dropped"
    );
    // The move was reverted: Ana still holds the identity.
    let model = ui.snap(&mut v).model.unwrap();
    let ana = rows_of(&model).into_iter().find(|r| r.id == "ana").unwrap();
    assert!(keys(&ana).contains(&"ana@laptop.local"));
    // Later frames leave the focus alone: it goes where the user puts it.
    focus_root(&ui, &mut v);
    settle(&mut v);
    let handle = row_handle(&ui, &mut v, "ana@laptop.local").unwrap();
    let root = v.read(|cx| view.read(cx).root_focus());
    assert!(is_focused(&ui, &mut v, &root));
    assert!(!is_focused(&ui, &mut v, &handle));
}

#[gpui::test]
fn merges_a_person_into_another_from_the_person_menu_and_focuses_its_name(cx: &mut TestAppContext) {
    let (ui, mut v) = identities_window(cx);
    click(&mut v, "contributors-id-actions-cat@example.com");
    assert!(shown(&mut v, "menu-action-merge"));
    click(&mut v, "menu-action-merge");
    assert!(
        shown(&mut v, "menu-action-merge:ana"),
        "the list of people opens"
    );
    assert!(
        !shown(&mut v, "menu-action-merge:cat@example.com"),
        "not into itself"
    );
    click(&mut v, "menu-action-merge:ana");
    settle(&mut v);
    let written = writes(&ui.fake);
    assert_eq!(written.len(), 1);
    let ana = person_in(&written[0], "ana").unwrap();
    assert_eq!(ana.1, "Ana Sol");
    assert_eq!(
        ana.2,
        strings(&["ana@example.com", "ana@laptop.local", "cat@example.com"])
    );
    let input = name_field(&ui, &mut v, "ana").unwrap();
    let handle = v.read(|cx| input.read(cx).focus_handle(cx));
    assert!(
        is_focused(&ui, &mut v, &handle),
        "the merged person's name has the focus"
    );
}

#[gpui::test]
fn escape_closes_a_menu_only_and_not_the_page(cx: &mut TestAppContext) {
    let (ui, mut v) = identities_window(cx);
    cx.update(crate::popover::bind);
    click(&mut v, "contributors-id-actions-cat@example.com");
    let view = ids(&ui, &mut v);
    assert!(v.read(|cx| view.read(cx).menu_is_open()));
    v.simulate_keystrokes("escape");
    settle(&mut v);
    assert!(!v.read(|cx| view.read(cx).menu_is_open()));
    assert!(!shown(&mut v, "menu-action-merge"));
    assert_eq!(ui.closed_count(), 0, "the page stays open");
}

#[gpui::test]
fn removing_a_person_from_its_menu_ungroups_its_identities(cx: &mut TestAppContext) {
    let (ui, mut v) = identities_window(cx);
    click(&mut v, "contributors-id-actions-ana");
    click(&mut v, "menu-action-remove-person");
    let written = writes(&ui.fake);
    assert_eq!(written.len(), 1);
    assert!(people_of(&written[0]).is_empty());
}

#[gpui::test]
fn cannot_remove_a_person_that_only_exists_through_observation(cx: &mut TestAppContext) {
    let (ui, mut v) = identities_window(cx);
    click(&mut v, "contributors-id-actions-cat@example.com");
    click(&mut v, "menu-action-remove-person");
    assert!(writes(&ui.fake).is_empty());
}

#[gpui::test]
fn creates_a_person_named_new_person_and_focuses_its_name(cx: &mut TestAppContext) {
    let (ui, mut v) = identities_window(cx);
    let (_, id) = add_person(&config());
    click(&mut v, "contributors-id-new-person");
    settle(&mut v);
    let written = writes(&ui.fake);
    assert_eq!(written.len(), 1);
    assert_eq!(person_in(&written[0], &id).unwrap().1, NEW_PERSON_NAME);
    assert!(shown(&mut v, sel("person", &id)));
    let input = name_field(&ui, &mut v, &id).unwrap();
    let handle = v.read(|cx| input.read(cx).focus_handle(cx));
    assert!(is_focused(&ui, &mut v, &handle));
    assert!(
        shown(&mut v, sel("drophint", &id)),
        "an empty card still takes a drop"
    );
}

// ---- merging off

fn merge_off(cx: &mut TestAppContext) -> (Ui, VisualTestContext) {
    let (ui, mut v) = identities_window(cx);
    click(&mut v, "contributors-toggle-merge");
    settle(&mut v);
    (ui, v)
}

#[gpui::test]
fn shows_the_merge_off_notice_and_renders_raw_identities_read_only(cx: &mut TestAppContext) {
    let (ui, mut v) = merge_off(cx);
    assert!(shown(&mut v, "contributors-id-merging-off"));
    assert!(shown(&mut v, "contributors-id-suggestions-off"));
    // Raw identities: a card per git identity, no mapping applied.
    assert!(shown(&mut v, "contributors-id-person-ana@laptop.local"));
    assert!(!shown(&mut v, "contributors-id-person-ana"));
    let view = ids(&ui, &mut v);
    assert!(
        v.read(|cx| view.read(cx).name_input("ana@laptop.local"))
            .is_some()
    );
    // Read only: no grips, menus, hints, suggestions.
    for selector in [
        "contributors-id-grab-ana@laptop.local",
        "contributors-id-actions-ana@laptop.local",
        "contributors-id-actions-identity-ana@laptop.local",
        "contributors-id-move-ana@laptop.local",
        "contributors-id-drophint-ana@laptop.local",
        "contributors-id-add-ci@build-box",
    ] {
        assert!(!shown(&mut v, selector), "{selector}");
    }
    assert!(!shown(
        &mut v,
        "contributors-id-dismiss-bob@example.com|bob@home.net"
    ));
    // New person is inert.
    click(&mut v, "contributors-id-new-person");
    assert!(writes(&ui.fake).is_empty());
    // Excluding an unmapped identity still works.
    assert!(shown(&mut v, "contributors-id-exclude-ci@build-box"));
}

#[gpui::test]
fn the_names_are_plain_text_while_merging_is_off(cx: &mut TestAppContext) {
    let (_, mut v) = merge_off(cx);
    assert!(shown(&mut v, "contributors-id-name-ana@laptop.local"));
    assert!(
        !shown(&mut v, "contributors-id-field-ana@laptop.local"),
        "no editable field"
    );
}

// ---- suggested merges

#[gpui::test]
fn merges_a_suggestion_into_the_busiest_person(cx: &mut TestAppContext) {
    let (ui, mut v) = identities_window(cx);
    click(&mut v, "contributors-id-merge-bob@example.com|bob@home.net");
    let written = writes(&ui.fake);
    assert_eq!(written.len(), 1);
    let bob = person_in(&written[0], "bob@example.com").unwrap();
    assert_eq!(bob.1, "Bob Stone");
    assert_eq!(bob.2, strings(&["bob@example.com", "bob@home.net"]));
}

#[gpui::test]
fn dismisses_a_suggestion_and_it_stays_gone(cx: &mut TestAppContext) {
    let (ui, mut v) = identities_window(cx);
    click(
        &mut v,
        "contributors-id-dismiss-bob@example.com|bob@home.net",
    );
    let written = writes(&ui.fake);
    assert_eq!(written[0]["dismissedMerges"].as_array().unwrap().len(), 1);
    settle(&mut v);
    assert!(!shown(
        &mut v,
        "contributors-id-suggestion-bob@example.com|bob@home.net"
    ));
    assert!(shown(&mut v, "contributors-id-no-suggestions"));
}

// ---- unmapped and excluded

#[gpui::test]
fn lists_the_identities_that_need_attention(cx: &mut TestAppContext) {
    let (_, mut v) = identities_window(cx);
    assert!(shown(&mut v, "contributors-id-unmapped-ci@build-box"));
    assert!(shown(
        &mut v,
        "contributors-id-unmapped-noreply@anthropic.com"
    ));
    for author in ["ana@example.com", "bob@example.com", "cat@example.com"] {
        assert!(!shown(&mut v, sel("unmapped", author)), "{author}");
    }
}

#[gpui::test]
fn excludes_an_identity_lists_it_under_excluded_and_includes_it_again(cx: &mut TestAppContext) {
    let (ui, mut v) = identities_window(cx);
    click(&mut v, "contributors-id-exclude-ci@build-box");
    let written = writes(&ui.fake);
    assert!(
        written[0]["excludeIdentities"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p == "ci@build-box")
    );
    settle(&mut v);
    assert!(!shown(&mut v, "contributors-id-unmapped-ci@build-box"));
    assert!(
        !shown(&mut v, "contributors-id-excluded-ci@build-box"),
        "folded away"
    );
    click(&mut v, "contributors-id-excluded-toggle");
    assert!(shown(&mut v, "contributors-id-excluded-ci@build-box"));
    click(&mut v, "contributors-id-include-ci@build-box");
    let written = writes(&ui.fake);
    assert_eq!(written.len(), 2);
    assert!(
        !written[1]["excludeIdentities"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p == "ci@build-box")
    );
    settle(&mut v);
    assert!(shown(&mut v, "contributors-id-unmapped-ci@build-box"));
}

#[gpui::test]
fn explains_a_wildcard_exclusion_instead_of_offering_to_lift_it(cx: &mut TestAppContext) {
    let (_, mut v) = identities_window(cx);
    let key = "49699333+dependabot[bot]@users.noreply.github.com";
    click(&mut v, "contributors-id-excluded-toggle");
    assert!(shown(&mut v, sel("excluded", key)));
    assert!(shown(&mut v, sel("why", key)));
    assert!(!shown(&mut v, sel("include", key)));
}

#[gpui::test]
fn flags_an_unmapped_agent_and_replaces_exclude_with_a_note(cx: &mut TestAppContext) {
    let (_, mut v) = identities_window(cx);
    assert!(shown(
        &mut v,
        "contributors-id-unmapped-agent-noreply@anthropic.com"
    ));
    assert!(shown(
        &mut v,
        "contributors-id-agent-note-noreply@anthropic.com"
    ));
    assert!(!shown(
        &mut v,
        "contributors-id-exclude-noreply@anthropic.com"
    ));
    assert!(shown(&mut v, "contributors-id-exclude-ci@build-box"));
}

#[gpui::test]
fn marks_an_agent_identity_in_a_person_card(cx: &mut TestAppContext) {
    let mut with_agent = config();
    with_agent.people[0]
        .identities
        .push("noreply@anthropic.com".into());
    let (_, mut v) = identities_window_of(cx, window_fake(Some(serialize_config(&with_agent))));
    assert!(shown(&mut v, "contributors-id-agent-noreply@anthropic.com"));
    assert!(!shown(&mut v, "contributors-id-agent-ana@example.com"));
}

#[gpui::test]
fn adds_an_unmapped_identity_to_a_person_through_the_menu(cx: &mut TestAppContext) {
    let (ui, mut v) = identities_window(cx);
    click(&mut v, "contributors-id-add-ci@build-box");
    click(&mut v, "menu-action-add:ana");
    let written = writes(&ui.fake);
    assert!(
        person_in(&written[0], "ana")
            .unwrap()
            .2
            .contains(&"ci@build-box".to_string())
    );
}

// ---- exclude from the cards

#[gpui::test]
fn excludes_one_identity_from_its_own_menu(cx: &mut TestAppContext) {
    let (ui, mut v) = identities_window(cx);
    click(&mut v, "contributors-id-actions-identity-cat@example.com");
    click(&mut v, "menu-action-exclude-identity");
    let written = writes(&ui.fake);
    assert!(
        written[0]["excludeIdentities"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p == "cat@example.com")
    );
    settle(&mut v);
    assert!(!shown(&mut v, "contributors-id-person-cat@example.com"));
}

#[gpui::test]
fn excludes_every_identity_of_a_person_but_never_an_agent(cx: &mut TestAppContext) {
    let mut with_agent = config();
    with_agent.people[0]
        .identities
        .push("noreply@anthropic.com".into());
    let (ui, mut v) = identities_window_of(cx, window_fake(Some(serialize_config(&with_agent))));
    click(&mut v, "contributors-id-actions-ana");
    click(&mut v, "menu-action-exclude-person");
    let written = writes(&ui.fake);
    let excluded: Vec<&str> = written[0]["excludeIdentities"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p.as_str().unwrap())
        .collect();
    assert!(excluded.contains(&"ana@example.com"));
    assert!(excluded.contains(&"ana@laptop.local"));
    assert!(!excluded.contains(&"noreply@anthropic.com"));
}

#[gpui::test]
fn an_agent_identity_cannot_be_excluded_from_its_menu(cx: &mut TestAppContext) {
    let mut with_agent = config();
    with_agent.people[0]
        .identities
        .push("noreply@anthropic.com".into());
    let (ui, mut v) = identities_window_of(cx, window_fake(Some(serialize_config(&with_agent))));
    click(
        &mut v,
        "contributors-id-actions-identity-noreply@anthropic.com",
    );
    click(&mut v, "menu-action-exclude-identity");
    assert!(writes(&ui.fake).is_empty());
}

#[gpui::test]
fn a_person_made_only_of_agents_cannot_be_excluded_from_its_menu(cx: &mut TestAppContext) {
    let mut only_agents = default_config();
    only_agents.people = vec![Person {
        id: "bot".into(),
        name: "Robot".into(),
        identities: vec!["noreply@anthropic.com".into()],
    }];
    let (ui, mut v) = identities_window_of(cx, window_fake(Some(serialize_config(&only_agents))));
    click(&mut v, "contributors-id-actions-bot");
    click(&mut v, "menu-action-exclude-person");
    assert!(writes(&ui.fake).is_empty());
}

// ---- the lock

/// The file was fine, then turned invalid and the page reloaded it: the config
/// falls back to the defaults and every write is blocked.
fn lock(ui: &Ui, v: &mut VisualTestContext) {
    ui.fake
        .with(|s| s.config.default = Ok(Some("{ nope".into())));
    click(v, "contributors-refresh");
    assert!(shown(v, "contributors-locked"));
    settle(v);
}

#[gpui::test]
fn disables_every_editing_control_and_writes_nothing_while_the_file_is_unreadable(
    cx: &mut TestAppContext,
) {
    let (ui, mut v) = identities_window(cx);
    lock(&ui, &mut v);
    // Names are plain text, nothing can be dragged, no menu is offered.
    assert!(shown(&mut v, "contributors-id-name-ana@example.com"));
    assert!(!shown(&mut v, "contributors-id-field-ana@example.com"));
    for selector in [
        "contributors-id-grab-ana@laptop.local",
        "contributors-id-actions-ana@example.com",
        "contributors-id-actions-identity-ana@laptop.local",
        "contributors-id-move-ana@laptop.local",
        "contributors-id-drophint-ana@example.com",
    ] {
        assert!(!shown(&mut v, selector), "{selector}");
    }
    // The buttons are drawn dimmed and do nothing.
    for selector in [
        "contributors-id-new-person",
        "contributors-id-dismiss-bob@example.com|bob@home.net",
        "contributors-id-merge-bob@example.com|bob@home.net",
        "contributors-id-exclude-ci@build-box",
        "contributors-id-add-ci@build-box",
    ] {
        click(&mut v, selector);
    }
    let view = ids(&ui, &mut v);
    assert!(!v.read(|cx| view.read(cx).menu_is_open()));
    assert!(writes(&ui.fake).is_empty());
}

#[gpui::test]
fn disables_include_again_for_an_identity_excluded_by_a_literal(cx: &mut TestAppContext) {
    // `noreply@github.com` is excluded by a literal of the default config.
    let mut raw = raw();
    raw.commits
        .push(commit("g1", &who("GitHub", "noreply@github.com"), 11));
    let (ui, mut v) = identities_window_of(cx, window_fake_of(raw, None));
    lock(&ui, &mut v);
    click(&mut v, "contributors-id-excluded-toggle");
    assert!(shown(&mut v, "contributors-id-include-noreply@github.com"));
    click(&mut v, "contributors-id-include-noreply@github.com");
    assert!(writes(&ui.fake).is_empty());
}

#[gpui::test]
fn a_name_typed_before_the_lock_landed_is_not_saved_after_it(cx: &mut TestAppContext) {
    let (ui, mut v) = identities_window(cx);
    type_name(&ui, &mut v, "ana", "Ana Maria");
    // The file turns invalid and is reloaded while the draft is still being typed.
    ui.fake
        .with(|s| s.config.default = Ok(Some("{ nope".into())));
    let view = ui.view(&mut v);
    view.update(&mut v, |view, cx| view.refresh(true, cx));
    settle(&mut v);
    assert!(shown(&mut v, "contributors-locked"));
    // Leaving the field now tries to commit the draft, which must not be written.
    let root = ids(&ui, &mut v);
    let handle = v.read(|cx| root.read(cx).root_focus());
    ui.window
        .update(&mut v, |_, window, cx| window.focus(&handle, cx))
        .unwrap();
    settle(&mut v);
    assert!(writes(&ui.fake).is_empty());
}

#[gpui::test]
fn a_failed_save_is_left_to_the_page_and_shows_no_alert_of_its_own(cx: &mut TestAppContext) {
    let (ui, mut v) = identities_window(cx);
    ui.fake
        .with(|s| s.write.once.push_back(Err("disk is full".into())));
    type_name(&ui, &mut v, "ana", "Ana Maria");
    focus_root(&ui, &mut v);
    settle(&mut v);
    assert!(
        shown(&mut v, "contributors-save-error"),
        "the page reports it once"
    );
    // Ana's card is back to the saved name in the model.
    let model = ui.snap(&mut v).model.unwrap();
    assert_eq!(person_in_model(&model, "ana"), "Ana Sol");
}

#[gpui::test]
fn a_failed_rename_puts_the_saved_name_back_in_the_unfocused_field(cx: &mut TestAppContext) {
    let (ui, mut v) = identities_window(cx);
    ui.fake
        .with(|s| s.write.once.push_back(Err("disk is full".into())));
    type_name(&ui, &mut v, "ana", "Ana Maria");
    focus_root(&ui, &mut v);
    settle(&mut v);
    settle(&mut v);
    assert_eq!(writes(&ui.fake).len(), 1, "the write was tried");
    assert_eq!(name_text(&ui, &mut v, "ana"), "Ana Sol");
}

#[gpui::test]
fn a_refused_rename_puts_the_saved_name_back_when_the_field_returns(cx: &mut TestAppContext) {
    let (ui, mut v) = identities_window(cx);
    type_name(&ui, &mut v, "ana", "Ana Maria");
    // The file turns invalid before the field is left: the save is refused.
    ui.fake
        .with(|s| s.config.default = Ok(Some("<<<<<<< HEAD".into())));
    let view = ui.view(&mut v);
    view.update(&mut v, |view, cx| view.refresh(true, cx));
    settle(&mut v);
    assert!(v.read(|cx| view.read(cx).locked(cx)));
    focus_root(&ui, &mut v);
    settle(&mut v);
    assert!(writes(&ui.fake).is_empty());
    // The file is fixed and reloaded.
    ui.fake
        .with(|s| s.config.default = Ok(Some(serialize_config(&config()))));
    view.update(&mut v, |view, cx| view.refresh(true, cx));
    settle(&mut v);
    settle(&mut v);
    assert!(!v.read(|cx| view.read(cx).locked(cx)));
    assert_eq!(name_text(&ui, &mut v, "ana"), "Ana Sol");
}
