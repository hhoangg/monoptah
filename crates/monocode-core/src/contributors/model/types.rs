//! Types of the Contributors model. The raw-data types are the ones the
//! backend produces (`contributors.rs`) and are re-exported here so the model
//! has one import path.

use super::serialize_number;
use serde::Serialize;

pub use crate::contributors::jobs::LoadResponse;
pub use crate::contributors::sync::Estimate as SyncEstimate;
pub use crate::contributors::{
    GitIdentity, RawBranchCommit, RawCommit, RawData, RawFile, RawPr, RawReview, RawSurvival,
};

/// Stable key of an identity: lowercased email, `gh:<login>`, or lowercased name.
pub type IdentityKey = String;

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Person {
    pub id: String,
    pub name: String,
    pub identities: Vec<String>,
}

/// Field order is the file's key order; do not reorder.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Weights {
    #[serde(serialize_with = "serialize_number")]
    pub delivery: f64,
    #[serde(serialize_with = "serialize_number")]
    pub review: f64,
    #[serde(serialize_with = "serialize_number")]
    pub care: f64,
    #[serde(serialize_with = "serialize_number")]
    pub consistency: f64,
    #[serde(serialize_with = "serialize_number")]
    pub breadth: f64,
}

/// The `.monoptah/contributors.json` file. Field order is the file's key
/// order; do not reorder.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContributorsConfig {
    /// Always 1.
    pub version: u8,
    /// Always `"monthly"`; only monthly is implemented, the field is reserved.
    pub season: String,
    pub weights: Weights,
    /// Globs of files that do not count.
    pub ignore: Vec<String>,
    /// Patterns on email, name or login, `*` wildcard, case-insensitive.
    pub exclude_identities: Vec<String>,
    /// Same pattern rules; agents win over exclude.
    pub agent_identities: Vec<String>,
    /// Identities are lowercased emails, `gh:<login>` or names.
    pub people: Vec<Person>,
    /// Suggestion ids the user dismissed.
    pub dismissed_merges: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GhState {
    Complete,
    Partial,
    Unavailable,
}

impl GhState {
    /// The `LoadResponse.gh_state` text.
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "complete" => Some(Self::Complete),
            "partial" => Some(Self::Partial),
            "unavailable" => Some(Self::Unavailable),
            _ => None,
        }
    }
}
