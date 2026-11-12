//! What Settings > Cloud reaches outside the window: the store that holds the Docker
//! accounts and project settings, Docker Hub, `gh`, and the home folder. The shell hands the
//! store over in [`super::SettingsContext`]; everything else comes from [`CloudServices`],
//! which tests replace with fakes through `cx.set_global`.
use crate::runtime::Database;
use gpui::{App, Global};
use monocode_core::{
    codespaces::{GhRunner, SystemGh},
    docker_accounts::{DockerHub, SystemDockerHub},
};
use std::{path::PathBuf, sync::Arc};

/// The native store, as Settings gets it from the shell. Equal while it is the same store.
#[derive(Clone)]
pub(crate) struct StoreHandle(pub Database);

impl std::fmt::Debug for StoreHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("StoreHandle")
    }
}
impl PartialEq for StoreHandle {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

#[cfg(test)]
struct Offline;
#[cfg(test)]
impl DockerHub for Offline {
    fn exchange(
        &self,
        _: &str,
        _: &str,
    ) -> Result<monocode_core::docker_accounts::Secret, monocode_core::docker_accounts::ExchangeError>
    {
        Err(monocode_core::docker_accounts::ExchangeError::Network(
            "offline".into(),
        ))
    }
}
#[cfg(test)]
impl GhRunner for Offline {
    fn run(
        &self,
        _: &[String],
        _: Option<&str>,
    ) -> Result<monocode_core::codespaces::GhOutput, String> {
        Err("gh is not installed".into())
    }
}

/// The outside world of the Cloud page.
#[derive(Clone)]
pub(crate) struct CloudServices {
    /// Exchanges a Docker personal access token for an access token.
    pub hub: Arc<dyn DockerHub>,
    pub gh: Arc<dyn GhRunner>,
    pub home: Option<PathBuf>,
    /// Finds a program the way a GUI app must (a short `PATH`).
    pub resolve: fn(&str) -> Option<PathBuf>,
}
impl Global for CloudServices {}

impl CloudServices {
    #[cfg_attr(test, allow(dead_code))]
    pub(crate) fn system() -> Self {
        Self {
            hub: Arc::new(SystemDockerHub),
            gh: Arc::new(SystemGh),
            home: monocode_core::accounts::home(),
            resolve: monocode_core::scm::resolve_gui_binary,
        }
    }

    /// The installed services, or the real ones (in tests, ones that reach nothing).
    pub(crate) fn get(cx: &App) -> Self {
        cx.try_global::<Self>().cloned().unwrap_or_else(|| {
            #[cfg(test)]
            {
                Self::offline()
            }
            #[cfg(not(test))]
            {
                Self::system()
            }
        })
    }

    /// Reaches nothing: the default in tests that install no fakes.
    #[cfg(test)]
    fn offline() -> Self {
        Self {
            hub: Arc::new(Offline),
            gh: Arc::new(Offline),
            home: None,
            resolve: |_| None,
        }
    }
}
