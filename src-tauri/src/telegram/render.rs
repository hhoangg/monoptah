//! Pure rendering of question messages (Telegram HTML) and inline keyboards.
//!
//! Every piece of dynamic text is escaped here, so callers never build HTML by hand.
// Consumed by Task 5, where this allow is removed.
#![allow(dead_code)]

use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::BTreeSet;

/// Option id the app uses for a free-text answer (mirrors `CUSTOM_OPTION_ID` in
/// `userQuestion.ts`).
pub const CUSTOM_OPTION_ID: &str = "__custom__";

/// Payload sent by the frontend when a question has been pending long enough.
#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ForwardPayload {
    pub project: String,
    pub session_id: String,
    pub request_id: u64,
    pub topic_title: String,
    /// project, session, ticket and workflow line.
    pub header: String,
    /// Tail of the agent's last message, at most 600 characters.
    pub context: String,
    pub questions: Vec<QuestionIn>,
}

#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct QuestionIn {
    pub id: String,
    pub header: Option<String>,
    pub prompt: String,
    pub multi_select: bool,
    pub allow_custom: bool,
    pub options: Vec<OptionIn>,
}

#[derive(Deserialize, Clone, Debug)]
pub struct OptionIn {
    pub id: String,
    pub label: String,
    pub description: Option<String>,
}

/// An option the app treats as the free-text choice (mirrors `isOtherOption`).
pub fn is_other(option: &OptionIn) -> bool {
    option.id == CUSTOM_OPTION_ID || option.label.trim().eq_ignore_ascii_case("other")
}

