//! The host's side of the web terminal: the `terminal.*` methods, who may
//! hold a shell, and the desktop's own controls over it (the per-device
//! toggle, the Kill button). The shells, codes and limits are in
//! [`super::terminal`].
use super::terminal::{self, Owner, Reason, Standing};
use super::*;
use base64::engine::general_purpose::STANDARD;
use std::path::{Component, Path};
use std::sync::Weak;

/// Whether a device row may hold a shell right now: a web device with
/// Control and its toggle on, while the web link is on. Anything else, a
/// missing row included, ends its shells at the next tick.
pub(super) fn terminal_rights(inner: Weak<Mutex<Inner>>) -> terminal::Rights {
    Arc::new(move |device_id| {
        let Some(inner) = inner.upgrade() else {
            return Standing::Revoked;
        };
        let Ok(inner) = inner.lock() else {
            return Standing::Revoked;
        };
        match inner
            .config
            .devices
            .iter()
            .find(|device| device.id == device_id)
        {
            Some(device)
                if device.kind == DeviceKind::Web
                    && device.permission == Permission::Control
                    && inner.config.web_enabled =>
            {
                if policy::terminal_allowed(&device.id) {
                    Standing::Allowed
                } else {
                    Standing::ToggleOff
                }
            }
            _ => Standing::Revoked,
        }
    })
}

/// The name of the published project `id`, if there is one.
pub(super) fn catalog_project_name(catalog: &Value, id: &str) -> Option<String> {
    catalog["projects"]
        .as_array()?
        .iter()
        .find(|project| project["id"].as_str() == Some(id))
        .map(|project| project["name"].as_str().unwrap_or(id).to_owned())
}

/// The folder a project id names: the app encodes the path as a URI component.
fn decode_project_id(id: &str) -> Option<String> {
    let bytes = id.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at] == b'%' {
            let hex = id.get(at + 1..at + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            at += 3;
        } else {
            out.push(bytes[at]);
            at += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// The folder a shell starts in: the project, or a folder inside it. A
/// relative path with no `..` that stays inside the project once symlinks are
/// resolved.
fn resolve_cwd(project_id: &str, subdir: Option<&str>) -> Result<std::path::PathBuf, Failure> {
    let missing = || Failure::from("The project folder is not available");
    let root = decode_project_id(project_id)
        .map(std::path::PathBuf::from)
        .filter(|path| path.is_absolute())
        .ok_or_else(missing)?;
    let root = root.canonicalize().map_err(|_| missing())?;
    let Some(subdir) = subdir.filter(|text| !text.trim().is_empty()) else {
        return Ok(root);
    };
    let relative = Path::new(subdir);
    let plain = relative
        .components()
        .all(|part| matches!(part, Component::Normal(_) | Component::CurDir))
        && !subdir.contains('\0');
    if !plain {
        return Err(Failure::from("The folder must be inside the project"));
    }
    let target = root
        .join(relative)
        .canonicalize()
        .map_err(|_| Failure::from("That folder does not exist in the project"))?;
    if !target.starts_with(&root) || !target.is_dir() {
        return Err(Failure::from("The folder must be inside the project"));
    }
    Ok(target)
}

fn text_param<'a>(params: &'a Value, key: &str, max: usize) -> Result<&'a str, Failure> {
    params[key]
        .as_str()
        .filter(|text| !text.is_empty() && text.len() <= max)
        .ok_or_else(|| Failure::from("Invalid mobile parameters"))
}

fn size_param(params: &Value) -> Result<(u16, u16), Failure> {
    let get = |key: &str, range: &std::ops::RangeInclusive<u16>| {
        params[key]
            .as_u64()
            .and_then(|value| u16::try_from(value).ok())
            .filter(|value| range.contains(value))
    };
    match (get("cols", &terminal::COLS), get("rows", &terminal::ROWS)) {
        (Some(cols), Some(rows)) => Ok((cols, rows)),
        _ => Err(Failure::from("Invalid terminal size")),
    }
}

