use super::*;
use crate::surface::Surface;
use crate::telegram::render::{OptionIn, QuestionIn};
use std::collections::{HashMap, HashSet};
use std::time::Instant;

fn prompt() -> Pending {
    Pending {
        id: "wire-7".into(),
        request_id: 7,
        auto_resolve: false,
        questions: vec![QuestionIn {
            id: "q1".into(),
            header: Some("Scope".into()),
            prompt: "Which files?".into(),
            multi_select: true,
            allow_custom: true,
            options: vec![
                OptionIn {
                    id: "a".into(),
                    label: "All".into(),
                    description: Some("Everything".into()),
                },
                OptionIn {
                    id: "b".into(),
                    label: "None".into(),
                    description: None,
                },
            ],
        }],
    }
}

fn asking() -> SessionInfo {
    SessionInfo {
        id: "s1".into(),
        cwd: "/work/acme".into(),
        title: "Fix the login".into(),
        provider: "claude".into(),
        pending: Some(prompt()),
        ..Default::default()
    }
}

fn block(role: &str, text: &str) -> Block {
    Block {
        role: role.into(),
        text: text.into(),
        internal: false,
    }
}

fn said(text: &str) -> String {
    let mut session = asking();
    session.blocks = vec![block("assistant", text)];
    question_context(&session)
}

fn count(text: &str) -> usize {
    text.chars().count()
}

/// Every whole word of the output must be a whole word of the source.
fn expect_whole_words(output: &str, source: &str) {
    let words: HashSet<&str> = source.split_whitespace().collect();
    for word in output.trim_start_matches("\u{2026} ").split_whitespace() {
        assert!(words.contains(word), "cut word: {word}");
    }
}

#[test]
fn is_forwardable_excludes_workers_inbox_ask_ephemeral_tui_mono_remote_and_auto_resolving() {
    assert!(is_forwardable(&asking()));
    let mut worker = asking();
    worker.orchestration_worker = true;
    assert!(!is_forwardable(&worker));
    let mut ask = asking();
    ask.inbox_ask = true;
    assert!(!is_forwardable(&ask));
    let mut ephemeral = asking();
    ephemeral.ephemeral = true;
    assert!(!is_forwardable(&ephemeral));
    let mut tui = asking();
    tui.surface = Surface::Tui;
    assert!(!is_forwardable(&tui));
    let mut mono = asking();
    mono.mono = true;
    assert!(!is_forwardable(&mono));
    let mut remote = asking();
    remote.cwd = "remote://host/work/acme".into();
    assert!(!is_forwardable(&remote));
    let mut timed = asking();
    timed.pending.as_mut().unwrap().auto_resolve = true;
    assert!(!is_forwardable(&timed));
}

#[test]
fn is_topic_linkable_ignores_the_pending_question_but_excludes_sessions_that_never_forward() {
    assert!(is_topic_linkable(&asking()));
    let mut idle = asking();
    idle.pending = None;
    assert!(is_topic_linkable(&idle));
    let mut timed = asking();
    timed.pending.as_mut().unwrap().auto_resolve = true;
    assert!(is_topic_linkable(&timed));
    let mut worker = asking();
    worker.orchestration_worker = true;
    assert!(!is_topic_linkable(&worker));
    let mut ephemeral = asking();
    ephemeral.ephemeral = true;
    assert!(!is_topic_linkable(&ephemeral));
    let mut tui = asking();
    tui.surface = Surface::Tui;
    assert!(!is_topic_linkable(&tui));
    let mut remote = asking();
    remote.cwd = "remote://host/work/acme".into();
    assert!(!is_topic_linkable(&remote));
    let mut mono = asking();
    mono.mono = true;
    assert!(!is_topic_linkable(&mono));
    let mut ask = asking();
    ask.inbox_ask = true;
    assert!(!is_topic_linkable(&ask));
}

#[test]
fn ticket_from_the_registry_when_the_inbox_card_is_gone() {
    let mut session = asking();
    session.ticket = Some("ENG-42".into());
    assert_eq!(session_ticket_id(&session).as_deref(), Some("ENG-42"));
}

#[test]
fn ticket_from_inbox_card_identifier() {
    let mut session = asking();
    session.inbox_identifier = Some("  ENG-42 ".into());
    assert_eq!(session_ticket_id(&session).as_deref(), Some("ENG-42"));
}

#[test]
fn ticket_from_workflow_item() {
    let mut session = asking();
    session.workflow = Some(Workflow {
        workflow_name: "Triage".into(),
        node_title: "Fix".into(),
        item_id: Some("CU-9".into()),
        ..Default::default()
    });
    assert_eq!(session_ticket_id(&session).as_deref(), Some("CU-9"));
}

