//! Launch of a provider's own interactive CLI in a terminal session. Pure: the
//! caller resolves the binary, account env and hook paths, and spawns the result.
//! Ports `tuiLaunch.ts`, `tuiBinary.ts` and the pure parts of `tuiSession.ts`.
use crate::agent::AccessMode;
use crate::provider::Kind;
use crate::surface::tui_caps;
use regex::Regex;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::Duration;

/// An exit sooner than this after launch reads as "never really started".
pub const EARLY_EXIT: Duration = Duration::from_secs(5);
const MAX_TITLE: usize = 80;
/// Hook events that tell the app a terminal turn started or ended.
const HOOK_EVENTS: [&str; 5] = [
    "UserPromptSubmit",
    "Stop",
    "StopFailure",
    "SessionEnd",
    "Notification",
];

pub struct LaunchInput {
    pub kind: Kind,
    /// Resolved CLI path (`tui_binary`), so a configured binary is honoured.
    pub binary: PathBuf,
    /// Session model in any stored form; none or `default` omits `--model`.
    pub model: Option<String>,
    pub access: AccessMode,
    /// Saved conversation id: resumed instead of starting a new conversation.
    pub provider_id: Option<String>,
    /// Id for a new conversation instead of minting one (claims a saved id the CLI never saw).
    pub new_id: Option<String>,
    /// First prompt of a new conversation only.
    pub initial_prompt: Option<String>,
    /// (this executable, spool dir) so Claude hooks report busy/idle. None = no hooks.
    pub hook: Option<(PathBuf, PathBuf)>,
    /// Windows runs a hook command through Git Bash or PowerShell and no one command is safe in both.
    pub windows: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TuiLaunch {
    pub program: OsString,
    pub args: Vec<OsString>,
    /// Set when the launch minted (or claimed) an id the caller must persist.
    pub minted_id: Option<String>,
}

fn non_blank(value: &Option<String>) -> Option<&str> {
    value.as_deref().filter(|v| !v.trim().is_empty())
}

/// Program and arguments that start the session's interactive CLI in a PTY.
/// Order: `--model`, `--permission-mode`, `--settings`, `--resume`/`--session-id`, prompt.
pub fn build(input: LaunchInput) -> TuiLaunch {
    let caps = tui_caps(input.kind);
    let mut args: Vec<OsString> = Vec::new();
    let mut minted_id = None;
    if caps.launch_flags {
        if let Some(model) = input
            .model
            .as_deref()
            .and_then(|m| crate::store::native_model(m, input.kind.id()))
        {
            args.extend(["--model".into(), model.into()]);
        }
        args.extend(["--permission-mode".into(), input.access.claude().into()]);
    }
    if input.kind == Kind::Claude && !input.windows {
        if let Some((cli, spool)) = &input.hook {
            args.extend(["--settings".into(), hook_settings_json(cli, spool).into()]);
        }
    }
    // `--resume` takes an optional value, so an id that looks like a flag would
    // be one: treat it as no id.
    let resume_id = non_blank(&input.provider_id).filter(|id| !id.trim_start().starts_with('-'));
    let resuming = caps.resume && resume_id.is_some();
    if caps.resume {
        if let Some(id) = resume_id {
            args.extend(["--resume".into(), id.into()]);
        } else {
            let id = non_blank(&input.new_id)
                .map(str::to_owned)
                .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
            args.extend(["--session-id".into(), id.clone().into()]);
            minted_id = Some(id);
        }
    }
    // A resumed conversation already has its first prompt.
    if let Some(prompt) = non_blank(&input.initial_prompt) {
        if caps.initial_prompt && !resuming {
            // `--` keeps a prompt that starts with a dash from parsing as a flag.
            if prompt.starts_with('-') {
                args.push("--".into());
            }
            args.push(prompt.into());
        }
    }
    TuiLaunch {
        program: input.binary.into_os_string(),
        args,
        minted_id,
    }
}

/// The `--settings` JSON that makes Claude call back into this app. Claude merges
/// it with the user's own hooks; `async` keeps a hook from ever slowing the CLI.
pub fn hook_settings_json(cli: &Path, spool: &Path) -> String {
    let command = format!("{} tui-hook --spool {}", shell_path(cli), shell_path(spool));
    let command = serde_json::to_string(&command).unwrap_or_default();
    let entry = format!(r#"[{{"hooks":[{{"type":"command","command":{command},"async":true}}]}}]"#);
    let hooks: Vec<String> = HOOK_EVENTS
        .iter()
        .map(|event| format!(r#""{event}":{entry}"#))
        .collect();
    format!(r#"{{"hooks":{{{}}}}}"#, hooks.join(","))
}

/// Quote a path for the POSIX shell Claude runs hook commands in: bare when it is
/// absolute and uses only safe characters, else single-quoted.
pub fn shell_path(path: &Path) -> String {
    let text = path.to_string_lossy();
    let safe = text.starts_with('/')
        && text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '/' | ':' | '-'));
    if safe {
        text.into_owned()
    } else {
        format!("'{}'", text.replace('\'', r"'\''"))
    }
}

/// Program that starts a provider's interactive CLI in a PTY. Antigravity is the
/// exception: its resolver finds the ACP server, which waits for JSON-RPC and is
/// not a terminal UI. The interactive CLI is `agy`, found on PATH at spawn.
pub fn tui_binary(kind: Kind) -> Result<PathBuf, String> {
    binary_with(kind, crate::provider::resolve)
}

fn binary_with(
    kind: Kind,
    resolve: impl FnOnce(Kind) -> Option<PathBuf>,
) -> Result<PathBuf, String> {
    if kind == Kind::Antigravity {
        return Ok(PathBuf::from("agy"));
    }
    resolve(kind).ok_or_else(|| format!("Could not find the {} CLI", kind.label()))
}

/// How a terminal process ended.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Exit {
    pub code: Option<i32>,
    /// The process never started, so there is no exit code.
    pub error: Option<String>,
    /// Exited within `EARLY_EXIT` of launch.
    pub early: bool,
    /// The CLI said it could not find the conversation it was asked to resume.
    pub no_conversation: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExitNotice {
    pub message: String,
    pub hint: Option<String>,
}

/// Plain-language bar copy; an instant exit is how "no interactive mode" shows.
pub fn exit_notice(kind: Kind, exit: &Exit) -> ExitNotice {
    let label = kind.label();
    if let Some(error) = &exit.error {
        return ExitNotice {
            message: format!("Could not start {label}"),
            hint: Some(error.clone()),
        };
    }
    let message = match exit.code {
        Some(code) => format!("Process exited (code {code})"),
        None => "Process exited".into(),
    };
    let hint = if exit.no_conversation && !exit.early {
        Some(format!(
            "{label} could not find the saved conversation. Start a new one to continue."
        ))
    } else if exit.early {
        Some(format!(
            "{label} stopped right after it started. It may not have an interactive mode, may need you to sign in, or may be missing. Check the output above."
        ))
    } else {
        None
    };
    ExitNotice { message, hint }
}

/// The error stored on an automation run whose CLI died or never started.
pub fn run_error(kind: Kind, exit: &Exit) -> String {
    let notice = exit_notice(kind, exit);
    match notice.hint {
        Some(hint) => format!("{}. {hint}", notice.message),
        None => notice.message,
    }
}

/// Resuming an id the CLI never saved fails at once. Offer a fresh start for
/// exactly that case, so a bad saved id cannot trap the session.
pub fn can_start_fresh(kind: Kind, exit: &Exit, resumed: bool) -> bool {
    exit.error.is_none() && (exit.early || exit.no_conversation) && resumed && tui_caps(kind).resume
}

/// Provider CLIs prefix the terminal title with a status glyph or spinner frame.
/// Keep the words only, so the tab does not flicker through animation frames.
pub fn clean_title(raw: &str) -> Option<String> {
    let spaced: String = raw
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let words = spaced
        .trim_start_matches(|c: char| !c.is_alphanumeric())
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if words.is_empty() {
        return None;
    }
    Some(if words.chars().count() > MAX_TITLE {
        let cut: String = words.chars().take(MAX_TITLE - 1).collect();
        format!("{}…", cut.trim_end())
    } else {
        words
    })
}

/// The CLI may only retitle a tab that still shows a placeholder or its own
/// earlier title. An Inbox title or a rename the user typed stays. Placeholders
/// are what `sessionDisplayTitle` reads as "New session" (the harness id, its
/// display title, and either behind an `<id> · ` prefix) plus the native
/// `New <label> session`.
pub fn can_retitle(current: &str, last_cli_title: Option<&str>, kind: Kind) -> bool {
    if last_cli_title == Some(current) {
        return true;
    }
    let (id, label) = (kind.id(), kind.label());
    let shown = current
        .strip_prefix(id)
        .and_then(|rest| rest.strip_prefix(" · "))
        .unwrap_or(current);
    current == id
        || current == label
        || shown == "New session"
        || current == format!("New {label} session")
}

/// Terminal output as the words a reader sees. Claude draws each word at an
/// absolute column (`ESC[4G`) with no space between, so cursor moves become a
/// space before the escapes are dropped.
pub fn plain_text(raw: &str) -> String {
    static PATTERNS: LazyLock<[(Regex, &str); 7]> = LazyLock::new(|| {
        let rule = |pattern: &str, with| (Regex::new(pattern).unwrap(), with);
        [
            rule(r"\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)", ""),
            rule(r"\x1b\[[0-9;?]*[ABCDEFGHJKf`d]", " "),
            rule(r"\x1b\[[0-9;?<>=! ]*[@-~]", ""),
            rule(r"\x1b[()][A-Za-z0-9]", ""),
            rule(r"\x1b[=>78]", ""),
            rule(r"[\x00-\x08\x0b-\x1f\x7f]", " "),
            rule(r"\s+", " "),
        ]
    });
    let mut text = raw.to_owned();
    for (pattern, with) in PATTERNS.iter() {
        text = pattern.replace_all(&text, *with).into_owned();
    }
    text.trim().to_owned()
}

/// Claude's answer to `--resume <id>` when it never wrote that conversation. The
/// exit code is not reliable for this (print mode exits 0), so match the message.
pub fn no_conversation(text: &str) -> bool {
    plain_text(text)
        .to_lowercase()
        .contains("no conversation found with session id")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::AccessMode;
    use crate::provider::Kind;
    use serde_json::Value;
    use std::ffi::OsString;
    use std::path::{Path, PathBuf};

    const BIN: &str = "/opt/bin/cli";

    fn input(kind: Kind) -> LaunchInput {
        LaunchInput {
            kind,
            binary: PathBuf::from(BIN),
            model: Some("claude:opus-5-5".into()),
            access: AccessMode::Supervised,
            provider_id: None,
            new_id: None,
            initial_prompt: None,
            hook: None,
            windows: false,
        }
    }
    fn claude() -> LaunchInput {
        input(Kind::Claude)
    }
    fn args(launch: &TuiLaunch) -> Vec<String> {
        launch
            .args
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }
    fn after<'a>(args: &'a [String], flag: &str) -> &'a str {
        let at = args.iter().position(|a| a == flag).unwrap();
        &args[at + 1]
    }
    fn is_uuid(id: &str) -> bool {
        let parts: Vec<_> = id.split('-').collect();
        parts.iter().map(|p| p.len()).collect::<Vec<_>>() == [8, 4, 4, 4, 12]
            && id.chars().all(|c| c == '-' || c.is_ascii_hexdigit())
    }
    fn hook() -> Option<(PathBuf, PathBuf)> {
        Some((
            PathBuf::from("/Applications/Monoptah.app/Contents/MacOS/monocode"),
            PathBuf::from("/Users/me/Library/Application Support/x/tui-hooks"),
        ))
    }

    // tuiLaunch.test.ts: new claude session
    #[test]
    fn new_session_mints_a_uuid_and_passes_it_as_session_id() {
        let launch = build(claude());
        assert_eq!(launch.program, OsString::from(BIN));
        let id = launch.minted_id.clone().unwrap();
        assert!(is_uuid(&id), "{id}");
        let args = args(&launch);
        assert_eq!(after(&args, "--session-id"), id);
        assert!(!args.contains(&"--resume".to_string()));
    }

    #[test]
    fn each_launch_mints_a_different_id() {
        assert_ne!(build(claude()).minted_id, build(claude()).minted_id);
    }

    #[test]
    fn passes_the_native_model_id() {
        let args = args(&build(claude()));
        assert_eq!(after(&args, "--model"), "claude-opus-5-5");
    }

    #[test]
    fn a_session_without_a_model_omits_the_flag() {
        for model in [None, Some("default".to_string()), Some("  ".to_string())] {
            let args = args(&build(LaunchInput { model, ..claude() }));
            assert!(!args.contains(&"--model".to_string()));
            assert!(args.contains(&"--permission-mode".to_string()));
        }
    }

    #[test]
    fn initial_prompt_is_the_final_positional_argument() {
        let launch = build(LaunchInput {
            initial_prompt: Some("fix the build".into()),
            ..claude()
        });
        let args = args(&launch);
        assert_eq!(args.last().unwrap(), "fix the build");
        assert_eq!(&args[args.len() - 2], after(&args, "--session-id"));
    }

    #[test]
    fn a_prompt_starting_with_a_dash_is_guarded_from_flag_parsing() {
        let launch = build(LaunchInput {
            initial_prompt: Some("--help me".into()),
            ..claude()
        });
        let args = args(&launch);
        assert_eq!(&args[args.len() - 2..], ["--", "--help me"]);
    }

    #[test]
    fn a_missing_or_blank_prompt_adds_no_positional() {
        let bare = build(claude());
        let blank = build(LaunchInput {
            initial_prompt: Some(" \n".into()),
            ..claude()
        });
        assert_eq!(args(&bare).last(), bare.minted_id.as_ref());
        assert_eq!(args(&blank).last(), blank.minted_id.as_ref());
    }

    // a saved id is a value to `--resume`, which takes an optional one
    #[test]
    fn an_id_that_looks_like_a_flag_is_not_resumed() {
        for id in ["--dangerously-skip-permissions", "-p", " --x"] {
            let launch = build(LaunchInput {
                provider_id: Some(id.into()),
                ..claude()
            });
            let args = args(&launch);
            assert!(!args.contains(&"--resume".to_string()), "{id}");
            assert!(
                !args.iter().any(|a| a.contains("dangerously") || a == "-p"),
                "{id}"
            );
            let minted = launch.minted_id.clone().unwrap();
            assert!(is_uuid(&minted));
            assert_eq!(after(&args, "--session-id"), minted);
        }
    }

    #[test]
    fn an_id_with_a_dash_inside_is_still_resumed() {
        let launch = build(LaunchInput {
            provider_id: Some("a-b".into()),
            ..claude()
        });
        assert_eq!(after(&args(&launch), "--resume"), "a-b");
        assert_eq!(launch.minted_id, None);
    }

    // resumed claude session
    #[test]
    fn a_saved_id_is_resumed_and_nothing_is_minted() {
        let launch = build(LaunchInput {
            provider_id: Some("prov-123".into()),
            ..claude()
        });
        let args = args(&launch);
        assert_eq!(after(&args, "--resume"), "prov-123");
        assert!(!args.contains(&"--session-id".to_string()));
        assert_eq!(launch.minted_id, None);
    }

    #[test]
    fn resuming_drops_the_initial_prompt() {
        let launch = build(LaunchInput {
            provider_id: Some("prov-123".into()),
            initial_prompt: Some("ignored".into()),
            ..claude()
        });
        let args = args(&launch);
        assert!(!args.contains(&"ignored".to_string()));
        assert_eq!(args.last().unwrap(), "prov-123");
    }

    #[test]
    fn a_blank_saved_id_starts_a_new_conversation() {
        let launch = build(LaunchInput {
            provider_id: Some("  ".into()),
            ..claude()
        });
        assert!(launch.minted_id.is_some());
        assert!(!args(&launch).contains(&"--resume".to_string()));
    }

    // permission mode
    #[test]
    fn access_maps_to_the_permission_mode() {
        let cases = [
            (AccessMode::ReadOnly, "plan"),
            (AccessMode::Supervised, "default"),
            (AccessMode::AutoAcceptEdits, "acceptEdits"),
            (AccessMode::Auto, "auto"),
            (AccessMode::FullAccess, "bypassPermissions"),
        ];
        for (access, expected) in cases {
            let args = args(&build(LaunchInput { access, ..claude() }));
            assert_eq!(after(&args, "--permission-mode"), expected);
        }
    }

    #[test]
    fn flag_order_is_model_mode_settings_session_prompt() {
        let launch = build(LaunchInput {
            hook: hook(),
            initial_prompt: Some("go".into()),
            ..claude()
        });
        let args = args(&launch);
        let id = launch.minted_id.unwrap();
        assert_eq!(
            [&args[0], &args[2], &args[4], &args[6]],
            ["--model", "--permission-mode", "--settings", "--session-id"]
        );
        assert_eq!(args[7], id);
        assert_eq!(args[8], "go");
        assert_eq!(args.len(), 9);
    }

    // providers without TUI flags
    #[test]
    fn other_providers_launch_the_bare_binary() {
        for kind in Kind::ALL.into_iter().filter(|k| *k != Kind::Claude) {
            let launch = build(LaunchInput {
                model: Some(format!("{}:some-model", kind.id())),
                access: AccessMode::FullAccess,
                provider_id: Some("prov-123".into()),
                initial_prompt: Some("ignored".into()),
                hook: hook(),
                new_id: Some("saved-id".into()),
                ..input(kind)
            });
            assert_eq!(launch.program, OsString::from(BIN), "{kind:?}");
            assert!(launch.args.is_empty(), "{kind:?}");
            assert_eq!(launch.minted_id, None, "{kind:?}");
        }
    }

    // claiming a saved id for a new conversation
    #[test]
    fn a_new_conversation_can_claim_a_saved_id() {
        let launch = build(LaunchInput {
            new_id: Some("saved-id".into()),
            ..claude()
        });
        assert_eq!(launch.minted_id.as_deref(), Some("saved-id"));
        let args = args(&launch);
        assert_eq!(after(&args, "--session-id"), "saved-id");
        assert!(!args.contains(&"--resume".to_string()));
    }

    #[test]
    fn a_claimed_id_is_ignored_when_there_is_an_id_to_resume() {
        let launch = build(LaunchInput {
            provider_id: Some("existing".into()),
            new_id: Some("other".into()),
            ..claude()
        });
        assert!(args(&launch).contains(&"--resume".to_string()));
        assert_eq!(launch.minted_id, None);
    }

    // hook injection
    #[test]
    fn no_settings_without_a_hook() {
        assert!(!args(&build(claude())).contains(&"--settings".to_string()));
    }

    #[test]
    fn no_settings_on_windows() {
        let launch = build(LaunchInput {
            hook: Some((
                PathBuf::from("C:\\Program Files\\Monoptah\\monocode.exe"),
                PathBuf::from("C:\\Users\\me\\AppData\\tui-hooks"),
            )),
            windows: true,
            ..claude()
        });
        assert!(!args(&launch).contains(&"--settings".to_string()));
    }

    #[test]
    fn hooks_are_registered_async_on_the_argv() {
        let args = args(&build(LaunchInput {
            hook: hook(),
            ..claude()
        }));
        let json: Value = serde_json::from_str(after(&args, "--settings")).unwrap();
        let hooks = json["hooks"].as_object().unwrap();
        let mut names: Vec<_> = hooks.keys().cloned().collect();
        names.sort();
        assert_eq!(
            names,
            [
                "Notification",
                "SessionEnd",
                "Stop",
                "StopFailure",
                "UserPromptSubmit"
            ]
        );
        for groups in hooks.values() {
            let entry = &groups[0]["hooks"][0];
            assert_eq!(entry["type"], "command");
            assert_eq!(entry["async"], true);
        }
    }

    #[test]
    fn settings_json_is_exact_and_quotes_paths_with_spaces() {
        let (cli, spool) = hook().unwrap();
        let command = "/Applications/Monoptah.app/Contents/MacOS/monocode tui-hook --spool '/Users/me/Library/Application Support/x/tui-hooks'";
        let entry =
            format!(r#"[{{"hooks":[{{"type":"command","command":"{command}","async":true}}]}}]"#);
        let expected = format!(
            r#"{{"hooks":{{"UserPromptSubmit":{entry},"Stop":{entry},"StopFailure":{entry},"SessionEnd":{entry},"Notification":{entry}}}}}"#
        );
        assert_eq!(hook_settings_json(&cli, &spool), expected);
        let spaced = hook_settings_json(Path::new("/My Apps/monocode"), &spool);
        let json: Value = serde_json::from_str(&spaced).unwrap();
        assert!(json["hooks"]["Stop"][0]["hooks"][0]["command"]
            .as_str()
            .unwrap()
            .contains("'/My Apps/monocode' tui-hook"));
    }

    #[test]
    fn resume_id_and_trailing_prompt_stay_in_place_with_hooks() {
        let fresh = build(LaunchInput {
            hook: hook(),
            initial_prompt: Some("go".into()),
            ..claude()
        });
        let a = args(&fresh);
        assert_eq!(a.last().unwrap(), "go");
        assert_eq!(Some(&a[a.len() - 2]), fresh.minted_id.as_ref());
        let resumed = args(&build(LaunchInput {
            hook: hook(),
            provider_id: Some("p1".into()),
            ..claude()
        }));
        assert_eq!(resumed.last().unwrap(), "p1");
        assert!(resumed.contains(&"--settings".to_string()));
    }

    #[test]
    fn non_claude_never_gets_hooks() {
        for kind in Kind::ALL.into_iter().filter(|k| *k != Kind::Claude) {
            let args = args(&build(LaunchInput {
                hook: hook(),
                ..input(kind)
            }));
            assert!(!args.contains(&"--settings".to_string()), "{kind:?}");
        }
    }

    // shell_path (orchestration.ts shellPath)
    #[test]
    fn shell_path_quotes_only_when_needed() {
        assert_eq!(shell_path(Path::new("/a/b-c_d.e:f")), "/a/b-c_d.e:f");
        assert_eq!(shell_path(Path::new("/My Apps/x")), "'/My Apps/x'");
        assert_eq!(shell_path(Path::new("/it's/x")), r"'/it'\''s/x'");
        assert_eq!(shell_path(Path::new("/a$b")), "'/a$b'");
        assert_eq!(shell_path(Path::new("rel/x")), "'rel/x'");
    }

    // tuiBinary.test.ts
    #[test]
    fn each_provider_resolves_through_its_own_resolver() {
        for kind in Kind::ALL.into_iter().filter(|k| *k != Kind::Antigravity) {
            let got = binary_with(kind, |k| Some(PathBuf::from(format!("/bin/{}", k.id()))));
            assert_eq!(got, Ok(PathBuf::from(format!("/bin/{}", kind.id()))));
        }
    }

    #[test]
    fn antigravity_launches_agy_and_never_the_acp_server() {
        let got = binary_with(Kind::Antigravity, |_| panic!("resolver must not run"));
        assert_eq!(got, Ok(PathBuf::from("agy")));
    }

    #[test]
    fn a_missing_binary_is_an_error_naming_the_provider() {
        let error = binary_with(Kind::Codex, |_| None).unwrap_err();
        assert!(error.contains("Codex"), "{error}");
    }

    // tuiSession.test.ts: exit notice
    fn exit(code: Option<i32>, early: bool) -> Exit {
        Exit {
            code,
            error: None,
            early,
            no_conversation: false,
        }
    }

    #[test]
    fn the_early_window_is_five_seconds() {
        assert_eq!(EARLY_EXIT, std::time::Duration::from_secs(5));
    }

    #[test]
    fn notice_names_the_exit_code() {
        assert_eq!(
            exit_notice(Kind::Claude, &exit(Some(130), false)),
            ExitNotice {
                message: "Process exited (code 130)".into(),
                hint: None
            }
        );
    }

    #[test]
    fn notice_omits_a_missing_code() {
        assert_eq!(
            exit_notice(Kind::Claude, &exit(None, false)).message,
            "Process exited"
        );
    }

    #[test]
    fn notice_explains_an_exit_right_after_launch() {
        let notice = exit_notice(Kind::Codex, &exit(Some(1), true));
        assert_eq!(notice.message, "Process exited (code 1)");
        assert_eq!(
            notice.hint.as_deref(),
            Some("Codex stopped right after it started. It may not have an interactive mode, may need you to sign in, or may be missing. Check the output above.")
        );
    }

    #[test]
    fn notice_shows_the_reason_a_process_could_not_start() {
        let notice = exit_notice(
            Kind::Grok,
            &Exit {
                code: None,
                error: Some("Failed to start grok: not found".into()),
                early: true,
                no_conversation: false,
            },
        );
        assert_eq!(
            notice,
            ExitNotice {
                message: "Could not start Grok Build".into(),
                hint: Some("Failed to start grok: not found".into())
            }
        );
    }

    #[test]
    fn notice_for_a_late_missing_conversation_offers_a_new_one() {
        let notice = exit_notice(
            Kind::Claude,
            &Exit {
                no_conversation: true,
                ..exit(Some(1), false)
            },
        );
        assert_eq!(
            notice.hint.as_deref(),
            Some("Claude Code could not find the saved conversation. Start a new one to continue.")
        );
    }

    // automationSession.test.ts: tuiRunError
    #[test]
    fn a_run_error_reports_a_cli_that_could_not_start() {
        let exit = Exit {
            code: None,
            error: Some("binary not found".into()),
            early: true,
            no_conversation: false,
        };
        assert_eq!(
            run_error(Kind::Claude, &exit),
            "Could not start Claude Code. binary not found"
        );
    }

    #[test]
    fn a_run_error_explains_an_early_exit_with_its_code() {
        let text = run_error(Kind::Claude, &exit(Some(1), true));
        assert!(text.starts_with("Process exited (code 1). "), "{text}");
        assert!(text.contains("stopped right after it started"), "{text}");
    }

    #[test]
    fn a_run_error_without_a_hint_is_just_the_message() {
        assert_eq!(
            run_error(Kind::Claude, &exit(Some(2), false)),
            "Process exited (code 2)"
        );
    }

    // starting fresh after a failed resume
    #[test]
    fn fresh_start_is_offered_when_a_resuming_provider_exits_at_once() {
        assert!(can_start_fresh(Kind::Claude, &exit(Some(1), true), true));
        let missing = Exit {
            no_conversation: true,
            ..exit(Some(0), false)
        };
        assert!(can_start_fresh(Kind::Claude, &missing, true));
    }

    #[test]
    fn fresh_start_is_not_offered_for_a_late_exit_first_launch_or_start_failure() {
        assert!(!can_start_fresh(Kind::Claude, &exit(Some(0), false), true));
        assert!(!can_start_fresh(Kind::Claude, &exit(Some(1), true), false));
        let failed = Exit {
            error: Some("no such file".into()),
            ..exit(None, true)
        };
        assert!(!can_start_fresh(Kind::Claude, &failed, true));
    }

    #[test]
    fn fresh_start_is_not_offered_for_a_provider_that_cannot_resume() {
        assert!(!can_start_fresh(Kind::Codex, &exit(Some(1), true), true));
    }

    // tab title from the terminal
    #[test]
    fn titles_drop_status_glyphs_and_spinner_frames() {
        assert_eq!(clean_title("✳ Claude Code").as_deref(), Some("Claude Code"));
        assert_eq!(
            clean_title("⠂ Fix the login bug").as_deref(),
            Some("Fix the login bug")
        );
        assert_eq!(
            clean_title("  \u{1b} spaced   out ").as_deref(),
            Some("spaced out")
        );
    }

    #[test]
    fn a_title_with_no_words_is_nothing() {
        assert_eq!(clean_title("✳"), None);
        assert_eq!(clean_title(""), None);
    }

    #[test]
    fn a_long_title_is_capped() {
        let cleaned = clean_title(&"a".repeat(200)).unwrap();
        assert_eq!(cleaned.chars().count(), 80);
        assert!(cleaned.ends_with('…'));
        assert_eq!(clean_title(&"b".repeat(80)).unwrap().chars().count(), 80);
    }

    #[test]
    fn the_cli_retitles_a_placeholder_but_not_a_name_the_app_or_user_gave() {
        assert!(can_retitle("Claude Code", None, Kind::Claude));
        assert!(!can_retitle("#12 Fix the bug", None, Kind::Claude));
        assert!(!can_retitle(
            "Renamed by me",
            Some("Claude Code"),
            Kind::Claude
        ));
    }

    #[test]
    fn every_placeholder_title_a_session_can_start_with_is_retitled() {
        // Tauri: the harness id and its display title, with or without the
        // "<id> · " prefix; native: "New <label> session".
        for current in [
            "claude",
            "Claude Code",
            "New session",
            "claude · New session",
            "New Claude Code session",
        ] {
            assert!(can_retitle(current, None, Kind::Claude), "{current}");
        }
        assert!(can_retitle("codex", None, Kind::Codex));
        assert!(can_retitle("Codex", None, Kind::Codex));
        assert!(can_retitle("New Grok Build session", None, Kind::Grok));
        assert!(can_retitle("grok · New session", None, Kind::Grok));
    }

    #[test]
    fn a_real_title_with_the_prefix_is_not_a_placeholder() {
        assert!(!can_retitle("claude · Fix the bug", None, Kind::Claude));
        assert!(!can_retitle("New session notes", None, Kind::Claude));
        // Another provider's placeholder is a name here.
        assert!(!can_retitle("codex", None, Kind::Claude));
        assert!(!can_retitle("Codex", None, Kind::Claude));
    }

    #[test]
    fn the_cli_keeps_following_its_own_title() {
        assert!(can_retitle(
            "Claude Code",
            Some("Claude Code"),
            Kind::Claude
        ));
        assert!(can_retitle(
            "Fix the bug",
            Some("Fix the bug"),
            Kind::Claude
        ));
        assert!(!can_retitle("Fix the bug", Some("Other"), Kind::Claude));
    }

    // recognising Claude's no-conversation exit
    const REAL_NO_CONVERSATION: &str = "\u{1b}7\u{1b}[r\u{1b}8\u{1b}[?25h\u{1b}[?25l\u{1b}[?2004h\u{1b}[?2031h\u{1b}[?1004h\u{1b}]11;?\u{7}\u{1b}[c\u{1b}[>0q\u{1b}[?u\u{1b}[c\u{1b}[>4m\u{1b}[?1004l\u{1b}[?2031l\u{1b}[?2004lNo\u{1b}[4Gconversation\u{1b}[17Gfound\u{1b}[23Gwith\u{1b}[28Gsession\u{1b}[36GID:\u{1b}[40G11111111-2222-3333-4444-555555555555\r\r\n\u{1b}[?25h\u{1b}(B\u{f}\u{1b}[?1016l\u{1b}]0;\u{7}";

    #[test]
    fn no_conversation_reads_output_where_words_are_placed_by_cursor_moves() {
        assert_eq!(
            plain_text(REAL_NO_CONVERSATION),
            "No conversation found with session ID: 11111111-2222-3333-4444-555555555555"
        );
        assert!(no_conversation(REAL_NO_CONVERSATION));
    }

    #[test]
    fn no_conversation_reads_the_plain_print_mode_line() {
        assert!(no_conversation(
            "No conversation found with session ID: 1111\r\n"
        ));
        assert!(no_conversation("NO CONVERSATION FOUND WITH SESSION ID: x"));
    }

    #[test]
    fn no_conversation_ignores_other_failures() {
        assert!(!no_conversation("Error: Session ID abc is already in use."));
        assert!(!no_conversation("command not found: claude"));
        assert!(!no_conversation(""));
    }
}
