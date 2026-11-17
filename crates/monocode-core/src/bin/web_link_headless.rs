//! Headless desktop binary for the web link (E-01 seam of the e2e harness).
//!
//! Commands (parsed by hand, clap is not a dependency):
//!
//! ```text
//! web_link_headless capabilities
//! web_link_headless check-vectors <path-to-vectors.json>
//! web_link_headless serve --relay <ws://|wss:// url> --token-file <path> --data-dir <dir>
//! ```
//!
//! Exit codes: 0 ok, 1 check-vectors found a failure, 2 usage or IO error, 3 `serve` is not implemented.
//!
//! `serve` is a stub: the relay client (`web_link::start`) is not merged yet (WS-D1 R-05). When it lands,
//! `serve` validates the same arguments and then calls `web_link::start` on a `MobileHost` built over
//! `--data-dir`, using the token read from `--token-file`.
use monocode_core::mobile::web_link::frame::{
    split_message, Frame, FrameType, Plaintext, MAX_REQUEST_BYTES,
};
use serde_json::{json, Value};
use std::io::Write;
use std::path::{Path, PathBuf};

const USAGE: &str = "usage:
  web_link_headless capabilities
  web_link_headless check-vectors <path-to-vectors.json>
  web_link_headless serve --relay <ws://|wss:// url> --token-file <path> --data-dir <dir>
  web_link_headless --help";

const CAPABILITIES_LINE: &str =
    r#"{"version":1,"serve":false,"reason":"web_link serve needs WS-D1 R-05","checkVectors":true}"#;
const SERVE_NOT_IMPLEMENTED: &str = "web_link serve not implemented yet: TODO(E-01:WS-D1 R-05)";

#[derive(Debug, PartialEq, Eq)]
enum Command {
    Capabilities,
    CheckVectors {
        path: PathBuf,
    },
    Serve {
        relay: String,
        token_file: PathBuf,
        data_dir: PathBuf,
    },
    Help,
}

fn parse_args(args: &[String]) -> Result<Command, String> {
    let Some(command) = args.first() else {
        return Err("missing command".into());
    };
    let rest = &args[1..];
    match command.as_str() {
        "--help" | "-h" | "help" => Ok(Command::Help),
        "capabilities" => {
            if !rest.is_empty() {
                return Err("capabilities takes no arguments".into());
            }
            Ok(Command::Capabilities)
        }
        "check-vectors" => match rest {
            [path] => Ok(Command::CheckVectors {
                path: PathBuf::from(path),
            }),
            _ => Err("check-vectors takes exactly one path".into()),
        },
        "serve" => parse_serve(rest),
        other => Err(format!("unknown command: {other}")),
    }
}

fn parse_serve(rest: &[String]) -> Result<Command, String> {
    let (mut relay, mut token_file, mut data_dir) = (None, None, None);
    let mut iter = rest.iter();
    while let Some(flag) = iter.next() {
        let slot = match flag.as_str() {
            "--relay" => &mut relay,
            "--token-file" => &mut token_file,
            "--data-dir" => &mut data_dir,
            other => return Err(format!("unknown option for serve: {other}")),
        };
        let value = iter.next().ok_or_else(|| format!("{flag} needs a value"))?;
        if slot.replace(value.clone()).is_some() {
            return Err(format!("{flag} given twice"));
        }
    }
    let relay = relay.ok_or("serve needs --relay")?;
    let token_file = token_file.ok_or("serve needs --token-file")?;
    let data_dir = data_dir.ok_or("serve needs --data-dir")?;
    let parsed = url::Url::parse(&relay).map_err(|e| format!("--relay is not a valid URL: {e}"))?;
    if !matches!(parsed.scheme(), "ws" | "wss") || parsed.host_str().is_none() {
        return Err("--relay must be a ws:// or wss:// URL with a host".into());
    }
    if token_file.is_empty() || data_dir.is_empty() {
        return Err("--token-file and --data-dir must not be empty".into());
    }
    Ok(Command::Serve {
        relay,
        token_file: PathBuf::from(token_file),
        data_dir: PathBuf::from(data_dir),
    })
}

// ---------------------------------------------------------------- check-vectors

fn unhex(text: &str) -> Result<Vec<u8>, String> {
    if !text.len().is_multiple_of(2) || !text.is_ascii() {
        return Err(format!("bad hex string of length {}", text.len()));
    }
    (0..text.len() / 2)
        .map(|i| {
            u8::from_str_radix(&text[2 * i..2 * i + 2], 16).map_err(|e| format!("bad hex: {e}"))
        })
        .collect()
}

