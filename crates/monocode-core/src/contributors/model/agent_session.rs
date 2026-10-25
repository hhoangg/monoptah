//! Pure helpers for starting an agent session from the Contributors page. The
//! app owns the session and the file; this module only shapes the data. Port
//! of `agentSession.ts` (its base64 helper is not needed: the native path
//! writes bytes, see `crate::attachment`).

use super::js_trim;
use crate::provider::Kind;

/// `contributors-2026-10.json` becomes "Contributors 2026-10"; the All time
/// snapshot, `contributors-all.json`, becomes "Contributors all time".
pub fn contributors_session_title(file_name: &str) -> String {
    // /^contributors-(.+)\.json$/ where `.` does not match a line break.
    let id = file_name
        .strip_prefix("contributors-")
        .and_then(|rest| rest.strip_suffix(".json"))
        .filter(|id| !id.is_empty() && !id.contains(['\n', '\r', '\u{2028}', '\u{2029}']));
    match id {
        Some("all") => "Contributors all time".into(),
        Some(id) => format!("Contributors {id}"),
        None => "Contributors".into(),
    }
}

/// The first message. A provider that cannot take attachments still gets the
/// snapshot: the prompt names the file so the agent can read it with its tools.
pub fn compose_session_prompt(prompt: &str, path: &str, attached: bool) -> String {
    if attached {
        return prompt.to_string();
    }
    format!("{prompt}\n\nThe snapshot could not be attached to this provider. Read it from this file: {path}")
}

/// The chosen provider, or `None` for the project default.
pub fn resolve_harness(raw: Option<&str>, known: &[Kind]) -> Result<Option<Kind>, String> {
    let Some(raw) = raw.filter(|r| !r.is_empty()) else {
        return Ok(None);
    };
    known
        .iter()
        .copied()
        .find(|kind| kind.id() == raw)
        .map(Some)
        .ok_or_else(|| format!("The provider \"{raw}\" is not available."))
}

/// Turns any failure into one readable line: `Could not <step>: <detail>`.
pub fn failure_message(step: &str, error: Option<&str>) -> String {
    let detail = js_trim(error.unwrap_or(""));
    if detail.is_empty() {
        format!("Could not {step}.")
    } else {
        format!("Could not {step}: {detail}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // utf8ToBase64 (3 tests) is not ported: the native attachment path writes
    // bytes, there is no base64 hop.

    #[test]
    fn contributors_session_title_names_the_period_from_the_file_name() {
        assert_eq!(
            contributors_session_title("contributors-2026-10.json"),
            "Contributors 2026-10"
        );
    }

    #[test]
    fn contributors_session_title_reads_the_all_time_snapshot_as_all_time() {
        assert_eq!(
            contributors_session_title("contributors-all.json"),
            "Contributors all time"
        );
    }

    #[test]
    fn contributors_session_title_falls_back_to_a_plain_title_for_other_names() {
        assert_eq!(contributors_session_title("notes.json"), "Contributors");
        assert_eq!(
            contributors_session_title("contributors-.json"),
            "Contributors"
        );
    }

    #[test]
    fn compose_session_prompt_keeps_the_prompt_when_the_snapshot_is_attached() {
        assert_eq!(
            compose_session_prompt("Question", "/tmp/x.json", true),
            "Question"
        );
    }

    #[test]
    fn compose_session_prompt_points_at_the_written_file_when_it_could_not_be_attached() {
        let path = "/tmp/monocode-attachments/1-contributors-2026-10.json";
        let text = compose_session_prompt("Question", path, false);
        assert!(text.starts_with("Question\n"));
        assert!(text.contains(path));
        assert!(text.contains("could not be attached"));
        assert_eq!(
            text,
            format!("Question\n\nThe snapshot could not be attached to this provider. Read it from this file: {path}")
        );
    }

    #[test]
    fn resolve_harness_returns_undefined_when_no_provider_was_chosen() {
        let known = [Kind::Claude, Kind::Codex];
        assert_eq!(resolve_harness(None, &known), Ok(None));
        assert_eq!(resolve_harness(Some(""), &known), Ok(None));
    }

    #[test]
    fn resolve_harness_accepts_a_known_provider_and_rejects_an_unknown_one() {
        let known = [Kind::Claude, Kind::Codex];
        assert_eq!(
            resolve_harness(Some("codex"), &known),
            Ok(Some(Kind::Codex))
        );
        assert_eq!(
            resolve_harness(Some("nope"), &known),
            Err("The provider \"nope\" is not available.".to_string())
        );
        // A real provider that the caller did not list is not available either.
        assert!(resolve_harness(Some("cursor"), &known).is_err());
    }

    #[test]
    fn failure_message_prefixes_the_step_and_keeps_the_original_text() {
        assert_eq!(
            failure_message(
                "write the snapshot",
                Some("File is too large to attach (maximum 20 MB).")
            ),
            "Could not write the snapshot: File is too large to attach (maximum 20 MB)."
        );
        assert_eq!(
            failure_message("attach the snapshot", Some("boom")),
            "Could not attach the snapshot: boom"
        );
    }

    #[test]
    fn failure_message_copes_with_values_that_carry_no_text() {
        assert_eq!(
            failure_message("write the snapshot", None),
            "Could not write the snapshot."
        );
        assert_eq!(
            failure_message("write the snapshot", Some("")),
            "Could not write the snapshot."
        );
        assert_eq!(
            failure_message("write the snapshot", Some("  \n")),
            "Could not write the snapshot."
        );
    }
}
