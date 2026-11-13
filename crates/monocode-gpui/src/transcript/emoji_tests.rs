use super::*;
use crate::markdown::{self, MarkdownRow, Part};
use crate::markdown_model::parse_full;
use crate::transcript::{Body, TranscriptView};
use base64::Engine;
use gpui::{
    AppContext, Bounds, TestAppContext, VisualTestContext, WindowBounds, WindowOptions, point, px,
    size,
};
use monocode_core::mono::emoji::{Emoji, Format};
use monocode_core::store::HistoryPage;
use monocode_core::transcript::Row;
use std::collections::HashMap;

const PNG: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==";
const GIF: &str = "R0lGODlhAQABAIAAAAAAAP///yH5BAEAAAAALAAAAAABAAEAAAIBRAA7";

struct Fixture {
    dir: PathBuf,
    emoji: MonoEmoji,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn row(role: &str, text: &str) -> Row {
    Row {
        role: role.into(),
        text: text.into(),
        first: true,
        sent_at: None,
    }
}

/// A library with three "happy" pictures, one "celebrate" GIF and one "sad" whose file is
/// missing, written into a temporary Mono emoji folder.
fn fixture(enabled: bool) -> Fixture {
    static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "monoptah-emoji-render-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
    ));
    let mut library = Library {
        enabled,
        ..Library::default()
    };
    let item = |file: &str, name: &str, format, emotion: &str| Emoji {
        file: file.into(),
        hash: format!("hash-{file}"),
        name: name.into(),
        format,
        emotion: Some(emotion.into()),
    };
    let pack_id = library.add_pack(
        "Pack",
        None,
        vec![
            item("h1.png", "Smile", Format::Png, "happy"),
            item("h2.png", "Grin", Format::Png, "happy"),
            item("h3.png", "Beam", Format::Png, "happy"),
            item("c1.gif", "Dance", Format::Gif, "celebrate"),
            item("s1.png", "Gone", Format::Png, "sad"),
        ],
    );
    let pack = library
        .packs
        .iter()
        .find(|p| p.id == pack_id)
        .unwrap()
        .clone();
    for emoji in &pack.emoji {
        if emoji.file == "s1.png" {
            continue;
        }
        let path = image_path(&dir, &pack, emoji);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let encoded = if emoji.format == Format::Gif {
            GIF
        } else {
            PNG
        };
        std::fs::write(
            path,
            base64::engine::general_purpose::STANDARD
                .decode(encoded)
                .unwrap(),
        )
        .unwrap();
    }
    Fixture {
        emoji: MonoEmoji {
            session: "7".into(),
            ..MonoEmoji::new(Arc::new(library), dir.clone())
        },
        dir,
    }
}

fn rows_of(text: &str, emoji: Option<&MonoEmoji>) -> Vec<MarkdownRow> {
    rows_of_stream(text, emoji, false)
}

fn rows_of_stream(text: &str, emoji: Option<&MonoEmoji>, streaming: bool) -> Vec<MarkdownRow> {
    let mut tokens = MessageTokens::new(emoji, text, streaming);
    let mut out = Vec::new();
    for block in parse_full(text).blocks {
        out.extend(tokens.apply(markdown::rows(&block.block)));
    }
    out
}

/// Runs of the first text rows as strings: plain text as is, a picture as `<emotion>` or
/// `<<emotion>>` when large.
fn shape(rows: &[MarkdownRow]) -> Vec<Vec<String>> {
    rows.iter()
        .filter_map(|row| match &row.part {
            Part::Text { runs, .. } => Some(
                runs.iter()
                    .map(|run| match &run.style.emoji {
                        Some(pick) if pick.large => format!("<<{}>>", pick.emotion),
                        Some(pick) => format!("<{}>", pick.emotion),
                        None => run.text.clone(),
                    })
                    .collect(),
            ),
            _ => None,
        })
        .collect()
}

fn picks(rows: &[MarkdownRow]) -> Vec<PathBuf> {
    rows.iter()
        .filter_map(|row| match &row.part {
            Part::Text { runs, .. } => Some(runs),
            _ => None,
        })
        .flatten()
        .filter_map(|run| run.style.emoji.as_ref().map(|p| p.path.clone()))
        .collect()
}

#[test]
fn an_inline_token_becomes_a_small_picture_with_one_space_each_side() {
    let fx = fixture(true);
    let rows = rows_of("Great :emo[happy] job", Some(&fx.emoji));
    assert_eq!(shape(&rows), [["Great ", "<happy>", "job"]]);
    assert_eq!(
        markdown::selection_text(&rows[0]),
        "Great job",
        "the picture adds no text and no second space"
    );
}

