//! The file mentions of an assistant message, found the way the chat draws them.
//!
//! A mention is what the chat turns into a clickable file chip or link: an inline code span that
//! looks like a file (no whitespace, and either its last segment has a dot or it starts with `/`,
//! `~/` or `./`), a markdown link destination with no scheme (or `file://`), and a bare word that
//! starts with `/`, `~/` or `./`. Fenced and indented code blocks are not mentions. Trailing
//! punctuation and a `:12`, `:12:3` or `#L12` suffix are stripped. The result is the path text as
//! written; resolving it against a folder is `files::links`'s job.
use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};

/// The longest mention kept, in characters.
pub const MAX_MENTION_CHARS: usize = 1024;

/// The mentions of `text`, in order, without duplicates.
pub fn extract_mentions(text: &str) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    let mut add = |mention: Option<String>| {
        if let Some(mention) = mention {
            if !found.contains(&mention) {
                found.push(mention);
            }
        }
    };
    let mut in_block = false;
    // Adjacent text events are one run: the parser splits at `_`, escapes and entities.
    let mut run = String::new();
    let options = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH;
    for event in Parser::new_ext(text, options) {
        if let (Event::Text(piece), false) = (&event, in_block) {
            run.push_str(piece);
            continue;
        }
        for token in run.split_whitespace() {
            add(bare_token(token));
        }
        run.clear();
        match event {
            Event::Start(Tag::CodeBlock(_)) => in_block = true,
            Event::End(TagEnd::CodeBlock) => in_block = false,
            Event::Code(code) if !in_block => add(code_span(&code)),
            Event::Start(Tag::Link { dest_url, .. }) if !in_block => add(link_dest(&dest_url)),
            _ => {}
        }
    }
    for token in run.split_whitespace() {
        add(bare_token(token));
    }
    found
}

fn has_scheme_marker(text: &str) -> bool {
    text.contains("://") && !text.starts_with("file://")
}

fn path_start(text: &str) -> bool {
    text.starts_with('/') || text.starts_with("~/") || text.starts_with("./")
}

/// An inline code span that names a file.
fn code_span(code: &str) -> Option<String> {
    let code = code.trim();
    if code.is_empty() || code.chars().any(char::is_whitespace) || has_scheme_marker(code) {
        return None;
    }
    // `foo.rs:12` has its dot in the file name; the suffix is judged after it is cut.
    let cut = clean(code, false)?;
    let name = cut.rsplit('/').next().unwrap_or(&cut);
    (path_start(&cut) || name.contains('.')).then_some(cut)
}

/// A markdown link destination that points at a file.
fn link_dest(dest: &str) -> Option<String> {
    let dest = dest.trim();
    if dest.is_empty()
        || dest.starts_with('#')
        || dest.starts_with("mailto:")
        || has_scheme_marker(dest)
    {
        return None;
    }
    clean(dest, false)
}

/// A bare word that starts like a path.
fn bare_token(token: &str) -> Option<String> {
    let token = token.trim_start_matches(['(', '[', '{', '"', '\'', '<', '*', '_', '|']);
    if !path_start(token) || has_scheme_marker(token) {
        return None;
    }
    clean(token, true)
}

/// Trims trailing punctuation (when `punctuation`) and the line suffix; refuses an empty,
/// overlong or control-character result.
fn clean(text: &str, punctuation: bool) -> Option<String> {
    let mut text = text;
    if punctuation {
        text = trim_trailing(text);
    }
    let text = strip_location(text);
    let text = if punctuation {
        trim_trailing(text)
    } else {
        text
    };
    (!text.is_empty()
        && text.chars().count() <= MAX_MENTION_CHARS
        && !text.chars().any(char::is_control))
    .then(|| text.to_owned())
}

fn trim_trailing(text: &str) -> &str {
    text.trim_end_matches([
        '.', ',', ';', ':', '!', '?', ')', ']', '}', '>', '"', '\'', '*', '|', '`',
    ])
}