#[test]
fn ticket_from_linked_github_issue_is_a_hash_number() {
    let mut session = asking();
    session.linked_number = Some(12);
    assert_eq!(session_ticket_id(&session).as_deref(), Some("#12"));
    assert_eq!(session_ticket_id(&asking()), None);
}

#[test]
fn ticket_precedence_is_registry_then_card_then_workflow_then_linked() {
    let mut session = asking();
    session.linked_number = Some(1);
    session.workflow = Some(Workflow {
        item_id: Some("CU-9".into()),
        ..Default::default()
    });
    session.inbox_identifier = Some("ENG-1".into());
    session.ticket = Some("REG-7".into());
    assert_eq!(session_ticket_id(&session).as_deref(), Some("REG-7"));
    session.ticket = None;
    assert_eq!(session_ticket_id(&session).as_deref(), Some("ENG-1"));
    session.inbox_identifier = Some("   ".into());
    assert_eq!(session_ticket_id(&session).as_deref(), Some("CU-9"));
    session.workflow = None;
    assert_eq!(session_ticket_id(&session).as_deref(), Some("#1"));
}

#[test]
fn topic_title_does_not_repeat_a_github_ticket_already_in_the_title() {
    let mut session = asking();
    session.title = "#12 Fix crash".into();
    session.linked_number = Some(12);
    assert_eq!(topic_title(&session), "[#12] Fix crash");
    assert_eq!(
        forward_payload(&session, "/work/acme", 300).unwrap().header,
        "acme \u{b7} Fix crash \u{b7} #12"
    );
}

#[test]
fn topic_title_does_not_repeat_a_tracker_key_already_in_the_title() {
    let mut session = asking();
    session.title = "ENG-42 Fix crash".into();
    session.ticket = Some("ENG-42".into());
    assert_eq!(topic_title(&session), "[ENG-42] Fix crash");
    assert_eq!(
        forward_payload(&session, "/work/acme", 300).unwrap().header,
        "acme \u{b7} Fix crash \u{b7} ENG-42"
    );
}

#[test]
fn native_inbox_titles_use_a_dot_separator_after_the_ticket() {
    let mut session = asking();
    session.title = "ENG-42 \u{b7} Fix crash".into();
    session.ticket = Some("ENG-42".into());
    assert_eq!(topic_title(&session), "[ENG-42] Fix crash");
    assert_eq!(
        forward_payload(&session, "/work/acme", 300).unwrap().header,
        "acme \u{b7} Fix crash \u{b7} ENG-42"
    );
}

#[test]
fn the_harness_prefix_is_dropped_from_the_title() {
    let mut session = asking();
    session.title = "claude \u{b7} Fix the login".into();
    assert_eq!(topic_title(&session), "Fix the login");
    session.title = "claude".into();
    assert_eq!(topic_title(&session), "New session");
}

#[test]
fn title_without_ticket_is_plain() {
    assert_eq!(topic_title(&asking()), "Fix the login");
}

#[test]
fn topic_title_prefixes_the_ticket() {
    let mut session = asking();
    session.linked_number = Some(3);
    assert_eq!(topic_title(&session), "[#3] Fix the login");
}

#[test]
fn topic_title_is_trimmed_to_128_characters() {
    let mut session = asking();
    session.title = "x".repeat(300);
    assert_eq!(count(&topic_title(&session)), 128);
    let mut ticketed = asking();
    ticketed.title = "y".repeat(300);
    ticketed.inbox_identifier = Some("ENG-42".into());
    let title = topic_title(&ticketed);
    assert_eq!(count(&title), 128);
    assert!(title.starts_with("[ENG-42] yyy"));
    let mut wide = asking();
    wide.title = "\u{1F600}".repeat(300);
    assert_eq!(count(&topic_title(&wide)), 128);
}

#[test]
fn context_uses_the_last_assistant_text_that_is_not_internal() {
    let mut session = asking();
    session.blocks = vec![
        block("assistant", "old reply"),
        block("assistant", "The plan is ready."),
        block("tool", "tool output"),
        Block {
            internal: true,
            ..block("assistant", "internal note")
        },
        block("user", "user text"),
    ];
    assert_eq!(question_context(&session), "The plan is ready.");
    assert_eq!(question_context(&asking()), "");
}

#[test]
fn context_passes_a_short_message_through_untouched() {
    assert_eq!(
        said("  I updated the parser. It now handles quotes.  "),
        "I updated the parser. It now handles quotes."
    );
}

