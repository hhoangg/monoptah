//! How a session is presented: the app chat or the provider's own CLI in a terminal.
use crate::provider::Kind;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Surface {
    #[default]
    Chat,
    Tui,
}
impl Surface {
    pub fn id(self) -> &'static str {
        match self {
            Self::Chat => "chat",
            Self::Tui => "tui",
        }
    }
    /// Only `chat` and `tui` are valid; anything else is not a surface.
    pub fn parse(id: &str) -> Option<Self> {
        match id {
            "chat" => Some(Self::Chat),
            "tui" => Some(Self::Tui),
            _ => None,
        }
    }
    /// A stored value read back: absent or unknown means chat.
    pub fn from_stored(id: Option<&str>) -> Self {
        id.and_then(Self::parse).unwrap_or_default()
    }
    pub fn is_tui(self) -> bool {
        self == Self::Tui
    }
}

/// What a provider's interactive CLI can take from the app (tuiLaunch.ts TUI_CAPS).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TuiCaps {
    pub resume: bool,
    pub initial_prompt: bool,
    pub launch_flags: bool,
}

/// Only Claude's interactive flags are known; everyone else launches the bare CLI.
pub fn tui_caps(kind: Kind) -> TuiCaps {
    match kind {
        Kind::Claude => TuiCaps {
            resume: true,
            initial_prompt: true,
            launch_flags: true,
        },
        _ => TuiCaps::default(),
    }
}

fn is_remote(cwd: &str) -> bool {
    cwd.replace('\\', "/")
        .starts_with(crate::remote::REMOTE_PATH_PREFIX)
}

/// The surface a new session opens in: the preference, except a project on
/// another machine has no local terminal to run the CLI in.
pub fn new_session_surface(_kind: Kind, cwd: &str, preferred: Surface) -> Surface {
    if is_remote(cwd) {
        Surface::Chat
    } else {
        preferred
    }
}

/// The surface an automation run opens in: a terminal only where it can honour
/// the run, since a CLI that cannot take a first prompt would drop the prompt.
pub fn automation_surface(kind: Kind, cwd: &str, preferred: Surface) -> Surface {
    if preferred.is_tui() && tui_caps(kind).initial_prompt && !is_remote(cwd) {
        Surface::Tui
    } else {
        Surface::Chat
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_accepts_only_chat_and_tui() {
        assert_eq!(Surface::parse("chat"), Some(Surface::Chat));
        assert_eq!(Surface::parse("tui"), Some(Surface::Tui));
        for bad in ["", "TUI", "Chat", "terminal", " tui"] {
            assert_eq!(Surface::parse(bad), None, "{bad:?}");
        }
        assert_eq!(Surface::default(), Surface::Chat);
        assert_eq!(Surface::Tui.id(), "tui");
    }

    #[test]
    fn stored_values_read_unknown_and_absent_as_chat() {
        assert_eq!(Surface::from_stored(Some("tui")), Surface::Tui);
        assert_eq!(Surface::from_stored(Some("chat")), Surface::Chat);
        assert_eq!(Surface::from_stored(Some("bogus")), Surface::Chat);
        assert_eq!(Surface::from_stored(None), Surface::Chat);
    }

    #[test]
    fn serializes_lowercase() {
        assert_eq!(serde_json::to_string(&Surface::Tui).unwrap(), "\"tui\"");
        assert_eq!(
            serde_json::from_str::<Surface>("\"chat\"").unwrap(),
            Surface::Chat
        );
    }

    #[test]
    fn only_claude_has_terminal_capabilities() {
        for kind in Kind::ALL {
            let caps = tui_caps(kind);
            let all = caps.resume && caps.initial_prompt && caps.launch_flags;
            let none = !caps.resume && !caps.initial_prompt && !caps.launch_flags;
            if kind == Kind::Claude {
                assert!(all, "{kind:?}");
            } else {
                assert!(none, "{kind:?}");
            }
        }
    }

    #[test]
    fn new_sessions_follow_the_preference_except_on_another_machine() {
        let remote = "remote://env/repo";
        assert_eq!(
            new_session_surface(Kind::Claude, "/repo", Surface::Tui),
            Surface::Tui
        );
        assert_eq!(
            new_session_surface(Kind::Codex, "/repo", Surface::Tui),
            Surface::Tui
        );
        assert_eq!(
            new_session_surface(Kind::Claude, "/repo", Surface::Chat),
            Surface::Chat
        );
        assert_eq!(
            new_session_surface(Kind::Claude, remote, Surface::Tui),
            Surface::Chat
        );
    }

    #[test]
    fn automations_open_a_terminal_only_when_it_can_take_the_prompt() {
        assert_eq!(
            automation_surface(Kind::Claude, "/repo", Surface::Tui),
            Surface::Tui
        );
        assert_eq!(
            automation_surface(Kind::Claude, "/repo", Surface::Chat),
            Surface::Chat
        );
        assert_eq!(
            automation_surface(Kind::Codex, "/repo", Surface::Tui),
            Surface::Chat
        );
        assert_eq!(
            automation_surface(Kind::Claude, "remote://env/repo", Surface::Tui),
            Surface::Chat
        );
    }
}
