//! Runs one assessment prompt through either the Anthropic API or the local
//! `claude` CLI, and owns the AI settings file.
//!
//! The two runners share the `Assessor` trait so the job manager does not care
//! which one the user picked. Both honour a cancel callback. The AI config
//! (including an optional API key) lives in the app data dir, never in the
//! repo, and is written with the same 0600 pattern as the ClickUp token.

use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

use super::rubric;

const API_URL: &str = "https://api.anthropic.com/v1/messages";
const API_VERSION: &str = "2023-06-01";
const API_BETA: &str = "server-side-fallback-2026-07-01";
const API_TIMEOUT: Duration = Duration::from_secs(300);
const API_MAX_ATTEMPTS: u32 = 4;
const API_DEFAULT_RETRY_SECS: u64 = 10;
const API_MAX_RETRY_SECS: u64 = 120;
const MAX_TOKENS: u64 = 16000;

const CLI_TIMEOUT: Duration = Duration::from_secs(600);
const CLI_POLL: Duration = Duration::from_millis(200);

const CANCELLED: &str = "Assessment cancelled";
const DEFAULT_MODEL: &str = "claude-opus-5-5";
const RUNNER_CLAUDE_CODE: &str = "claude-code";
pub(super) const RUNNER_API: &str = "api";
const CONFIG_FILE: &str = "contributors-ai.json";