#[test]
fn context_keeps_whole_sentences_from_the_end_and_marks_the_cut() {
    let sentences: Vec<String> = (0..30)
        .map(|i| format!("Sentence number {i} explains one more detail."))
        .collect();
    let context = said(&sentences.join(" "));
    assert!(count(&context) <= 300);
    assert!(count(&context) > 200);
    assert!(context.starts_with("\u{2026} Sentence number "));
    assert!(context.ends_with("Sentence number 29 explains one more detail."));
    // Only whole sentences: what follows the marker is a full sentence from the source.
    let body = context.trim_start_matches("\u{2026} ");
    let parts: Vec<&str> = body.split(". ").collect();
    for (ix, part) in parts.iter().enumerate() {
        let sentence = if ix + 1 < parts.len() {
            format!("{part}.")
        } else {
            part.to_string()
        };
        assert!(sentences.contains(&sentence), "not a sentence: {sentence}");
    }
}

#[test]
fn context_has_no_marker_when_the_selection_starts_at_the_beginning() {
    let context = said("First point. Second point. Third point.");
    assert!(!context.starts_with('\u{2026}'));
    assert_eq!(context, "First point. Second point. Third point.");
}

#[test]
fn context_strips_headings_links_images_emphasis_inline_code_and_html() {
    let text = [
        "## Findings",
        "",
        "I checked [the docs](https://example.com/docs) and ![diagram](x.png) with <b>care</b>.",
        "The **bold** and __also bold__ and *italic* and _more italic_ words use `npm run check` and ~~old~~ names.",
    ]
    .join("\n");
    assert_eq!(
        said(&text),
        "Findings I checked the docs and with care. The bold and also bold and italic and more italic words use npm run check and old names."
    );
}

#[test]
fn context_keeps_snake_case_identifiers_and_multiplication_stars_intact() {
    assert_eq!(
        said("Rename some_var_name and compute 2 * 3 * 4."),
        "Rename some_var_name and compute 2 * 3 * 4."
    );
}

#[test]
fn context_drops_fenced_code_blocks_including_an_unterminated_one() {
    let fenced = [
        "I changed the loader.",
        "```ts",
        "const secret = 1;",
        "```",
        "Tests pass now.",
    ]
    .join("\n");
    assert_eq!(said(&fenced), "I changed the loader. Tests pass now.");
    let open = ["Here is the patch.", "~~~", "diff --git a b", "+x"].join("\n");
    assert_eq!(said(&open), "Here is the patch.");
}

#[test]
fn context_is_empty_when_the_message_is_only_code_or_markup() {
    assert_eq!(said("```\nconst a = 1;\n```"), "");
    assert_eq!(said("---\n\n| --- | --- |"), "");
    assert_eq!(said("   \n  "), "");
}

#[test]
fn context_does_not_reach_back_to_an_older_message_when_the_last_is_only_code() {
    let mut session = asking();
    session.blocks = vec![
        block("assistant", "An older explanation."),
        block("assistant", "```\nconst a = 1;\n```"),
    ];
    assert_eq!(question_context(&session), "");
}

#[test]
fn context_keeps_list_items_as_plain_dash_lines() {
    let text = [
        "Two options are open:",
        "",
        "* Keep the cache",
        "1. Drop the cache",
        "",
        "I lean towards the first.",
    ]
    .join("\n");
    assert_eq!(
        said(&text),
        "Two options are open:\n- Keep the cache\n- Drop the cache\nI lean towards the first."
    );
}

#[test]
fn context_turns_table_rows_into_plain_text_and_drops_the_rules() {
    let text = ["| File | Change |", "| --- | --- |", "| a.ts | rename |"].join("\n");
    assert_eq!(said(&text), "File, Change a.ts, rename");
}

#[test]
fn context_skips_a_last_paragraph_that_only_repeats_the_question() {
    let text = [
        "The migration touches three tables and takes a few minutes on a large database.",
        "",
        "Which files?",
    ]
    .join("\n");
    assert_eq!(
        said(&text),
        "The migration touches three tables and takes a few minutes on a large database."
    );
}

#[test]
fn context_skips_a_trailing_sentence_that_repeats_the_question_inside_a_paragraph() {
    assert_eq!(
        said("I found two loaders. Which files? "),
        "I found two loaders."
    );
}

#[test]
fn context_keeps_the_question_text_when_nothing_else_was_said() {
    assert_eq!(said("Which files?"), "Which files?");
    assert_eq!(
        said("Which files?\n\nWhich files?"),
        "Which files? Which files?"
    );
}

