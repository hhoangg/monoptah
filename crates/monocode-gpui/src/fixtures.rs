//! Local reference layout for the chat presentation.
use monocode_core::{
    tool::{ToolCall, ToolState},
    transcript::Transcript,
};
pub(crate) fn chat() -> Transcript {
    let mut t = Transcript::default();
    t.begin("assistant");
    t.append("The new regression tests pass. They cover Markdown across storage chunks, repeated tool snapshots, and expanding reasoning while output changes. I also fixed a scroll issue where opening a disclosure could jump to the tail, and limited syntax highlighting to one background job at a time.");
    for (id, name, input, output, state) in [
        (
            "read",
            "Read",
            r#"{"file_path":"docs/native/migration.md"}"#,
            "",
            ToolState::Completed,
        ),
        (
            "edit1",
            "Edit",
            r#"{"file_path":"crates/monocode-core/src/provider.rs"}"#,
            "",
            ToolState::Completed,
        ),
        (
            "cmd1",
            "Bash",
            r#"{"command":"python3 — <<'PY'"}"#,
            "",
            ToolState::Completed,
        ),
        (
            "cmd2",
            "Bash",
            r#"{"command":"cargo fmt --all"}"#,
            "error: let chains are only allowed in Rust 2024 or later",
            ToolState::Failed,
        ),
        (
            "edit2",
            "Edit",
            r#"{"file_path":"crates/monocode-core/src/provider.rs"}"#,
            "",
            ToolState::Completed,
        ),
        (
            "find",
            "Grep",
            r#"{"pattern":"fn simulate_click|fn bounds_for|debug_bounds"}"#,
            "",
            ToolState::Running,
        ),
    ] {
        t.begin("tool");
        t.rows.last_mut().unwrap().text = serde_json::to_string(&ToolCall {
            id: id.into(),
            name: name.into(),
            input: input.into(),
            output: output.into(),
            state,
            panel: None,
            display: None,
        })
        .unwrap();
    }
    t
}

/// Normal-session examples; no provider credentials or live turn are needed.
pub(crate) fn visuals() -> Transcript {
    let mut transcript = Transcript::default();
    transcript.begin("user");
    transcript
        .append("Show interactive examples for exploring data and three-dimensional geometry.");
    for (id, title, source) in [
        (
            "example-dashboard",
            "Throughput explorer",
            include_str!("../../../docs/native/examples/visual-dashboard.js"),
        ),
        (
            "example-geometry",
            "Geometry explorer",
            include_str!("../../../docs/native/examples/visual-scene.js"),
        ),
    ] {
        let document = monocode_core::visual::VisualDocument {
            id: id.into(),
            title: title.into(),
            source: source.into(),
        };
        transcript.begin("visual");
        transcript.append(&serde_json::to_string(&document).unwrap());
    }
    transcript.begin("assistant");
    transcript.append("The examples use a sample dataset. Their controls update locally, so you can explore them without starting an agent turn.");
    transcript
}

#[cfg(test)]
mod tests {
    #[test]
    fn native_visual_examples_pass_the_real_runtime_and_persist_as_messages() {
        let rows = super::visuals().rows;
        let mut index = 0;
        let mut count = 0;
        while index < rows.len() {
            let end = (index + 1..rows.len())
                .find(|&next| rows[next].first)
                .unwrap_or(rows.len());
            if rows[index].role == "visual" {
                let source = rows[index..end]
                    .iter()
                    .map(|row| row.text.as_str())
                    .collect::<String>();
                let document: monocode_core::visual::VisualDocument =
                    serde_json::from_str(&source).unwrap();
                document.validate().unwrap();
                monocode_core::visual::check(&document.source).unwrap();
                count += 1;
            }
            index = end;
        }
        assert_eq!(count, 2);
    }
}