#[test]
fn a_token_alone_on_its_line_is_large_and_the_line_breaks_around_it() {
    let fx = fixture(true);
    let rows = rows_of("Hello\n:emo[celebrate]\nbye", Some(&fx.emoji));
    assert_eq!(shape(&rows), [["Hello", "<<celebrate>>", "bye"]]);
}

#[test]
fn a_token_ending_the_streamed_text_stays_inline_until_a_line_break_follows() {
    let fx = fixture(true);
    let own_line = "Done.\n:emo[celebrate]";
    assert_eq!(
        shape(&rows_of_stream(own_line, Some(&fx.emoji), true)),
        [["Done. ", "<celebrate>"]],
        "no line break after it yet"
    );
    let with_break = format!("{own_line}\n");
    assert_eq!(
        shape(&rows_of_stream(&with_break, Some(&fx.emoji), true)),
        [["Done.", "<<celebrate>>"]]
    );
    let paragraph = "Done.\n\n:emo[celebrate]";
    assert_eq!(
        shape(&rows_of_stream(paragraph, Some(&fx.emoji), true)),
        [vec!["Done."], vec!["<celebrate>"]]
    );
    assert_eq!(
        shape(&rows_of_stream(paragraph, Some(&fx.emoji), false)),
        [vec!["Done."], vec!["<<celebrate>>"]],
        "a finished message is laid out as usual"
    );
}

#[test]
fn a_paragraph_of_only_tokens_is_large() {
    let fx = fixture(true);
    let rows = rows_of("Done.\n\n:emo[celebrate]\n\nNext", Some(&fx.emoji));
    assert_eq!(
        shape(&rows),
        [vec!["Done."], vec!["<<celebrate>>"], vec!["Next"]]
    );
}

#[test]
fn several_tokens_are_each_sized_by_their_own_place() {
    let fx = fixture(true);
    let rows = rows_of(
        "a :emo[happy] b :emo[celebrate] c\n:emo[happy]\n",
        Some(&fx.emoji),
    );
    assert_eq!(
        shape(&rows),
        [["a ", "<happy>", "b ", "<celebrate>", "c", "<<happy>>"]]
    );
}

#[test]
fn an_unknown_empty_or_missing_file_token_is_removed_without_a_double_space() {
    let fx = fixture(true);
    for token in ["nonsense", "", "sad"] {
        let rows = rows_of(&format!("Hi :emo[{token}] there"), Some(&fx.emoji));
        assert_eq!(shape(&rows), [["Hi there"]], "token {token:?}");
    }
    let rows = rows_of("Bye :emo[nonsense]", Some(&fx.emoji));
    assert_eq!(shape(&rows), [["Bye"]]);
    let rows = rows_of("Before\n\n:emo[nonsense]\n\nAfter", Some(&fx.emoji));
    assert_eq!(
        shape(&rows),
        [vec!["Before"], vec!["After"]],
        "a paragraph left empty is dropped"
    );
}

#[test]
fn the_match_ignores_case_and_surrounding_space() {
    let fx = fixture(true);
    let rows = rows_of("x :emo[ HAPPY ] y", Some(&fx.emoji));
    assert_eq!(shape(&rows), [["x ", "<happy>", "y"]]);
}

#[test]
fn a_disabled_or_missing_library_removes_every_token() {
    let off = fixture(false);
    let rows = rows_of("Hi :emo[happy] there", Some(&off.emoji));
    assert_eq!(shape(&rows), [["Hi there"]]);
    let rows = rows_of("Hi :emo[happy] there", None);
    assert_eq!(shape(&rows), [["Hi there"]]);
}

#[test]
fn code_keeps_the_literal_token() {
    let fx = fixture(true);
    let rows = rows_of(
        "Run `:emo[happy]` ok :emo[happy]\n\n```\n:emo[happy]\n```\n",
        Some(&fx.emoji),
    );
    let Part::Text { runs, .. } = &rows[0].part else {
        panic!("text row")
    };
    assert!(
        runs.iter().any(|r| r.style.code && r.text == ":emo[happy]"),
        "inline code keeps its token"
    );
    assert_eq!(picks(&rows).len(), 1, "only the plain token is a picture");
    let Part::Code { fence, .. } = &rows[1].part else {
        panic!("code row")
    };
    assert!(fence.code.contains(":emo[happy]"));
}

#[test]
fn links_are_not_scanned_and_tables_are() {
    let fx = fixture(true);
    let rows = rows_of(
        "[:emo[happy]](https://example.com)\n\n| a | b |\n|---|---|\n| :emo[happy] | x |\n",
        Some(&fx.emoji),
    );
    let Part::Text { runs, .. } = &rows[0].part else {
        panic!("text row")
    };
    assert!(runs.iter().all(|r| r.style.emoji.is_none()));
    let Part::Table { cells, .. } = &rows[1].part else {
        panic!("table row")
    };
    let cell = &cells[1][0];
    assert!(cell.iter().any(|r| r.style.emoji.is_some()));
    assert!(markdown::needs_rich(cell));
}