/// Escapes the three characters Telegram HTML parse mode treats as markup.
fn esc(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Shows at most `max` characters of a button label.
fn clip(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let kept: String = text.chars().take(max.saturating_sub(3)).collect();
    format!("{kept}...")
}

fn header_lines(p: &ForwardPayload) -> String {
    format!("<b>{}</b>\n", esc(&p.header))
}

/// Question header (when present), position in a multi-question prompt, and the prompt.
fn question_lines(p: &ForwardPayload, q: &QuestionIn, q_index: usize) -> String {
    let mut out = String::new();
    if p.questions.len() > 1 {
        out.push_str(&format!(
            "Question {} of {}\n",
            q_index + 1,
            p.questions.len()
        ));
    }
    if let Some(header) = q.header.as_deref().filter(|h| !h.trim().is_empty()) {
        out.push_str(&format!("<b>{}</b>\n", esc(header)));
    }
    out.push_str(&esc(&q.prompt));
    out.push('\n');
    out
}

fn option_lines(q: &QuestionIn) -> String {
    let mut out = String::new();
    for (index, option) in q.options.iter().enumerate() {
        out.push_str(&format!("{}. <b>{}</b>", index + 1, esc(&option.label)));
        if let Some(description) = option
            .description
            .as_deref()
            .filter(|d| !d.trim().is_empty())
        {
            out.push_str(&format!(" - {}", esc(description)));
        }
        out.push('\n');
    }
    out
}

pub fn question_html(p: &ForwardPayload, q_index: usize, selected: &BTreeSet<String>) -> String {
    let Some(q) = p.questions.get(q_index) else {
        return header_lines(p);
    };
    let mut out = header_lines(p);
    out.push_str("Waiting for 5 min\n");
    if !p.context.trim().is_empty() {
        out.push_str(&format!("\n<blockquote>{}</blockquote>\n", esc(&p.context)));
    }
    out.push('\n');
    out.push_str(&question_lines(p, q, q_index));
    out.push('\n');
    out.push_str(&option_lines(q));
    if q.multi_select && !selected.is_empty() {
        let labels: Vec<String> = q
            .options
            .iter()
            .filter(|option| selected.contains(&option.id))
            .map(|option| esc(&option.label))
            .collect();
        if !labels.is_empty() {
            out.push_str(&format!("\nSelected: {}\n", labels.join(", ")));
        }
    }
    out
}

fn button(text: String, data: String) -> Value {
    json!({ "text": text, "callback_data": data })
}

pub fn keyboard(token: &str, q: &QuestionIn, selected: &BTreeSet<String>) -> Value {
    let mut rows: Vec<Value> = Vec::new();
    for (index, option) in q.options.iter().enumerate() {
        let mark = if q.multi_select && selected.contains(&option.id) {
            "\u{2713} "
        } else {
            ""
        };
        let text = clip(&format!("{mark}{}", option.label), 48);
        rows.push(json!([button(text, format!("o:{token}:{index}"))]));
    }
    // An option the app already treats as "Other" makes a second button redundant.
    if q.allow_custom && !q.options.iter().any(is_other) {
        let mark = if selected.contains(CUSTOM_OPTION_ID) {
            "\u{2713} "
        } else {
            ""
        };
        rows.push(json!([button(
            format!("{mark}Other (reply with text)"),
            format!("c:{token}")
        )]));
    }
    let skip = button("Skip".into(), format!("k:{token}"));
    if q.multi_select {
        rows.push(json!([button("Send".into(), format!("s:{token}")), skip]));
    } else {
        rows.push(json!([skip]));
    }
    json!({ "inline_keyboard": rows })
}

/// The message after the question is settled: no options, no buttons, one status line.
pub fn status_html(p: &ForwardPayload, q_index: usize, status: &str) -> String {
    let mut out = header_lines(p);
    out.push('\n');
    if let Some(q) = p.questions.get(q_index) {
        out.push_str(&question_lines(p, q, q_index));
        out.push('\n');
    }
    out.push_str(&format!("<b>{}</b>", esc(status)));
    out
}

const TOKEN_LEN: usize = 10;
const TOKEN_ALPHABET: &[u8; 62] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";

/// A random callback token of 10 characters from `[a-zA-Z0-9]`.
pub fn new_token() -> String {
    // Bytes 6 and 8 of a v4 UUID carry the fixed version and variant bits, so they are
    // not random: skip them. Bytes of 248 and above are rejected (248 = 4 * 62), which
    // keeps every symbol equally likely.
    let mut token = String::with_capacity(TOKEN_LEN);
    while token.len() < TOKEN_LEN {
        for (index, byte) in uuid::Uuid::new_v4().into_bytes().iter().enumerate() {
            if index == 6 || index == 8 || *byte >= 248 {
                continue;
            }
            token.push(TOKEN_ALPHABET[(*byte % 62) as usize] as char);
            if token.len() == TOKEN_LEN {
                break;
            }
        }
    }
    token
}

#[cfg(test)]
mod tests {
    use super::*;

    fn option(id: &str, label: &str) -> OptionIn {
        OptionIn {
            id: id.into(),
            label: label.into(),
            description: None,
        }
    }

    fn question(id: &str, multi: bool, options: Vec<OptionIn>) -> QuestionIn {
        QuestionIn {
            id: id.into(),
            header: None,
            prompt: format!("Prompt {id}"),
            multi_select: multi,
            allow_custom: false,
            options,
        }
    }

    fn payload(questions: Vec<QuestionIn>) -> ForwardPayload {
        ForwardPayload {
            project: "proj".into(),
            session_id: "s1".into(),
            request_id: 7,
            topic_title: "Topic".into(),
            header: "proj - Session".into(),
            context: "Because of the build".into(),
            questions,
        }
    }

    #[test]
    fn renders_escaped_html() {
        let mut q = question("q1", false, vec![option("a", "A <i>&</i>")]);
        q.prompt = "Use <b>bold</b> & more?".into();
        q.header = Some("Head <x>".into());
        q.options[0].description = Some("desc <u>".into());
        let mut p = payload(vec![q]);
        p.header = "Proj <b>&".into();
        p.context = "ctx <script>".into();

        let html = question_html(&p, 0, &BTreeSet::new());
        assert!(
            html.contains("Use &lt;b&gt;bold&lt;/b&gt; &amp; more?"),
            "{html}"
        );
        assert!(html.contains("Proj &lt;b&gt;&amp;"), "{html}");
        assert!(html.contains("ctx &lt;script&gt;"), "{html}");
        assert!(html.contains("Head &lt;x&gt;"), "{html}");
        assert!(html.contains("A &lt;i&gt;&amp;&lt;/i&gt;"), "{html}");
        assert!(html.contains("desc &lt;u&gt;"), "{html}");
        assert!(
            !html.contains("<script>") && !html.contains("<u>"),
            "{html}"
        );
        assert!(html.contains("Waiting for 5 min"), "{html}");
    }

    #[test]
    fn status_html_escapes_and_has_no_options() {
        let mut p = payload(vec![question("q1", false, vec![option("a", "Alpha")])]);
        p.questions[0].prompt = "Go <now>?".into();
        let html = status_html(&p, 0, "Answered by @a<b>: Alpha");
        assert!(html.contains("Go &lt;now&gt;?"), "{html}");
        assert!(html.contains("Answered by @a&lt;b&gt;: Alpha"), "{html}");
        assert!(!html.contains("Waiting for"), "{html}");
    }

    fn all_callback_data(kb: &Value) -> Vec<String> {
        kb["inline_keyboard"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|row| row.as_array().unwrap().iter())
            .map(|button| button["callback_data"].as_str().unwrap().to_string())
            .collect()
    }

    #[test]
    fn callback_data_fits_64_bytes() {
        let options: Vec<OptionIn> = (0..100)
            .map(|i| option(&format!("id{i}"), &format!("Option {i}")))
            .collect();
        let mut q = question("q1", true, options);
        q.allow_custom = true;
        let token = new_token();
        let kb = keyboard(&token, &q, &BTreeSet::new());
        let data = all_callback_data(&kb);
        assert!(data.iter().any(|d| d == &format!("o:{token}:99")));
        assert!(data.iter().all(|d| d.len() <= 64), "{data:?}");
        for prefix in ["s:", "k:", "c:"] {
            assert!(data.iter().any(|d| d == &format!("{prefix}{token}")));
        }
    }

    #[test]
    fn multi_select_keyboard_marks_selected_and_has_send() {
        let q = question("q1", true, vec![option("a", "Alpha"), option("b", "Beta")]);
        let selected: BTreeSet<String> = ["b".to_string()].into();
        let kb = keyboard("tok", &q, &selected);
        let text = kb.to_string();
        assert!(
            text.contains("\\u2713 Beta") || text.contains("✓ Beta"),
            "{text}"
        );
        assert!(!text.contains("✓ Alpha"));
        let data = all_callback_data(&kb);
        assert!(data.contains(&"s:tok".to_string()));
        let single = keyboard(
            "tok",
            &question("q1", false, vec![option("a", "A")]),
            &selected,
        );
        assert!(!all_callback_data(&single).contains(&"s:tok".to_string()));
    }

    #[test]
    fn tokens_are_ten_alphanumeric_and_distinct() {
        let tokens: BTreeSet<String> = (0..2000).map(|_| new_token()).collect();
        assert_eq!(tokens.len(), 2000);
        for token in &tokens {
            assert_eq!(token.len(), 10);
            assert!(token.chars().all(|c| c.is_ascii_alphanumeric()), "{token}");
        }
    }
}