#[test]
fn context_keeps_a_last_paragraph_that_goes_beyond_the_question() {
    let text = "Scanning is done. Which files should I touch first?";
    assert_eq!(said(text), text);
}

#[test]
fn context_cuts_a_single_huge_sentence_at_a_word_boundary_and_keeps_its_end() {
    let words: Vec<String> = (0..400).map(|i| format!("word{i}")).collect();
    let text = words.join(" ");
    let context = said(&text);
    assert!(count(&context) <= 300);
    assert!(context.starts_with("\u{2026} word"));
    assert!(context.ends_with("word399"));
    expect_whole_words(&context, &text);
}

#[test]
fn context_never_splits_a_multi_byte_character_and_counts_characters() {
    let emoji = said(&vec!["ok \u{1F600}"; 200].join(" "));
    assert!(count(&emoji) <= 300);
    let cjk = said(
        &"\u{4eca}\u{65e5}\u{306f}\u{5929}\u{6c17}\u{304c}\u{3044}\u{3044}\u{3002}".repeat(60),
    );
    assert!(count(&cjk) <= 300);
    assert!(cjk.starts_with("\u{2026} "));
    assert!(cjk.ends_with('\u{3002}'));
    let unbroken = said(&"\u{1F600}".repeat(700));
    assert!(count(&unbroken) <= 300);
    assert!(unbroken.starts_with("\u{2026} "));
}

#[test]
fn context_keeps_the_contents_of_inline_code_exactly_as_written() {
    assert_eq!(said("Open `__init__.py` now."), "Open __init__.py now.");
    assert_eq!(
        said("Edit `src/__tests__/foo.test.ts` first."),
        "Edit src/__tests__/foo.test.ts first."
    );
    assert_eq!(
        said("Match `src/**/*.ts` files."),
        "Match src/**/*.ts files."
    );
    assert_eq!(
        said("It returns `Result<T, E>` or `Vec<String>`."),
        "It returns Result<T, E> or Vec<String>."
    );
    assert_eq!(
        said("Call `f(*args, **kwargs)` here."),
        "Call f(*args, **kwargs) here."
    );
    assert_eq!(said("Use `a**b**c` there."), "Use a**b**c there.");
    assert_eq!(
        said("Mail `<ops@example.com>` today."),
        "Mail <ops@example.com> today."
    );
    assert_eq!(
        said("Link `[a](b)` and `![i](j)` stay."),
        "Link [a](b) and ![i](j) stay."
    );
}

#[test]
fn context_unwraps_code_spans_next_to_emphasis_and_handles_longer_fences() {
    assert_eq!(
        said("Call **`init()`** then **bold**`tail`."),
        "Call init() then boldtail."
    );
    assert_eq!(said("Use ``a`b`` and ```x``` here."), "Use a`b and x here.");
    assert_eq!(said("Pad ` spaced ` ok."), "Pad spaced ok.");
}

#[test]
fn context_leaves_an_unclosed_backtick_as_plain_text() {
    assert_eq!(
        said("Use the `broken quote here."),
        "Use the `broken quote here."
    );
    assert_eq!(
        said("Keep `ok` and a stray ` tick."),
        "Keep ok and a stray ` tick."
    );
}

#[test]
fn context_does_not_split_a_sentence_inside_an_inline_code_span() {
    assert_eq!(
        said("Bump to `v1.2.3` in `src/app.ts`. Then rebuild."),
        "Bump to v1.2.3 in src/app.ts. Then rebuild."
    );
    // The span's own full stop is not a sentence end, so the cut stays at a real one.
    let long = format!("Start {}then `x. y` end.", "w ".repeat(150));
    let context = said(&long);
    assert!(context.starts_with("\u{2026} "));
    assert!(context.ends_with("then x. y end."));
    assert!(count(&context) > 100);
}

#[test]
fn context_never_lets_a_placeholder_leak_even_for_text_that_looks_like_one() {
    assert_eq!(said("\u{E000}0\u{E001} then `x`"), "0 then x");
    let spans: Vec<String> = (0..80).map(|i| format!("see `file{i}.ts`")).collect();
    let context = said(&format!("{} done", spans.join(" ")));
    assert!(!context.contains(['\u{E000}', '\u{E001}']));
    assert!(context.ends_with("see file79.ts done"));
}