#[test]
fn the_pick_is_the_same_on_every_render_and_follows_the_content() {
    let fx = fixture(true);
    let text = "a :emo[happy] b :emo[happy] c :emo[happy] d :emo[happy]";
    let first = picks(&rows_of(text, Some(&fx.emoji)));
    assert_eq!(first, picks(&rows_of(text, Some(&fx.emoji))));
    assert_eq!(first.len(), 4);
    // The text after a token does not move it (a streaming reply keeps its pictures).
    let longer = format!("{text} and then more");
    assert_eq!(first, picks(&rows_of(&longer, Some(&fx.emoji))));
    let varied = (0..12)
        .map(|n| picks(&rows_of(&format!("{n} {text}"), Some(&fx.emoji))))
        .collect::<std::collections::HashSet<_>>();
    assert!(varied.len() > 1, "other messages draw other pictures");
}

fn open_view(
    cx: &mut TestAppContext,
    emoji: Option<MonoEmoji>,
    mono: bool,
) -> gpui::WindowHandle<TranscriptView> {
    cx.update(|cx| cx.set_reduce_motion(true));
    cx.update(|cx| {
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                    point(px(0.), px(0.)),
                    size(px(900.), px(700.)),
                ))),
                ..Default::default()
            },
            |_, cx| {
                cx.new(|_| {
                    let mut view = TranscriptView::new();
                    if mono {
                        view.mono_mode("cat", 0xe86b5f, "MonoCat");
                    }
                    view.emoji = emoji;
                    view
                })
            },
        )
        .unwrap()
    })
}

fn markdown_texts(view: &TranscriptView) -> Vec<String> {
    view.rows
        .iter()
        .filter_map(|row| match &row.body {
            Body::Markdown(md) => Some(markdown::selection_text(md)),
            _ => None,
        })
        .collect()
}

#[gpui::test]
fn an_os_notification_for_a_mono_reply_has_no_tokens(cx: &mut TestAppContext) {
    let fx = fixture(true);
    let window = open_view(cx, Some(fx.emoji.clone()), true);
    window
        .update(cx, |view, _, cx| {
            view.patch(
                0,
                vec![
                    row("user", "hi"),
                    row("assistant", "Nice :emo[happy] work :emo[celebrate]"),
                ],
                cx,
            );
            assert_eq!(view.notification_reply_body(), "Nice work");
        })
        .unwrap();
}

#[gpui::test]
fn a_mono_transcript_draws_inline_and_large_pictures_and_none_for_unknown(cx: &mut TestAppContext) {
    let fx = fixture(true);
    let window = open_view(cx, Some(fx.emoji.clone()), true);
    window
        .update(cx, |view, _, cx| {
            view.patch(
                0,
                vec![
                    row("user", "hi"),
                    row(
                        "assistant",
                        "Nice :emo[happy] work\n\n:emo[celebrate]\n\nHmm :emo[nonsense] ok",
                    ),
                ],
                cx,
            )
        })
        .unwrap();
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    let inline = visual.debug_bounds("mono-emoji-happy").unwrap();
    assert_eq!(inline.size, size(px(20.), px(20.)));
    let large = visual.debug_bounds("mono-emoji-large-celebrate").unwrap();
    assert_eq!(large.size, size(px(56.), px(56.)));
    assert!(visual.debug_bounds("mono-emoji-nonsense").is_none());
    assert!(visual.debug_bounds("mono-emoji-large-nonsense").is_none());
    window
        .read_with(cx, |view, _| {
            let texts = markdown_texts(view);
            assert_eq!(texts, ["Nice work", "", "Hmm ok"]);
        })
        .unwrap();
}

#[gpui::test]
fn a_transcript_that_is_not_a_mono_shows_the_text_as_is(cx: &mut TestAppContext) {
    let fx = fixture(true);
    let window = open_view(cx, Some(fx.emoji.clone()), false);
    window
        .update(cx, |view, _, cx| {
            view.patch(
                0,
                vec![row("user", "hi"), row("assistant", "Nice :emo[happy] work")],
                cx,
            )
        })
        .unwrap();
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    assert!(visual.debug_bounds("mono-emoji-happy").is_none());
    window
        .read_with(cx, |view, _| {
            assert_eq!(markdown_texts(view), ["Nice :emo[happy] work"]);
        })
        .unwrap();
}