/// `{"hex"}` or `{"prefix"?, "fill", "length"}`: the prefix, then the fill byte repeated up to `length` bytes.
fn bytes(spec: &Value) -> Result<Vec<u8>, String> {
    if let Some(hex) = spec.get("hex").and_then(Value::as_str) {
        return unhex(hex);
    }
    let mut out = match spec.get("prefix").and_then(Value::as_str) {
        Some(prefix) => unhex(prefix)?,
        None => Vec::new(),
    };
    let fill = unhex(
        spec.get("fill")
            .and_then(Value::as_str)
            .ok_or("byte string needs hex or fill")?,
    )?;
    let [fill] = fill[..] else {
        return Err("fill must be one byte".into());
    };
    let length = spec
        .get("length")
        .and_then(Value::as_u64)
        .ok_or("fill needs a length")? as usize;
    if length < out.len() {
        return Err("fill length is shorter than the prefix".into());
    }
    out.resize(length, fill);
    Ok(out)
}

fn uuid(text: &str) -> Result<[u8; 16], String> {
    unhex(&text.replace('-', ""))?
        .try_into()
        .map_err(|_| format!("bad uuid {text}"))
}

fn field<'a>(value: &'a Value, key: &str) -> Result<&'a Value, String> {
    value.get(key).ok_or_else(|| format!("missing field {key}"))
}

fn str_field<'a>(value: &'a Value, key: &str) -> Result<&'a str, String> {
    field(value, key)?
        .as_str()
        .ok_or_else(|| format!("field {key} is not a string"))
}

fn u64_field(value: &Value, key: &str) -> Result<u64, String> {
    field(value, key)?
        .as_u64()
        .ok_or_else(|| format!("field {key} is not a number"))
}

fn list<'a>(root: &'a Value, group: &str, key: &str) -> Result<&'a Vec<Value>, String> {
    field(field(root, group)?, key)?
        .as_array()
        .ok_or_else(|| format!("{group}.{key} is not an array"))
}

fn check_valid_frame(item: &Value) -> Result<(), String> {
    let wire = bytes(field(item, "frame")?)?;
    let decoded = field(item, "decoded")?;
    let type_code =
        u8::try_from(u64_field(decoded, "typeCode")?).map_err(|_| "typeCode out of range")?;
    let kind = FrameType::from_code(type_code).ok_or("unknown typeCode")?;
    if kind.name() != str_field(decoded, "type")? {
        return Err("type name does not match typeCode".into());
    }
    let frame = Frame {
        kind,
        channel: u16::try_from(u64_field(decoded, "channel")?)
            .map_err(|_| "channel out of range")?,
        device_id: uuid(str_field(decoded, "deviceId")?)?,
        body: bytes(field(decoded, "body")?)?,
    };
    let encoded = frame
        .encode()
        .map_err(|e| format!("encode failed: {}", e.as_str()))?;
    if encoded != wire {
        return Err("encode(decoded) differs from the frame bytes".into());
    }
    let back = Frame::decode(&wire).map_err(|e| format!("decode failed: {}", e.as_str()))?;
    if back != frame {
        return Err("decode(frame) differs from decoded".into());
    }
    Ok(())
}

fn check_invalid_frame(item: &Value) -> Result<(), String> {
    let want = str_field(item, "error")?;
    match Frame::decode(&bytes(field(item, "frame")?)?) {
        Ok(_) => Err(format!("decoded fine, expected {want}")),
        Err(e) if e.as_str() == want => Ok(()),
        Err(e) => Err(format!("failed with {}, expected {want}", e.as_str())),
    }
}

fn expected_plaintext(decoded: &Value) -> Result<Plaintext, String> {
    Ok(match str_field(decoded, "kind")? {
        "msg" => Plaintext::Msg(bytes(field(decoded, "payload")?)?),
        "chunk" => Plaintext::Chunk {
            msg_id: u32::try_from(u64_field(decoded, "msgId")?)
                .map_err(|_| "msgId out of range")?,
            index: u16::try_from(u64_field(decoded, "index")?).map_err(|_| "index out of range")?,
            last: field(decoded, "last")?
                .as_bool()
                .ok_or("last is not a bool")?,
            data: bytes(field(decoded, "data")?)?,
        },
        "rekey" => Plaintext::Rekey,
        other => return Err(format!("unknown plaintext kind {other}")),
    })
}