#[test]
fn context_stays_fast_on_huge_pathological_messages_and_keeps_the_end() {
    let inputs = vec![
        format!("{}the end.", "*a ".repeat(7000)),
        format!("{}the end.", "**a_ ".repeat(4000)),
        format!("{} the end.", "[".repeat(20000)),
        format!("{}the end.", "<a ".repeat(7000)),
        format!("x {} the end.", "`".repeat(20000)),
        format!("{}the end.", "x ".repeat(10000)),
        format!("{}the end.", "[a ".repeat(7000)),
        format!("{}the end.", "`a ".repeat(7000)),
        format!("{}the end.", "x `a` ".repeat(30000)),
        format!(
            "{}the end.",
            (0..20000)
                .map(|i| format!("{}a ", "`".repeat(1 + (i % 7))))
                .collect::<String>()
        ),
        format!(
            "{}Last sentence here.",
            "Sentence number one. ".repeat(1000)
        ),
    ];
    for input in inputs {
        let started = Instant::now();
        let context = said(&input);
        assert!(
            started.elapsed().as_millis() < 2000,
            "slow: {:?}",
            started.elapsed()
        );
        assert!(count(&context) <= 300);
        assert!(context.starts_with("\u{2026} "), "{context}");
        assert!(
            context.ends_with("the end.") || context.ends_with("Last sentence here."),
            "{context}"
        );
    }
}

mod windowing {
    use super::*;

    const QUESTION: &str = "Shall I commit this change?";

    fn line_of(i: usize) -> String {
        format!(
            "Step {i}: the loader in `src/app.ts` stops `if (!ready)` early, then `init()` runs twice."
        )
    }

    /// No unpaired code and no glued words: every output word is a source word.
    fn expect_faithful(output: &str, source: &str) {
        let known: HashSet<&str> = source.split_whitespace().collect();
        let known: HashSet<String> = known.iter().map(|w| w.replace('`', "")).collect();
        assert!(output.ends_with(QUESTION), "{output}");
        assert!(!output.contains('`'));
        for word in output.trim_start_matches("\u{2026} ").split_whitespace() {
            assert!(
                known.contains(word),
                "glued or cut word {word:?} in: {output}"
            );
        }
        assert!(output.contains("src/app.ts"));
    }

    #[test]
    fn never_shifts_code_spans_when_the_window_cuts_a_multi_line_paragraph() {
        for pad in 0..60 {
            let lines: Vec<String> = (0..60).map(line_of).collect();
            let text = format!("{}\n{}{QUESTION}", lines.join("\n"), "p ".repeat(pad));
            expect_faithful(&said(&text), &text);
        }
    }

    #[test]
    fn never_shifts_code_spans_in_one_huge_line() {
        for pad in 0..60 {
            let lines: Vec<String> = (0..60).map(line_of).collect();
            let text = format!("{} {}{QUESTION}", lines.join(" "), "p ".repeat(pad));
            expect_faithful(&said(&text), &text);
        }
    }

    #[test]
    fn starts_at_a_paragraph_boundary_when_the_window_has_one() {
        let first: Vec<String> = (0..30).map(line_of).collect();
        let second: Vec<String> = (0..20).map(|i| line_of(100 + i)).collect();
        let text = format!("{}\n\n{}\n{QUESTION}", first.join("\n"), second.join("\n"));
        let context = said(&text);
        expect_faithful(&context, &text);
        assert!(context.starts_with("\u{2026} "));
    }

    #[test]
    fn does_not_drop_a_short_final_message_after_a_huge_one() {
        let text = format!("{}\n\nDone. {QUESTION}", "word ".repeat(3000));
        // Text before the paragraph was left out, so the marker stays.
        assert_eq!(said(&text), format!("\u{2026} Done. {QUESTION}"));
    }
}

mod spans_stay_in_their_block {
    use super::*;

    #[test]
    fn does_not_pair_a_stray_backtick_with_one_in_a_later_list_item() {
        let text = [
            "I made these changes:",
            "- Escaped the backtick (`) in the template",
            "- Renamed `foo` to `bar`",
            "- Updated `README.md` with the new flag",
            "",
            "Shall I commit this change?",
        ]
        .join("\n");
        assert_eq!(
            said(&text),
            [
                "I made these changes:",
                "- Escaped the backtick (`) in the template",
                "- Renamed foo to bar",
                "- Updated README.md with the new flag",
                "Shall I commit this change?",
            ]
            .join("\n")
        );
    }

    #[test]
    fn does_not_pair_a_stray_backtick_in_an_ordered_list_item_either() {
        assert_eq!(
            said("1. Typed a ` by mistake\n2. Ran `npm test` after"),
            "- Typed a ` by mistake\n- Ran npm test after"
        );
    }

    #[test]
    fn keeps_a_headings_stray_backtick_away_from_the_prose_below_it() {
        assert_eq!(
            said("## The ` char\nUse `real` span here."),
            "The ` char Use real span here."
        );
    }