impl MobileHost {
    /// This host with a terminal hub that has the given limits (tests).
    #[cfg(test)]
    pub(super) fn with_terminal_limits(mut self, limits: terminal::Limits) -> Self {
        let hub = terminal::Hub::new(Arc::new(terminal::SystemClock::default()), limits);
        hub.set_rights(terminal_rights(Arc::downgrade(&self.inner)));
        self.terminals = hub;
        self
    }

    /// The web terminal's hub.
    pub(crate) fn terminals(&self) -> &Arc<terminal::Hub> {
        &self.terminals
    }

    /// Gives the app's shell backend to the web terminal. Without one the
    /// desktop answers `unsupported` to `terminal.open`.
    pub fn set_shell_spawner(&self, spawner: Arc<dyn terminal::ShellSpawner>) {
        self.terminals.set_spawner(spawner);
    }

    /// Open shells and the codes waiting to be read out, for the indicator.
    pub fn terminal_status(&self) -> terminal::TerminalStatus {
        self.terminals.status()
    }

    /// The Kill button: ends one shell and its process group.
    pub fn terminal_kill(&self, id: &str) -> bool {
        self.terminals.kill(id)
    }

    /// Ends every shell (the user's "kill all").
    pub fn terminal_kill_all(&self) -> usize {
        self.terminals.kill_all(Reason::Killed)
    }

    /// The desktop's "Allow web terminal" switch for one web device. Only the
    /// desktop's settings page calls this; there is no wire path to it, and the
    /// grant sync never touches it. Turning it on needs a device with Control.
    /// A save that fails leaves the switch off.
    pub fn web_set_terminal_allowed(&self, device_id: &str, on: bool) -> Result<(), String> {
        let device = {
            let inner = self.inner.lock().map_err(unavailable)?;
            inner
                .config
                .devices
                .iter()
                .find(|device| device.id == device_id && device.kind == DeviceKind::Web)
                .cloned()
                .ok_or("Unknown web device")?
        };
        if on && device.permission != Permission::Control {
            return Err("Only a device with control can open a terminal".into());
        }
        policy::set_terminal_allowed(device_id, on);
        if !on {
            self.terminals.kill_device(device_id, Reason::ToggleOff);
        }
        let saved = self
            .path
            .parent()
            .map(|dir| policy::persist_settings(&dir.join(policy::SETTINGS_FILE)))
            .unwrap_or_else(|| Err("The settings folder is unavailable".into()));
        if let Err(error) = saved {
            // Fail closed: what could not be saved is not left on.
            policy::set_terminal_allowed(device_id, false);
            self.terminals.kill_device(device_id, Reason::ToggleOff);
            self.web.command(LinkCommand::Resend {
                fingerprint: device.fingerprint.clone(),
            });
            return Err(error);
        }
        audit::record_lifecycle(
            audit::Lifecycle::TerminalToggle,
            &device.id,
            &device.fingerprint,
            Transport::Web,
            Some(if on { "on" } else { "off" }),
        );
        self.web.command(LinkCommand::Resend {
            fingerprint: device.fingerprint,
        });
        Ok(())
    }

    /// Forgets a removed device's terminal toggle and ends its shells.
    pub(super) fn terminal_forget_device(&self, device: &Device) {
        self.terminals.kill_device(&device.id, Reason::Revoked);
        self.terminal_clear_toggle(&device.id);
    }

    /// Switches a device's "Allow web terminal" off and saves that. Used when
    /// the device loses Control: raising it again must not bring the shell
    /// back without the desktop user turning the toggle on once more.
    pub(super) fn terminal_clear_toggle(&self, device_id: &str) {
        if policy::terminal_allowed(device_id) {
            policy::set_terminal_allowed(device_id, false);
            if let Some(dir) = self.path.parent() {
                if let Err(error) = policy::persist_settings(&dir.join(policy::SETTINGS_FILE)) {
                    eprintln!("Could not save the web terminal switch for a device: {error}");
                }
            }
        }
    }

