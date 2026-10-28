//! The Identities section of the GitHub tab (`IdentitiesTab.tsx` and
//! `identities/*`): suggested merges, the people with their identities
//! (rename, menus, drag between cards), the unmapped identities and the
//! excluded list. This file holds the rules (what each action writes to the
//! config, which suggestions are shown); `view.rs` is the entity that draws
//! and handles them, `drag.rs` the drag of an identity onto a person.
//! Every edit goes through the core identity helpers and then the save queue
//! of the `GithubView` (blocked while the file is unreadable).
use super::github::GithubView;
use gpui::{AnyElement, Context, prelude::*};
use monocode_core::contributors::model::{
    identity::{
        MergeSuggestion, SuggestPerson, create_person, dismiss_merge, exclude_identity,
        include_identity, merge_people, move_person_identity, person_id_for, rename_person,
        suggest_merges_with,
    },
    locale_cmp,
    pipeline::ContributorsModel,
    types::ContributorsConfig,
    unmapped::{ExcludedIdentity, PersonRow, ensure_config_person, remove_person},
};
use std::collections::HashMap;

mod drag;
mod draw;
mod view;

#[cfg(test)]
pub(super) use drag::DragState;
#[cfg(test)]
pub(super) use draw::{
    AGENT_NOTE, MERGING_OFF, SAVED_PATH, SAVED_REST, SAVED_TO, UNMAPPED_INTRO, WIDE_AT,
    counts_text, person_color, plural, reason_text, unmapped_sub,
};
pub(super) use view::IdentitiesView;

pub(super) const NEW_PERSON_NAME: &str = "New person";

/// A suggested merge with the people it involves, the one kept first.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Suggestion {
    pub suggestion: MergeSuggestion,
    pub people: Vec<PersonRow>,
}

/// The busiest person is kept; a config person wins a tie, then the name.
pub(super) fn keep_order(mut involved: Vec<PersonRow>) -> Vec<PersonRow> {
    involved.sort_by(|a, b| {
        b.commits
            .cmp(&a.commits)
            .then(b.in_config.cmp(&a.in_config))
            .then_with(|| locale_cmp(&a.name, &b.name))
    });
    involved
}

/// The merges worth offering: none while merging is off (raw identities are not
/// what the config maps).
pub(super) fn suggestions(model: &ContributorsModel, rows: &[PersonRow]) -> Vec<Suggestion> {
    if !model.merge {
        return vec![];
    }
    let by_id: HashMap<&str, &PersonRow> = rows.iter().map(|r| (r.id.as_str(), r)).collect();
    let people: Vec<SuggestPerson> = model
        .resolver
        .people()
        .iter()
        .filter(|p| by_id.contains_key(p.id.as_str()) && !p.identities.is_empty())
        .map(|p| SuggestPerson {
            id: p.id.clone(),
            name: p.name.clone(),
            identities: p.identities.clone(),
            observed: p.observed.clone(),
        })
        .collect();
    suggest_merges_with(&people, &model.config.dismissed_merges, &model.config)
        .into_iter()
        .filter_map(|suggestion| {
            let involved: Vec<PersonRow> = suggestion
                .person_ids
                .iter()
                .filter_map(|id| by_id.get(id.as_str()).map(|row| (*row).clone()))
                .collect();
            (involved.len() >= 2).then(|| Suggestion {
                suggestion,
                people: keep_order(involved),
            })
        })
        .collect()
}

// ---- what each action writes ---------------------------------------------------

/// `None` when the edit changes nothing, so nothing is saved.
fn changed(config: &ContributorsConfig, next: ContributorsConfig) -> Option<ContributorsConfig> {
    (next != *config).then_some(next)
}

/// A blank or unchanged name writes nothing; a person that only exists through
/// observation becomes a config person.
pub(super) fn rename(
    config: &ContributorsConfig,
    row: &PersonRow,
    name: &str,
) -> Option<ContributorsConfig> {
    let name = name.trim();
    if name.is_empty() || name == row.name {
        return None;
    }
    changed(
        config,
        rename_person(&ensure_config_person(config, row), &row.id, name),
    )
}

/// Moves one identity to `target`. The source is listed too, so the rest of a
/// mailmap group stays with it instead of following the identity.
pub(super) fn move_identity(
    config: &ContributorsConfig,
    rows: &[PersonRow],
    key: &str,
    target: &PersonRow,
) -> Option<ContributorsConfig> {
    if target.identities.iter().any(|i| i.key == key) {
        return None;
    }
    let source = rows
        .iter()
        .find(|row| row.identities.iter().any(|i| i.key == key));
    let mut next = ensure_config_person(config, target);
    if let Some(source) = source {
        next = ensure_config_person(&next, source);
    }
    changed(
        config,
        move_person_identity(&next, key, Some(&target.id), None),
    )
}

