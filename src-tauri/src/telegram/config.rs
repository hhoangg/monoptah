//! Telegram config file (bot token, group, topic bindings) and one-time connect/link codes.
//!
//! The file holds the bot token, so it is written 0600 and neither its content nor the
//! token is ever logged.
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Serialize, Deserialize, Default, Clone)]
#[serde(default)]
pub struct ProjectConfig {
    pub token: String,
    pub bot_username: String,
    /// The bot's own user id, needed to look up its rights in the group.
    pub bot_id: i64,
    pub chat_id: Option<i64>,
    pub chat_title: Option<String>,
    /// Whether the group has Topics enabled; `None` until checked.
    pub is_forum: Option<bool>,
    /// Whether the bot may create topics; `None` until checked.
    pub can_manage_topics: Option<bool>,
    pub enabled: bool,
    /// sessionId -> message_thread_id
    pub topics: BTreeMap<String, i64>,
    pub problem: Option<String>,
}

#[derive(Serialize, Deserialize, Default)]
#[serde(default)]
pub struct TelegramConfig {
    /// projectKey -> config
    pub projects: BTreeMap<String, ProjectConfig>,
}

pub fn config_path(app_data: &Path) -> PathBuf {
    app_data.join("telegram-config.json")
}

/// Missing file or invalid content yields the default config. An invalid file is logged
/// with a generic message only, because its content includes the bot token.
pub fn load(path: &Path) -> TelegramConfig {
    let raw = match std::fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(error) => {
            if error.kind() != std::io::ErrorKind::NotFound {
                eprintln!("monoptah: telegram config could not be read");
            }
            return TelegramConfig::default();
        }
    };
    match serde_json::from_str(&raw) {
        Ok(cfg) => cfg,
        Err(_) => {
            eprintln!("monoptah: telegram config is invalid, using defaults");
            TelegramConfig::default()
        }
    }
}

/// Writes the config through the shared 0600 secret-file writer.
pub fn save(path: &Path, cfg: &TelegramConfig) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let value = serde_json::to_string(cfg).map_err(|error| error.to_string())?;
    crate::clickup::write_secret_file(path, &value)
}

/// How long a connect/link code stays valid.
const CODE_TTL_MS: u64 = 15 * 60 * 1000;
const CODE_LEN: usize = 8;
/// A-Z and 2-9 without I and O: 32 symbols, so the low 5 bits of a uniformly random byte
/// map onto it without modulo bias, and no symbol looks like 0, 1 or another letter.
const CODE_ALPHABET: &[u8; 32] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";

#[derive(Clone, Debug, PartialEq)]
pub enum CodePurpose {
    Connect { project: String },
    Link { project: String, session_id: String },
}

/// One-time codes, kept in memory only. A code is valid for 15 minutes and works once.
#[derive(Default)]
pub struct Codes {
    pending: BTreeMap<String, (CodePurpose, u64)>,
}

impl Codes {
    /// A new Connect code replaces the live one of the same project, so only the code
    /// shown last in the dialog can connect it.
    pub fn issue(&mut self, purpose: CodePurpose, now_ms: u64) -> String {
        self.prune(now_ms);
        if let CodePurpose::Connect { project } = &purpose {
            self.pending.retain(|_, (existing, _)| {
                !matches!(existing, CodePurpose::Connect { project: other } if other == project)
            });
        }
        loop {
            let code = random_code();
            if !self.pending.contains_key(&code) {
                self.pending
                    .insert(code.clone(), (purpose, now_ms.saturating_add(CODE_TTL_MS)));
                return code;
            }
        }
    }

    /// Consumes the code: a second call with the same code, or a call after expiry,
    /// returns `None`. Input is trimmed and upper-cased so a typed code still matches.
    pub fn take(&mut self, code: &str, now_ms: u64) -> Option<CodePurpose> {
        let (purpose, expires_at) = self.pending.remove(&code.trim().to_ascii_uppercase())?;
        (now_ms <= expires_at).then_some(purpose)
    }

    fn prune(&mut self, now_ms: u64) {
        self.pending
            .retain(|_, (_, expires_at)| now_ms <= *expires_at);
    }
}

