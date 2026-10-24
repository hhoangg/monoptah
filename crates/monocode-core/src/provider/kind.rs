//! Provider identity is shared by discovery, persistence, pickers and transports.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Codex,
    Claude,
    Cursor,
    Grok,
    OpenCode,
    Antigravity,
    Pi,
    Omp,
    Fx,
    Hermes,
    Devin,
}
impl Kind {
    pub const ALL: [Self; 11] = [
        Self::Claude,
        Self::Codex,
        Self::Cursor,
        Self::Grok,
        Self::OpenCode,
        Self::Pi,
        Self::Omp,
        Self::Fx,
        Self::Hermes,
        Self::Antigravity,
        Self::Devin,
    ];
    pub fn id(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::Claude => "claude",
            Self::Cursor => "cursor",
            Self::Grok => "grok",
            Self::OpenCode => "opencode",
            Self::Antigravity => "antigravity",
            Self::Pi => "pi",
            Self::Omp => "omp",
            Self::Fx => "fx",
            Self::Hermes => "hermes",
            Self::Devin => "devin",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Codex => "Codex",
            Self::Claude => "Claude Code",
            Self::Cursor => "Cursor",
            Self::Grok => "Grok Build",
            Self::OpenCode => "OpenCode",
            Self::Antigravity => "Antigravity",
            Self::Pi => "Pi",
            Self::Omp => "omp",
            Self::Fx => "fx",
            Self::Hermes => "Hermes Agent",
            Self::Devin => "Devin",
        }
    }
    pub fn parse(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.id() == id)
    }
    pub fn from_id(id: &str) -> Self {
        Self::parse(id).unwrap_or(Self::Codex)
    }
    pub fn binary_name(self) -> &'static str {
        match self {
            Self::Cursor => "cursor-agent",
            Self::Antigravity => "agy_acp_server.par",
            _ => self.id(),
        }
    }
    pub(super) fn install_dirs(self) -> &'static [&'static str] {
        match self {
            Self::Grok => &[".grok/bin"],
            Self::OpenCode => &[".opencode/bin"],
            Self::Fx => &[".fx/bin"],
            Self::Antigravity => &[".local/share/agy-acp"],
            Self::Hermes => &[
                ".hermes/hermes-agent/venv/bin",
                ".hermes/hermes-agent/.venv/bin",
            ],
            _ => &[],
        }
    }
    pub fn is_acp(self) -> bool {
        !matches!(self, Self::Codex | Self::Claude | Self::Pi | Self::Omp)
    }
    pub fn is_pi(self) -> bool {
        matches!(self, Self::Pi | Self::Omp)
    }
    pub fn subscription_usage(self) -> bool {
        matches!(self, Self::Codex | Self::Claude)
    }
    pub fn login_hint(self) -> &'static str {
        match self {
            Self::Codex => "codex login",
            Self::Claude => "claude auth login",
            Self::Cursor => "agent login",
            Self::Grok => "grok login",
            Self::OpenCode => "opencode auth login",
            Self::Antigravity => "agy",
            Self::Pi => "pi",
            Self::Omp => "omp",
            Self::Fx => "fx login",
            Self::Hermes => "hermes model",
            Self::Devin => "devin auth login",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn registry_matches_the_react_harnesses() {
        let source = include_str!("../../../../src/integrations/harness/core/register.ts");
        for kind in Kind::ALL {
            assert_eq!(Kind::parse(kind.id()), Some(kind));
            assert!(source.contains(&format!("../providers/{}/", kind.id())));
        }
        assert_eq!(Kind::parse("unknown"), None);
        assert_eq!(Kind::Cursor.binary_name(), "cursor-agent");
    }
}
