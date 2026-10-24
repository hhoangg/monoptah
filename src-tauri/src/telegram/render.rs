//! Pure rendering of question messages (Telegram HTML) and inline keyboards.
//!
//! Every piece of dynamic text is escaped here, so callers never build HTML by hand.
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
    /// How long the question waited before it was sent; 0 omits the "Waiting for" line.
    /// Payloads from before the setting existed carry none and meant 5 minutes.
    #[serde(default = "default_delay_secs")]
    pub delay_secs: u64,
    pub questions: Vec<QuestionIn>,
}

fn default_delay_secs() -> u64 {
    super::config::DEFAULT_DELAY_SECS
}

/// "30 s", "1 min", "5 min", "1 h": the compact form of a wait. Whole hours and whole
/// minutes use the larger unit, anything else is shown in seconds.
pub fn wait_label(secs: u64) -> String {
    if secs > 0 && secs.is_multiple_of(3600) {
        format!("{} h", secs / 3600)
    } else if secs > 0 && secs.is_multiple_of(60) {
        format!("{} min", secs / 60)
    } else {
        format!("{secs} s")
    }
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

/// Telegram rejects a message whose text is longer than this many characters.
const MESSAGE_LIMIT: usize = 4096;
/// Room kept for the "Selected: ..." line that multi-select edits add.
const SELECTED_RESERVE: usize = 600;
/// Room for the status line of a settled question.
const STATUS_RESERVE: usize = 600;
const ELLIPSIS: char = '\u{2026}';

/// Length Telegram could count: the escaped HTML (tags and entities included, which is
/// more than the parsed text) in UTF-16 units, so emoji count as two.
fn units(text: &str) -> usize {
    text.encode_utf16().count()
}

/// Keeps at most `cap` characters of `text`, ending in an ellipsis when it cut. This works
/// on raw text, before escaping, so an entity or tag is never split.
fn clip_raw(text: &str, cap: usize) -> String {
    if text.chars().count() <= cap {
        return text.to_string();
    }
    if cap == 0 {
        return String::new();
    }
    let kept: String = text.chars().take(cap - 1).collect();
    format!("{}{ELLIPSIS}", kept.trim_end())
}

/// Largest cap in `lo..=hi` for which `fits` holds, assuming a larger cap never makes the
/// message shorter. `None` when even `lo` does not fit.
fn largest_fitting(lo: usize, hi: usize, fits: impl Fn(usize) -> bool) -> Option<usize> {
    if !fits(lo) {
        return None;
    }
    let (mut good, mut bad) = (lo, hi + 1);
    while bad - good > 1 {
        let mid = good + (bad - good) / 2;
        if fits(mid) {
            good = mid;
        } else {
            bad = mid;
        }
    }
    Some(good)
}

/// Escaped, clipped text that takes at most `max_units`.
fn esc_within(text: &str, max_units: usize) -> String {
    let escaped = |cap: usize| esc(&clip_raw(text, cap));
    let hi = text.chars().count();
    match largest_fitting(0, hi, |cap| units(&escaped(cap)) <= max_units) {
        Some(cap) => escaped(cap),
        None => String::new(),
    }
}

/// Per-part limits (raw characters) the fitting loop tightens until the message fits.
#[derive(Clone)]
struct Caps {
    context: usize,
    description: usize,
    prompt: usize,
    label: usize,
    question_header: usize,
    header: usize,
    /// How many option lines are listed.
    options: usize,
}

impl Caps {
    fn loose() -> Self {
        Caps {
            context: MESSAGE_LIMIT,
            description: MESSAGE_LIMIT,
            prompt: MESSAGE_LIMIT,
            label: MESSAGE_LIMIT,
            question_header: MESSAGE_LIMIT,
            header: MESSAGE_LIMIT,
            options: usize::MAX,
        }
    }
}

type CapField = fn(&mut Caps) -> &mut usize;

/// Parts in the order they are trimmed (the context goes first, headers and labels last),
/// each with the smallest cap it may reach. The option list is the final resort.
const TRIM_ORDER: [(CapField, usize, usize); 7] = [
    (|c| &mut c.context, 0, MESSAGE_LIMIT),
    (|c| &mut c.description, 0, MESSAGE_LIMIT),
    (|c| &mut c.prompt, 40, MESSAGE_LIMIT),
    (|c| &mut c.label, 24, MESSAGE_LIMIT),
    (|c| &mut c.question_header, 40, MESSAGE_LIMIT),
    (|c| &mut c.header, 60, MESSAGE_LIMIT),
    // The ceiling of the option list is the number of options, filled in by `fit`.
    (|c| &mut c.options, 0, usize::MAX),
];

/// Builds the message with the loosest caps that keep it within `budget` units. The
/// message is always produced: the last stage drops option lines, and what remains is
/// bounded by the floors above.
fn fit(budget: usize, option_count: usize, build: impl Fn(&Caps) -> String) -> String {
    let mut caps = Caps::loose();
    for (field, floor, ceiling) in TRIM_ORDER {
        if units(&build(&caps)) <= budget {
            break;
        }
        let hi = if ceiling == usize::MAX {
            option_count
        } else {
            ceiling
        };
        let best = largest_fitting(floor, hi, |cap| {
            let mut trial = caps.clone();
            *field(&mut trial) = cap;
            units(&build(&trial)) <= budget
        });
        *field(&mut caps) = best.unwrap_or(floor);
    }
    build(&caps)
}

fn header_lines(p: &ForwardPayload, caps: &Caps) -> String {
    format!("<b>{}</b>\n", esc(&clip_raw(&p.header, caps.header)))
}

/// Question header (when present), position in a multi-question prompt, and the prompt.
fn question_lines(p: &ForwardPayload, q: &QuestionIn, q_index: usize, caps: &Caps) -> String {
    let mut out = String::new();
    if p.questions.len() > 1 {
        out.push_str(&format!(
            "Question {} of {}\n",
            q_index + 1,
            p.questions.len()
        ));
    }
    if let Some(header) = q.header.as_deref().filter(|h| !h.trim().is_empty()) {
        out.push_str(&format!(
            "<b>{}</b>\n",
            esc(&clip_raw(header, caps.question_header))
        ));
    }
    out.push_str(&esc(&clip_raw(&q.prompt, caps.prompt)));
    out.push('\n');
    out
}

fn option_lines(q: &QuestionIn, caps: &Caps) -> String {
    let mut out = String::new();
    for (index, option) in q.options.iter().enumerate().take(caps.options) {
        out.push_str(&format!(
            "{}. <b>{}</b>",
            index + 1,
            esc(&clip_raw(&option.label, caps.label))
        ));
        if let Some(description) = option
            .description
            .as_deref()
            .filter(|d| !d.trim().is_empty() && caps.description > 0)
        {
            out.push_str(&format!(
                " - {}",
                esc(&clip_raw(description, caps.description))
            ));
        }
        out.push('\n');
    }
    if q.options.len() > caps.options {
        out.push_str(&format!(
            "{ELLIPSIS} and {} more\n",
            q.options.len() - caps.options
        ));
    }
    out
}

pub fn question_html(p: &ForwardPayload, q_index: usize, selected: &BTreeSet<String>) -> String {
    let Some(q) = p.questions.get(q_index) else {
        return header_lines(p, &Caps::loose());
    };
    // A multi-select message keeps room for the "Selected: ..." line, whether or not it
    // is shown yet, so choosing options never changes the rest of the text.
    let reserve = if q.multi_select { SELECTED_RESERVE } else { 0 };
    let mut out = fit(MESSAGE_LIMIT - reserve, q.options.len(), |caps| {
        let mut out = header_lines(p, caps);
        if p.delay_secs > 0 {
            out.push_str(&format!("Waiting for {}\n", wait_label(p.delay_secs)));
        }
        if caps.context > 0 && !p.context.trim().is_empty() {
            out.push_str(&format!(
                "\n<blockquote>{}</blockquote>\n",
                esc(&clip_raw(&p.context, caps.context))
            ));
        }
        out.push('\n');
        out.push_str(&question_lines(p, q, q_index, caps));
        out.push('\n');
        out.push_str(&option_lines(q, caps));
        out
    });
    if q.multi_select && !selected.is_empty() {
        let labels: Vec<&str> = q
            .options
            .iter()
            .filter(|option| selected.contains(&option.id))
            .map(|option| option.label.as_str())
            .collect();
        if !labels.is_empty() {
            let prefix = "\nSelected: ";
            let room = SELECTED_RESERVE - units(prefix) - 1;
            out.push_str(&format!(
                "{prefix}{}\n",
                esc_within(&labels.join(", "), room)
            ));
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
    let status_line = format!("<b>{}</b>", esc_within(status, STATUS_RESERVE - 7));
    let mut out = fit(MESSAGE_LIMIT - units(&status_line), 0, |caps| {
        let mut out = header_lines(p, caps);
        out.push('\n');
        if let Some(q) = p.questions.get(q_index) {
            out.push_str(&question_lines(p, q, q_index, caps));
            out.push('\n');
        }
        out
    });
    out.push_str(&status_line);
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
            delay_secs: 300,
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
    fn wait_label_is_compact() {
        for (secs, label) in [
            (1, "1 s"),
            (30, "30 s"),
            (60, "1 min"),
            (90, "90 s"),
            (120, "2 min"),
            (300, "5 min"),
            (1800, "30 min"),
            (3600, "1 h"),
            (7200, "2 h"),
        ] {
            assert_eq!(wait_label(secs), label, "{secs}");
        }
    }

    #[test]
    fn header_shows_the_configured_delay() {
        for (secs, line) in [
            (30, "Waiting for 30 s"),
            (60, "Waiting for 1 min"),
            (300, "Waiting for 5 min"),
            (3600, "Waiting for 1 h"),
        ] {
            let mut p = payload(vec![question("q1", false, vec![option("a", "A")])]);
            p.delay_secs = secs;
            let html = question_html(&p, 0, &BTreeSet::new());
            assert!(html.contains(line), "{secs}: {html}");
            if secs != 300 {
                assert!(!html.contains("Waiting for 5 min"), "{secs}: {html}");
            }
        }
    }

    #[test]
    fn zero_delay_omits_the_waiting_line() {
        let mut p = payload(vec![question("q1", false, vec![option("a", "A")])]);
        p.delay_secs = 0;
        let html = question_html(&p, 0, &BTreeSet::new());
        assert!(!html.contains("Waiting"), "{html}");
        // The header and the context still read as separate blocks.
        assert!(html.starts_with("<b>proj - Session</b>\n"), "{html}");
        assert!(html.contains("<blockquote>Because of the build</blockquote>"));
    }

    #[test]
    fn zero_delay_message_still_fits_the_limit() {
        let mut q = question("q1", false, vec![option("a", "Alpha")]);
        q.prompt = "word ".repeat(5000);
        let mut p = payload(vec![q]);
        p.delay_secs = 0;
        p.context = "c".repeat(600);
        assert_fits(&question_html(&p, 0, &BTreeSet::new()));
    }

    #[test]
    fn payload_without_delay_secs_still_parses_as_five_minutes() {
        let old = r#"{"project":"p","sessionId":"s","requestId":1,"topicTitle":"t",
            "header":"h","context":"","questions":[]}"#;
        let parsed: ForwardPayload = serde_json::from_str(old).unwrap();
        assert_eq!(parsed.delay_secs, 300);
        let zero = old.replace("\"questions\"", "\"delaySecs\":0,\"questions\"");
        let parsed: ForwardPayload = serde_json::from_str(&zero).unwrap();
        assert_eq!(parsed.delay_secs, 0);
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

    // ----- 4096 limit -----

    fn units(text: &str) -> usize {
        text.encode_utf16().count()
    }

    /// No '&' outside a known entity, and the tags this module emits are balanced.
    fn assert_well_formed(html: &str) {
        let mut rest = html;
        while let Some(at) = rest.find('&') {
            let tail = &rest[at..];
            assert!(
                tail.starts_with("&amp;") || tail.starts_with("&lt;") || tail.starts_with("&gt;"),
                "broken entity near {:?}",
                tail.chars().take(10).collect::<String>()
            );
            rest = &tail[1..];
        }
        for tag in ["b", "blockquote"] {
            assert_eq!(
                html.matches(&format!("<{tag}>")).count(),
                html.matches(&format!("</{tag}>")).count(),
                "unbalanced <{tag}>"
            );
        }
        assert!(
            !html
                .replace("<b>", "")
                .replace("</b>", "")
                .replace("<blockquote>", "")
                .replace("</blockquote>", "")
                .contains('<'),
            "stray tag in {html}"
        );
    }

    fn assert_fits(html: &str) {
        assert!(units(html) <= 4096, "{} units", units(html));
        assert_well_formed(html);
    }

    #[test]
    fn very_long_prompt_is_trimmed_with_ellipsis() {
        let mut q = question("q1", false, vec![option("a", "Alpha"), option("b", "Beta")]);
        q.prompt = "word ".repeat(5000);
        let p = payload(vec![q]);
        let html = question_html(&p, 0, &BTreeSet::new());
        assert_fits(&html);
        assert!(html.contains('\u{2026}'), "no ellipsis");
        assert!(html.contains("<b>Alpha</b>") && html.contains("<b>Beta</b>"));
        assert!(html.contains("Waiting for 5 min") && html.contains("proj - Session"));
    }

    #[test]
    fn very_long_context_is_trimmed_first() {
        let mut q = question("q1", false, vec![option("a", "Alpha")]);
        q.prompt = "p".repeat(3000);
        let mut p = payload(vec![q]);
        p.context = "c".repeat(5000);
        let html = question_html(&p, 0, &BTreeSet::new());
        assert_fits(&html);
        // The prompt survives whole: context is trimmed before the prompt.
        assert!(html.contains(&"p".repeat(3000)), "prompt was trimmed");
    }

    #[test]
    fn thirty_options_with_long_descriptions_keep_every_label() {
        let options: Vec<OptionIn> = (0..30)
            .map(|i| {
                let mut o = option(&format!("id{i}"), &format!("Label number {i}"));
                o.description = Some("d".repeat(400));
                o
            })
            .collect();
        let p = payload(vec![question("q1", false, options)]);
        let html = question_html(&p, 0, &BTreeSet::new());
        assert_fits(&html);
        for i in 0..30 {
            assert!(html.contains(&format!("<b>Label number {i}</b>")), "{i}");
        }
    }

    #[test]
    fn multi_select_with_many_long_selected_labels_fits_and_keeps_reserve() {
        let options: Vec<OptionIn> = (0..40)
            .map(|i| option(&format!("id{i}"), &format!("{i} {}", "L".repeat(150))))
            .collect();
        let selected: BTreeSet<String> = (0..40).map(|i| format!("id{i}")).collect();
        let p = payload(vec![question("q1", true, options)]);
        let none = question_html(&p, 0, &BTreeSet::new());
        let all = question_html(&p, 0, &selected);
        assert_fits(&none);
        assert_fits(&all);
        assert!(all.contains("Selected: "), "{all}");
        // Choosing options must not push the unchanged part of the message around.
        let static_part = none.trim_end();
        assert!(all.starts_with(static_part), "static part shifted");
    }

    #[test]
    fn status_from_a_long_prompt_and_long_answer_fits() {
        let mut q = question("q1", false, vec![option("a", "Alpha")]);
        q.prompt = "x".repeat(9000);
        let mut p = payload(vec![q]);
        p.context = "c".repeat(900);
        let status = format!("Answered by @alice: {}", "y".repeat(9000));
        let html = status_html(&p, 0, &status);
        assert_fits(&html);
        assert!(html.contains("Answered by @alice: "), "status was dropped");
        assert!(html.ends_with("</b>"));
    }

    #[test]
    fn entity_heavy_text_is_measured_after_escaping() {
        let mut q = question("q1", false, vec![option("a", "&<>&<>")]);
        q.prompt = "&<>".repeat(1000); // 3000 raw chars, 12000 escaped
        q.options[0].description = Some("<&>".repeat(500));
        let mut p = payload(vec![q]);
        p.context = "&".repeat(600);
        let html = question_html(&p, 0, &BTreeSet::new());
        assert_fits(&html);
        let status = status_html(&p, 0, &"<&>".repeat(800));
        assert_fits(&status);
    }

    #[test]
    fn multi_byte_text_is_counted_in_utf16_units() {
        let mut q = question("q1", false, vec![option("a", "Alpha")]);
        q.prompt = "\u{1F600}".repeat(3000); // 3000 chars, 6000 UTF-16 units
        let p = payload(vec![q]);
        let html = question_html(&p, 0, &BTreeSet::new());
        assert_fits(&html);
        assert!(html.contains('\u{2026}'));
    }

    #[test]
    fn message_at_exactly_the_limit_is_not_trimmed() {
        let mut q = question("q1", false, vec![option("a", "Alpha")]);
        q.prompt = String::new();
        let mut p = payload(vec![q]);
        let base = units(&question_html(&p, 0, &BTreeSet::new()));
        p.questions[0].prompt = "a".repeat(4096 - base);
        let html = question_html(&p, 0, &BTreeSet::new());
        assert_eq!(units(&html), 4096);
        assert!(!html.contains('\u{2026}'), "trimmed at the limit");

        p.questions[0].prompt = "a".repeat(4096 - base + 1);
        let html = question_html(&p, 0, &BTreeSet::new());
        assert!(units(&html) <= 4096);
        assert!(html.contains('\u{2026}'), "not trimmed above the limit");
    }

    #[test]
    fn absurd_inputs_still_produce_a_message() {
        let options: Vec<OptionIn> = (0..300)
            .map(|i| {
                let mut o = option(&format!("id{i}"), &format!("{i} {}", "L".repeat(500)));
                o.description = Some("d".repeat(500));
                o
            })
            .collect();
        let mut q = question("q1", true, options);
        q.prompt = "p".repeat(10_000);
        q.header = Some("h".repeat(5000));
        let mut p = payload(vec![q]);
        p.header = "H".repeat(5000);
        p.context = "c".repeat(5000);
        let selected: BTreeSet<String> = (0..300).map(|i| format!("id{i}")).collect();
        let html = question_html(&p, 0, &selected);
        assert_fits(&html);
        assert!(html.contains("Waiting for 5 min"));
    }
}