fn check_valid_plaintext(item: &Value) -> Result<(), String> {
    let wire = bytes(field(item, "plaintext")?)?;
    let expected = expected_plaintext(field(item, "decoded")?)?;
    let encoded = expected
        .encode()
        .map_err(|e| format!("encode failed: {}", e.as_str()))?;
    if encoded != wire {
        return Err("encode(decoded) differs from the plaintext bytes".into());
    }
    let back = Plaintext::decode(&wire).map_err(|e| format!("decode failed: {}", e.as_str()))?;
    if back != expected {
        return Err("decode(plaintext) differs from decoded".into());
    }
    Ok(())
}

fn check_invalid_plaintext(item: &Value) -> Result<(), String> {
    let want = str_field(item, "error")?;
    match Plaintext::decode(&bytes(field(item, "plaintext")?)?) {
        Ok(_) => Err(format!("decoded fine, expected {want}")),
        Err(e) if e.as_str() == want => Ok(()),
        Err(e) => Err(format!("failed with {}, expected {want}", e.as_str())),
    }
}

fn check_split(item: &Value) -> Result<(), String> {
    let fill = unhex(str_field(item, "messageFill")?)?;
    let [fill] = fill[..] else {
        return Err("messageFill must be one byte".into());
    };
    let message = vec![fill; u64_field(item, "messageLength")? as usize];
    let msg_id = u32::try_from(u64_field(item, "msgId")?).map_err(|_| "msgId out of range")?;
    let want: Vec<u64> = field(item, "partLengths")?
        .as_array()
        .ok_or("partLengths is not an array")?
        .iter()
        .map(|v| v.as_u64().ok_or("partLength is not a number"))
        .collect::<Result<_, _>>()?;
    let parts = split_message(&message, msg_id, MAX_REQUEST_BYTES)
        .map_err(|_| "split_message refused the message")?;
    let got: Vec<u64> = parts.iter().map(|p| p.len() as u64).collect();
    if got != want {
        return Err(format!("part lengths {got:?}, expected {want:?}"));
    }
    for (i, part) in parts.iter().enumerate() {
        let decoded = Plaintext::decode(part)
            .map_err(|e| format!("part {i} does not decode: {}", e.as_str()))?;
        let last = i + 1 == parts.len();
        match (parts.len(), decoded) {
            (1, Plaintext::Msg(_)) => {}
            (
                n,
                Plaintext::Chunk {
                    msg_id: id,
                    index,
                    last: l,
                    ..
                },
            ) if n > 1 => {
                if id != msg_id || usize::from(index) != i || l != last {
                    return Err(format!("part {i} has the wrong chunk header"));
                }
            }
            _ => return Err(format!("part {i} has the wrong kind")),
        }
    }
    Ok(())
}

/// Runs one group of vector checks and records the failures. Returns how many items ran.
fn run_group(
    label: &str,
    items: Result<&Vec<Value>, String>,
    check: fn(&Value) -> Result<(), String>,
    failures: &mut Vec<Value>,
) -> usize {
    let items = match items {
        Ok(items) => items,
        Err(e) => {
            failures.push(json!({"group": label, "name": null, "error": e}));
            return 0;
        }
    };
    if items.is_empty() {
        failures.push(json!({"group": label, "name": null, "error": "the group is empty"}));
    }
    for item in items {
        if let Err(e) = check(item) {
            let name = item.get("name").and_then(Value::as_str).unwrap_or("?");
            failures.push(json!({"group": label, "name": name, "error": e}));
        }
    }
    items.len()
}

/// Validates the frame and plaintext vectors of `vectors.json`. Returns the summary and whether all passed.
fn check_vectors(root: &Value) -> (Value, bool) {
    let mut failures = Vec::new();
    let frames_valid = run_group(
        "frames.valid",
        list(root, "frames", "valid"),
        check_valid_frame,
        &mut failures,
    );
    let frames_invalid = run_group(
        "frames.invalid",
        list(root, "frames", "invalid"),
        check_invalid_frame,
        &mut failures,
    );
    let plaintext_valid = run_group(
        "plaintext.valid",
        list(root, "plaintext", "valid"),
        check_valid_plaintext,
        &mut failures,
    );
    let plaintext_invalid = run_group(
        "plaintext.invalid",
        list(root, "plaintext", "invalid"),
        check_invalid_plaintext,
        &mut failures,
    );
    let plaintext_split = run_group(
        "plaintext.split",
        list(root, "plaintext", "split"),
        check_split,
        &mut failures,
    );
    let ok = failures.is_empty();
    let summary = json!({
        "ok": ok,
        "checked": {
            "framesValid": frames_valid,
            "framesInvalid": frames_invalid,
            "plaintextValid": plaintext_valid,
            "plaintextInvalid": plaintext_invalid,
            "plaintextSplit": plaintext_split,
        },
        "failures": failures,
    });
    (summary, ok)
}

