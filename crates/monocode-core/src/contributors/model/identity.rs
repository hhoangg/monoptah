//! Who is who: identity keys, agent and exclude verdicts, `.mailmap`, the
//! resolver that turns identities into people, merge suggestions and the
//! config editing helpers.

use super::patterns::matches_any_identity;
use super::types::{ContributorsConfig, IdentityKey, Person};
use super::{js_cmp, js_len, js_space, js_trim, OrderedMap};
use crate::contributors::GitIdentity;
use regex::Regex;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use unicode_normalization::UnicodeNormalization;

fn noreply_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)^(?:[0-9]+\+)?([^@+\s]+)@users\.noreply\.github\.com$").expect("regex")
    })
}

/// Lowercased email; `gh:<login>` when only a login is known; else the lowercased name.
pub fn identity_key(id: &GitIdentity) -> IdentityKey {
    let email = js_trim(&id.email).to_lowercase();
    if !email.is_empty() {
        return email;
    }
    let login = id
        .login
        .as_deref()
        .map_or_else(String::new, |l| js_trim(l).to_lowercase());
    if !login.is_empty() {
        return format!("gh:{login}");
    }
    js_trim(&id.name).to_lowercase()
}

/// `123+login@users.noreply.github.com` or `login@users.noreply.github.com`.
pub fn github_login_from_email(email: &str) -> Option<String> {
    noreply_regex()
        .captures(js_trim(email))
        .map(|m| m[1].to_lowercase())
}

#[derive(Clone, Copy)]
struct Verdicts {
    agent: bool,
    excluded: bool,
}

/// Whether an identity is an agent or excluded, remembered per pair of
/// pattern lists. A model build asks this for every author, committer and
/// co-author of every commit, PR and review, but only a few dozen distinct
/// identities exist, so each is matched against the lists once. A memo is
/// found by comparing the lists, so an edited list never reads a stale answer.
struct Memo {
    agent: Vec<String>,
    exclude: Vec<String>,
    verdicts: HashMap<String, Verdicts>,
}

const MEMO_SLOTS: usize = 4;
const MEMO_VERDICTS: usize = 20_000;

thread_local! {
    static MEMOS: RefCell<Vec<Memo>> = const { RefCell::new(Vec::new()) };
}

fn verdicts(config: &ContributorsConfig, id: &GitIdentity) -> Verdicts {
    MEMOS.with(|memos| {
        let mut memos = memos.borrow_mut();
        let at = match memos.iter().position(|m| {
            m.agent == config.agent_identities && m.exclude == config.exclude_identities
        }) {
            Some(at) => at,
            None => {
                if memos.len() >= MEMO_SLOTS {
                    memos.remove(0);
                }
                memos.push(Memo {
                    agent: config.agent_identities.clone(),
                    exclude: config.exclude_identities.clone(),
                    verdicts: HashMap::new(),
                });
                memos.len() - 1
            }
        };
        let memo = &mut memos[at];
        let key = format!(
            "{}\u{0}{}\u{0}{}",
            id.email,
            id.name,
            id.login.as_deref().unwrap_or("")
        );
        if let Some(found) = memo.verdicts.get(&key) {
            return *found;
        }
        // An identity matching both lists counts as an agent, not as excluded.
        let agent = matches_any_identity(&config.agent_identities, id);
        let found = Verdicts {
            agent,
            excluded: !agent && matches_any_identity(&config.exclude_identities, id),
        };
        if memo.verdicts.len() >= MEMO_VERDICTS {
            memo.verdicts.clear();
        }
        memo.verdicts.insert(key, found);
        found
    })
}

pub fn is_agent(config: &ContributorsConfig, id: &GitIdentity) -> bool {
    verdicts(config, id).agent
}

/// An identity matching both lists counts as an agent, not as excluded.
pub fn is_excluded(config: &ContributorsConfig, id: &GitIdentity) -> bool {
    verdicts(config, id).excluded
}

pub fn is_human(config: &ContributorsConfig, id: &GitIdentity) -> bool {
    let v = verdicts(config, id);
    !v.agent && !v.excluded
}

#[derive(Clone, Debug, PartialEq)]
pub struct MailmapPerson {
    /// Empty when no line names the person.
    pub name: String,
    pub identities: Vec<String>,
}

fn mailmap_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"^([^<>]*)<([^<>]*)>(?:\s*([^<>]*)<([^<>]*)>)?\s*(?:#[^\r\n\x{2028}\x{2029}]*)?$",
        )
        .expect("regex")
    })
}

/// Parses the four git `.mailmap` forms. Groups by proper email; identities are
/// lowercased emails, the proper one first. `name` is "" when no line names it.
pub fn parse_mailmap(text: &str) -> Vec<MailmapPerson> {
    let mut groups: OrderedMap<MailmapPerson> = OrderedMap::new();
    for raw in text.split('\n') {
        let line = js_trim(raw);
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some(m) = mailmap_regex().captures(line) else {
            continue;
        };
        let name = js_trim(&m[1]);
        let proper = js_trim(&m[2]).to_lowercase();
        let commit = m.get(4).map(|c| js_trim(c.as_str()).to_lowercase());
        if proper.is_empty() {
            continue;
        }
        let group = groups.entry(&proper, || MailmapPerson {
            name: String::new(),
            identities: vec![proper.clone()],
        });
        if group.name.is_empty() && !name.is_empty() {
            group.name = name.to_string();
        }
        if let Some(commit) = commit.filter(|c| !c.is_empty()) {
            if !group.identities.contains(&commit) {
                group.identities.push(commit);
            }
        }
    }
    groups.into_values().collect()
}

#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedPerson {
    pub id: String,
    pub name: String,
    pub identities: Vec<String>,
    pub observed: Vec<GitIdentity>,
}

#[derive(Clone, Debug, Default)]
pub struct ResolverOptions {
    pub mailmap: Vec<MailmapPerson>,
    pub observed_identities: Vec<GitIdentity>,
    pub merge: bool,
}

struct Group {
    name: String,
    identities: Vec<String>,
    canonical: String,
}

#[derive(Default)]
struct State {
    observed: Vec<GitIdentity>,
    seen: HashSet<String>,
    resolved: HashMap<String, String>,
    cache: Option<Arc<Vec<ResolvedPerson>>>,
    index: HashMap<String, usize>,
}

/// Turns identities into people. `person_of` takes `&self`: it registers
/// identities it has not seen (listing them in `people`) behind a lock, but a
/// first sighting never changes any answer.
pub struct Resolver {
    merge: bool,
    config_people: Vec<Person>,
    config_by_id: HashMap<String, Person>,
    config_index: HashMap<String, String>,
    groups: Vec<Group>,
    mailmap_index: HashMap<String, usize>,
    mailmap_by_canonical: HashMap<String, usize>,
    /// gh:<login> -> identity key of a commit identity seen with that login
    gh_links: HashMap<String, String>,
    state: Mutex<State>,
}

fn best_name(identities: &[GitIdentity]) -> String {
    let mut counts: OrderedMap<usize> = OrderedMap::new();
    for id in identities {
        let name = js_trim(&id.name);
        if !name.is_empty() {
            *counts.entry(name, || 0) += 1;
        }
    }
    let mut best = "";
    let mut best_count = 0;
    for (name, &count) in counts.iter() {
        if count > best_count || (count == best_count && js_len(name) > js_len(best)) {
            best = name;
            best_count = count;
        }
    }
    best.to_string()
}

/// Aliases an identity string can be looked up by: itself plus the login of a noreply email.
fn listed_aliases(identity: &str) -> Vec<String> {
    let lower = js_trim(identity).to_lowercase();
    match github_login_from_email(&lower) {
        Some(login) => vec![lower, format!("gh:{login}")],
        None => vec![lower],
    }
}

fn unique_strings(values: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut seen = HashSet::new();
    values
        .into_iter()
        .filter(|v| seen.insert(v.clone()))
        .collect()
}

fn seen_key(id: &GitIdentity) -> String {
    format!(
        "{}\u{0}{}",
        identity_key(id),
        id.login
            .as_deref()
            .map_or_else(String::new, |l| js_trim(l).to_lowercase())
    )
}

/// Layers, lowest to highest priority: observed identity < mailmap < config people.
/// Observed identities passed up front also teach the resolver which `gh:<login>`
/// belongs to which commit identity, so login-only reviewers can be attributed.
/// The link table is fixed when the resolver is built; identities first seen
/// through `person_of` are listed in `people` but never change any answer, so the
/// result does not depend on call order.
pub fn build_resolver(config: &ContributorsConfig, opts: ResolverOptions) -> Resolver {
    let merge = opts.merge;
    let config_people: Vec<Person> = if merge { config.people.clone() } else { vec![] };
    let mut config_by_id = HashMap::new();
    for person in &config_people {
        config_by_id.insert(person.id.clone(), person.clone());
    }
    let mut config_index: HashMap<String, String> = HashMap::new();
    for person in &config_people {
        for identity in &person.identities {
            for alias in listed_aliases(identity) {
                config_index
                    .entry(alias)
                    .or_insert_with(|| person.id.clone());
            }
        }
    }

    let mut groups: Vec<Group> = Vec::new();
    let mut mailmap_index: HashMap<String, usize> = HashMap::new();
    let mut mailmap_by_canonical: HashMap<String, usize> = HashMap::new();
    if merge {
        for entry in &opts.mailmap {
            let identities = unique_strings(
                entry
                    .identities
                    .iter()
                    .map(|s| js_trim(s).to_lowercase())
                    .filter(|s| !s.is_empty()),
            );
            let Some(canonical) = identities.first().cloned() else {
                continue;
            };
            let at = groups.len();
            for identity in &identities {
                for alias in listed_aliases(identity) {
                    mailmap_index.entry(alias).or_insert(at);
                }
            }
            mailmap_by_canonical.insert(canonical.clone(), at);
            groups.push(Group {
                name: js_trim(&entry.name).to_string(),
                identities,
                canonical,
            });
        }
    }

    let mut gh_links: HashMap<String, String> = HashMap::new();
    let mut state = State::default();
    for id in &opts.observed_identities {
        state.observed.push(id.clone());
        state.seen.insert(seen_key(id));
        if merge {
            link_logins(&mut gh_links, id);
        }
    }

    Resolver {
        merge,
        config_people,
        config_by_id,
        config_index,
        groups,
        mailmap_index,
        mailmap_by_canonical,
        gh_links,
        state: Mutex::new(state),
    }
}