    #[test]
    fn keeps_a_table_rows_stray_backtick_away_from_the_next_row() {
        assert_eq!(
            said("| a | ` | c |\n| x | `npm test` | y |"),
            "a, `, c x, npm test, y"
        );
    }

    #[test]
    fn still_joins_a_span_that_runs_over_several_lines_of_one_paragraph() {
        assert_eq!(said("Run `npm\nrun build` now."), "Run npm run build now.");
        assert_eq!(
            said("- Run `npm\n  run build` now"),
            "- Run npm run build now"
        );
    }

    #[test]
    fn handles_windows_line_endings() {
        assert_eq!(
            said("Run `npm\r\nrun build` now."),
            "Run npm run build now."
        );
        assert_eq!(
            said("Options:\r\n- Keep `a`\r\n- Drop `b`\r\n"),
            "Options:\n- Keep a\n- Drop b"
        );
        assert_eq!(said("# Title\r\n\r\nBody `x`.\r\n"), "Title Body x.");
    }

    #[test]
    fn bounds_the_work_for_a_message_of_a_couple_of_megabytes() {
        let lines: Vec<String> = (0..30000)
            .map(|i| format!("Line {i} has `code` and plenty of words for padding this test."))
            .collect();
        let many = format!(
            "{}\nFinal answer is ready now. Shall I commit this change?",
            lines.join("\n")
        );
        let one_line = format!(
            "{}Done. Final answer is ready now. Shall I commit this change?",
            "word `x` ".repeat(250000)
        );
        for text in [many, one_line] {
            assert!(text.len() > 1_900_000);
            let started = Instant::now();
            let context = said(&text);
            assert!(
                started.elapsed().as_millis() < 3000,
                "slow: {:?}",
                started.elapsed()
            );
            assert!(context.starts_with("\u{2026} "));
            assert!(context.ends_with("Final answer is ready now. Shall I commit this change?"));
        }
    }
}

#[test]
fn context_is_deterministic() {
    let text = format!("{}```x```", "Alpha one. Beta two. ".repeat(40));
    assert_eq!(said(&text), said(&text));
}

#[test]
fn context_caps_a_long_realistic_message_below_the_maximum() {
    let text = [
        "## Plan",
        "I read `src/app.ts` and the **loader**. Both call `init()` twice.",
        "```ts",
        "init(); init();",
        "```",
        "- Remove the second call in the loader to stop the double start",
        "- Keep the one in app.ts because tests rely on it being there",
        "That keeps behaviour the same. It also removes a race that showed up in CI last week. I verified it with the full suite and it passes on every platform we build for.",
        "Should I go ahead and remove the second call?",
    ]
    .join("\n");
    let mut session = asking();
    session.blocks = vec![block("assistant", &text)];
    session.pending = Some(Pending {
        id: "wire-9".into(),
        request_id: 9,
        auto_resolve: false,
        questions: vec![QuestionIn {
            id: "q".into(),
            header: None,
            prompt: "Should I go ahead and remove the second call?".into(),
            multi_select: false,
            allow_custom: false,
            options: vec![OptionIn {
                id: "y".into(),
                label: "Yes".into(),
                description: None,
            }],
        }],
    });
    let context = question_context(&session);
    assert!(count(&context) <= CONTEXT_MAX);
    assert!(!context.contains(['`', '*', '#']));
    assert!(!context.contains("init(); init"));
    assert!(context.ends_with("passes on every platform we build for."));
}

#[test]
fn forward_payload_is_none_without_a_pending_question() {
    let mut session = asking();
    session.pending = None;
    assert!(forward_payload(&session, "/p", 300).is_none());
}

#[test]
fn forward_payload_maps_the_question_and_builds_the_header() {
    let mut session = asking();
    session.linked_number = Some(12);
    session.blocks = vec![block("assistant", "Need input")];
    session.workflow = Some(Workflow {
        workflow_name: "Triage".into(),
        node_title: "Fix".into(),
        item_label: Some("Crash".into()),
        item_id: None,
        round: Some(2),
    });
    let payload = forward_payload(&session, "/work/acme", 90).unwrap();
    assert_eq!(payload.project, "/work/acme");
    assert_eq!(payload.session_id, "s1");
    assert_eq!(payload.request_id, 7);
    assert_eq!(payload.topic_title, "[#12] Fix the login");
    assert_eq!(
        payload.header,
        "acme \u{b7} Fix the login \u{b7} #12\nWorkflow Triage \u{b7} Fix \u{b7} Crash \u{b7} round 2"
    );
    assert_eq!(payload.context, "Need input");
    assert_eq!(payload.delay_secs, 90);
    assert_eq!(payload.questions.len(), 1);
    let question = &payload.questions[0];
    assert_eq!(question.id, "q1");
    assert_eq!(question.header.as_deref(), Some("Scope"));
    assert_eq!(question.prompt, "Which files?");
    assert!(question.multi_select && question.allow_custom);
    assert_eq!(question.options.len(), 2);
    assert_eq!(
        question.options[0].description.as_deref(),
        Some("Everything")
    );
    assert_eq!(question.options[1].description, None);
}