    /// A `terminal.*` call of a web device that policy already admitted.
    pub(super) async fn terminal_call(
        &self,
        device: &Device,
        method: &str,
        params: &Value,
        project_name: Option<String>,
    ) -> Result<Value, Failure> {
        let owner = Owner {
            device_id: device.id.clone(),
            fingerprint: device.fingerprint.clone(),
            name: device.name.clone(),
            relay_id: device.server_device_id.clone(),
        };
        let hub = &self.terminals;
        match method {
            "terminal.open" => {
                let project = text_param(params, "projectId", 2048)?;
                let name = project_name.ok_or_else(|| policy::Code::NotFound.failure())?;
                let subdir = match &params["subdir"] {
                    Value::Null => None,
                    Value::String(text) if text.len() <= 512 => Some(text.as_str()),
                    _ => return Err(Failure::from("Invalid mobile parameters")),
                };
                let (cols, rows) = size_param(params)?;
                let label = match subdir.filter(|text| !text.trim().is_empty()) {
                    Some(sub) => format!("{name}/{}", sub.trim_matches('/')),
                    None => name,
                };
                hub.start_ticker();
                if params["challengeId"].is_null() && params["code"].is_null() {
                    return Ok(hub.challenge(&owner, project, &label)?.to_value());
                }
                let challenge_id = text_param(params, "challengeId", 128)?;
                let code = text_param(params, "code", 64)?;
                let cwd = resolve_cwd(project, subdir)?;
                let request = terminal::OpenRequest {
                    project: project.to_owned(),
                    label,
                    cwd,
                    cols,
                    rows,
                    challenge_id: challenge_id.to_owned(),
                    code: code.to_owned(),
                };
                // Starting the pty blocks: keep it off the async workers.
                let hub = hub.clone();
                let opened = tokio::task::spawn_blocking(move || hub.open(&owner, request))
                    .await
                    .map_err(|_| Failure::from("The terminal could not be opened"))??;
                Ok(opened.to_value())
            }
            "terminal.read" => {
                let id = text_param(params, "terminalId", 128)?;
                let from = params["from"].as_u64().unwrap_or(0);
                let wait = params["waitMs"].as_u64().unwrap_or(15_000);
                let max = params["maxBytes"]
                    .as_u64()
                    .map_or(terminal::MAX_READ_BYTES, |max| {
                        usize::try_from(max).unwrap_or(usize::MAX)
                    });
                Ok(hub.read(&owner, id, from, wait, max).await?.to_value())
            }
            "terminal.write" => {
                use base64::Engine as _;
                let id = text_param(params, "terminalId", 128)?;
                let text = params["data"]
                    .as_str()
                    .filter(|text| text.len() <= terminal::MAX_INPUT_BYTES * 2)
                    .ok_or_else(|| Failure::from("Invalid mobile parameters"))?;
                let data = STANDARD
                    .decode(text)
                    .map_err(|_| Failure::from("Invalid mobile parameters"))?;
                // A write waits up to two seconds on a full pty.
                let (hub, id) = (hub.clone(), id.to_owned());
                tokio::task::spawn_blocking(move || hub.write(&owner, &id, &data))
                    .await
                    .map_err(|_| Failure::from("The terminal could not be written to"))??;
                Ok(json!({}))
            }
            "terminal.resize" => {
                let id = text_param(params, "terminalId", 128)?;
                let (cols, rows) = size_param(params)?;
                hub.resize(&owner, id, cols, rows)?;
                Ok(json!({}))
            }
            "terminal.close" => {
                let id = text_param(params, "terminalId", 128)?;
                hub.close(&owner, id)?;
                Ok(json!({}))
            }
            _ => Err(unsupported()),
        }
    }
}

#[cfg(test)]
mod tests;