fn random_code() -> String {
    // Bytes 6 and 8 of a v4 UUID carry the fixed version and variant bits, so their low
    // 5 bits are not uniform (byte 6 is always 0x4X). Skip them; the other 14 bytes are
    // fully random.
    uuid::Uuid::new_v4()
        .into_bytes()
        .iter()
        .enumerate()
        .filter(|(index, _)| *index != 6 && *index != 8)
        .map(|(_, byte)| CODE_ALPHABET[(byte & 0x1f) as usize] as char)
        .take(CODE_LEN)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIN_MS: u64 = 60_000;

    fn temp_dir() -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("monoptah-telegram-config-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn sample() -> TelegramConfig {
        let mut project = ProjectConfig {
            token: "123:secret".into(),
            bot_username: "my_bot".into(),
            chat_id: Some(-100123),
            chat_title: Some("Team".into()),
            enabled: true,
            problem: Some("No Manage topics right".into()),
            ..ProjectConfig::default()
        };
        project.topics.insert("session-1".into(), 42);
        let mut cfg = TelegramConfig::default();
        cfg.projects.insert("proj".into(), project);
        cfg
    }

    #[test]
    fn round_trips_and_is_0600() {
        let dir = temp_dir();
        let path = config_path(&dir);
        assert_eq!(path, dir.join("telegram-config.json"));
        save(&path, &sample()).unwrap();

        let loaded = load(&path);
        let project = &loaded.projects["proj"];
        assert_eq!(project.token, "123:secret");
        assert_eq!(project.bot_username, "my_bot");
        assert_eq!(project.chat_id, Some(-100123));
        assert_eq!(project.chat_title.as_deref(), Some("Team"));
        assert!(project.enabled);
        assert_eq!(project.topics["session-1"], 42);
        assert_eq!(project.problem.as_deref(), Some("No Manage topics right"));

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn invalid_file_loads_default() {
        let dir = temp_dir();
        let path = config_path(&dir);
        assert!(load(&path).projects.is_empty(), "missing file");
        std::fs::write(&path, "{ not json 123:secret").unwrap();
        assert!(load(&path).projects.is_empty(), "invalid file");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn file_missing_newer_fields_keeps_token() {
        let dir = temp_dir();
        let path = config_path(&dir);
        std::fs::write(
            &path,
            r#"{"projects":{"proj":{"token":"123:secret","bot_username":"my_bot"}}}"#,
        )
        .unwrap();
        let loaded = load(&path);
        let project = &loaded.projects["proj"];
        assert_eq!(project.token, "123:secret");
        assert!(!project.enabled);
        assert!(project.topics.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn connect() -> CodePurpose {
        CodePurpose::Connect {
            project: "proj".into(),
        }
    }

    #[test]
    fn code_single_use() {
        let mut codes = Codes::default();
        let link = CodePurpose::Link {
            project: "proj".into(),
            session_id: "s1".into(),
        };
        let code = codes.issue(link.clone(), 1_000);
        assert_eq!(codes.take("WRONG234", 2_000), None);
        assert_eq!(codes.take(&code, 2_000), Some(link));
        assert_eq!(codes.take(&code, 2_000), None);
    }

    #[test]
    fn new_connect_code_revokes_the_old_one_for_that_project() {
        let mut codes = Codes::default();
        let old = codes.issue(connect(), 1_000);
        let other_project = codes.issue(
            CodePurpose::Connect {
                project: "other".into(),
            },
            1_000,
        );
        let link = codes.issue(
            CodePurpose::Link {
                project: "proj".into(),
                session_id: "s1".into(),
            },
            1_000,
        );
        let fresh = codes.issue(connect(), 2_000);
        assert_eq!(codes.take(&old, 3_000), None, "old code must be revoked");
        assert_eq!(codes.take(&fresh, 3_000), Some(connect()));
        // Other projects and link codes are not affected.
        assert!(codes.take(&other_project, 3_000).is_some());
        assert!(codes.take(&link, 3_000).is_some());
    }

    #[test]
    fn code_expires_after_15_min() {
        let mut codes = Codes::default();
        let code = codes.issue(connect(), 1_000);
        assert_eq!(codes.take(&code, 1_000 + 15 * MIN_MS + 1), None);

        let code = codes.issue(connect(), 1_000);
        assert_eq!(codes.take(&code, 1_000 + 15 * MIN_MS), Some(connect()));
    }

    #[test]
    fn codes_are_unambiguous_chars() {
        let mut codes = Codes::default();
        for _ in 0..500 {
            let code = codes.issue(connect(), 0);
            assert_eq!(code.len(), 8);
            assert!(
                code.chars()
                    .all(|c| (c.is_ascii_uppercase() || ('2'..='9').contains(&c))
                        && c != 'I'
                        && c != 'O'),
                "ambiguous char in {code}"
            );
        }
    }

    #[test]
    fn every_code_position_uses_the_whole_alphabet() {
        let mut codes = Codes::default();
        let mut late_half_seen = [false; CODE_LEN];
        for _ in 0..2000 {
            let code = codes.issue(connect(), 0);
            for (position, c) in code.bytes().enumerate() {
                let index = CODE_ALPHABET.iter().position(|a| *a == c).unwrap();
                if index >= CODE_ALPHABET.len() / 2 {
                    late_half_seen[position] = true;
                }
            }
        }
        assert_eq!(late_half_seen, [true; CODE_LEN]);
    }
}
