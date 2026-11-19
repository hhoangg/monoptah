//! The text a web push shows: a one-line, plain preview of a message.
//! The desktop builds it, the Worker only encrypts and forwards it
//! (architecture.md section 8), so every limit lives here too.

/// Longest preview, in characters.
pub const PREVIEW_MAX_CHARS: usize = 180;
/// Longest title, in characters.
pub const TITLE_MAX_CHARS: usize = 64;
/// Hard byte cap for the preview, well under the push payload limit.
pub const PREVIEW_MAX_BYTES: usize = 600;
/// Hard byte cap for the title.
pub const TITLE_MAX_BYTES: usize = 200;

/// `text` as one plain line: markdown and code fences removed, whitespace
/// collapsed, cut to [`PREVIEW_MAX_CHARS`] characters (and
/// [`PREVIEW_MAX_BYTES`] bytes) on a character boundary with an ellipsis.
/// Empty when nothing readable is left.
pub fn preview(text: &str) -> String {
    clip(&plain(text), PREVIEW_MAX_CHARS, PREVIEW_MAX_BYTES)
}

/// A name as a one-line title, cut to [`TITLE_MAX_CHARS`] characters.
pub fn title(name: &str) -> String {
    clip(&plain(name), TITLE_MAX_CHARS, TITLE_MAX_BYTES)
}

fn plain(text: &str) -> String {
    let mut lines = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with("```") || line.starts_with("~~~") {
            continue;
        }
        let line = line.trim_start_matches('#').trim_start();
        let line = line.trim_start_matches('>').trim_start();
        let line = strip_list_marker(line);
        lines.push(inline(line));
    }
    lines
        .join(" ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn strip_list_marker(line: &str) -> &str {
    for marker in ["- ", "* ", "+ "] {
        if let Some(rest) = line.strip_prefix(marker) {
            return rest;
        }
    }
    let digits = line.chars().take_while(char::is_ascii_digit).count();
    if digits > 0 {
        if let Some(rest) = line[digits..].strip_prefix(". ") {
            return rest;
        }
    }
    line
}

/// Links and images keep their text, emphasis and code marks go.
fn inline(line: &str) -> String {
    let chars: Vec<char> = line.chars().collect();
    let mut out = String::with_capacity(line.len());
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '!' if chars.get(i + 1) == Some(&'[') => i += 1,
            '[' => {
                if let Some((text_end, link_end)) = link_at(&chars, i) {
                    out.extend(&chars[i + 1..text_end]);
                    i = link_end + 1;
                    continue;
                }
                out.push('[');
                i += 1;
            }
            '`' => i += 1,
            // Only doubled marks are emphasis: a lone `*` or `~` is a glob or a path.
            c @ ('*' | '~' | '_') if chars.get(i + 1) == Some(&c) => i += 2,
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    out
}

/// For `[text](url)` starting at `open`: the index of `]` and of `)`.
fn link_at(chars: &[char], open: usize) -> Option<(usize, usize)> {
    let close = open + 1 + chars[open + 1..].iter().position(|c| *c == ']')?;
    if chars.get(close + 1) != Some(&'(') {
        return None;
    }
    let end = close + 2 + chars[close + 2..].iter().position(|c| *c == ')')?;
    Some((close, end))
}

fn clip(text: &str, max_chars: usize, max_bytes: usize) -> String {
    let fits = |s: &str| s.chars().count() <= max_chars && s.len() <= max_bytes;
    if fits(text) {
        return text.to_owned();
    }
    let mut out = String::new();
    for c in text.chars() {
        // One character is kept free for the ellipsis (3 bytes).
        if out.chars().count() + 1 >= max_chars || out.len() + c.len_utf8() + 3 > max_bytes {
            break;
        }
        out.push(c);
    }
    let mut out = out.trim_end().to_owned();
    out.push('\u{2026}');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_and_fences_become_one_plain_line() {
        let text = "# Done\n\n```rust\nlet x = 1;\n```\n- **bold** and `code`\n1. see [docs](https://x.test/a)\n> quoted";
        assert_eq!(
            preview(text),
            "Done let x = 1; bold and code see docs quoted"
        );
    }

    #[test]
    fn whitespace_collapses_and_snake_case_survives() {
        assert_eq!(preview("a \t b\n\n c_d"), "a b c_d");
        assert_eq!(preview("ls ~/x src/*.rs"), "ls ~/x src/*.rs");
        assert_eq!(preview("```\n```"), "");
    }

    #[test]
    fn long_text_is_cut_on_a_character_boundary_with_an_ellipsis() {
        let long = "e\u{301}".repeat(500);
        let out = preview(&long);
        assert!(out.chars().count() <= PREVIEW_MAX_CHARS);
        assert!(out.len() <= PREVIEW_MAX_BYTES);
        assert!(out.ends_with('\u{2026}'));
        let wide = "\u{1F600}".repeat(400);
        let out = preview(&wide);
        assert!(out.len() <= PREVIEW_MAX_BYTES, "{} bytes", out.len());
        assert!(out.chars().count() <= PREVIEW_MAX_CHARS);
        assert!(out.ends_with('\u{2026}'));
    }

    #[test]
    fn short_text_is_untouched_and_titles_are_capped() {
        assert_eq!(preview("All good"), "All good");
        let out = title(&"x".repeat(200));
        assert_eq!(out.chars().count(), TITLE_MAX_CHARS);
        assert!(out.ends_with('\u{2026}'));
        assert_eq!(title("Bach\nTieu"), "Bach Tieu");
    }
}