fn link_logins(gh_links: &mut HashMap<String, String>, id: &GitIdentity) {
    let key = identity_key(id);
    if key.is_empty() || key.starts_with("gh:") {
        return;
    }
    let mut logins: Vec<String> = Vec::new();
    if let Some(own) = id
        .login
        .as_deref()
        .map(|l| js_trim(l).to_lowercase())
        .filter(|l| !l.is_empty())
    {
        logins.push(own);
    }
    if let Some(from_email) = github_login_from_email(&id.email) {
        if !logins.contains(&from_email) {
            logins.push(from_email);
        }
    }
    for login in logins {
        let alias = format!("gh:{login}");
        let current_is_noreply = gh_links
            .get(&alias)
            .is_some_and(|c| github_login_from_email(c).is_some());
        let replace = match gh_links.get(&alias) {
            None => true,
            Some(_) => current_is_noreply && github_login_from_email(&key).is_none(),
        };
        if replace {
            gh_links.insert(alias, key.clone());
        }
    }
}

impl Resolver {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn lookup(&self, candidates: &[String]) -> Option<String> {
        for c in candidates {
            if let Some(person_id) = self.config_index.get(c) {
                return Some(person_id.clone());
            }
        }
        for c in candidates {
            let Some(&at) = self.mailmap_index.get(c) else {
                continue;
            };
            let group = &self.groups[at];
            for identity in &group.identities {
                for alias in listed_aliases(identity) {
                    if let Some(person_id) = self.config_index.get(&alias) {
                        return Some(person_id.clone());
                    }
                }
            }
            return Some(group.canonical.clone());
        }
        None
    }

    fn resolve(&self, state: &mut State, id: &GitIdentity) -> String {
        let cache_key = seen_key(id);
        if let Some(hit) = state.resolved.get(&cache_key) {
            return hit.clone();
        }
        let key = identity_key(id);
        let mut out = key.clone();
        if self.merge {
            let mut candidates = vec![key.clone()];
            if let Some(login) = id
                .login
                .as_deref()
                .map(|l| js_trim(l).to_lowercase())
                .filter(|l| !l.is_empty())
            {
                candidates.push(format!("gh:{login}"));
            }
            if let Some(from_email) = github_login_from_email(&id.email) {
                candidates.push(format!("gh:{from_email}"));
            }
            let linked = if key.starts_with("gh:") {
                self.gh_links.get(&key).cloned()
            } else {
                None
            };
            if let Some(linked) = &linked {
                candidates.push(linked.clone());
            }
            let candidates = unique_strings(candidates);
            out = self.lookup(&candidates).or(linked).unwrap_or(key);
        }
        state.resolved.insert(cache_key, out.clone());
        out
    }

    fn describe(&self, person_id: &str, ids: Vec<GitIdentity>) -> ResolvedPerson {
        if let Some(configured) = self.config_by_id.get(person_id) {
            return ResolvedPerson {
                id: person_id.into(),
                name: if configured.name.is_empty() {
                    person_id.into()
                } else {
                    configured.name.clone()
                },
                identities: configured.identities.clone(),
                observed: ids,
            };
        }
        if let Some(&at) = self.mailmap_by_canonical.get(person_id) {
            let group = &self.groups[at];
            let name = if !group.name.is_empty() {
                group.name.clone()
            } else {
                Some(best_name(&ids))
                    .filter(|n| !n.is_empty())
                    .unwrap_or_else(|| person_id.into())
            };
            return ResolvedPerson {
                id: person_id.into(),
                name,
                identities: group.identities.clone(),
                observed: ids,
            };
        }
        let login = ids
            .iter()
            .find_map(|i| i.login.as_deref().map(js_trim).filter(|l| !l.is_empty()))
            .map(String::from);
        let name = Some(best_name(&ids))
            .filter(|n| !n.is_empty())
            .or(login)
            .unwrap_or_else(|| person_id.into());
        ResolvedPerson {
            id: person_id.into(),
            name,
            identities: vec![person_id.into()],
            observed: ids,
        }
    }

    fn build(&self, state: &mut State) -> Arc<Vec<ResolvedPerson>> {
        let observed = state.observed.clone();
        let mut by_person: OrderedMap<Vec<GitIdentity>> = OrderedMap::new();
        for id in observed {
            let person_id = self.resolve(state, &id);
            by_person.entry(&person_id, Vec::new).push(id);
        }
        let pairs: Vec<(String, Vec<GitIdentity>)> = by_person.into_pairs().collect();
        let present: HashSet<String> = pairs.iter().map(|(id, _)| id.clone()).collect();
        let mut out = Vec::new();
        for (person_id, ids) in pairs {
            out.push(self.describe(&person_id, ids));
        }
        for person in &self.config_people {
            if !present.contains(&person.id) {
                out.push(self.describe(&person.id, vec![]));
            }
        }
        let out = Arc::new(out);
        state.index.clear();
        for (i, p) in out.iter().enumerate() {
            state.index.entry(p.id.clone()).or_insert(i);
        }
        state.cache = Some(out.clone());
        out
    }

    /// Person id of an identity.
    pub fn person_of(&self, id: &GitIdentity) -> String {
        let mut state = self.lock();
        if !state.seen.contains(&seen_key(id)) {
            state.observed.push(id.clone());
            state.seen.insert(seen_key(id));
            state.cache = None;
        }
        self.resolve(&mut state, id)
    }

    pub fn name_of(&self, person_id: &str) -> String {
        let mut state = self.lock();
        let people = match &state.cache {
            Some(cache) => cache.clone(),
            None => self.build(&mut state),
        };
        state
            .index
            .get(person_id)
            .map_or_else(|| person_id.to_string(), |&i| people[i].name.clone())
    }

