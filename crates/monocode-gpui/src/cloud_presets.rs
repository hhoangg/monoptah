//! The option lists and labels shared by the project's Cloud settings dialog and the
//! composer's RUN ON CLOUD panel, so both offer the same presets under the same names: sizes,
//! stop-when-idle and runtime limits, machine and account labels. Pure data and formatting.
use monocode_core::{
    codespaces::MachineType,
    docker_accounts::{DockerAccountStatus, DockerAccountSummary},
};

/// The Claude choice that keeps the Codespaces secret (the default for existing projects).
pub(crate) const CODESPACE_SECRET_LABEL: &str = "Codespaces secret (CLAUDE_CODE_OAUTH_TOKEN)";
/// The Claude choice of a sandbox without an account.
pub(crate) const SANDBOX_NO_CLAUDE_LABEL: &str = "None";
/// The Docker choice that keeps the machine's `sbx login`.
pub(crate) const MACHINE_SIGN_IN_LABEL: &str = "Machine sign-in (sbx login)";

/// A Claude account a cloud session can use: one that has a setup-token or an API key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AccountChoice {
    pub id: String,
    pub label: String,
    /// `setup-token` or `API key`.
    pub kind: &'static str,
}

/// The sizes the cloud offers, `(cpus, memory)`.
pub(crate) const SIZES: [(u32, &str); 5] =
    [(1, "2g"), (2, "4g"), (4, "8g"), (8, "16g"), (16, "32g")];
pub(crate) const IDLE_MINUTES: [u32; 4] = [30, 60, 120, 240];
pub(crate) const RUNTIMES: [&str; 4] = ["1h", "2h", "4h", "8h"];

/// `4g` or `512m` as MiB.
pub(crate) fn memory_mib(memory: &str) -> Option<u64> {
    let memory = memory.trim().to_ascii_lowercase();
    if let Some(g) = memory.strip_suffix('g') {
        return g.parse::<u64>().ok().map(|g| g * 1024);
    }
    memory.strip_suffix('m')?.parse().ok()
}

/// `16384` MiB as `16 GB`.
pub(crate) fn gb_text(mib: u64) -> String {
    if mib >= 1024 && mib.is_multiple_of(1024) {
        format!("{} GB", mib / 1024)
    } else if mib >= 1024 {
        format!("{:.1} GB", mib as f64 / 1024.0)
    } else {
        format!("{mib} MB")
    }
}

pub(crate) fn plural(count: u32, unit: &str) -> String {
    if count == 1 {
        format!("{count} {unit}")
    } else {
        format!("{count} {unit}s")
    }
}

pub(crate) fn size_label(cpus: u32, memory: &str) -> String {
    let memory = memory_mib(memory).map_or_else(|| memory.to_owned(), gb_text);
    format!("{}, {memory}", plural(cpus, "CPU"))
}

pub(crate) fn idle_label(minutes: u32) -> String {
    let span = match minutes {
        m if m >= 60 && m % 60 == 0 => plural(m / 60, "hour"),
        m => plural(m, "minute"),
    };
    format!("After {span}")
}

pub(crate) fn runtime_label(ttl: &str) -> String {
    let long = match ttl.strip_suffix('h').and_then(|n| n.parse::<u32>().ok()) {
        Some(hours) => plural(hours, "hour"),
        None => match ttl.strip_suffix('m').and_then(|n| n.parse::<u32>().ok()) {
            Some(minutes) => plural(minutes, "minute"),
            None => ttl.to_owned(),
        },
    };
    format!("{long}, then stop")
}

/// The sizes to offer: the presets, plus the current value when it is none of them.
pub(crate) fn size_options(cpus: u32, memory: &str) -> Vec<(u32, String)> {
    let mut options: Vec<(u32, String)> =
        SIZES.iter().map(|(c, m)| (*c, (*m).to_owned())).collect();
    if !options.iter().any(|(c, m)| *c == cpus && m == memory) {
        options.push((cpus, memory.to_owned()));
    }
    options
}

pub(crate) fn idle_options(current: u32) -> Vec<u32> {
    let mut options = IDLE_MINUTES.to_vec();
    if !options.contains(&current) {
        options.push(current);
    }
    options
}

pub(crate) fn runtime_options(current: &str) -> Vec<String> {
    let mut options: Vec<String> = RUNTIMES.iter().map(|r| (*r).to_owned()).collect();
    if !options.iter().any(|r| r == current) {
        options.push(current.to_owned());
    }
    options
}

pub(crate) fn machine_label(machine: &MachineType) -> String {
    if machine.display_name.is_empty() {
        machine.name.clone()
    } else {
        machine.display_name.clone()
    }
}

/// A Docker account as the select shows it.
pub(crate) fn docker_account_label(account: &DockerAccountSummary) -> String {
    match account.status {
        DockerAccountStatus::Revoked => format!("{} (token needs replacing)", account.docker_id),
        DockerAccountStatus::MissingScope => {
            format!("{} (needs Cloud Sandboxes access)", account.docker_id)
        }
        DockerAccountStatus::Valid | DockerAccountStatus::Unchecked => account.docker_id.clone(),
    }
}