/// `path:12`, `path:12:3` and `path#L12` (also `#L12-L20`) without the suffix.
fn strip_location(text: &str) -> &str {
    if let Some((file, line)) = text.rsplit_once("#L") {
        if line
            .split('-')
            .next()
            .is_some_and(|n| n.parse::<usize>().is_ok())
        {
            return file;
        }
    }
    let mut text = text;
    for _ in 0..2 {
        match text.rsplit_once(':') {
            Some((prefix, number))
                if !number.is_empty() && number.bytes().all(|b| b.is_ascii_digit()) =>
            {
                text = prefix;
            }
            _ => break,
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    fn found(text: &str) -> Vec<String> {
        extract_mentions(text)
    }

    #[test]
    fn code_spans_that_look_like_files_are_mentions() {
        assert_eq!(found("Open `report.html` now"), ["report.html"]);
        assert_eq!(
            found("see `docs/uiux-findings.md`"),
            ["docs/uiux-findings.md"]
        );
        assert_eq!(
            found("`~/Documents/x/report.html`"),
            ["~/Documents/x/report.html"]
        );
        assert_eq!(found("`./build`"), ["./build"]);
        assert_eq!(found("`/var/log/app`"), ["/var/log/app"]);
    }

    #[test]
    fn code_spans_that_do_not_look_like_files_are_not() {
        assert!(found("run `cargo test` now").is_empty());
        assert!(found("the `Vec` type").is_empty());
        assert!(found("`https://example.com/a.html`").is_empty());
        assert!(found("`two words.txt`").is_empty());
        assert!(found("`` ``").is_empty());
    }

    #[test]
    fn line_suffixes_and_trailing_punctuation_are_stripped() {
        assert_eq!(found("`src/a.rs:12`"), ["src/a.rs"]);
        assert_eq!(found("`src/a.rs:12:3`"), ["src/a.rs"]);
        assert_eq!(found("`src/a.rs#L12`"), ["src/a.rs"]);
        assert_eq!(found("`src/a.rs#L12-L20`"), ["src/a.rs"]);
        assert_eq!(found("see ./notes/a.md."), ["./notes/a.md"]);
        assert_eq!(found("(see ~/x/a.md:7)."), ["~/x/a.md"]);
        assert_eq!(found("it is at /tmp/out.html, then"), ["/tmp/out.html"]);
    }

    #[test]
    fn markdown_links_count_unless_they_have_a_scheme() {
        assert_eq!(
            found("[the report](report.html) and [x](docs/a.md#L3)"),
            ["report.html", "docs/a.md"]
        );
        assert_eq!(
            found("[x](file:///tmp/a%20b.html)"),
            ["file:///tmp/a%20b.html"]
        );
        assert!(found("[x](https://example.com/a.html)").is_empty());
        assert!(found("[x](#section)").is_empty());
        assert!(found("[x](mailto:a@b.c)").is_empty());
        assert_eq!(found("[x](<my file.html>)"), ["my file.html"]);
    }

    #[test]
    fn bare_words_need_a_path_prefix() {
        assert_eq!(
            found("wrote ~/Documents/x/report.html today"),
            ["~/Documents/x/report.html"]
        );
        assert_eq!(
            found("wrote ./out.html and /tmp/a.md"),
            ["./out.html", "/tmp/a.md"]
        );
        assert!(found("wrote out.html and src/lib.rs").is_empty());
        assert!(found("see https://example.com/a/b.html").is_empty());
    }

    #[test]
    fn code_blocks_are_not_mentions() {
        let text = "```\ncat /etc/passwd\n`x.html`\n```\n\n    ~/indented/a.md\n\nok `real.md`";
        assert_eq!(found(text), ["real.md"]);
    }

    #[test]
    fn a_path_split_by_the_parser_is_one_mention() {
        assert_eq!(
            found("see ~/my_file_name_v2.md now"),
            ["~/my_file_name_v2.md"]
        );
    }

    #[test]
    fn duplicates_collapse_and_order_is_kept() {
        assert_eq!(found("`b.md` `a.md` `b.md`"), ["b.md", "a.md"]);
    }

    #[test]
    fn overlong_and_control_character_mentions_are_dropped() {
        let long = format!("`{}.md`", "a".repeat(2000));
        assert!(found(&long).is_empty());
    }
}