#[gpui::test]
fn setting_the_library_lays_the_messages_out_again(cx: &mut TestAppContext) {
    let fx = fixture(true);
    let window = open_view(cx, None, true);
    window
        .update(cx, |view, _, cx| {
            view.patch(
                0,
                vec![row("user", "hi"), row("assistant", "Nice :emo[happy] work")],
                cx,
            );
            assert_eq!(markdown_texts(view), ["Nice work"], "no library: removed");
            view.set_emoji(Some(fx.emoji.clone()), cx);
        })
        .unwrap();
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    assert!(visual.debug_bounds("mono-emoji-happy").is_some());
    window
        .update(cx, |view, _, cx| view.set_emoji(None, cx))
        .unwrap();
    visual.run_until_parked();
    assert!(visual.debug_bounds("mono-emoji-happy").is_none());
}

fn first_pick(view: &TranscriptView) -> PathBuf {
    view.rows
        .iter()
        .find_map(|row| match &row.body {
            Body::Markdown(md) => picks(std::slice::from_ref(&**md)).into_iter().next(),
            _ => None,
        })
        .expect("a picture")
}

#[gpui::test]
fn a_history_page_above_the_window_does_not_change_the_pictures(cx: &mut TestAppContext) {
    let fx = fixture(true);
    let text = "a :emo[happy] b :emo[happy]";
    let window = open_view(cx, Some(fx.emoji.clone()), true);
    let before = window
        .update(cx, |view, _, cx| {
            view.patch(0, vec![row("user", "hi"), row("assistant", text)], cx);
            first_pick(view)
        })
        .unwrap();
    // The same text one page lower: the view rows move by 5, the store rows do not.
    let page = HistoryPage {
        rows: (0..5)
            .map(|i| row(if i % 2 == 0 { "user" } else { "assistant" }, "old"))
            .collect(),
        turns: HashMap::new(),
        before: None,
    };
    let after = window
        .update(cx, |view, _, cx| {
            view.prepend_history(page, HashMap::new(), cx);
            assert_eq!(view.base, 5);
            first_pick(view)
        })
        .unwrap();
    assert_eq!(before, after);
}

#[gpui::test]
fn two_views_with_different_window_bases_pick_the_same_emoji(cx: &mut TestAppContext) {
    let fx = fixture(true);
    let text = "a :emo[happy] b :emo[happy]";
    let picks_in = |cx: &mut TestAppContext, lead: usize| {
        let window = open_view(cx, Some(fx.emoji.clone()), true);
        window
            .update(cx, |view, _, cx| {
                let mut rows = Vec::new();
                for _ in 0..lead {
                    rows.push(row("user", "hi"));
                    rows.push(row("assistant", "old"));
                }
                rows.push(row("user", "hi"));
                rows.push(row("assistant", text));
                view.patch(0, rows, cx);
                first_pick(view)
            })
            .unwrap()
    };
    // The same message sits at row 1 in one view and at row 7 in the other, as after a
    // restart that loads another history window.
    assert_eq!(picks_in(cx, 0), picks_in(cx, 3));
}

#[gpui::test]
fn copy_and_outward_text_carry_no_token_and_no_double_space(cx: &mut TestAppContext) {
    let fx = fixture(true);
    let window = open_view(cx, Some(fx.emoji.clone()), true);
    window
        .update(cx, |view, _, cx| {
            view.patch(
                0,
                vec![
                    row("user", "hi"),
                    row("assistant", "Nice :emo[happy] work\n:emo[celebrate]\nEnd"),
                ],
                cx,
            );
            assert_eq!(view.response_text(1), "Nice work\nEnd");
            assert_eq!(view.last_assistant_text(), "Nice work\nEnd");
        })
        .unwrap();
}

#[gpui::test]
fn an_unfinished_token_is_hidden_while_the_reply_streams(cx: &mut TestAppContext) {
    let fx = fixture(true);
    let window = open_view(cx, Some(fx.emoji.clone()), true);
    window
        .update(cx, |view, _, cx| {
            view.patch(0, vec![row("user", "hi")], cx);
            view.streaming(true, cx);
            view.patch(1, vec![row("assistant", "Hello :emo[ha")], cx);
            // A Mono shows prose once it yielded to its work.
            view.set_yielded(1, cx);
            let texts = markdown_texts(view);
            assert!(
                texts
                    .iter()
                    .all(|t| !t.contains(":emo") && !t.contains('[')),
                "{texts:?}"
            );
            assert_eq!(texts, ["Hello"]);
            view.patch(1, vec![row("assistant", "Hello :emo[happy] there")], cx);
            assert_eq!(markdown_texts(view), ["Hello there"]);
            assert_eq!(picks_in(view), 1);
        })
        .unwrap();
}

fn picks_in(view: &TranscriptView) -> usize {
    view.rows
        .iter()
        .map(|row| match &row.body {
            Body::Markdown(md) => picks(std::slice::from_ref(&**md)).len(),
            _ => 0,
        })
        .sum()
}
