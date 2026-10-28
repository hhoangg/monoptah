//! Identities that need someone to look at them, and the Identities tab rows.
//! Port of `unmapped.ts`.

use super::identity::{
    github_login_from_email, identity_key, is_agent, is_excluded, MailmapPerson, ResolvedPerson,
    Resolver,
};
use super::patterns::matches_any_identity;
use super::scoring::observed_identities;
use super::types::{ContributorsConfig, Person};
use super::{js_trim, locale_cmp, OrderedMap};
use crate::contributors::{GitIdentity, RawData};
use regex::Regex;
use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnmappedReason {
    Agent,
    Machine,
}

#[derive(Clone, Debug, PartialEq)]
pub struct UnmappedIdentity {
    pub key: String,
    pub identity: GitIdentity,
    /// Commits in the raw data authored with this identity.
    pub commits: u64,
    pub agent: bool,
    /// Why it needs attention: an agent needs an owner, a machine account an
    /// owner or an exclusion.
    pub reason: UnmappedReason,
}

const MACHINE_LOCAL_PARTS: [&str; 9] = [
    "root", "admin", "ubuntu", "runner", "ci", "build", "jenkins", "git", "github",
];

static RUNNER_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(^|[^a-z])runner([^a-z]|$)").expect("runner pattern"));

/// An address no person reads: a host without a dot (`dev@laptop`), a `.local`
/// host, a service-account local-part, or a CI runner. `runner` must stand
/// alone as a word, so a surname such as Brunner is not a machine.
pub fn is_machine_identity(identity: &GitIdentity) -> bool {
    let email = js_trim(&identity.email).to_lowercase();
    let Some(at) = email.rfind('@').filter(|at| *at > 0) else {
        return false;
    };
    let (local, domain) = (&email[..at], &email[at + 1..]);
    if !domain.contains('.') || domain.ends_with(".local") {
        return true;
    }
    if MACHINE_LOCAL_PARTS.contains(&local) {
        return true;
    }
    RUNNER_RE.is_match(local)
}

/// Every string a listed identity can be matched by: itself plus the login of a noreply email.
fn aliases_of(identity: &str) -> Vec<String> {
    let lower = js_trim(identity).to_lowercase();
    match github_login_from_email(&lower) {
        Some(login) => vec![lower, format!("gh:{login}")],
        None => vec![lower],
    }
}

/// The login an identity carries, or the one in its noreply email.
fn login_of(id: &GitIdentity) -> Option<String> {
    // `||`: an empty login string must still fall back to the noreply email.
    own_login(id).or_else(|| github_login_from_email(&id.email))
}

/// The login an identity carries itself, lowercased; an empty one is none.
fn own_login(id: &GitIdentity) -> Option<String> {
    id.login
        .as_deref()
        .map(|l| js_trim(l).to_lowercase())
        .filter(|l| !l.is_empty())
}

fn commits_by_author_key(raw: &RawData) -> HashMap<String, u64> {
    let mut commits = HashMap::new();
    for c in &raw.commits {
        *commits.entry(identity_key(&c.author)).or_insert(0) += 1;
    }
    commits
}

