//! The size of a Docker sandbox: pairing a lone `cpus` or `memory` with its shape, and raising
//! a saved size that is smaller than the devcontainer asks for (`hostRequirements`).
//!
//! `sbx` itself ignores `hostRequirements` (only Codespaces reads it), so a sandbox smaller than
//! the devcontainer needs is created without complaint and then runs out of memory while it
//! builds. Pairing a lone `cpus` or `memory` with its shape keeps a request from failing at
//! Send ("not a billable shape").
//!
//! This is for a start that names no size of its own (the Mono API). The composer and Cloud
//! settings keep every size selectable and only warn when one is below the devcontainer.
//! The shapes come from [`crate::sandboxes::SHAPES`], the requirement from
//! [`super::project_cloud::devcontainer_info`] (a small local file read: no `sbx` call, no
//! network, nothing is woken) and the memory units from [`super::docker_config::memory_mib`].
use super::docker_config::{memory_mib, DockerProjectConfig};
use super::project_cloud::DevcontainerInfo;
use crate::sandboxes::{is_shape, shapes_text, SHAPES};

/// The largest shape a start raises a saved size to by itself: 8 CPUs / 16g. A devcontainer
/// that asks for more never bills a bigger sandbox unasked: the start keeps the saved size,
/// says so, and a bigger one is given with `cpus` / `memory`.
pub const AUTO_RAISE_CAP: (u32, &str) = (8, "16g");

/// The size a start uses, and why it differs from the saved one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SizeFit {
    pub cpus: u32,
    pub memory: String,
    /// Set when the size is not the saved one, or the saved one falls short of the
    /// devcontainer and was not raised: a sentence for the caller.
    pub note: Option<String>,
}

/// The shapes as `(cpus, memory, memory in MiB)`, smallest first.
fn shapes() -> impl Iterator<Item = (u32, &'static str, u64)> {
    SHAPES
        .iter()
        .map(|(c, m)| (u32::from(*c), *m, memory_mib(m).unwrap_or(0)))
}

/// The smallest shape with at least `cpus` and `memory` (never a smaller one), the largest
/// when none has that much.
fn shape_at_least(cpus: u32, memory: &str) -> (u32, String) {
    let mib = memory_mib(memory).unwrap_or(0);
    shapes()
        .find(|(c, _, m)| *c >= cpus && *m >= mib)
        .or_else(|| shapes().last())
        .map_or((2, "4g".to_owned()), |(c, m, _)| (c, m.to_owned()))
}

fn meets(info: &DevcontainerInfo, cpus: u32, mib: u64) -> bool {
    info.host_cpus.is_none_or(|need| cpus >= need)
        && info.host_memory_mib.is_none_or(|need| mib >= need)
}

fn label(cpus: u32, memory: &str) -> String {
    format!("{cpus} CPU / {memory}")
}

/// The size a start with no `cpus` / `memory` of its own uses, from the saved one:
///
/// * a saved size that is not a shape is rounded UP to the smallest shape with at least its
///   CPUs and memory (never down), with a note;
/// * a size below `info` (the devcontainer's `hostRequirements`) is raised to the smallest
///   shape that meets it, but only up to [`AUTO_RAISE_CAP`]: a requirement beyond that keeps
///   the size and says it falls short;
/// * anything else is kept.
pub fn fit_size(cpus: u32, memory: &str, info: &DevcontainerInfo) -> SizeFit {
    let memory = memory.trim().to_ascii_lowercase();
    let mut notes = Vec::new();
    let (mut cpus_now, mut memory_now) = if is_shape(cpus, &memory) {
        (cpus, memory.clone())
    } else {
        let shape = shape_at_least(cpus, &memory);
        notes.push(format!(
            "{} is not a size Docker Sandboxes offers; using {}.",
            label(cpus, &memory),
            label(shape.0, &shape.1)
        ));
        shape
    };
    if let Some(asks) = info.asks_for() {
        let mib = memory_mib(&memory_now).unwrap_or(0);
        if !meets(info, cpus_now, mib) {
            let cap_mib = memory_mib(AUTO_RAISE_CAP.1).unwrap_or(0);
            let target = shapes().find(|(c, _, m)| {
                meets(info, *c, *m) && *c <= AUTO_RAISE_CAP.0 && *m <= cap_mib && *c >= cpus_now
            });
            match target {
                Some((c, m, _)) => {
                    notes.push(format!(
                        "The devcontainer asks for {asks}; raised {} to {} for this start.",
                        label(cpus_now, &memory_now),
                        label(c, m)
                    ));
                    (cpus_now, memory_now) = (c, m.to_owned());
                }
                None => notes.push(format!(
                    "The devcontainer asks for {asks}, more than a start raises a size to by itself ({}); using {}. Give cpus and memory for a larger one.",
                    label(AUTO_RAISE_CAP.0, AUTO_RAISE_CAP.1),
                    label(cpus_now, &memory_now)
                )),
            }
        }
    }
    SizeFit {
        cpus: cpus_now,
        memory: memory_now,
        note: (!notes.is_empty()).then(|| notes.join(" ")),
    }
}

/// The size of a start that may name `cpus` and/or `memory`. Only one named: the other is the
/// matching shape's, so `cpus: 4` alone means 4 CPUs / 8g (not 4 CPUs with the saved 4g, which
/// `sbx` refuses). Both named: kept as they are, and refused before anything is created when
/// they are not a shape. Neither: the saved size through [`fit_size`].
pub fn requested_size(
    saved: &DockerProjectConfig,
    cpus: Option<u32>,
    memory: Option<&str>,
    info: &DevcontainerInfo,
) -> Result<SizeFit, String> {
    let memory = memory.map(|m| m.trim().to_ascii_lowercase());
    let (cpus, memory) = match (cpus, memory) {
        (None, None) => return Ok(fit_size(saved.cpus, &saved.memory, info)),
        (Some(cpus), Some(memory)) => (cpus, memory),
        (Some(cpus), None) => match shapes().find(|(c, _, _)| *c == cpus) {
            Some((_, memory, _)) => (cpus, memory.to_owned()),
            None => {
                return Err(format!(
                    "{cpus} CPUs is not a size Docker Sandboxes offers. Choose one of: {}",
                    shapes_text()
                ))
            }
        },
        (None, Some(memory)) => match shapes().find(|(_, m, _)| *m == memory) {
            Some((cpus, ..)) => (cpus, memory),
            None => {
                return Err(format!(
                    "{memory} memory is not a size Docker Sandboxes offers. Choose one of: {}",
                    shapes_text()
                ))
            }
        },
    };
    if !is_shape(cpus, &memory) {
        return Err(format!(
            "{cpus} CPUs with {memory} memory is not a size Docker Sandboxes offers. Choose one of: {}",
            shapes_text()
        ));
    }
    Ok(SizeFit {
        cpus,
        memory,
        note: None,
    })
}