/// Merges `others` into `target`, which keeps its name.
pub(super) fn merge_into(
    config: &ContributorsConfig,
    target: &PersonRow,
    others: &[PersonRow],
) -> ContributorsConfig {
    let base = others
        .iter()
        .fold(ensure_config_person(config, target), |next, other| {
            ensure_config_person(&next, other)
        });
    let ids: Vec<String> = std::iter::once(target.id.clone())
        .chain(others.iter().map(|p| p.id.clone()))
        .collect();
    merge_people(&base, &ids, Some(&target.name))
}

pub(super) fn dismiss(config: &ContributorsConfig, id: &str) -> Option<ContributorsConfig> {
    changed(config, dismiss_merge(config, id))
}

/// A person of the config file can be removed (its identities ungroup); one
/// that only exists through observation cannot.
pub(super) fn can_remove(row: &PersonRow) -> bool {
    row.in_config
}

pub(super) fn remove(config: &ContributorsConfig, row: &PersonRow) -> Option<ContributorsConfig> {
    changed(config, remove_person(config, &row.id))
}

/// The new config and the id the new person gets, so its name can take the focus.
pub(super) fn add_person(config: &ContributorsConfig) -> (ContributorsConfig, String) {
    (
        create_person(config, NEW_PERSON_NAME),
        person_id_for(config, NEW_PERSON_NAME),
    )
}

/// Agents win over exclusion, so excluding one would do nothing.
pub(super) fn exclude_identity_of(
    config: &ContributorsConfig,
    row: &PersonRow,
    key: &str,
) -> Option<ContributorsConfig> {
    if row.identities.iter().any(|i| i.key == key && i.agent) {
        return None;
    }
    changed(config, exclude_identity(config, key))
}

/// Excludes every non-agent identity of the person.
pub(super) fn exclude_person_of(
    config: &ContributorsConfig,
    row: &PersonRow,
) -> Option<ContributorsConfig> {
    let next = row
        .identities
        .iter()
        .filter(|i| !i.agent)
        .fold(config.clone(), |next, i| exclude_identity(&next, &i.key));
    changed(config, next)
}

/// An unmapped identity excluded from its own row.
pub(super) fn exclude_key(config: &ContributorsConfig, key: &str) -> Option<ContributorsConfig> {
    changed(config, exclude_identity(config, key))
}

/// Lifts the literal patterns that exclude the identity.
pub(super) fn include_again(
    config: &ContributorsConfig,
    item: &ExcludedIdentity,
) -> ContributorsConfig {
    item.patterns.iter().fold(config.clone(), |next, pattern| {
        include_identity(&next, pattern)
    })
}

// ---- copy ------------------------------------------------------------------------

/// What the Exclude person item says it will do.
pub(super) fn exclude_person_text(row: &PersonRow) -> String {
    let count = excludable(row);
    format!(
        "Excludes all {count} {} from the scores.",
        if count == 1 { "identity" } else { "identities" }
    )
}

/// How many identities of the person Exclude person would take (agents never).
pub(super) fn excludable(row: &PersonRow) -> usize {
    row.identities.iter().filter(|i| !i.agent).count()
}

pub(super) fn exclude_identity_text(agent: bool) -> &'static str {
    if agent {
        "Agents cannot be excluded."
    } else {
        "Leaves it out of the scores."
    }
}

/// Why an exclusion cannot be lifted from here; `None` when it can.
pub(super) fn excluded_text(item: &ExcludedIdentity) -> Option<String> {
    (!item.removable).then(|| {
        format!(
            "Excluded by pattern {}. Edit excludeIdentities in .monoptah/contributors.json to change this.",
            item.patterns.join(", ")
        )
    })
}

impl GithubView {
    /// The Identities section for the built model: an entity that reads the
    /// model and the lock from this view on every frame, like the formula sidebar.
    pub(super) fn identities_body(
        &mut self,
        _model: &std::sync::Arc<ContributorsModel>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let weak = cx.weak_entity();
        self.identities_ui
            .get_or_insert_with(|| cx.new(|cx| IdentitiesView::new(weak, cx)))
            .clone()
            .into_any_element()
    }
}