// ---------------------------------------------------------------- entry

/// Runs a command; returns the exit code. Output goes to `out` and `err` so tests can capture it.
fn run(args: &[String], out: &mut dyn Write, err: &mut dyn Write) -> i32 {
    let command = match parse_args(args) {
        Ok(Command::Help) => {
            let _ = writeln!(out, "{USAGE}");
            return 2;
        }
        Ok(command) => command,
        Err(message) => {
            let _ = writeln!(err, "error: {message}\n{USAGE}");
            return 2;
        }
    };
    match command {
        Command::Help => unreachable!("handled above"),
        Command::Capabilities => {
            let _ = writeln!(out, "{CAPABILITIES_LINE}");
            0
        }
        Command::CheckVectors { path } => run_check_vectors(&path, out, err),
        Command::Serve { token_file, .. } => {
            // The token comes from a file so it never appears in argv or in a process listing. It is only
            // checked for presence here; it is never printed.
            match std::fs::read_to_string(&token_file) {
                Ok(token) if !token.trim().is_empty() => {}
                Ok(_) => {
                    let _ = writeln!(
                        err,
                        "error: the token file {} is empty",
                        token_file.display()
                    );
                    return 2;
                }
                Err(e) => {
                    let _ = writeln!(
                        err,
                        "error: cannot read the token file {}: {e}",
                        token_file.display()
                    );
                    return 2;
                }
            }
            // TODO(E-01:WS-D1 R-05): wire this to `web_link::start` once the relay client is merged.
            let _ = writeln!(err, "{SERVE_NOT_IMPLEMENTED}");
            3
        }
    }
}