    /// Every person seen so far plus all config people.
    pub fn people(&self) -> Arc<Vec<ResolvedPerson>> {
        let mut state = self.lock();
        match &state.cache {
            Some(cache) => cache.clone(),
            None => self.build(&mut state),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SuggestPerson {
    pub id: String,
    pub name: String,
    pub identities: Vec<String>,
    pub observed: Vec<GitIdentity>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MergeReason {
    SameName,
    SimilarEmail,
    GithubNoreply,
}

impl MergeReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SameName => "same-name",
            Self::SimilarEmail => "similar-email",
            Self::GithubNoreply => "github-noreply",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct MergeSuggestion {
    /// The sorted person ids joined with `|`; no reason in it, so a dismissal
    /// survives the reason changing.
    pub id: String,
    pub person_ids: Vec<String>,
    pub reason: MergeReason,
    pub label: String,
}

const GENERIC_LOCALS: [&str; 21] = [
    "dev", "info", "admin", "noreply", "no-reply", "root", "me", "mail", "hello", "contact",
    "support", "team", "git", "github", "test", "user", "bot", "build", "office", "ci", "jenkins",
];

const PLACEHOLDER_NAMES: [&str; 7] = [
    "your name",
    "root",
    "unknown",
    "admin",
    "user",
    "ubuntu",
    "github",
];

const REASON_SUFFIXES: [&str; 3] = [":same-name", ":similar-email", ":github-noreply"];

fn reason_suffix(entry: &str) -> &'static str {
    REASON_SUFFIXES
        .iter()
        .copied()
        .find(|s| entry.ends_with(s))
        .unwrap_or("")
}

fn normalize_name(name: &str) -> String {
    let lower = name.to_lowercase().replace('đ', "d");
    let plain: String = lower
        .nfd()
        .filter(|c| !('\u{300}'..='\u{36f}').contains(c))
        .collect();
    plain
        .split(js_space)
        .filter(|w| !w.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Local part without dots, `+tag` suffix and the `12345+` GitHub id prefix.
fn normalize_local(raw: &str) -> String {
    let lower = raw.to_lowercase();
    let digits = lower.bytes().take_while(u8::is_ascii_digit).count();
    let local = if digits > 0 && lower[digits..].starts_with('+') && lower.len() > digits + 1 {
        lower[digits + 1..].to_string()
    } else {
        match lower.find('+') {
            Some(plus) if plus > 0 => lower[..plus].to_string(),
            _ => lower,
        }
    };
    local.replace('.', "")
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

struct LocalEntry {
    person_id: String,
    domain: String,
    noreply: bool,
}

struct NameBucket {
    display: String,
    ids: Vec<String>,
}

/// Suggestions against the default config.
pub fn suggest_merges(people: &[SuggestPerson], dismissed: &[String]) -> Vec<MergeSuggestion> {
    suggest_merges_with(people, dismissed, &super::config::DEFAULT_CONFIG)
}

pub fn suggest_merges_with(
    people: &[SuggestPerson],
    dismissed: &[String],
    config: &ContributorsConfig,
) -> Vec<MergeSuggestion> {
    let mut names: OrderedMap<NameBucket> = OrderedMap::new();
    let mut locals: OrderedMap<Vec<LocalEntry>> = OrderedMap::new();

    for person in people {
        let observed = &person.observed;
        let mut probes = vec![GitIdentity {
            name: person.name.clone(),
            email: String::new(),
            login: None,
        }];
        probes.extend(person.identities.iter().map(|i| pseudo_identity(i)));
        probes.extend(observed.iter().cloned());
        if probes
            .iter()
            .any(|p| is_agent(config, p) || is_excluded(config, p))
        {
            continue;
        }

        for raw in
            std::iter::once(person.name.as_str()).chain(observed.iter().map(|o| o.name.as_str()))
        {
            let norm = normalize_name(raw);
            // One-word names (Minh, Sam) are too common to link people on.
            if js_len(&norm) < 3
                || norm.contains('@')
                || !norm.contains(' ')
                || PLACEHOLDER_NAMES.contains(&norm.as_str())
            {
                continue;
            }
            let bucket = names.entry(&norm, || NameBucket {
                display: js_trim(raw).to_string(),
                ids: Vec::new(),
            });
            if !bucket.ids.contains(&person.id) {
                bucket.ids.push(person.id.clone());
            }
        }

        let mut add_local = |raw: &str, domain: &str, noreply: bool| {
            let local = normalize_local(raw);
            if js_len(&local) < 3 || GENERIC_LOCALS.contains(&local.as_str()) {
                return;
            }
            locals.entry(&local, Vec::new).push(LocalEntry {
                person_id: person.id.clone(),
                domain: domain.into(),
                noreply,
            });
        };
        let emails = unique_strings(
            person
                .identities
                .iter()
                .filter(|i| i.contains('@'))
                .cloned()
                .chain(
                    observed
                        .iter()
                        .map(|o| o.email.clone())
                        .filter(|e| !e.is_empty()),
                )
                .map(|e| js_trim(&e).to_lowercase()),
        );
        for email in emails {
            if let Some(login) = github_login_from_email(&email) {
                add_local(&login, "users.noreply.github.com", true);
            } else if let Some(at) = email.rfind('@').filter(|&at| at > 0) {
                add_local(&email[..at], &email[at + 1..], false);
            }
        }
        let logins = unique_strings(
            person
                .identities
                .iter()
                .filter_map(|i| i.strip_prefix("gh:").map(String::from))
                .chain(
                    observed
                        .iter()
                        .filter_map(|o| o.login.clone())
                        .filter(|l| !l.is_empty()),
                ),
        );
        for login in logins {
            add_local(&login, "", true);
        }
    }

    let mut out: Vec<MergeSuggestion> = Vec::new();
    let mut seen_sets: HashSet<String> = HashSet::new();
    let mut push = |ids: Vec<String>, reason: MergeReason, label: String| {
        let mut person_ids = unique_strings(ids);
        person_ids.sort_by(|a, b| js_cmp(a, b));
        if person_ids.len() < 2 {
            return;
        }
        let set_key = person_ids.join("|");
        if !seen_sets.insert(set_key.clone()) {
            return;
        }
        out.push(MergeSuggestion {
            id: set_key,
            person_ids,
            reason,
            label,
        });
    };

    for (_, bucket) in names.iter() {
        push(
            bucket.ids.clone(),
            MergeReason::SameName,
            format!("Same name \"{}\"", bucket.display),
        );
    }
    for (local, entries) in locals.iter() {
        let included: Vec<&LocalEntry> = entries
            .iter()
            .filter(|e| {
                entries
                    .iter()
                    .any(|o| o.person_id != e.person_id && o.domain != e.domain)
            })
            .collect();
        let noreply = included.iter().any(|e| e.noreply);
        push(
            included.iter().map(|e| e.person_id.clone()).collect(),
            if noreply {
                MergeReason::GithubNoreply
            } else {
                MergeReason::SimilarEmail
            },
            if noreply {
                format!("GitHub login \"{local}\" matches another email")
            } else {
                format!("Similar email \"{local}\"")
            },
        );
    }

    let skip: HashSet<&str> = dismissed
        .iter()
        .map(|d| &d[..d.len() - reason_suffix(d).len()])
        .collect();
    out.into_iter()
        .filter(|s| !skip.contains(s.id.as_str()))
        .collect()
}

// ---- Config editing helpers: each returns a new config ----

fn lower_key(key: &str) -> String {
    js_trim(key).to_lowercase()
}

fn slugify(name: &str) -> String {
    let mut slug = String::new();
    let mut dash = false;
    for ch in normalize_name(name).chars() {
        if ch.is_ascii_lowercase() || ch.is_ascii_digit() {
            if dash {
                slug.push('-');
                dash = false;
            }
            slug.push(ch);
        } else {
            dash = true;
        }
    }
    slug
}

fn person_id_for_people(people: &[Person], name: &str) -> String {
    let slug = slugify(name);
    let base = format!("person-{}", if slug.is_empty() { "person" } else { &slug });
    let taken: HashSet<&str> = people.iter().map(|p| p.id.as_str()).collect();
    let mut candidate = base.clone();
    let mut n = 2;
    while taken.contains(candidate.as_str()) {
        candidate = format!("{base}-{n}");
        n += 1;
    }
    candidate
}

/// The id `create_person` will give a person with this name.
pub fn person_id_for(config: &ContributorsConfig, name: &str) -> String {
    person_id_for_people(&config.people, name)
}

pub fn create_person(config: &ContributorsConfig, name: &str) -> ContributorsConfig {
    let trimmed = js_trim(name);
    if trimmed.is_empty() {
        return config.clone();
    }
    let mut next = config.clone();
    next.people.push(Person {
        id: person_id_for(config, trimmed),
        name: trimmed.into(),
        identities: vec![],
    });
    next
}

/// Renaming a person that only exists through observation creates a config person for it.
pub fn rename_person(
    config: &ContributorsConfig,
    person_id: &str,
    name: &str,
) -> ContributorsConfig {
    let trimmed = js_trim(name);
    if trimmed.is_empty() {
        return config.clone();
    }
    let mut next = config.clone();
    if next.people.iter().any(|p| p.id == person_id) {
        for p in next.people.iter_mut().filter(|p| p.id == person_id) {
            p.name = trimmed.into();
        }
    } else {
        next.people.push(Person {
            id: person_id.into(),
            name: trimmed.into(),
            identities: vec![person_id.into()],
        });
    }
    next
}

/// Rewrites dismissed suggestion ids so they follow a person that got a new id.
fn rename_in_dismissed(dismissed: &[String], old_id: &str, new_id: &str) -> Vec<String> {
    unique_strings(dismissed.iter().map(|entry| {
        let suffix = reason_suffix(entry);
        let ids: Vec<&str> = entry[..entry.len() - suffix.len()].split('|').collect();
        if !ids.contains(&old_id) {
            return entry.clone();
        }
        let mut renamed = unique_strings(ids.iter().map(|i| {
            if *i == old_id {
                new_id.to_string()
            } else {
                i.to_string()
            }
        }));
        renamed.sort_by(|a, b| js_cmp(a, b));
        format!("{}{suffix}", renamed.join("|"))
    }))
}

/// Moves an identity key to another person (`None`: to nobody). Missing
/// targets are created, named `created_name` or after the key.
pub fn move_person_identity(
    config: &ContributorsConfig,
    identity_key: &str,
    to_person_id: Option<&str>,
    created_name: Option<&str>,
) -> ContributorsConfig {
    let key = lower_key(identity_key);
    let mut emptied: HashSet<String> = HashSet::new();
    let mut people: Vec<Person> = config
        .people
        .iter()
        .map(|p| {
            let identities: Vec<String> = p
                .identities
                .iter()
                .filter(|i| **i != key)
                .cloned()
                .collect();
            if identities.is_empty() && !p.identities.is_empty() {
                emptied.insert(p.id.clone());
            }
            Person {
                identities,
                ..p.clone()
            }
        })
        .collect();
    if let Some(to) = to_person_id {
        if people.iter().any(|p| p.id == to) {
            for p in people.iter_mut().filter(|p| p.id == to) {
                p.identities.push(key.clone());
            }
        } else {
            let name = created_name.map(js_trim).filter(|n| !n.is_empty());
            people.push(Person {
                id: to.into(),
                name: name.unwrap_or(&key).into(),
                identities: vec![key.clone()],
            });
        }
    }
    // Only people this move emptied go away; others may be empty on purpose.
    people.retain(|p| !emptied.contains(&p.id) || !p.identities.is_empty());
    // A person whose id is the moved key would otherwise keep claiming that
    // identity (an unmapped identity resolves to its own key), so give it a new id.
    let mut dismissed = config.dismissed_merges.clone();
    if to_person_id != Some(key.as_str()) {
        let mut taken: Vec<Person> = people.iter().filter(|p| p.id != key).cloned().collect();
        for p in people.iter_mut().filter(|p| p.id == key) {
            let id = person_id_for_people(&taken, &p.name);
            taken.push(Person {
                id: id.clone(),
                ..p.clone()
            });
            dismissed = rename_in_dismissed(&dismissed, &key, &id);
            p.id = id;
        }
    }
    ContributorsConfig {
        people,
        dismissed_merges: dismissed,
        ..config.clone()
    }
}

/// Merges `ids` into the first one. Ids that are config people have their
/// identities moved; any other id is an observed identity key and is added as
/// an identity. The first id becomes a config person when it is not one yet.
pub fn merge_people(
    config: &ContributorsConfig,
    ids: &[String],
    into_name: Option<&str>,
) -> ContributorsConfig {
    let distinct = unique_strings(ids.iter().cloned());
    if distinct.len() < 2 {
        return config.clone();
    }
    let (target_id, rest) = (&distinct[0], &distinct[1..]);
    let mut by_id: HashMap<&str, &Person> = HashMap::new();
    for p in &config.people {
        by_id.insert(p.id.as_str(), p);
    }
    let existing = by_id.get(target_id.as_str()).copied();

    let mut identities: Vec<String> = match existing {
        Some(p) => p.identities.clone(),
        None => vec![lower_key(target_id)],
    };
    for id in rest {
        match by_id.get(id.as_str()) {
            Some(other) => identities.extend(other.identities.iter().cloned()),
            None => identities.push(lower_key(id)),
        }
    }
    let name = into_name
        .map(js_trim)
        .filter(|n| !n.is_empty())
        .or(existing.map(|p| p.name.as_str()).filter(|n| !n.is_empty()))
        .unwrap_or(target_id);
    let merged = Person {
        id: target_id.clone(),
        name: name.into(),
        identities: unique_strings(identities),
    };

    let removed: HashSet<&String> = rest.iter().collect();
    let mut people: Vec<Person> = config
        .people
        .iter()
        .filter(|p| !removed.contains(&p.id))
        .map(|p| {
            if p.id == *target_id {
                merged.clone()
            } else {
                p.clone()
            }
        })
        .collect();
    if existing.is_none() {
        people.push(merged);
    }
    ContributorsConfig {
        people,
        ..config.clone()
    }
}

/// Adds an exact (lowercased) pattern. Wildcard characters in the key act as wildcards.
pub fn exclude_identity(config: &ContributorsConfig, identity_key: &str) -> ContributorsConfig {
    let key = lower_key(identity_key);
    if key.is_empty()
        || config
            .exclude_identities
            .iter()
            .any(|p| lower_key(p) == key)
    {
        return config.clone();
    }
    let mut next = config.clone();
    next.exclude_identities.push(key);
    next
}

/// Removes an exact exclude pattern; wildcard patterns that cover the identity are not touched.
pub fn include_identity(config: &ContributorsConfig, pattern: &str) -> ContributorsConfig {
    let key = lower_key(pattern);
    let next: Vec<String> = config
        .exclude_identities
        .iter()
        .filter(|p| lower_key(p) != key)
        .cloned()
        .collect();
    if next.len() == config.exclude_identities.len() {
        return config.clone();
    }
    ContributorsConfig {
        exclude_identities: next,
        ..config.clone()
    }
}

pub fn dismiss_merge(config: &ContributorsConfig, suggestion_id: &str) -> ContributorsConfig {
    if config.dismissed_merges.iter().any(|d| d == suggestion_id) {
        return config.clone();
    }
    let mut next = config.clone();
    next.dismissed_merges.push(suggestion_id.into());
    next
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contributors::model::config::default_config;
    use crate::contributors::model::types::{ContributorsConfig, Person};
    use crate::contributors::GitIdentity;

    fn cfg(patch: impl FnOnce(&mut ContributorsConfig)) -> ContributorsConfig {
        let mut config = default_config();
        patch(&mut config);
        config
    }

    fn id(name: &str, email: &str) -> GitIdentity {
        GitIdentity {
            name: name.into(),
            email: email.into(),
            login: None,
        }
    }

    fn idl(name: &str, email: &str, login: &str) -> GitIdentity {
        GitIdentity {
            login: Some(login.into()),
            ..id(name, email)
        }
    }

    fn strs(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    fn person(pid: &str, name: &str, identities: &[&str]) -> Person {
        Person {
            id: pid.into(),
            name: name.into(),
            identities: strs(identities),
        }
    }

    fn opts(merge: bool, observed: Vec<GitIdentity>) -> ResolverOptions {
        ResolverOptions {
            merge,
            observed_identities: observed,
            ..Default::default()
        }
    }

    fn people_ids(config: &ContributorsConfig) -> Vec<&str> {
        config.people.iter().map(|p| p.id.as_str()).collect()
    }

    // identityKey

    #[test]
    fn identity_key_uses_the_lowercased_email() {
        assert_eq!(
            identity_key(&id("Alice", "Alice@Example.COM")),
            "alice@example.com"
        );
    }

    #[test]
    fn identity_key_trims_the_email() {
        assert_eq!(identity_key(&id("A", "  a@x.com ")), "a@x.com");
    }

    #[test]
    fn identity_key_uses_gh_login_when_the_email_is_empty() {
        assert_eq!(identity_key(&idl("Alice", "", "Alice-Dev")), "gh:alice-dev");
    }

    #[test]
    fn identity_key_prefers_the_email_over_the_login() {
        assert_eq!(identity_key(&idl("Alice", "a@x.com", "alice")), "a@x.com");
    }

    #[test]
    fn identity_key_falls_back_to_the_lowercased_name() {
        assert_eq!(identity_key(&id("Nguyễn Văn Đạt", "")), "nguyễn văn đạt");
        assert_eq!(identity_key(&id("  Bob ", "  ")), "bob");
    }

    #[test]
    fn identity_key_returns_an_empty_string_when_nothing_is_known() {
        assert_eq!(identity_key(&id("", "")), "");
    }

    // githubLoginFromEmail

    #[test]
    fn github_login_from_email_parses_the_id_plus_login_form() {
        assert_eq!(
            github_login_from_email("12345+TuanN@users.noreply.github.com").as_deref(),
            Some("tuann")
        );
    }

    #[test]
    fn github_login_from_email_parses_the_bare_login_form() {
        assert_eq!(
            github_login_from_email("tuann@users.noreply.github.com").as_deref(),
            Some("tuann")
        );
    }

    #[test]
    fn github_login_from_email_is_case_insensitive_on_the_domain_and_trims() {
        assert_eq!(
            github_login_from_email(" 1+a@Users.NoReply.GitHub.com ").as_deref(),
            Some("a")
        );
    }

    #[test]
    fn github_login_from_email_keeps_bot_suffixes() {
        assert_eq!(
            github_login_from_email("49699333+dependabot[bot]@users.noreply.github.com").as_deref(),
            Some("dependabot[bot]")
        );
    }

    #[test]
    fn github_login_from_email_returns_none_for_other_addresses() {
        for email in [
            "a@example.com",
            "noreply@github.com",
            "a@noreply.github.com",
            "",
            "@users.noreply.github.com",
            "1+@users.noreply.github.com",
        ] {
            assert_eq!(github_login_from_email(email), None, "{email}");
        }
    }

    // isAgent / isExcluded / isHuman

    #[test]
    fn classification_recognizes_ai_agents_by_email_name_and_login() {
        let config = cfg(|_| {});
        assert!(is_agent(&config, &id("Claude", "noreply@anthropic.com")));
        assert!(is_agent(&config, &id("Claude", "x@y.z")));
        assert!(is_agent(&config, &idl("", "", "copilot-swe-agent[bot]")));
        assert!(is_agent(&config, &id("Devin", "devin@cognition.ai")));
        assert!(is_agent(&config, &id("Codex", "codex@openai.com")));
        assert!(is_agent(
            &config,
            &id("Cursor Agent", "cursoragent@cursor.com")
        ));
    }

    #[test]
    fn classification_does_not_treat_humans_who_share_a_first_name_with_an_agent_as_agents() {
        let config = cfg(|_| {});
        for human in [
            id("Claude Dupont", "claude.dupont@corp.io"),
            id("Devin Nguyen", "devin@acme.com"),
            id("Gemini Rodriguez", "gemini.r@acme.com"),
            id("Codex Fan", "fan@acme.com"),
        ] {
            assert!(!is_agent(&config, &human), "{}", human.name);
            assert!(is_human(&config, &human), "{}", human.name);
        }
    }

    #[test]
    fn classification_recognizes_generic_bots_as_excluded_not_agents() {
        let config = cfg(|_| {});
        let dependabot = id(
            "dependabot[bot]",
            "49699333+dependabot[bot]@users.noreply.github.com",
        );
        assert!(!is_agent(&config, &dependabot));
        assert!(is_excluded(&config, &dependabot));
        assert!(!is_human(&config, &dependabot));
        assert!(is_excluded(&config, &id("GitHub", "noreply@github.com")));
        assert!(is_excluded(&config, &id("CI", "build@ci-runner-3.corp")));
    }

    #[test]
    fn classification_lets_agent_win_over_excluded() {
        let config = cfg(|_| {});
        let bot = id(
            "copilot-swe-agent[bot]",
            "1+Copilot@users.noreply.github.com",
        );
        assert!(is_agent(&config, &bot));
        assert!(!is_excluded(&config, &bot));
        assert!(!is_human(&config, &bot));
    }

    #[test]
    fn classification_treats_everyone_else_as_human() {
        let config = cfg(|_| {});
        let alice = id("Alice", "alice@example.com");
        assert!(!is_agent(&config, &alice));
        assert!(!is_excluded(&config, &alice));
        assert!(is_human(&config, &alice));
    }

    #[test]
    fn classification_honors_custom_patterns() {
        let custom = cfg(|c| {
            c.exclude_identities = strs(&["old@x.com"]);
            c.agent_identities = vec![];
        });
        assert!(is_excluded(&custom, &id("Old", "OLD@x.com")));
        assert!(!is_agent(&custom, &id("Claude", "noreply@anthropic.com")));
    }

    // classification memo

    #[test]
    fn memo_returns_the_same_verdicts_on_repeated_asks_and_for_other_fields_of_the_same_identity() {
        let config = cfg(|_| {});
        let bot = id("dependabot[bot]", "bot@users.noreply.github.com");
        for _ in 0..3 {
            assert!(is_excluded(&config, &bot));
            assert!(!is_agent(&config, &bot));
        }
        // Same email, different name or login: not the same identity.
        assert!(is_agent(&config, &id("Claude", "x@y.z")));
        assert!(!is_agent(&config, &id("Ann", "x@y.z")));
        assert!(is_agent(
            &config,
            &idl("Ann", "x@y.z", "copilot-swe-agent[bot]")
        ));
    }

    #[test]
    fn memo_follows_a_config_whose_lists_were_replaced() {
        let mut config = cfg(|c| {
            c.exclude_identities = vec![];
            c.agent_identities = vec![];
        });
        let dev = id("Dev", "dev@x.com");
        assert!(!is_excluded(&config, &dev));
        config.exclude_identities = strs(&["dev@x.com"]);
        assert!(is_excluded(&config, &dev));
    }

    #[test]
    fn memo_follows_a_list_that_grew_in_place() {
        let mut config = cfg(|c| {
            c.exclude_identities = vec![];
            c.agent_identities = vec![];
        });
        let dev = id("Dev", "dev@x.com");
        assert!(!is_agent(&config, &dev));
        config.agent_identities.push("dev@x.com".into());
        assert!(is_agent(&config, &dev));
        assert!(!is_excluded(&config, &dev));
    }

    #[test]
    fn memo_keeps_separate_verdicts_for_separate_configs() {
        let a = cfg(|c| c.exclude_identities = strs(&["dev@x.com"]));
        let b = cfg(|c| c.exclude_identities = vec![]);
        let dev = id("Dev", "dev@x.com");
        assert!(is_excluded(&a, &dev));
        assert!(!is_excluded(&b, &dev));
        assert!(is_human(&b, &dev));
        // and back again: the first config's answer did not leak into the second
        assert!(is_excluded(&a, &dev));
    }

    #[test]
    fn memo_follows_a_list_edited_in_place_to_a_different_pattern_of_the_same_length() {
        let mut config = cfg(|c| {
            c.exclude_identities = strs(&["dev@x.com"]);
            c.agent_identities = vec![];
        });
        let dev = id("Dev", "dev@x.com");
        assert!(is_excluded(&config, &dev));
        config.exclude_identities[0] = "other@x.com".into();
        assert!(!is_excluded(&config, &dev));
    }

    // parseMailmap

    fn mm(name: &str, identities: &[&str]) -> MailmapPerson {
        MailmapPerson {
            name: name.into(),
            identities: strs(identities),
        }
    }

    #[test]
    fn parse_mailmap_returns_nothing_for_empty_input() {
        assert_eq!(parse_mailmap(""), vec![]);
        assert_eq!(parse_mailmap("\n\n  \n"), vec![]);
    }

    #[test]
    fn parse_mailmap_parses_the_proper_name_proper_email_form() {
        assert_eq!(
            parse_mailmap("Jane Doe <Jane@X.com>"),
            vec![mm("Jane Doe", &["jane@x.com"])]
        );
    }

    #[test]
    fn parse_mailmap_parses_the_proper_email_commit_email_form() {
        assert_eq!(
            parse_mailmap("<jane@x.com> <jd@old.com>"),
            vec![mm("", &["jane@x.com", "jd@old.com"])]
        );
    }

    #[test]
    fn parse_mailmap_parses_the_proper_name_proper_email_commit_email_form() {
        assert_eq!(
            parse_mailmap("Jane Doe <jane@x.com> <jd@old.com>"),
            vec![mm("Jane Doe", &["jane@x.com", "jd@old.com"])]
        );
    }

    #[test]
    fn parse_mailmap_parses_the_proper_name_proper_email_commit_name_commit_email_form() {
        assert_eq!(
            parse_mailmap("Jane Doe <jane@x.com> Jane D <JANE.D@laptop>"),
            vec![mm("Jane Doe", &["jane@x.com", "jane.d@laptop"])]
        );
    }

    #[test]
    fn parse_mailmap_groups_lines_by_proper_email_and_skips_comments_and_junk() {
        let text = [
            "# authors",
            "",
            "Jane Doe <jane@x.com>",
            "<jane@x.com> <jd@old.com>",
            "Jane Doe <jane@x.com> Jane D <j@other>",
            "garbage line",
            "<unclosed",
            "Bob <bob@x.com>",
            "   # indented comment",
        ]
        .join("\n");
        assert_eq!(
            parse_mailmap(&text),
            vec![
                mm("Jane Doe", &["jane@x.com", "jd@old.com", "j@other"]),
                mm("Bob", &["bob@x.com"]),
            ]
        );
    }

    #[test]
    fn parse_mailmap_dedupes_identities_and_handles_crlf() {
        let text = "A <a@x.com> <b@x.com>\r\nA <a@x.com> <B@X.com>\r\n";
        assert_eq!(parse_mailmap(text), vec![mm("A", &["a@x.com", "b@x.com"])]);
    }

    #[test]
    fn parse_mailmap_groups_case_insensitively_by_proper_email_and_keeps_the_first_name() {
        let text = "<a@x.com> <b@x.com>\nAnna <A@x.com> <c@x.com>";
        assert_eq!(
            parse_mailmap(text),
            vec![mm("Anna", &["a@x.com", "b@x.com", "c@x.com"])]
        );
    }

    #[test]
    fn parse_mailmap_strips_a_trailing_comment() {
        assert_eq!(
            parse_mailmap("Jane <jane@new.com> <jane@old.com> # moved in 2020"),
            vec![mm("Jane", &["jane@new.com", "jane@old.com"])]
        );
        assert_eq!(
            parse_mailmap("Bob <bob@x.com>   # just a name"),
            vec![mm("Bob", &["bob@x.com"])]
        );
    }

    #[test]
    fn parse_mailmap_handles_unicode_names() {
        assert_eq!(
            parse_mailmap("Nguyễn Văn Đạt <dat@x.com> <d@old.com>"),
            vec![mm("Nguyễn Văn Đạt", &["dat@x.com", "d@old.com"])]
        );
    }

    // buildResolver

    #[test]
    fn resolver_without_merging_every_distinct_key_is_its_own_person() {
        let r = build_resolver(
            &cfg(|c| c.people = vec![person("p", "P", &["a@x.com"])]),
            opts(
                false,
                vec![
                    id("Alice", "a@x.com"),
                    id("Alice", "A@X.com"),
                    id("Alice", "a@y.com"),
                ],
            ),
        );
        assert_eq!(r.person_of(&id("Alice", "A@x.com")), "a@x.com");
        assert_eq!(r.person_of(&id("Alice", "a@y.com")), "a@y.com");
        let mut ids: Vec<String> = r.people().iter().map(|p| p.id.clone()).collect();
        ids.sort();
        assert_eq!(ids, ["a@x.com", "a@y.com"]);
        assert_eq!(r.name_of("a@x.com"), "Alice");
    }

    #[test]
    fn resolver_maps_unmapped_identities_to_themselves_when_merging() {
        let r = build_resolver(&cfg(|_| {}), opts(true, vec![id("Bob", "bob@x.com")]));
        assert_eq!(r.person_of(&id("Bob", "BOB@x.com")), "bob@x.com");
        assert_eq!(r.name_of("bob@x.com"), "Bob");
    }

    #[test]
    fn resolver_maps_listed_identities_to_the_configured_person() {
        let r = build_resolver(
            &cfg(|c| {
                c.people = vec![person(
                    "alice",
                    "Alice A",
                    &["a@x.com", "alice@y.com", "gh:alicea"],
                )]
            }),
            opts(true, vec![id("A", "a@x.com"), id("Alice", "Alice@Y.com")]),
        );
        assert_eq!(r.person_of(&id("A", "a@x.com")), "alice");
        assert_eq!(r.person_of(&id("Alice", "ALICE@y.com")), "alice");
        assert_eq!(r.person_of(&idl("", "", "AliceA")), "alice");
        assert_eq!(r.name_of("alice"), "Alice A");
        let people = r.people();
        let alice = people.iter().find(|p| p.id == "alice").unwrap();
        // the login-only identity looked up above is registered too
        assert_eq!(alice.observed.len(), 3);
    }

    #[test]
    fn resolver_maps_a_gh_login_to_a_person_listing_a_noreply_email_with_that_login() {
        let r = build_resolver(
            &cfg(|c| {
                c.people = vec![person(
                    "tuan",
                    "Tuan",
                    &["12345+tuann@users.noreply.github.com"],
                )]
            }),
            opts(true, vec![]),
        );
        assert_eq!(r.person_of(&idl("", "", "TuanN")), "tuan");
    }

    #[test]
    fn resolver_maps_a_noreply_email_to_a_person_listing_the_gh_login() {
        let r = build_resolver(
            &cfg(|c| c.people = vec![person("tuan", "Tuan", &["gh:tuann"])]),
            opts(true, vec![]),
        );
        assert_eq!(
            r.person_of(&id("T", "12345+tuann@users.noreply.github.com")),
            "tuan"
        );
        assert_eq!(r.person_of(&idl("T", "other@x.com", "tuann")), "tuan");
    }

    #[test]
    fn resolver_links_a_login_only_reviewer_to_the_commit_identity_seen_with_that_login() {
        let r = build_resolver(
            &cfg(|_| {}),
            opts(
                true,
                vec![
                    idl("Carol", "carol@corp.com", "carolc"),
                    idl("", "", "carolc"),
                ],
            ),
        );
        assert_eq!(r.person_of(&idl("", "", "CarolC")), "carol@corp.com");
    }

    #[test]
    fn resolver_links_a_login_only_reviewer_to_a_noreply_email_seen_on_commits() {
        let r = build_resolver(
            &cfg(|_| {}),
            opts(true, vec![id("Dan", "99+dand@users.noreply.github.com")]),
        );
        assert_eq!(
            r.person_of(&idl("", "", "dand")),
            "99+dand@users.noreply.github.com"
        );
    }

    #[test]
    fn resolver_does_not_link_a_login_only_reviewer_when_merging_is_off() {
        let r = build_resolver(
            &cfg(|_| {}),
            opts(false, vec![idl("Carol", "carol@corp.com", "carolc")]),
        );
        assert_eq!(r.person_of(&idl("", "", "carolc")), "gh:carolc");
    }

    #[test]
    fn resolver_does_not_link_two_commit_emails_that_share_a_login() {
        let r = build_resolver(
            &cfg(|_| {}),
            opts(
                true,
                vec![idl("E", "e@a.com", "ee"), idl("E", "e@b.com", "ee")],
            ),
        );
        assert_eq!(r.person_of(&idl("E", "e@a.com", "ee")), "e@a.com");
        assert_eq!(r.person_of(&idl("E", "e@b.com", "ee")), "e@b.com");
    }

    #[test]
    fn resolver_applies_mailmap_people_under_config_people() {
        let r = build_resolver(
            &cfg(|c| c.people = vec![person("boss", "The Boss", &["jd@old.com"])]),
            ResolverOptions {
                merge: true,
                mailmap: vec![
                    mm("Jane Doe", &["jane@x.com", "jd@old.com", "j@other"]),
                    mm("Bob", &["bob@x.com", "b@old.com"]),
                ],
                observed_identities: vec![
                    id("Jane", "jane@x.com"),
                    id("J", "j@other"),
                    id("B", "b@old.com"),
                    id("Bobby", "bob@x.com"),
                ],
            },
        );
        // config wins: the config person lists one email of the mailmap group
        assert_eq!(r.person_of(&id("Jane", "jane@x.com")), "boss");
        assert_eq!(r.person_of(&id("J", "j@other")), "boss");
        assert_eq!(r.person_of(&id("B", "b@old.com")), "bob@x.com");
        assert_eq!(r.person_of(&id("Bobby", "bob@x.com")), "bob@x.com");
        assert_eq!(r.name_of("bob@x.com"), "Bob");
        assert_eq!(r.name_of("boss"), "The Boss");
    }

    #[test]
    fn resolver_falls_back_to_observed_names_when_the_mailmap_name_is_empty() {
        let r = build_resolver(
            &cfg(|_| {}),
            ResolverOptions {
                merge: true,
                mailmap: vec![mm("", &["a@x.com", "b@x.com"])],
                observed_identities: vec![id("Anna", "a@x.com"), id("Anna", "b@x.com")],
            },
        );
        assert_eq!(r.person_of(&id("Anna", "b@x.com")), "a@x.com");
        assert_eq!(r.name_of("a@x.com"), "Anna");
    }

    #[test]
    fn resolver_resolves_an_identity_whose_key_equals_a_config_person_id() {
        let r = build_resolver(
            &cfg(|c| c.people = vec![person("a@x.com", "Alice", &["b@x.com"])]),
            opts(true, vec![id("A", "a@x.com"), id("B", "b@x.com")]),
        );
        assert_eq!(r.person_of(&id("A", "a@x.com")), "a@x.com");
        assert_eq!(r.person_of(&id("B", "b@x.com")), "a@x.com");
        assert_eq!(r.people().len(), 1);
        assert_eq!(r.name_of("a@x.com"), "Alice");
    }

    #[test]
    fn resolver_answers_through_shared_references_to_its_parts() {
        let resolver = build_resolver(&cfg(|_| {}), opts(true, vec![id("Bob", "bob@x.com")]));
        let person_of = |i: &GitIdentity| resolver.person_of(i);
        let name_of = |p: &str| resolver.name_of(p);
        let people = resolver.people();
        assert_eq!(person_of(&id("Bob", "bob@x.com")), "bob@x.com");
        assert_eq!(name_of("bob@x.com"), "Bob");
        assert_eq!(
            people.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(),
            ["bob@x.com"]
        );
    }

    fn commit() -> GitIdentity {
        idl("Alice", "alice@acme.com", "alice")
    }

    fn reviewer() -> GitIdentity {
        idl("", "", "alice")
    }

    #[test]
    fn order_independence_resolves_a_login_only_identity_the_same_before_and_after_the_commit_identity(
    ) {
        let r = build_resolver(&cfg(|_| {}), opts(true, vec![commit(), reviewer()]));
        let first = r.person_of(&reviewer());
        r.person_of(&commit());
        let second = r.person_of(&reviewer());
        assert_eq!(first, "alice@acme.com");
        assert_eq!(second, first);
        let ids: Vec<String> = r.people().iter().map(|p| p.id.clone()).collect();
        assert_eq!(ids, ["alice@acme.com"]);
    }

    #[test]
    fn order_independence_does_not_learn_links_lazily_from_identities_that_were_not_observed_up_front(
    ) {
        let r = build_resolver(&cfg(|_| {}), opts(true, vec![]));
        let first = r.person_of(&reviewer());
        assert_eq!(r.person_of(&commit()), "alice@acme.com");
        assert_eq!(r.person_of(&reviewer()), first);

        let reversed = build_resolver(&cfg(|_| {}), opts(true, vec![]));
        assert_eq!(reversed.person_of(&commit()), "alice@acme.com");
        assert_eq!(reversed.person_of(&reviewer()), first);
    }

    #[test]
    fn order_independence_gives_the_same_answers_for_any_call_order() {
        let observed = vec![commit(), reviewer(), idl("Bob", "bob@x.com", "bobb")];
        let calls = vec![
            reviewer(),
            commit(),
            idl("Bob", "bob@x.com", "bobb"),
            idl("", "", "bobb"),
        ];
        let answers = |order: &[GitIdentity]| {
            let r = build_resolver(&cfg(|_| {}), opts(true, observed.clone()));
            let mut out: Vec<(String, String)> = order
                .iter()
                .map(|i| (identity_key(i), r.person_of(i)))
                .collect();
            out.sort();
            out
        };
        let a = answers(&calls);
        let mut reversed = calls.clone();
        reversed.reverse();
        assert_eq!(a, answers(&reversed));
    }

    #[test]
    fn order_independence_links_a_config_person_listing_a_noreply_email_to_the_login_regardless_of_order(
    ) {
        let config =
            cfg(|c| c.people = vec![person("t", "T", &["1+tuan@users.noreply.github.com"])]);
        let r = build_resolver(&config, opts(true, vec![]));
        let before = r.person_of(&idl("", "", "tuan"));
        r.person_of(&id("T", "1+tuan@users.noreply.github.com"));
        assert_eq!(r.person_of(&idl("", "", "tuan")), before);
        assert_eq!(before, "t");
    }

    #[test]
    fn resolver_picks_the_most_frequent_display_name_ties_to_the_longest() {
        let r = build_resolver(
            &cfg(|_| {}),
            opts(
                true,
                vec![
                    id("Al", "a@x.com"),
                    id("Alice", "a@x.com"),
                    id("Al", "a@x.com"),
                    id("Bo", "b@x.com"),
                    id("Bobby", "b@x.com"),
                ],
            ),
        );
        assert_eq!(r.name_of("a@x.com"), "Al");
        assert_eq!(r.name_of("b@x.com"), "Bobby");
    }

    #[test]
    fn resolver_uses_the_login_or_key_when_no_name_is_known() {
        let r = build_resolver(&cfg(|_| {}), opts(true, vec![idl("", "", "ghost")]));
        assert_eq!(r.name_of("gh:ghost"), "ghost");
        assert_eq!(r.name_of("unknown-id"), "unknown-id");
    }

    #[test]
    fn resolver_lists_config_people_even_when_never_observed() {
        let r = build_resolver(
            &cfg(|c| c.people = vec![person("p", "P", &["p@x.com"])]),
            opts(true, vec![]),
        );
        assert_eq!(
            *r.people(),
            vec![ResolvedPerson {
                id: "p".into(),
                name: "P".into(),
                identities: strs(&["p@x.com"]),
                observed: vec![],
            }]
        );
    }

    #[test]
    fn resolver_registers_identities_first_seen_through_person_of() {
        let r = build_resolver(&cfg(|_| {}), opts(true, vec![]));
        assert_eq!(r.person_of(&id("Late", "late@x.com")), "late@x.com");
        let ids: Vec<String> = r.people().iter().map(|p| p.id.clone()).collect();
        assert_eq!(ids, ["late@x.com"]);
        assert_eq!(r.name_of("late@x.com"), "Late");
    }

    #[test]
    fn resolver_is_stable_for_repeated_calls() {
        let r = build_resolver(&cfg(|_| {}), opts(true, vec![id("A", "a@x.com")]));
        assert_eq!(
            r.person_of(&id("A", "a@x.com")),
            r.person_of(&id("A", "a@x.com"))
        );
    }

    #[test]
    fn resolver_handles_unicode_names_and_uppercase_emails() {
        let r = build_resolver(
            &cfg(|_| {}),
            opts(true, vec![id("Nguyễn Văn Đạt", "DAT@X.COM")]),
        );
        assert_eq!(r.person_of(&id("x", "dat@x.com")), "dat@x.com");
        assert_eq!(r.name_of("dat@x.com"), "Nguyễn Văn Đạt");
    }

    #[test]
    fn resolver_can_be_shared_between_threads() {
        fn assert_sync<T: Send + Sync>() {}
        assert_sync::<Resolver>();
    }

    // suggestMerges

    fn sp(pid: &str, name: &str, identities: &[&str]) -> SuggestPerson {
        SuggestPerson {
            id: pid.into(),
            name: name.into(),
            identities: strs(identities),
            observed: vec![],
        }
    }

    fn spo(
        pid: &str,
        name: &str,
        identities: &[&str],
        observed: Vec<GitIdentity>,
    ) -> SuggestPerson {
        SuggestPerson {
            observed,
            ..sp(pid, name, identities)
        }
    }

    #[test]
    fn suggest_merges_suggests_persons_with_the_same_normalized_display_name() {
        let out = suggest_merges(
            &[
                sp("a@x.com", "Nguyễn  Văn A", &["a@x.com"]),
                sp("b@y.com", "nguyen van a", &["b@y.com"]),
                sp("c@z.com", "Someone Else", &["c@z.com"]),
            ],
            &[],
        );
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].reason, MergeReason::SameName);
        assert_eq!(out[0].person_ids, ["a@x.com", "b@y.com"]);
        assert_eq!(out[0].id, "a@x.com|b@y.com");
        assert!(out[0].label.contains("name"));
    }

    #[test]
    fn suggest_merges_matches_on_any_observed_display_name() {
        let out = suggest_merges(
            &[
                spo(
                    "a@x.com",
                    "A",
                    &["a@x.com"],
                    vec![id("Dana Scully", "a@x.com")],
                ),
                spo(
                    "b@y.com",
                    "B",
                    &["b@y.com"],
                    vec![id("dana scully", "b@y.com")],
                ),
            ],
            &[],
        );
        assert_eq!(
            out.iter().map(|s| s.reason).collect::<Vec<_>>(),
            [MergeReason::SameName]
        );
    }

    #[test]
    fn suggest_merges_groups_three_persons_into_one_suggestion() {
        let out = suggest_merges(
            &[
                sp("c@x.com", "Sam Lee", &["c@x.com"]),
                sp("a@x.com", "sam lee", &["a@x.com"]),
                sp("b@x.com", "SAM LEE", &["b@x.com"]),
            ],
            &[],
        );
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].person_ids, ["a@x.com", "b@x.com", "c@x.com"]);
        assert_eq!(out[0].id, "a@x.com|b@x.com|c@x.com");
    }

    #[test]
    fn suggest_merges_never_suggests_a_person_with_itself() {
        let out = suggest_merges(
            &[spo(
                "a@x.com",
                "Sam",
                &["a@x.com", "sam@y.com"],
                vec![id("Sam", "a@x.com"), id("sam", "sam@y.com")],
            )],
            &[],
        );
        assert_eq!(out, vec![]);
    }

    #[test]
    fn suggest_merges_suggests_similar_emails_across_different_domains() {
        let out = suggest_merges(
            &[
                sp("p1", "One", &["john.doe@a.com"]),
                sp("p2", "Two", &["johndoe+work@b.com"]),
            ],
            &[],
        );
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].reason, MergeReason::SimilarEmail);
        assert_eq!(out[0].person_ids, ["p1", "p2"]);
    }

    #[test]
    fn suggest_merges_does_not_suggest_similar_emails_on_the_same_domain() {
        let out = suggest_merges(
            &[
                sp("p1", "One", &["john.doe@a.com"]),
                sp("p2", "Two", &["johndoe@a.com"]),
            ],
            &[],
        );
        assert_eq!(out, vec![]);
    }

    #[test]
    fn suggest_merges_does_not_suggest_tiny_or_generic_local_parts() {
        let out = suggest_merges(
            &[
                sp("p1", "One", &["ab@a.com"]),
                sp("p2", "Two", &["ab@b.com"]),
                sp("p3", "Three", &["admin@a.com"]),
                sp("p4", "Four", &["admin@b.com"]),
            ],
            &[],
        );
        assert_eq!(out, vec![]);
    }

    #[test]
    fn suggest_merges_suggests_a_github_noreply_login_that_equals_another_email_local_part() {
        let out = suggest_merges(
            &[
                sp("p1", "One", &["12345+tuann@users.noreply.github.com"]),
                sp("p2", "Two", &["tuann@corp.com"]),
            ],
            &[],
        );
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].reason, MergeReason::GithubNoreply);
        assert_eq!(out[0].id, "p1|p2");
    }

    #[test]
    fn suggest_merges_suggests_a_gh_identity_that_equals_another_email_local_part() {
        let out = suggest_merges(
            &[
                sp("p1", "One", &["gh:tuann"]),
                sp("p2", "Two", &["tuann@corp.com"]),
            ],
            &[],
        );
        assert_eq!(
            out.iter().map(|s| s.reason).collect::<Vec<_>>(),
            [MergeReason::GithubNoreply]
        );
    }

    #[test]
    fn suggest_merges_keeps_one_suggestion_per_person_set_preferring_same_name() {
        let out = suggest_merges(
            &[
                sp("p1", "Tuan Nguyen", &["tuann@a.com"]),
                sp("p2", "Tuan Nguyen", &["tuann@b.com"]),
            ],
            &[],
        );
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].reason, MergeReason::SameName);
    }

    #[test]
    fn suggest_merges_never_suggests_agents_or_excluded_identities() {
        let out = suggest_merges(
            &[
                spo(
                    "c1",
                    "Claude",
                    &["noreply@anthropic.com"],
                    vec![id("Claude", "noreply@anthropic.com")],
                ),
                spo(
                    "c2",
                    "Claude",
                    &["claude@other.com"],
                    vec![id("Claude", "claude@other.com")],
                ),
                sp(
                    "d1",
                    "dependabot[bot]",
                    &["1+dependabot[bot]@users.noreply.github.com"],
                ),
                sp(
                    "d2",
                    "dependabot[bot]",
                    &["2+dependabot[bot]@users.noreply.github.com"],
                ),
                sp("g1", "GitHub", &["noreply@github.com"]),
                sp("g2", "GitHub", &["gh:github"]),
            ],
            &[],
        );
        assert_eq!(out, vec![]);
    }

    #[test]
    fn suggest_merges_respects_a_custom_config() {
        let custom = cfg(|c| {
            c.agent_identities = vec![];
            c.exclude_identities = strs(&["*@ci.com"]);
        });
        let out = suggest_merges_with(
            &[
                sp("a", "Robo Cop", &["r1@ci.com"]),
                sp("b", "Robo Cop", &["r2@ci.com"]),
                sp("c", "Real Person", &["r@x.com"]),
                sp("d", "Real Person", &["r@y.org"]),
            ],
            &[],
            &custom,
        );
        assert_eq!(
            out.iter().map(|s| s.person_ids.clone()).collect::<Vec<_>>(),
            [strs(&["c", "d"])]
        );
    }

    #[test]
    fn suggest_merges_filters_dismissed_suggestions() {
        let people = [
            sp("a@x.com", "Sam Lee", &["a@x.com"]),
            sp("b@x.com", "Sam Lee", &["b@x.com"]),
        ];
        let first = suggest_merges(&people, &[]);
        assert_eq!(first.len(), 1);
        assert_eq!(suggest_merges(&people, &[first[0].id.clone()]), vec![]);
        assert_eq!(suggest_merges(&people, &strs(&["other|ids"])).len(), 1);
    }

    #[test]
    fn suggest_merges_keeps_a_dismissal_valid_when_the_reason_changes() {
        let by_email = [
            sp("p1", "One", &["john.doe@a.com"]),
            sp("p2", "Two", &["johndoe@b.com"]),
        ];
        let by_name = [
            sp("p1", "John Doe", &["john.doe@a.com"]),
            sp("p2", "John Doe", &["johndoe@b.com"]),
        ];
        let first = suggest_merges(&by_email, &[]);
        assert_eq!(first[0].reason, MergeReason::SimilarEmail);
        assert_eq!(suggest_merges(&by_name, &[first[0].id.clone()]), vec![]);
    }

    #[test]
    fn suggest_merges_still_honors_legacy_dismissals_that_carry_a_reason_suffix() {
        let people = [
            sp("a@x.com", "Sam Lee", &["a@x.com"]),
            sp("b@x.com", "Sam Lee", &["b@x.com"]),
        ];
        assert_eq!(
            suggest_merges(&people, &strs(&["a@x.com|b@x.com:same-name"])),
            vec![]
        );
    }

    #[test]
    fn suggest_merges_does_not_suggest_single_word_names() {
        assert_eq!(
            suggest_merges(
                &[
                    sp("a@x.com", "Minh", &["a@x.com"]),
                    sp("b@y.com", "Minh", &["b@y.com"])
                ],
                &[]
            ),
            vec![]
        );
    }

    #[test]
    fn suggest_merges_does_not_suggest_placeholder_names() {
        for name in [
            "Your Name",
            "root",
            "unknown",
            "admin",
            "user",
            "ubuntu",
            "github",
            "GitHub",
        ] {
            assert_eq!(
                suggest_merges(
                    &[
                        sp("a@x.com", name, &["a@x.com"]),
                        sp("b@y.com", name, &["b@y.com"])
                    ],
                    &[]
                ),
                vec![],
                "{name}"
            );
        }
    }

    #[test]
    fn suggest_merges_does_not_suggest_generic_local_parts() {
        for local in [
            "dev", "info", "admin", "noreply", "root", "me", "mail", "hello", "contact", "support",
            "team", "test", "user", "bot", "build", "office", "ci", "jenkins", "git", "github",
            "no-reply",
        ] {
            let a = format!("{local}@a.com");
            let b = format!("{local}@b.com");
            assert_eq!(
                suggest_merges(
                    &[
                        sp("p1", "One", &[a.as_str()]),
                        sp("p2", "Two", &[b.as_str()])
                    ],
                    &[]
                ),
                vec![],
                "{local}"
            );
        }
    }

    #[test]
    fn suggest_merges_returns_nothing_for_empty_input() {
        assert_eq!(suggest_merges(&[], &[]), vec![]);
        assert_eq!(suggest_merges(&[sp("a", "", &[])], &[]), vec![]);
    }

    // config editing helpers

    fn base() -> ContributorsConfig {
        cfg(|c| {
            c.people = vec![
                person("alice", "Alice", &["a@x.com", "gh:alice"]),
                person("bob", "Bob", &["b@x.com"]),
            ]
        })
    }

    fn snapshot(config: &ContributorsConfig) -> String {
        crate::contributors::model::config::serialize_config(config)
    }

    #[test]
    fn rename_person_renames_an_existing_person_without_mutating() {
        let b = base();
        let before = snapshot(&b);
        let next = rename_person(&b, "alice", "  Alice Q ");
        assert_eq!(next.people[0].name, "Alice Q");
        assert_eq!(next.people[1], b.people[1]);
        assert_eq!(snapshot(&b), before);
    }

    #[test]
    fn rename_person_creates_a_config_person_for_an_unmapped_id() {
        let next = rename_person(&base(), "c@x.com", "Carol");
        assert_eq!(next.people.len(), 3);
        assert_eq!(next.people[2], person("c@x.com", "Carol", &["c@x.com"]));
    }

    #[test]
    fn rename_person_ignores_a_blank_name() {
        assert_eq!(rename_person(&base(), "alice", "   "), base());
    }

    fn find<'a>(config: &'a ContributorsConfig, pid: &str) -> Option<&'a Person> {
        config.people.iter().find(|p| p.id == pid)
    }

    #[test]
    fn move_identity_moves_an_identity_to_another_existing_person() {
        let next = move_person_identity(&base(), "gh:alice", Some("bob"), None);
        assert_eq!(find(&next, "alice").unwrap().identities, ["a@x.com"]);
        assert_eq!(
            find(&next, "bob").unwrap().identities,
            ["b@x.com", "gh:alice"]
        );
    }

    #[test]
    fn move_identity_removes_the_identity_from_every_person_when_the_target_is_none() {
        let next = move_person_identity(&base(), "A@x.com", None, None);
        assert_eq!(find(&next, "alice").unwrap().identities, ["gh:alice"]);
    }

    #[test]
    fn move_identity_creates_the_target_when_it_does_not_exist() {
        let next = move_person_identity(&base(), "c@x.com", Some("carol"), Some("Carol C"));
        assert_eq!(
            find(&next, "carol").unwrap(),
            &person("carol", "Carol C", &["c@x.com"])
        );
    }

    #[test]
    fn move_identity_names_a_created_target_after_the_identity_key_by_default() {
        let next = move_person_identity(&base(), "c@x.com", Some("carol"), None);
        assert_eq!(find(&next, "carol").unwrap().name, "c@x.com");
    }

    #[test]
    fn move_identity_removes_the_identity_from_other_persons_first() {
        let next = move_person_identity(&base(), "a@x.com", Some("bob"), None);
        let holders: Vec<&str> = next
            .people
            .iter()
            .filter(|p| p.identities.iter().any(|i| i == "a@x.com"))
            .map(|p| p.id.as_str())
            .collect();
        assert_eq!(holders, ["bob"]);
    }

    #[test]
    fn move_identity_drops_config_people_left_without_identities() {
        let next = move_person_identity(&base(), "b@x.com", Some("alice"), None);
        assert_eq!(people_ids(&next), ["alice"]);
        let gone = move_person_identity(&base(), "b@x.com", None, None);
        assert_eq!(people_ids(&gone), ["alice"]);
    }

    #[test]
    fn move_identity_keeps_unrelated_empty_people_including_one_made_by_create_person() {
        let with_empty = create_person(&base(), "New Hire");
        let next = move_person_identity(&with_empty, "c@x.com", Some("bob"), None);
        assert_eq!(people_ids(&next), ["alice", "bob", "person-new-hire"]);
        let hand = cfg(|c| {
            c.people = vec![
                person("ghost", "Ghost", &[]),
                person("bob", "Bob", &["b@x.com"]),
            ]
        });
        assert_eq!(
            people_ids(&move_person_identity(&hand, "z@x.com", Some("bob"), None)),
            ["ghost", "bob"]
        );
    }

    #[test]
    fn move_identity_can_move_an_identity_into_a_person_made_by_create_person() {
        let with_empty = create_person(&base(), "New Hire");
        let next = move_person_identity(&with_empty, "b@x.com", Some("person-new-hire"), None);
        assert_eq!(people_ids(&next), ["alice", "person-new-hire"]);
        assert_eq!(next.people[1].identities, ["b@x.com"]);
    }

    #[test]
    fn move_identity_detaches_an_identity_whose_key_is_also_a_person_id() {
        let mut c = rename_person(&cfg(|_| {}), "alice@x.com", "Alice");
        c = merge_people(&c, &strs(&["alice@x.com", "bob@y.com"]), None);
        c = move_person_identity(&c, "alice@x.com", None, None);
        let r = build_resolver(
            &c,
            opts(
                true,
                vec![id("Alice", "alice@x.com"), id("Bob", "bob@y.com")],
            ),
        );
        let alice = r.person_of(&id("Alice", "alice@x.com"));
        let bob = r.person_of(&id("Bob", "bob@y.com"));
        assert_eq!(alice, "alice@x.com");
        assert_ne!(bob, alice);
        assert_eq!(c.people.len(), 1);
        assert_eq!(c.people[0].identities, ["bob@y.com"]);
        assert_eq!(c.people[0].name, "Alice");
    }

    #[test]
    fn move_identity_re_ids_a_person_whose_id_equals_the_moved_key_when_moving_elsewhere() {
        let mut c = rename_person(&cfg(|_| {}), "alice@x.com", "Alice");
        c = merge_people(&c, &strs(&["alice@x.com", "bob@y.com"]), None);
        c = move_person_identity(&c, "alice@x.com", Some("carol"), Some("Carol"));
        let r = build_resolver(
            &c,
            opts(true, vec![id("A", "alice@x.com"), id("B", "bob@y.com")]),
        );
        assert_eq!(r.person_of(&id("A", "alice@x.com")), "carol");
        assert_ne!(r.person_of(&id("B", "bob@y.com")), "carol");
    }

    #[test]
    fn move_identity_rewrites_dismissed_merges_that_reference_a_re_ided_person() {
        let mut c = rename_person(&cfg(|_| {}), "alice@x.com", "Alice");
        c = merge_people(&c, &strs(&["alice@x.com", "bob@y.com"]), None);
        c.dismissed_merges = strs(&[
            "alice@x.com|zed@z.com",
            "alice@x.com|zed@z.com:same-name",
            "bob@y.com|carol@c.com",
            "carol@c.com|zed@z.com",
        ]);
        let next = move_person_identity(&c, "alice@x.com", None, None);
        assert_eq!(next.people[0].id, "person-alice");
        assert_eq!(
            next.dismissed_merges,
            [
                "person-alice|zed@z.com",
                "person-alice|zed@z.com:same-name",
                "bob@y.com|carol@c.com",
                "carol@c.com|zed@z.com",
            ]
        );
    }

    #[test]
    fn move_identity_keeps_dismissed_ids_sorted_and_deduped_after_a_re_id() {
        let mut c = rename_person(&cfg(|_| {}), "alice@x.com", "Alice");
        c = merge_people(&c, &strs(&["alice@x.com", "bob@y.com"]), None);
        c.dismissed_merges = strs(&["alice@x.com|zzz@z.com", "aaa@a.com|alice@x.com"]);
        let next = move_person_identity(&c, "alice@x.com", None, None);
        assert_eq!(
            next.dismissed_merges,
            ["person-alice|zzz@z.com", "aaa@a.com|person-alice"]
        );
    }

    #[test]
    fn move_identity_does_not_touch_dismissed_merges_when_nothing_is_re_ided() {
        let mut c = base();
        c.dismissed_merges = strs(&["alice|bob"]);
        assert_eq!(
            move_person_identity(&c, "b@x.com", Some("alice"), None).dismissed_merges,
            ["alice|bob"]
        );
    }

    #[test]
    fn move_identity_does_not_re_id_a_person_that_receives_its_own_key() {
        let mut c = rename_person(&cfg(|_| {}), "alice@x.com", "Alice");
        c = move_person_identity(&c, "alice@x.com", Some("alice@x.com"), None);
        assert_eq!(people_ids(&c), ["alice@x.com"]);
    }

    #[test]
    fn move_identity_is_idempotent_and_does_not_duplicate() {
        let once = move_person_identity(&base(), "b@x.com", Some("alice"), None);
        let twice = move_person_identity(&once, "b@x.com", Some("alice"), None);
        assert_eq!(twice, once);
    }

    #[test]
    fn move_identity_does_not_mutate_the_input() {
        let b = base();
        let before = snapshot(&b);
        move_person_identity(&b, "b@x.com", Some("alice"), None);
        assert_eq!(snapshot(&b), before);
    }

    // mergePeople

    #[test]
    fn merge_people_merges_later_people_into_the_first_and_keeps_its_name() {
        let next = merge_people(&base(), &strs(&["alice", "bob"]), None);
        assert_eq!(
            next.people,
            vec![person(
                "alice",
                "Alice",
                &["a@x.com", "gh:alice", "b@x.com"]
            )]
        );
    }

    #[test]
    fn merge_people_uses_the_provided_name() {
        assert_eq!(
            merge_people(&base(), &strs(&["bob", "alice"]), Some("Team Person")).people[0].name,
            "Team Person"
        );
    }

    #[test]
    fn merge_people_turns_unmapped_ids_into_identities_of_the_target() {
        let next = merge_people(&base(), &strs(&["alice", "c@x.com", "d@y.com"]), None);
        assert_eq!(next.people.len(), 2);
        assert_eq!(
            next.people[0].identities,
            ["a@x.com", "gh:alice", "c@x.com", "d@y.com"]
        );
    }

    #[test]
    fn merge_people_creates_the_target_when_the_first_id_is_unmapped() {
        let next = merge_people(&base(), &strs(&["c@x.com", "bob"]), Some("Carol"));
        assert_eq!(
            find(&next, "c@x.com").unwrap(),
            &person("c@x.com", "Carol", &["c@x.com", "b@x.com"])
        );
        assert!(find(&next, "bob").is_none());
    }

    #[test]
    fn merge_people_dedupes_identities_and_ignores_duplicate_ids() {
        let next = merge_people(&base(), &strs(&["alice", "alice", "a@x.com"]), None);
        assert_eq!(next.people[0].identities, ["a@x.com", "gh:alice"]);
    }

    #[test]
    fn merge_people_does_nothing_for_fewer_than_two_distinct_ids() {
        assert_eq!(merge_people(&base(), &strs(&["alice"]), None), base());
        assert_eq!(merge_people(&base(), &[], None), base());
        assert_eq!(
            merge_people(&base(), &strs(&["alice", "alice"]), None),
            base()
        );
    }

    #[test]
    fn merge_people_does_not_mutate_the_input() {
        let b = base();
        let before = snapshot(&b);
        merge_people(&b, &strs(&["alice", "bob"]), None);
        assert_eq!(snapshot(&b), before);
    }

    // createPerson / personIdFor

    #[test]
    fn create_person_appends_an_empty_person_with_a_slug_id() {
        let b = base();
        let next = create_person(&b, " Nguyễn Văn Đạt ");
        let created = &next.people[2];
        assert_eq!(created.name, "Nguyễn Văn Đạt");
        assert_eq!(created.identities, Vec::<String>::new());
        assert_eq!(created.id, "person-nguyen-van-dat");
        assert_eq!(person_id_for(&b, " Nguyễn Văn Đạt "), created.id);
    }

    #[test]
    fn create_person_makes_ids_unique() {
        let once = create_person(&base(), "Sam");
        let twice = create_person(&once, "Sam");
        assert_eq!(&people_ids(&twice)[2..], ["person-sam", "person-sam-2"]);
    }

    #[test]
    fn person_id_for_falls_back_to_a_generic_slug_for_symbol_only_names() {
        assert_eq!(person_id_for(&base(), "***"), "person-person");
    }

    #[test]
    fn create_person_ignores_blank_names() {
        assert_eq!(create_person(&base(), "  "), base());
    }

    // exclude / include / dismiss

    #[test]
    fn exclude_identity_adds_a_lowercased_key_once() {
        let b = base();
        let once = exclude_identity(&b, "Bot@X.com");
        assert!(once.exclude_identities.iter().any(|p| p == "bot@x.com"));
        let twice = exclude_identity(&once, "bot@x.com");
        assert_eq!(
            twice
                .exclude_identities
                .iter()
                .filter(|p| *p == "bot@x.com")
                .count(),
            1
        );
        assert!(!b.exclude_identities.iter().any(|p| p == "bot@x.com"));
    }

    #[test]
    fn exclude_identity_makes_the_identity_excluded() {
        let next = exclude_identity(&base(), "gh:ghost");
        assert!(is_excluded(&next, &idl("", "", "ghost")));
    }

    #[test]
    fn include_identity_removes_a_pattern_case_insensitively() {
        let next = include_identity(&base(), "NOREPLY@github.com");
        assert!(!next
            .exclude_identities
            .iter()
            .any(|p| p == "noreply@github.com"));
        assert!(next.exclude_identities.iter().any(|p| p == "*@ci-runner*"));
    }

    #[test]
    fn include_identity_leaves_the_config_alone_when_the_pattern_is_absent() {
        assert_eq!(include_identity(&base(), "missing@x.com"), base());
    }

    #[test]
    fn dismiss_merge_adds_an_id_once() {
        let b = base();
        let once = dismiss_merge(&b, "a|b:same-name");
        assert_eq!(once.dismissed_merges, ["a|b:same-name"]);
        assert_eq!(
            dismiss_merge(&once, "a|b:same-name").dismissed_merges,
            ["a|b:same-name"]
        );
        assert_eq!(b.dismissed_merges, Vec::<String>::new());
    }
}