#[test]
fn forward_payload_drops_empty_headers_and_descriptions() {
    let mut session = asking();
    let question = &mut session.pending.as_mut().unwrap().questions[0];
    question.header = Some(String::new());
    question.options[0].description = Some(String::new());
    let payload = forward_payload(&session, "/p", 5).unwrap();
    assert_eq!(payload.questions[0].header, None);
    assert_eq!(payload.questions[0].options[0].description, None);
}

#[test]
fn header_without_ticket_or_workflow_is_one_line() {
    assert_eq!(
        forward_payload(&asking(), "/work/acme", 300)
            .unwrap()
            .header,
        "acme \u{b7} Fix the login"
    );
}

#[test]
fn project_name_follows_the_display_rules() {
    assert_eq!(project_name("/work/acme"), "acme");
    assert_eq!(project_name("/work/acme/"), "acme");
    assert_eq!(project_name(""), "~");
    assert_eq!(project_name("~"), "~");
    assert_eq!(project_name("/Users/me"), "~");
    assert_eq!(project_name("/home/me"), "~");
    assert_eq!(project_name("/Users/me/code"), "code");
    assert_eq!(project_name("C:\\Users\\me"), "~");
    assert_eq!(project_name("C:\\work\\acme"), "acme");
    assert_eq!(project_name("C:/"), "C:");
    assert_eq!(project_name("/"), "/");
}

#[test]
fn project_key_is_the_files_path_key() {
    assert_eq!(project_key("/work/acme/"), "/work/acme");
    assert_eq!(project_key("C:\\Work\\Acme"), "c:/work/acme");
}

#[test]
fn forward_delay_uses_the_projects_delay_and_falls_back_to_five_minutes() {
    assert_eq!(forward_delay_ms(Some(30)), 30_000);
    assert_eq!(forward_delay_ms(Some(0)), 0);
    assert_eq!(forward_delay_ms(None), FORWARD_DELAY_MS);
    assert_eq!(FORWARD_DELAY_MS, 300_000);
}

#[test]
fn question_ids_hash_to_a_stable_non_zero_number() {
    let a = request_id_of("\"wire-1\"");
    assert_eq!(a, request_id_of("\"wire-1\""));
    assert_ne!(a, request_id_of("\"wire-2\""));
    assert!((1..1 << 53).contains(&a));
    assert!(request_id_of("") >= 1);
}

fn due<'a>(
    sessions: &'a [SessionInfo],
    first: &HashMap<String, u64>,
    sent: &HashSet<String>,
    now: u64,
    delay: impl Fn(&SessionInfo) -> u64,
) -> Vec<&'a str> {
    due_forwards(sessions, first, sent, now, delay)
        .into_iter()
        .map(|s| s.id.as_str())
        .collect()
}

#[test]
fn due_only_after_the_delay_and_only_once() {
    let session = asking();
    let key = question_key("s1", 7);
    let first = HashMap::from([(key.clone(), 1_000)]);
    let mut sent = HashSet::new();
    let five = |_: &SessionInfo| FORWARD_DELAY_MS;
    let all = [session];
    assert!(due(&all, &first, &sent, 1_000 + FORWARD_DELAY_MS - 1, five).is_empty());
    assert_eq!(
        due(&all, &first, &sent, 1_000 + FORWARD_DELAY_MS, five),
        ["s1"]
    );
    sent.insert(key);
    assert!(due(&all, &first, &sent, 1_000 + FORWARD_DELAY_MS, five).is_empty());
}

#[test]
fn due_uses_the_delay_of_each_session() {
    let quick = asking();
    let mut slow = asking();
    slow.id = "s2".into();
    let first = HashMap::from([
        (question_key("s1", 7), 1_000),
        (question_key("s2", 7), 1_000),
    ]);
    let delay = |s: &SessionInfo| if s.id == "s1" { 30_000 } else { 600_000 };
    let all = [quick, slow];
    let sent = HashSet::new();
    assert!(due(&all, &first, &sent, 1_000 + 29_999, delay).is_empty());
    assert_eq!(due(&all, &first, &sent, 1_000 + 30_000, delay), ["s1"]);
    assert_eq!(
        due(&all, &first, &sent, 1_000 + 600_000, delay),
        ["s1", "s2"]
    );
}