/// Identities seen in the raw data that no config person and no mailmap person
/// lists AND that need someone to look at them: agents (they need an owner) and
/// machine accounts such as CI runners. An ordinary author who is simply their
/// own person is not listed. Excluded identities are not listed. One shared
/// selector so the header badge and the Identities tab agree.
pub fn unmapped_identities(
    raw: &RawData,
    config: &ContributorsConfig,
    mailmap: &[MailmapPerson],
) -> Vec<UnmappedIdentity> {
    let mut listed: HashSet<String> = HashSet::new();
    for person in &config.people {
        for id in &person.identities {
            listed.extend(aliases_of(id));
        }
    }
    for group in mailmap {
        for id in &group.identities {
            listed.extend(aliases_of(id));
        }
    }

    let observed = observed_identities(raw);
    // A login-only identity (a reviewer) belongs to the commit identity that
    // carries the same login; it is not a second unmapped person.
    let mut known_logins: HashSet<String> = HashSet::new();
    for id in &observed {
        if js_trim(&id.email).is_empty() {
            continue;
        }
        if let Some(login) = login_of(id) {
            known_logins.insert(login);
        }
    }

    let commits = commits_by_author_key(raw);
    let mut out: OrderedMap<UnmappedIdentity> = OrderedMap::new();
    let mut seen: HashSet<String> = HashSet::new();
    for id in &observed {
        let key = identity_key(id);
        if seen.contains(&key) || key.is_empty() {
            continue;
        }
        if is_excluded(config, id) {
            continue;
        }
        let mut candidates: HashSet<String> = aliases_of(&key).into_iter().collect();
        candidates.insert(key.clone());
        if let Some(login) = own_login(id) {
            candidates.insert(format!("gh:{login}"));
        }
        if candidates.iter().any(|c| listed.contains(c)) {
            continue;
        }
        if key
            .strip_prefix("gh:")
            .is_some_and(|login| known_logins.contains(login))
        {
            continue;
        }
        let agent = is_agent(config, id);
        if !agent && !is_machine_identity(id) {
            continue;
        }
        seen.insert(key.clone());
        out.entry(&key, || UnmappedIdentity {
            key: key.clone(),
            identity: id.clone(),
            commits: commits.get(&key).copied().unwrap_or(0),
            agent,
            reason: if agent {
                UnmappedReason::Agent
            } else {
                UnmappedReason::Machine
            },
        });
    }
    let mut list: Vec<UnmappedIdentity> = out.into_values().collect();
    list.sort_by(|a, b| {
        b.commits
            .cmp(&a.commits)
            .then_with(|| locale_cmp(&a.key, &b.key))
    });
    list
}

#[derive(Clone, Debug, PartialEq)]
pub struct ExcludedIdentity {
    pub key: String,
    pub identity: GitIdentity,
    pub commits: u64,
    /// Exclude patterns that match this identity.
    pub patterns: Vec<String>,
    /// True when every matching pattern is a literal, so "Include again" can
    /// lift the exclusion. A wildcard such as `*[bot]*` covers other
    /// identities too and is edited in the config file instead.
    pub removable: bool,
}