pub(crate) trait Assessor: Sync {
    /// Sends one prompt and returns the model's reply text.
    fn assess(
        &self,
        system: &str,
        user: &str,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<String, String>;

    /// `(runner, model)` recorded next to each stored assessment.
    fn label(&self) -> (&'static str, String);
}

// ---- API runner ----

pub(crate) struct ApiAssessor {
    pub api_key: String,
    pub model: String,
}

pub(crate) fn api_request_body(model: &str, system: &str, user: &str) -> Value {
    json!({
        "model": model,
        "max_tokens": MAX_TOKENS,
        "fallbacks": "default",
        "system": system,
        "messages": [{ "role": "user", "content": user }],
        "output_config": {
            "effort": "low",
            "format": { "type": "json_schema", "schema": rubric::reply_schema() }
        }
    })
}

/// Joins the `text` blocks of a Messages response. A refusal is checked before
/// `content` is read; `thinking` and `fallback` blocks are ignored.
pub(crate) fn api_reply_text(response: &Value) -> Result<String, String> {
    if response.get("stop_reason").and_then(Value::as_str) == Some("refusal") {
        return Err("The model declined to assess these tickets".into());
    }
    let text = response
        .get("content")
        .and_then(Value::as_array)
        .map(|blocks| {
            blocks
                .iter()
                .filter(|block| block.get("type").and_then(Value::as_str) == Some("text"))
                .filter_map(|block| block.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("")
        })
        .unwrap_or_default();
    if text.trim().is_empty() {
        return Err("The model returned no text".into());
    }
    Ok(text)
}

fn api_error_message(status: u16, body: &str) -> String {
    let detail = serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|value| {
            value
                .pointer("/error/message")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_default();
    match status {
        401 | 403 => "The Anthropic API key was rejected. Check it in Settings.".into(),
        _ if detail.is_empty() => format!("Anthropic API request failed ({status})"),
        _ => format!("Anthropic API request failed ({status}): {detail}"),
    }
}

fn retry_after_secs(value: Option<&str>) -> u64 {
    value
        .and_then(|raw| raw.trim().parse::<u64>().ok())
        .unwrap_or(API_DEFAULT_RETRY_SECS)
        .min(API_MAX_RETRY_SECS)
}

/// Sleeps for `secs`, waking every poll tick to honour a cancel.
fn wait_cancellable(secs: u64, cancelled: &dyn Fn() -> bool) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < deadline {
        if cancelled() {
            return Err(CANCELLED.into());
        }
        thread::sleep(CLI_POLL);
    }
    Ok(())
}

impl Assessor for ApiAssessor {
    fn assess(
        &self,
        system: &str,
        user: &str,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<String, String> {
        let payload = api_request_body(&self.model, system, user).to_string();
        let agent = ureq::AgentBuilder::new().timeout(API_TIMEOUT).build();
        let mut attempt = 0;
        loop {
            if cancelled() {
                return Err(CANCELLED.into());
            }
            attempt += 1;
            let result = agent
                .post(API_URL)
                .set("x-api-key", &self.api_key)
                .set("anthropic-version", API_VERSION)
                .set("content-type", "application/json")
                .set("anthropic-beta", API_BETA)
                .send_string(&payload);
            match result {
                Ok(response) => {
                    let value: Value = response
                        .into_string()
                        .ok()
                        .and_then(|text| serde_json::from_str(&text).ok())
                        .ok_or_else(|| "Anthropic returned an unreadable response".to_string())?;
                    return api_reply_text(&value);
                }
                Err(ureq::Error::Status(429 | 529, response)) if attempt < API_MAX_ATTEMPTS => {
                    wait_cancellable(retry_after_secs(response.header("retry-after")), cancelled)?;
                }
                Err(ureq::Error::Status(status, response)) => {
                    let text = response.into_string().unwrap_or_default();
                    return Err(api_error_message(status, &text));
                }
                Err(_) => return Err("Could not reach the Anthropic API".into()),
            }
        }
    }

    fn label(&self) -> (&'static str, String) {
        (RUNNER_API, self.model.clone())
    }
}

// ---- Claude Code CLI runner ----

pub(crate) struct ClaudeCliAssessor {
    pub binary: String,
    pub model: String,
}

pub(crate) fn cli_args(model: &str) -> Vec<String> {
    [
        "-p",
        "--output-format",
        "json",
        "--tools",
        "",
        "--setting-sources",
        "",
        "--model",
        model,
        "--no-session-persistence",
        "--strict-mcp-config",
        "--mcp-config",
        "{\"mcpServers\":{}}",
    ]
    .iter()
    .map(|arg| arg.to_string())
    .collect()
}

/// Reads the `result` field of `claude -p --output-format json`.
pub(crate) fn cli_reply_text(stdout: &str) -> Result<String, String> {
    let value: Value = serde_json::from_str(stdout.trim())
        .map_err(|_| "Claude Code returned an unreadable reply".to_string())?;
    let result = value
        .get("result")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if value.get("is_error").and_then(Value::as_bool) == Some(true) {
        return Err(if result.trim().is_empty() {
            "Claude Code reported an error".into()
        } else {
            format!("Claude Code reported an error: {}", result.trim())
        });
    }
    if result.trim().is_empty() {
        return Err("Claude Code returned no text".into());
    }
    Ok(result.to_string())
}

impl Assessor for ClaudeCliAssessor {
    fn assess(
        &self,
        system: &str,
        user: &str,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<String, String> {
        if cancelled() {
            return Err(CANCELLED.into());
        }
        // An empty scratch cwd keeps the agent away from the app's files and
        // any project CLAUDE.md; it is removed on every exit path.
        let scratch = ScratchDir::create()?;
        let mut command = Command::new(&self.binary);
        command
            .args(cli_args(&self.model))
            .args(["--system-prompt", system])
            .current_dir(&scratch.path)
            // Force subscription auth: a stray key in the environment would
            // silently bill the API instead.
            .env_remove("ANTHROPIC_API_KEY")
            .env_remove("ANTHROPIC_AUTH_TOKEN")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        // Own process group so a cancel can take down anything the CLI spawned.
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        let mut child = command
            .spawn()
            .map_err(|_| "Could not start Claude Code".to_string())?;

        let prompt = user.to_string();
        let stdin = child.stdin.take();
        let writer = thread::spawn(move || {
            if let Some(mut stdin) = stdin {
                let _ = stdin.write_all(prompt.as_bytes());
            }
        });
        let stdout = child.stdout.take();
        let stdout_reader = thread::spawn(move || read_all(stdout));
        let stderr = child.stderr.take();
        let stderr_reader = thread::spawn(move || read_all(stderr));

        let started = Instant::now();
        let outcome = loop {
            match child.try_wait() {
                Ok(Some(status)) => break Ok(status),
                Ok(None) => {}
                Err(error) => {
                    kill_tree(&mut child);
                    break Err(error.to_string());
                }
            }
            if cancelled() {
                kill_tree(&mut child);
                break Err(CANCELLED.to_string());
            }
            if started.elapsed() >= CLI_TIMEOUT {
                kill_tree(&mut child);
                break Err("Claude Code took too long to answer".to_string());
            }
            thread::sleep(CLI_POLL);
        };
        let _ = writer.join();
        let stdout = stdout_reader.join().unwrap_or_default();
        let stderr = stderr_reader.join().unwrap_or_default();

        let status = outcome?;
        if !status.success() {
            // A failed run still prints a JSON error object on stdout.
            if let Err(message) = cli_reply_text(&stdout) {
                if message.starts_with("Claude Code reported an error") {
                    return Err(message);
                }
            }
            let detail = stderr.trim();
            return Err(if detail.is_empty() {
                "Claude Code exited with an error".into()
            } else {
                format!("Claude Code exited with an error: {detail}")
            });
        }
        cli_reply_text(&stdout)
    }

    fn label(&self) -> (&'static str, String) {
        (RUNNER_CLAUDE_CODE, self.model.clone())
    }
}

/// Kills the child and, on unix, its whole process group, then reaps it.
fn kill_tree(child: &mut std::process::Child) {
    #[cfg(unix)]
    {
        // process_group(0) made the child's pid its group id.
        unsafe {
            libc::kill(-(child.id() as i32), libc::SIGKILL);
        }
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// A fresh empty directory under the system temp dir, removed on drop.
struct ScratchDir {
    path: std::path::PathBuf,
}

impl ScratchDir {
    fn create() -> Result<Self, String> {
        let path = std::env::temp_dir().join(format!("monoptah-assess-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&path)
            .map_err(|_| "Could not create a scratch folder for Claude Code".to_string())?;
        Ok(Self { path })
    }
}

impl Drop for ScratchDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn read_all<R: Read>(source: Option<R>) -> String {
    let mut text = String::new();
    if let Some(mut source) = source {
        let mut bytes = Vec::new();
        let _ = source.read_to_end(&mut bytes);
        text = String::from_utf8_lossy(&bytes).into_owned();
    }
    text
}

// ---- AI config ----

// No Debug derive: it would print the API key.
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct AiConfig {
    /// `"claude-code"` or `"api"`.
    pub runner: String,
    pub model: String,
    pub api_key: Option<String>,
}

impl Default for AiConfig {
    fn default() -> Self {
        Self {
            runner: RUNNER_CLAUDE_CODE.to_string(),
            model: DEFAULT_MODEL.to_string(),
            api_key: None,
        }
    }
}

/// What the UI may see: the key itself never leaves the backend.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AiConfigStatus {
    pub runner: String,
    pub model: String,
    pub has_api_key: bool,
}

impl AiConfig {
    pub(crate) fn status(&self) -> AiConfigStatus {
        AiConfigStatus {
            runner: self.runner.clone(),
            model: self.model.clone(),
            has_api_key: self.api_key.is_some(),
        }
    }
}

/// Parses the config file, trimming fields and filling blanks with defaults.
fn parse_ai_config(raw: &str) -> Result<AiConfig, String> {
    let mut config: AiConfig = serde_json::from_str(raw)
        .map_err(|_| "The assessment AI settings are invalid".to_string())?;
    normalize(&mut config);
    Ok(config)
}

fn normalize(config: &mut AiConfig) {
    let defaults = AiConfig::default();
    config.runner = config.runner.trim().to_string();
    if config.runner != RUNNER_API && config.runner != RUNNER_CLAUDE_CODE {
        config.runner = defaults.runner;
    }
    config.model = config.model.trim().to_string();
    if config.model.is_empty() {
        config.model = defaults.model;
    }
    config.api_key = config
        .api_key
        .take()
        .map(|key| key.trim().to_string())
        .filter(|key| !key.is_empty());
}

fn ai_config_path(data_dir: &Path) -> PathBuf {
    data_dir.join(CONFIG_FILE)
}

/// Reads `contributors-ai.json` of `data_dir`; defaults when there is none.
pub(crate) fn read_ai_config(data_dir: &Path) -> Result<AiConfig, String> {
    match std::fs::read_to_string(ai_config_path(data_dir)) {
        Ok(raw) => parse_ai_config(&raw),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(AiConfig::default()),
        Err(error) => Err(error.to_string()),
    }
}

pub(crate) fn write_ai_config(data_dir: &Path, config: &AiConfig) -> Result<(), String> {
    let mut config = config.clone();
    normalize(&mut config);
    let path = ai_config_path(data_dir);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let value = serde_json::to_string(&config).map_err(|error| error.to_string())?;
    crate::inbox::secret_file::write_secret_file(&path, &value)
}

pub(crate) fn assessor_for(config: &AiConfig) -> Result<Box<dyn Assessor>, String> {
    if config.runner == RUNNER_API {
        let api_key = config
            .api_key
            .as_deref()
            .map(str::trim)
            .filter(|key| !key.is_empty())
            .ok_or_else(|| {
                "Add an Anthropic API key in Settings to assess with the API.".to_string()
            })?;
        return Ok(Box::new(ApiAssessor {
            api_key: api_key.to_string(),
            model: config.model.clone(),
        }));
    }
    let binary = crate::provider::resolve(crate::provider::Kind::Claude)
        .ok_or_else(|| {
            "Claude Code CLI not found. Install it from https://claude.com/product/claude-code and run `claude auth login`, then retry."
                .to_string()
        })?
        .to_string_lossy()
        .into_owned();
    Ok(Box::new(ClaudeCliAssessor {
        binary,
        model: config.model.clone(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn api_request_body_has_schema_effort_and_fallbacks() {
        let body = api_request_body("claude-opus-5-5", "sys", "usr");
        assert_eq!(body["model"], "claude-opus-5-5");
        assert_eq!(body["max_tokens"], 16000);
        assert_eq!(body["output_config"]["effort"], "low");
        assert_eq!(body["output_config"]["format"]["type"], "json_schema");
        assert_eq!(
            body["output_config"]["format"]["schema"],
            rubric::reply_schema()
        );
        assert_eq!(body["fallbacks"], "default");
        assert_eq!(body["system"], "sys");
        assert_eq!(body["messages"][0]["content"], "usr");
        assert!(body.get("thinking").is_none());
        assert!(body.get("temperature").is_none());
    }

    #[test]
    fn api_reply_text_rejects_refusal() {
        let response = json!({
            "stop_reason": "refusal",
            "content": [{ "type": "text", "text": "{}" }]
        });
        assert!(api_reply_text(&response).is_err());
    }

    #[test]
    fn api_reply_text_skips_non_text_blocks() {
        let response = json!({
            "stop_reason": "end_turn",
            "content": [
                { "type": "thinking", "thinking": "hmm" },
                { "type": "fallback", "model": "other" },
                { "type": "text", "text": "{\"a\":" },
                { "type": "text", "text": "1}" }
            ]
        });
        assert_eq!(api_reply_text(&response).unwrap(), "{\"a\":1}");
        let empty = json!({ "stop_reason": "end_turn", "content": [] });
        assert!(api_reply_text(&empty).is_err());
    }

    #[test]
    fn cli_args_pin_model_and_disable_mcp() {
        let args = cli_args("claude-haiku-5-5");
        let at = |flag: &str| args.iter().position(|arg| arg == flag).unwrap();
        assert_eq!(args[at("--model") + 1], "claude-haiku-5-5");
        assert_eq!(args[at("--output-format") + 1], "json");
        assert_eq!(args[at("--mcp-config") + 1], "{\"mcpServers\":{}}");
        assert_eq!(args[at("--tools") + 1], "");
        assert_eq!(args[at("--setting-sources") + 1], "");
        assert!(args.contains(&"-p".to_string()));
        assert!(args.contains(&"--strict-mcp-config".to_string()));
        assert!(args.contains(&"--no-session-persistence".to_string()));
    }

    #[test]
    fn cli_reply_text_reads_result_field() {
        let stdout = r#"{"type":"result","is_error":false,"result":"{\"t1\":1}"}"#;
        assert_eq!(cli_reply_text(stdout).unwrap(), "{\"t1\":1}");
        assert!(cli_reply_text("not json").is_err());
    }

    #[test]
    fn cli_reply_text_errors_on_is_error_true() {
        let stdout = r#"{"type":"result","is_error":true,"result":"Not logged in"}"#;
        let error = cli_reply_text(stdout).unwrap_err();
        assert!(error.contains("Not logged in"));
    }

    #[test]
    fn assessor_for_api_without_key_explains_what_to_do() {
        let config = AiConfig {
            runner: "api".into(),
            model: DEFAULT_MODEL.into(),
            api_key: None,
        };
        let error = assessor_for(&config).err().unwrap();
        assert_eq!(
            error,
            "Add an Anthropic API key in Settings to assess with the API."
        );
        let blank = AiConfig {
            api_key: Some("  ".into()),
            ..config
        };
        assert!(assessor_for(&blank).is_err());
    }

    #[test]
    fn ai_config_defaults_to_claude_code_and_opus() {
        let config = AiConfig::default();
        assert_eq!(config.runner, "claude-code");
        assert_eq!(config.model, "claude-opus-5-5");
        assert!(config.api_key.is_none());
        let parsed = parse_ai_config("{}").unwrap();
        assert_eq!(parsed.runner, "claude-code");
        assert_eq!(parsed.model, "claude-opus-5-5");
        let partial = parse_ai_config(r#"{"runner":"api","apiKey":" k ","model":""}"#).unwrap();
        assert_eq!(partial.runner, "api");
        assert_eq!(partial.model, "claude-opus-5-5");
        assert_eq!(partial.api_key.as_deref(), Some("k"));
        assert!(partial.status().has_api_key);
    }

    #[test]
    fn retry_after_defaults_and_caps() {
        assert_eq!(retry_after_secs(None), 10);
        assert_eq!(retry_after_secs(Some("3")), 3);
        assert_eq!(retry_after_secs(Some("soon")), 10);
        assert_eq!(retry_after_secs(Some("9999")), 120);
    }
}