#[test]
fn a_zero_delay_is_due_as_soon_as_the_question_was_seen() {
    let all = [asking()];
    let first = HashMap::from([(question_key("s1", 7), 1_000)]);
    assert_eq!(due(&all, &first, &HashSet::new(), 1_000, |_| 0), ["s1"]);
    // Still never for a question that was not seen (disabled or unconfigured at first sight).
    assert!(due(&all, &HashMap::new(), &HashSet::new(), 1_000, |_| 0).is_empty());
}

#[test]
fn due_skips_unseen_unforwardable_and_questionless_sessions() {
    let mut hidden = asking();
    hidden.id = "hidden".into();
    hidden.ephemeral = true;
    let mut none = asking();
    none.id = "none".into();
    none.pending = None;
    let mut unseen = asking();
    unseen.id = "unseen".into();
    let first = HashMap::from([(question_key("hidden", 7), 0), (question_key("none", 7), 0)]);
    let all = [hidden, none, unseen];
    assert!(
        due(&all, &first, &HashSet::new(), FORWARD_DELAY_MS * 2, |_| {
            FORWARD_DELAY_MS
        })
        .is_empty()
    );
}

#[test]
fn strong_emphasis_pairs_like_the_original_pattern() {
    assert_eq!(
        strip_strong("**bold** and __also__ done"),
        "bold and also done"
    );
    assert_eq!(strip_strong("a ** b ** c"), "a ** b ** c");
    assert_eq!(strip_strong("**a b**"), "a b");
    assert_eq!(strip_strong("2 ** 3"), "2 ** 3");
}

#[test]
fn underscore_emphasis_needs_a_non_word_before_and_after() {
    assert_eq!(strip_underscore("_a_ _b c_ end"), "a b c end");
    assert_eq!(strip_underscore("some_var_name"), "some_var_name");
    assert_eq!(strip_underscore("_a_b"), "_a_b");
    assert_eq!(strip_underscore("see _it_."), "see it.");
    assert_eq!(strip_underscore("_ a _"), "_ a _");
}

#[test]
fn a_native_question_becomes_a_pending_question() {
    use crate::agent::{Question, QuestionItem, QuestionOption};
    let native = Question {
        id: "\"wire-1\"".into(),
        title: "The agent has a question".into(),
        auto_resolve_at: None,
        items: vec![QuestionItem {
            id: "scope".into(),
            prompt: "Which files?".into(),
            multiple: true,
            allow_custom: true,
            options: vec![
                QuestionOption {
                    id: "a".into(),
                    label: "All".into(),
                    description: "Everything".into(),
                },
                QuestionOption {
                    id: "b".into(),
                    label: "None".into(),
                    description: String::new(),
                },
            ],
        }],
    };
    let pending = Pending::from_question(&native);
    assert_eq!(pending.id, native.id);
    assert_eq!(pending.request_id, request_id_of(&native.id));
    assert!(!pending.auto_resolve);
    let question = &pending.questions[0];
    assert_eq!(question.id, "scope");
    assert_eq!(question.header, None);
    assert_eq!(question.prompt, "Which files?");
    assert!(question.multi_select && question.allow_custom);
    assert_eq!(question.options[0].id, "a");
    assert_eq!(question.options[0].label, "All");
    assert_eq!(
        question.options[0].description.as_deref(),
        Some("Everything")
    );
    assert_eq!(question.options[1].description, None);
    let timed = Question {
        auto_resolve_at: Some(5),
        ..native
    };
    assert!(Pending::from_question(&timed).auto_resolve);
}

#[test]
fn the_inbox_ticket_follows_the_provider() {
    use crate::inbox::Item;
    let item = |provider: &str, identifier: &str| Item {
        provider: provider.into(),
        identifier: identifier.into(),
        number: 12,
        ..Default::default()
    };
    assert_eq!(inbox_ticket(&item("linear", " ENG-42 ")), "ENG-42");
    assert_eq!(inbox_ticket(&item("jira", "PROJ-7")), "PROJ-7");
    assert_eq!(inbox_ticket(&item("clickup", "CU-9")), "CU-9");
    assert_eq!(inbox_ticket(&item("linear", "  ")), "#12");
    assert_eq!(inbox_ticket(&item("github", "acme/app#12")), "#12");
    assert_eq!(inbox_ticket(&item("gitlab", "")), "#12");
}