fn run_check_vectors(path: &Path, out: &mut dyn Write, err: &mut dyn Write) -> i32 {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) => {
            let _ = writeln!(err, "error: cannot read {}: {e}", path.display());
            return 2;
        }
    };
    let root: Value = match serde_json::from_str(&text) {
        Ok(root) => root,
        Err(e) => {
            let _ = writeln!(err, "error: {} is not valid JSON: {e}", path.display());
            return 2;
        }
    };
    let (summary, ok) = check_vectors(&root);
    let _ = writeln!(out, "{summary}");
    if ok {
        0
    } else {
        1
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let stdout = std::io::stdout();
    let stderr = std::io::stderr();
    let code = run(&args, &mut stdout.lock(), &mut stderr.lock());
    std::process::exit(code);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    fn vectors_path() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../web/protocol/vectors.json")
    }

    fn run_capture(list: &[&str]) -> (i32, String, String) {
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = run(&args(list), &mut out, &mut err);
        (
            code,
            String::from_utf8(out).unwrap(),
            String::from_utf8(err).unwrap(),
        )
    }

    #[test]
    fn parses_each_command() {
        assert_eq!(
            parse_args(&args(&["capabilities"])),
            Ok(Command::Capabilities)
        );
        assert_eq!(parse_args(&args(&["--help"])), Ok(Command::Help));
        assert_eq!(
            parse_args(&args(&["check-vectors", "v.json"])),
            Ok(Command::CheckVectors {
                path: "v.json".into()
            })
        );
        assert_eq!(
            parse_args(&args(&[
                "serve",
                "--relay",
                "wss://relay.example/ws",
                "--token-file",
                "t",
                "--data-dir",
                "d"
            ])),
            Ok(Command::Serve {
                relay: "wss://relay.example/ws".into(),
                token_file: "t".into(),
                data_dir: "d".into()
            })
        );
        // Option order does not matter.
        assert!(parse_args(&args(&[
            "serve",
            "--data-dir",
            "d",
            "--token-file",
            "t",
            "--relay",
            "ws://127.0.0.1:8787/x"
        ]))
        .is_ok());
    }

    #[test]
    fn rejects_bad_arguments() {
        for bad in [
            &[][..],
            &["nope"],
            &["capabilities", "extra"],
            &["check-vectors"],
            &["check-vectors", "a", "b"],
            &["serve"],
            &[
                "serve",
                "--relay",
                "http://x",
                "--token-file",
                "t",
                "--data-dir",
                "d",
            ],
            &[
                "serve",
                "--relay",
                "ws://",
                "--token-file",
                "t",
                "--data-dir",
                "d",
            ],
            &[
                "serve",
                "--relay",
                "not a url",
                "--token-file",
                "t",
                "--data-dir",
                "d",
            ],
            &["serve", "--relay", "ws://x", "--token-file", "t"],
            &[
                "serve",
                "--relay",
                "ws://x",
                "--relay",
                "ws://y",
                "--token-file",
                "t",
                "--data-dir",
                "d",
            ],
            &[
                "serve",
                "--relay",
                "ws://x",
                "--token",
                "secret",
                "--token-file",
                "t",
                "--data-dir",
                "d",
            ],
            &["serve", "--relay"],
        ] {
            assert!(parse_args(&args(bad)).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn capabilities_prints_one_json_line() {
        let (code, out, _) = run_capture(&["capabilities"]);
        assert_eq!(code, 0);
        assert_eq!(out.lines().count(), 1);
        let value: Value = serde_json::from_str(out.trim()).unwrap();
        assert_eq!(
            value,
            json!({"version": 1, "serve": false, "reason": "web_link serve needs WS-D1 R-05", "checkVectors": true})
        );
    }

    #[test]
    fn help_and_unknown_commands_exit_2() {
        assert_eq!(run_capture(&["--help"]).0, 2);
        let (code, _, err) = run_capture(&["bogus"]);
        assert_eq!(code, 2);
        assert!(err.contains("usage:"));
    }

    #[test]
    fn check_vectors_passes_on_the_real_file() {
        let path = vectors_path();
        let (code, out, err) = run_capture(&["check-vectors", path.to_str().unwrap()]);
        assert_eq!(code, 0, "stdout: {out} stderr: {err}");
        let summary: Value = serde_json::from_str(out.trim()).unwrap();
        assert_eq!(summary["ok"], json!(true));
        assert!(summary["checked"]["framesValid"].as_u64().unwrap() > 0);
        assert!(summary["checked"]["framesInvalid"].as_u64().unwrap() > 0);
        assert!(summary["checked"]["plaintextValid"].as_u64().unwrap() > 0);
        assert!(summary["checked"]["plaintextSplit"].as_u64().unwrap() > 0);
    }

    #[test]
    fn check_vectors_fails_on_a_corrupted_copy() {
        let mut root: Value =
            serde_json::from_str(&std::fs::read_to_string(vectors_path()).unwrap()).unwrap();
        // Change the channel of the first valid frame: encode(decoded) no longer equals the bytes.
        root["frames"]["valid"][0]["decoded"]["channel"] = json!(4242);
        // And break the expected error of the first invalid frame.
        root["frames"]["invalid"][0]["error"] = json!("too_long");
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("vectors.json");
        std::fs::write(&path, serde_json::to_string(&root).unwrap()).unwrap();
        let (code, out, _) = run_capture(&["check-vectors", path.to_str().unwrap()]);
        assert_eq!(code, 1, "{out}");
        let summary: Value = serde_json::from_str(out.trim()).unwrap();
        assert_eq!(summary["ok"], json!(false));
        assert_eq!(summary["failures"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn check_vectors_exit_2_on_io_error_and_bad_json() {
        assert_eq!(
            run_capture(&["check-vectors", "/nonexistent/vectors.json"]).0,
            2
        );
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.json");
        std::fs::write(&path, "{ not json").unwrap();
        assert_eq!(run_capture(&["check-vectors", path.to_str().unwrap()]).0, 2);
    }

    #[test]
    fn serve_reads_the_token_file_then_exits_3_without_printing_the_token() {
        let dir = tempfile::tempdir().unwrap();
        let token = dir.path().join("token");
        std::fs::write(&token, "obviously-fake-token-value\n").unwrap();
        let (code, out, err) = run_capture(&[
            "serve",
            "--relay",
            "ws://127.0.0.1:8787/relay",
            "--token-file",
            token.to_str().unwrap(),
            "--data-dir",
            dir.path().to_str().unwrap(),
        ]);
        assert_eq!(code, 3);
        assert!(err.contains("web_link serve not implemented yet: TODO(E-01:WS-D1 R-05)"));
        assert!(!out.contains("obviously-fake") && !err.contains("obviously-fake"));
    }

    #[test]
    fn serve_exit_2_when_the_token_file_is_missing_or_empty() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().to_str().unwrap().to_string();
        let missing = dir.path().join("missing");
        assert_eq!(
            run_capture(&[
                "serve",
                "--relay",
                "ws://x",
                "--token-file",
                missing.to_str().unwrap(),
                "--data-dir",
                &data
            ])
            .0,
            2
        );
        let empty = dir.path().join("empty");
        std::fs::write(&empty, "  \n").unwrap();
        assert_eq!(
            run_capture(&[
                "serve",
                "--relay",
                "ws://x",
                "--token-file",
                empty.to_str().unwrap(),
                "--data-dir",
                &data
            ])
            .0,
            2
        );
    }
}