/// Observed identities the config excludes (agents are never excluded).
pub fn excluded_identities(raw: &RawData, config: &ContributorsConfig) -> Vec<ExcludedIdentity> {
    let commits = commits_by_author_key(raw);
    let mut out: Vec<ExcludedIdentity> = vec![];
    let mut seen: HashSet<String> = HashSet::new();
    for id in observed_identities(raw) {
        let key = identity_key(&id);
        if key.is_empty() || seen.contains(&key) || !is_excluded(config, &id) {
            continue;
        }
        let patterns: Vec<String> = config
            .exclude_identities
            .iter()
            .filter(|p| matches_any_identity(&[p], &id))
            .cloned()
            .collect();
        seen.insert(key.clone());
        out.push(ExcludedIdentity {
            commits: commits.get(&key).copied().unwrap_or(0),
            removable: !patterns.is_empty() && patterns.iter().all(|p| !p.contains('*')),
            patterns,
            identity: id,
            key,
        });
    }
    out.sort_by(|a, b| {
        b.commits
            .cmp(&a.commits)
            .then_with(|| locale_cmp(&a.key, &b.key))
    });
    out
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IdentityRow {
    pub key: String,
    pub agent: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PersonRow {
    pub id: String,
    pub name: String,
    pub identities: Vec<IdentityRow>,
    /// Commits in the raw data authored by any identity of this person.
    pub commits: u64,
    /// A person stored in the config file; the others only exist through observation.
    pub in_config: bool,
}

fn pseudo_identity(value: &str) -> GitIdentity {
    if value.contains('@') {
        return GitIdentity {
            name: String::new(),
            email: value.into(),
            login: None,
        };
    }
    if let Some(login) = value.strip_prefix("gh:") {
        return GitIdentity {
            name: String::new(),
            email: String::new(),
            login: Some(login.into()),
        };
    }
    GitIdentity {
        name: value.into(),
        email: String::new(),
        login: None,
    }
}

/// The observed identity a listed identity string stands for, else a stand-in
/// built from the string.
fn identity_for(value: &str, observed: &[GitIdentity]) -> GitIdentity {
    let lower = js_trim(value).to_lowercase();
    let login = github_login_from_email(&lower);
    observed
        .iter()
        .find(|o| {
            if identity_key(o) == lower {
                return true;
            }
            own_login(o)
                .is_some_and(|own| lower == format!("gh:{own}") || Some(&own) == login.as_ref())
        })
        .cloned()
        .unwrap_or_else(|| pseudo_identity(&lower))
}

/// Listed identities plus observed ones that resolve to the person without
/// being listed (joined through a mailmap group), so none of them vanishes
/// from the tab. A login-only identity already tied to another identity is not
/// extra.
fn identity_keys_of(person: &ResolvedPerson) -> Vec<String> {
    let mut keys: Vec<String> = person
        .identities
        .iter()
        .map(|k| js_trim(k).to_lowercase())
        .collect();
    let mut known: HashSet<String> = keys.iter().flat_map(|k| aliases_of(k)).collect();
    let mut logins: HashSet<String> = HashSet::new();
    for o in &person.observed {
        if js_trim(&o.email).is_empty() {
            continue;
        }
        if let Some(login) = login_of(o) {
            logins.insert(login);
        }
    }
    for o in &person.observed {
        let key = identity_key(o);
        if key.is_empty() || known.contains(&key) {
            continue;
        }
        if let Some(login) = key.strip_prefix("gh:") {
            if logins.contains(login) || known.contains(&format!("gh:{login}")) {
                continue;
            }
        }
        known.extend(aliases_of(&key));
        keys.push(key);
    }
    keys
}

/// People for the Identities tab, from the model's resolver (so merge off
/// gives one person per raw identity). Auto people made only of agents or
/// excluded identities are left out: they show in the unmapped and excluded
/// lists.
pub fn person_rows(
    raw: &RawData,
    config: &ContributorsConfig,
    resolver: &Resolver,
) -> Vec<PersonRow> {
    let config_ids: HashSet<&str> = config.people.iter().map(|p| p.id.as_str()).collect();
    let mut commits: HashMap<String, u64> = HashMap::new();
    for c in &raw.commits {
        *commits.entry(resolver.person_of(&c.author)).or_insert(0) += 1;
    }

    let mut rows: Vec<PersonRow> = vec![];
    for person in resolver.people().iter() {
        let in_config = config_ids.contains(person.id.as_str());
        let only_non_human = !person.observed.is_empty()
            && person
                .observed
                .iter()
                .all(|o| is_agent(config, o) || is_excluded(config, o));
        if !in_config && (only_non_human || person.identities.is_empty()) {
            continue;
        }
        rows.push(PersonRow {
            id: person.id.clone(),
            name: person.name.clone(),
            identities: identity_keys_of(person)
                .into_iter()
                .map(|key| IdentityRow {
                    agent: is_agent(config, &identity_for(&key, &person.observed)),
                    key,
                })
                .collect(),
            commits: commits.get(&person.id).copied().unwrap_or(0),
            in_config,
        });
    }
    rows.sort_by(|a, b| {
        b.commits
            .cmp(&a.commits)
            .then_with(|| locale_cmp(&a.name, &b.name))
    });
    rows
}

/// Makes the person a config person that lists every identity the row shows,
/// so edits can target it and moving one identity out leaves the rest
/// (including mailmap members) with this person.
pub fn ensure_config_person(config: &ContributorsConfig, row: &PersonRow) -> ContributorsConfig {
    let mut next = config.clone();
    if let Some(existing) = next.people.iter_mut().find(|p| p.id == row.id) {
        for identity in &row.identities {
            if !existing.identities.contains(&identity.key) {
                existing.identities.push(identity.key.clone());
            }
        }
        return next;
    }
    next.people.push(Person {
        id: row.id.clone(),
        name: row.name.clone(),
        identities: row.identities.iter().map(|i| i.key.clone()).collect(),
    });
    next
}

/// Ungroups a config person; its identities fall back to mailmap or to their own people.
pub fn remove_person(config: &ContributorsConfig, person_id: &str) -> ContributorsConfig {
    let mut next = config.clone();
    next.people.retain(|p| p.id != person_id);
    next
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contributors::model::config::default_config;
    use crate::contributors::model::identity::{build_resolver, ResolverOptions};
    use crate::contributors::model::scoring::observed_identities;
    use crate::contributors::model::test_fixtures::*;
    use crate::contributors::model::types::{ContributorsConfig, Person};
    use crate::contributors::RawData;

    fn build_box() -> GitIdentity {
        ident("Build Box", "ci@build-box")
    }

    fn runner() -> GitIdentity {
        ident("runner", "runner@ip-10-0-0-5.ec2.internal")
    }

    fn person(id: &str, name: &str, identities: &[&str]) -> Person {
        Person {
            id: id.into(),
            name: name.into(),
            identities: identities.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn strs(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    fn unmapped(raw: &RawData, config: &ContributorsConfig) -> Vec<UnmappedIdentity> {
        unmapped_identities(raw, config, &[])
    }

    // unmappedIdentities

    #[test]
    fn unmapped_identities_lists_agents_and_machine_identities_most_commits_first() {
        let raw = raw_with(|r| {
            r.commits = vec![
                make_commit("c1", &claude(), &oct12(1)),
                make_commit("b1", &build_box(), &oct12(2)),
                make_commit("b2", &build_box(), &oct12(3)),
            ];
        });
        let out = unmapped(&raw, &default_config());
        let got: Vec<(&str, UnmappedReason, u64)> = out
            .iter()
            .map(|u| (u.key.as_str(), u.reason, u.commits))
            .collect();
        assert_eq!(
            got,
            [
                ("ci@build-box", UnmappedReason::Machine, 2),
                ("noreply@anthropic.com", UnmappedReason::Agent, 1),
            ]
        );
        assert!(out[1].agent);
        assert!(!out[0].agent);
    }

    #[test]
    fn unmapped_identities_does_not_list_ordinary_people_who_are_simply_their_own_person() {
        let raw = raw_with(|r| {
            r.commits = vec![
                make_commit("a1", &ana(), &oct12(1)),
                make_commit("b1", &bob(), &oct12(2)),
            ];
        });
        assert!(unmapped(&raw, &default_config()).is_empty());
    }

    #[test]
    fn unmapped_identities_lists_machine_identities_by_domain_and_local_part() {
        let cases: Vec<(&str, GitIdentity)> = vec![
            ("a domain without a dot", ident("Dev", "dev@laptop")),
            ("a .local domain", ident("Dev", "dev@Mac-Studio.local")),
            ("a root local-part", ident("root", "root@example.com")),
            ("an admin local-part", ident("Admin", "admin@example.com")),
            (
                "an ubuntu local-part",
                ident("ubuntu", "ubuntu@example.com"),
            ),
            (
                "a jenkins local-part",
                ident("Jenkins", "jenkins@example.com"),
            ),
            ("a git local-part", ident("git", "git@example.com")),
            ("a github local-part", ident("GH", "github@example.com")),
            ("a build local-part", ident("b", "build@example.com")),
            ("a ci local-part", ident("ci", "ci@example.com")),
            ("a runner local-part", runner()),
            (
                "a ci-runner local-part",
                ident("x", "ci-runner-7@example.com"),
            ),
            (
                "a gitlab-runner local-part",
                ident("x", "gitlab-runner@example.com"),
            ),
        ];
        for (label, who) in cases {
            let raw = raw_with(|r| r.commits = vec![make_commit("m1", &who, &oct12(1))]);
            let out = unmapped(&raw, &default_config());
            assert_eq!(
                out.iter().map(|u| u.reason).collect::<Vec<_>>(),
                [UnmappedReason::Machine],
                "lists {label} as a machine identity"
            );
        }
    }

    #[test]
    fn unmapped_identities_does_not_mistake_a_human_for_a_machine_by_a_lookalike_name() {
        let raw = raw_with(|r| {
            r.commits = vec![
                make_commit(
                    "h1",
                    &ident("Ann Brunner", "ann.brunner@example.com"),
                    &oct12(1),
                ),
                make_commit("h2", &ident("Gita", "gita@example.com"), &oct12(2)),
                make_commit("h3", &ident("Cia", "cian@example.com"), &oct12(3)),
                make_commit("h4", &ident("Dev", "dev@corp.example.co.uk"), &oct12(4)),
            ];
        });
        assert!(unmapped(&raw, &default_config()).is_empty());
    }

    #[test]
    fn unmapped_identities_skips_identities_listed_by_a_config_person_by_email_or_login() {
        let raw = raw_with(|r| {
            r.commits = vec![
                make_commit("a1", &build_box(), &oct12(1)),
                make_commit(
                    "b1",
                    &ident_login("Jenkins", "jenkins@example.com", "jenk"),
                    &oct12(2),
                ),
            ];
        });
        let config = with_config(|c| {
            c.people = vec![
                person("ops", "Ops", &["ci@build-box"]),
                person("jenk", "Jenkins", &["gh:jenk"]),
            ];
        });
        assert!(unmapped(&raw, &config).is_empty());
    }

    #[test]
    fn unmapped_identities_skips_identities_that_a_mailmap_group_lists() {
        let raw = raw_with(|r| r.commits = vec![make_commit("a1", &build_box(), &oct12(1))]);
        let mailmap = [MailmapPerson {
            name: "Ops".into(),
            identities: strs(&["CI@Build-Box"]),
        }];
        assert!(unmapped_identities(&raw, &default_config(), &mailmap).is_empty());
    }

    #[test]
    fn unmapped_identities_skips_excluded_machine_identities_and_keeps_agents_flagged() {
        let raw = raw_with(|r| {
            r.commits = vec![
                make_commit("d1", &dependabot(), &oct12(1)),
                make_commit("c1", &claude(), &oct12(2)),
                make_commit("a1", &ana(), &oct12(3)),
                make_commit("b1", &build_box(), &oct12(4)),
            ];
        });
        let config = with_config(|c| c.exclude_identities.push("ci@build-box".into()));
        let out = unmapped(&raw, &config);
        let got: Vec<(&str, bool)> = out.iter().map(|u| (u.key.as_str(), u.agent)).collect();
        assert_eq!(got, [("noreply@anthropic.com", true)]);
    }

    // login handling

    #[test]
    fn unmapped_identities_login_handling_marks_the_login_of_a_noreply_email_as_known_even_when_login_is_an_empty_string(
    ) {
        let noreply = GitIdentity {
            name: "Ana".into(),
            email: "12345+ana@users.noreply.github.com".into(),
            login: Some(String::new()),
        };
        let raw = raw_with(|r| {
            r.commits = vec![make_commit("a1", &noreply, &oct12(1))];
            r.reviews = vec![make_review(
                "1",
                &ident_login("", "", "ana"),
                &oct12(2),
                0,
                "APPROVED",
            )];
        });
        // Ana is an ordinary person, and her reviewer login is not a second one.
        assert!(unmapped(&raw, &default_config()).is_empty());
    }

    #[test]
    fn unmapped_identities_login_handling_lists_a_noreply_agent_once_even_when_a_reviewer_login_names_the_same_agent(
    ) {
        let copilot = GitIdentity {
            name: "Copilot".into(),
            email: "198982749+Copilot@users.noreply.github.com".into(),
            login: Some(String::new()),
        };
        let raw = raw_with(|r| {
            r.commits = vec![make_commit("a1", &copilot, &oct12(1))];
            r.reviews = vec![make_review(
                "1",
                &ident_login("", "", "Copilot"),
                &oct12(2),
                0,
                "APPROVED",
            )];
        });
        let out = unmapped(&raw, &default_config());
        assert_eq!(
            out.iter().map(|u| u.key.as_str()).collect::<Vec<_>>(),
            ["198982749+copilot@users.noreply.github.com"]
        );
    }

    // excludedIdentities

    #[test]
    fn excluded_identities_lists_excluded_observed_identities_with_their_commits_and_matching_patterns(
    ) {
        let raw = raw_with(|r| {
            r.commits = vec![
                make_commit("d1", &dependabot(), &oct12(1)),
                make_commit("d2", &dependabot(), &oct12(2)),
                make_commit("c1", &claude(), &oct12(3)),
                make_commit("a1", &ana(), &oct12(4)),
            ];
        });
        let out = excluded_identities(&raw, &default_config());
        assert_eq!(
            out.iter().map(|e| e.key.as_str()).collect::<Vec<_>>(),
            ["49699333+dependabot[bot]@users.noreply.github.com"]
        );
        assert_eq!(out[0].commits, 2);
        assert!(out[0].patterns.contains(&"*[bot]*".to_string()));
    }

    #[test]
    fn excluded_identities_is_removable_only_when_every_matching_pattern_is_a_literal() {
        let raw = raw_with(|r| {
            r.commits = vec![
                make_commit("g1", &ident("GitHub", "noreply@github.com"), &oct12(1)),
                make_commit("d1", &dependabot(), &oct12(2)),
                make_commit("a1", &ana(), &oct12(3)),
            ];
        });
        let config = with_config(|c| {
            c.exclude_identities = strs(&["noreply@github.com", "*[bot]*", "ana@example.com"]);
        });
        let out = excluded_identities(&raw, &config);
        let by_key = |key: &str| out.iter().find(|e| e.key == key);
        assert!(by_key("noreply@github.com").unwrap().removable);
        assert!(by_key("ana@example.com").unwrap().removable);
        assert_eq!(
            by_key("ana@example.com").unwrap().patterns,
            ["ana@example.com"]
        );
        let bot = out.iter().find(|e| e.key.contains("dependabot")).unwrap();
        assert!(!bot.removable);
    }

    #[test]
    fn excluded_identities_never_lists_agents_agents_win_over_exclude() {
        let raw = raw_with(|r| r.commits = vec![make_commit("c1", &claude(), &oct12(1))]);
        let config = with_config(|c| c.exclude_identities = strs(&["noreply@anthropic.com"]));
        assert!(excluded_identities(&raw, &config).is_empty());
    }

    // personRows

    fn rows_with(
        raw: &RawData,
        config: &ContributorsConfig,
        merge: bool,
        mailmap: Vec<MailmapPerson>,
    ) -> Vec<PersonRow> {
        let resolver = build_resolver(
            config,
            ResolverOptions {
                merge,
                mailmap,
                observed_identities: observed_identities(raw),
            },
        );
        person_rows(raw, config, &resolver)
    }

    fn rows(raw: &RawData, config: &ContributorsConfig) -> Vec<PersonRow> {
        rows_with(raw, config, true, vec![])
    }

    fn keys(row: &PersonRow) -> Vec<&str> {
        row.identities.iter().map(|i| i.key.as_str()).collect()
    }

    #[test]
    fn person_rows_lists_resolved_people_with_their_identities_and_commit_totals() {
        let raw = raw_with(|r| {
            r.commits = vec![
                make_commit("a1", &ana(), &oct12(1)),
                make_commit("a2", &ident("Ana", "ana@laptop.local"), &oct12(2)),
                make_commit("b1", &bob(), &oct12(3)),
            ];
        });
        let config = with_config(|c| {
            c.people = vec![person(
                "ana",
                "Ana Sol",
                &["ana@example.com", "ana@laptop.local"],
            )];
        });
        let out = rows(&raw, &config);
        assert_eq!(
            out.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(),
            ["Ana Sol", "Bob"]
        );
        assert_eq!(
            (out[0].id.as_str(), out[0].commits, out[0].in_config),
            ("ana", 2, true)
        );
        assert_eq!(keys(&out[0]), ["ana@example.com", "ana@laptop.local"]);
        assert_eq!(
            (out[1].id.as_str(), out[1].in_config),
            ("bob@example.com", false)
        );
    }

    #[test]
    fn person_rows_keeps_an_empty_config_person_so_a_new_person_is_visible() {
        let config = with_config(|c| c.people = vec![person("person-new", "New person", &[])]);
        let raw = raw_with(|r| r.commits = vec![make_commit("a", &ana(), &oct12(1))]);
        let out = rows(&raw, &config);
        let new = out.iter().find(|p| p.id == "person-new").unwrap();
        assert!(new.identities.is_empty());
        assert_eq!(new.commits, 0);
        assert!(new.in_config);
    }

    #[test]
    fn person_rows_hides_auto_people_that_are_only_agents_or_excluded_identities() {
        let raw = raw_with(|r| {
            r.commits = vec![
                make_commit("d1", &dependabot(), &oct12(1)),
                make_commit("c1", &claude(), &oct12(2)),
                make_commit("a1", &ana(), &oct12(3)),
            ];
        });
        let out = rows(&raw, &default_config());
        assert_eq!(
            out.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(),
            ["Ana"]
        );
    }

    #[test]
    fn person_rows_flags_agent_identities_inside_a_person() {
        let raw = raw_with(|r| {
            r.commits = vec![
                make_commit("a1", &ana(), &oct12(1)),
                make_commit("c1", &claude(), &oct12(2)),
            ];
        });
        let config = with_config(|c| {
            c.people = vec![person(
                "ana",
                "Ana",
                &["ana@example.com", "noreply@anthropic.com"],
            )];
        });
        let out = rows(&raw, &config);
        let got: Vec<(&str, bool)> = out[0]
            .identities
            .iter()
            .map(|i| (i.key.as_str(), i.agent))
            .collect();
        assert_eq!(
            got,
            [("ana@example.com", false), ("noreply@anthropic.com", true)]
        );
    }

    #[test]
    fn person_rows_with_merging_off_every_raw_identity_is_its_own_person() {
        let raw = raw_with(|r| {
            r.commits = vec![
                make_commit("a1", &ana(), &oct12(1)),
                make_commit("a2", &ident("Ana", "ana@laptop.local"), &oct12(2)),
            ];
        });
        let config = with_config(|c| {
            c.people = vec![person(
                "ana",
                "Ana Sol",
                &["ana@example.com", "ana@laptop.local"],
            )];
        });
        let out = rows_with(&raw, &config, false, vec![]);
        let mut got: Vec<&str> = out.iter().map(|p| p.id.as_str()).collect();
        got.sort();
        assert_eq!(got, ["ana@example.com", "ana@laptop.local"]);
        assert!(out.iter().all(|p| !p.in_config));
    }

    #[test]
    fn person_rows_lists_identities_a_mailmap_group_adds_to_a_config_person() {
        let raw = raw_with(|r| {
            r.commits = vec![
                make_commit("a1", &ana(), &oct12(1)),
                make_commit("a2", &ident("Ana", "ana@laptop.local"), &oct12(2)),
            ];
        });
        let config =
            with_config(|c| c.people = vec![person("ana", "Ana Sol", &["ana@example.com"])]);
        let out = rows_with(
            &raw,
            &config,
            true,
            vec![MailmapPerson {
                name: "Ana".into(),
                identities: strs(&["ana@example.com", "ana@laptop.local"]),
            }],
        );
        assert_eq!(out.len(), 1);
        assert_eq!(keys(&out[0]), ["ana@example.com", "ana@laptop.local"]);
    }

    #[test]
    fn person_rows_does_not_list_a_login_only_reviewer_that_is_already_linked_to_a_listed_identity()
    {
        let raw = raw_with(|r| {
            r.commits = vec![make_commit("a1", &ana(), &oct12(1))];
            r.reviews = vec![make_review(
                "1",
                &ident_login("", "", "ana"),
                &oct12(2),
                0,
                "APPROVED",
            )];
        });
        let config = with_config(|c| c.people = vec![person("ana", "Ana", &["ana@example.com"])]);
        assert_eq!(keys(&rows(&raw, &config)[0]), ["ana@example.com"]);
    }

    #[test]
    fn person_rows_orders_by_commits_then_name() {
        let raw = raw_with(|r| {
            r.commits = vec![
                make_commit("c1", &cat(), &oct12(1)),
                make_commit("b1", &bob(), &oct12(2)),
                make_commit("b2", &bob(), &oct12(3)),
                make_commit("a1", &ana(), &oct12(4)),
            ];
        });
        let out = rows(&raw, &default_config());
        assert_eq!(
            out.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(),
            ["Bob", "Ana", "Cat"]
        );
    }

    #[test]
    fn person_rows_uses_the_shared_resolver_of_the_model() {
        let raw = raw_with(|r| r.commits = vec![make_commit("a1", &ana(), &oct12(1))]);
        let resolver = make_resolver(&raw);
        assert_eq!(person_rows(&raw, &default_config(), &resolver).len(), 1);
    }

    // config helpers for the Identities tab

    fn row(id: &str, name: &str, identities: &[&str], commits: u64, in_config: bool) -> PersonRow {
        PersonRow {
            id: id.into(),
            name: name.into(),
            identities: identities
                .iter()
                .map(|k| IdentityRow {
                    key: k.to_string(),
                    agent: false,
                })
                .collect(),
            commits,
            in_config,
        }
    }

    #[test]
    fn ensure_config_person_materializes_a_person_that_only_exists_through_observation() {
        let r = row("ana@example.com", "Ana", &["ana@example.com"], 1, false);
        let next = ensure_config_person(&default_config(), &r);
        assert_eq!(
            next.people,
            [person("ana@example.com", "Ana", &["ana@example.com"])]
        );
    }

    #[test]
    fn ensure_config_person_returns_the_same_config_for_a_config_person() {
        let config = with_config(|c| c.people = vec![person("ana", "Ana", &["ana@example.com"])]);
        let r = row("ana", "Ana", &["ana@example.com"], 1, true);
        assert_eq!(ensure_config_person(&config, &r), config);
    }

    #[test]
    fn ensure_config_person_lists_identities_a_config_person_only_has_through_observation() {
        let config = with_config(|c| c.people = vec![person("ana", "Ana", &["ana@example.com"])]);
        let r = row(
            "ana",
            "Ana",
            &["ana@example.com", "ana@laptop.local"],
            2,
            true,
        );
        assert_eq!(
            ensure_config_person(&config, &r).people[0].identities,
            ["ana@example.com", "ana@laptop.local"]
        );
    }

    #[test]
    fn remove_person_drops_a_config_person_and_nothing_else() {
        let config = with_config(|c| {
            c.people = vec![
                person("ana", "Ana", &["ana@example.com"]),
                person("bob", "Bob", &["bob@example.com"]),
            ];
        });
        assert_eq!(
            remove_person(&config, "ana")
                .people
                .iter()
                .map(|p| p.id.as_str())
                .collect::<Vec<_>>(),
            ["bob"]
        );
        assert_eq!(remove_person(&config, "zed"), config);
    }
}
