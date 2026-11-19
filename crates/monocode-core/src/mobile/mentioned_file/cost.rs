//! What a mentioned HTML file would cost the style inliner, estimated from the text alone.
//!
//! The inliner copies every declaration of every rule into each element the rule matches, so
//! its time and memory grow with the declaration bytes times the elements they reach. The work
//! of one document is bounded by two numbers, both computed without running a selector engine:
//!
//! * the **scan** cost: elements times style bytes (selectors included), which is what matching
//!   alone costs;
//! * the **copy** cost: for each rule, its declaration bytes times an upper bound of the
//!   elements it can match. The bound comes from the rule's rightmost compound selector: the
//!   rarest class, id or tag name in it can match no more elements than the word occurs in the
//!   source (counted over the WHOLE source, nothing is skipped, so a decoy tag or comment can
//!   only make the counts larger). A selector with nothing to count (`*`, an attribute, a
//!   pseudo class, an escape) can match every element. A source with numeric character
//!   references (which can spell a class name without writing it) counts every rule as matching
//!   every element.
use std::collections::HashMap;

/// Most distinct words remembered; a word past the limit counts as "everywhere".
const MAX_WORDS: usize = 200_000;
/// At-rules whose body holds ordinary rules.
const CONTAINERS: &[&str] = &[
    "media",
    "supports",
    "layer",
    "container",
    "document",
    "scope",
    "starting-style",
];

/// Elements and word counts of one document.
#[derive(Debug, Default)]
pub struct Census {
    pub elements: u64,
    words: HashMap<String, u64>,
    /// Some word could not be counted: every rule may match every element.
    opaque: bool,
}

fn word_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_') || byte >= 0x80
}

impl Census {
    /// Counts the elements (every `<` followed by a letter, with no skipping) and the words of
    /// `source`.
    pub fn of(source: &str) -> Self {
        let bytes = source.as_bytes();
        let mut census = Self {
            elements: bytes
                .windows(2)
                .filter(|pair| pair[0] == b'<' && pair[1].is_ascii_alphabetic())
                .count() as u64,
            opaque: bytes.windows(2).any(|pair| pair == b"&#"),
            ..Self::default()
        };
        let mut at = 0;
        while at < bytes.len() {
            if !word_byte(bytes[at]) {
                at += 1;
                continue;
            }
            let start = at;
            while at < bytes.len() && word_byte(bytes[at]) {
                at += 1;
            }
            let word = String::from_utf8_lossy(&bytes[start..at]).to_lowercase();
            if let Some(count) = census.words.get_mut(&word) {
                *count += 1;
            } else if census.words.len() < MAX_WORDS {
                census.words.insert(word, 1);
            } else {
                census.opaque = true;
            }
        }
        census
    }

    fn count(&self, word: &str) -> u64 {
        if self.opaque {
            return self.elements;
        }
        self.words
            .get(&word.to_lowercase())
            .copied()
            .unwrap_or(0)
            .min(self.elements)
    }

    /// An upper bound of the elements `selector` (one complex selector) can match.
    fn reach(&self, selector: &str) -> u64 {
        if self.opaque || selector.contains('\\') {
            return self.elements;
        }
        let compound = rightmost_compound(selector);
        let bytes = compound.as_bytes();
        let mut best = self.elements;
        let mut at = 0;
        let mut counted = false;
        while at < bytes.len() {
            match bytes[at] {
                b'.' | b'#' => {
                    at += 1;
                    let start = at;
                    while at < bytes.len() && word_byte(bytes[at]) {
                        at += 1;
                    }
                    best = best.min(self.count(&compound[start..at]));
                    counted = true;
                }
                b'[' => {
                    at = skip_group(bytes, at, b'[', b']');
                }
                b':' => {
                    at += 1;
                    while at < bytes.len() && (bytes[at] == b':' || word_byte(bytes[at])) {
                        at += 1;
                    }
                    if bytes.get(at) == Some(&b'(') {
                        at = skip_group(bytes, at, b'(', b')');
                    }
                }
                byte if word_byte(byte) => {
                    let start = at;
                    while at < bytes.len() && word_byte(bytes[at]) {
                        at += 1;
                    }
                    best = best.min(self.count(&compound[start..at]));
                    counted = true;
                }
                _ => at += 1,
            }
        }
        if counted {
            best
        } else {
            self.elements
        }
    }

    /// An upper bound of the elements a rule with this selector list can match.
    fn reach_list(&self, prelude: &str) -> u64 {
        let mut total = 0u64;
        let mut depth = 0i32;
        let mut start = 0;
        let bytes = prelude.as_bytes();
        for (at, byte) in bytes.iter().enumerate() {
            match byte {
                b'(' | b'[' => depth += 1,
                b')' | b']' => depth -= 1,
                b',' if depth <= 0 => {
                    total = total.saturating_add(self.reach(&prelude[start..at]));
                    start = at + 1;
                }
                _ => {}
            }
        }
        total = total.saturating_add(self.reach(&prelude[start..]));
        total.min(self.elements)
    }
}

/// The index just past the group opened at `at` (nesting counted), or the end.
fn skip_group(bytes: &[u8], at: usize, open: u8, close: u8) -> usize {
    let mut depth = 0i32;
    let mut index = at;
    while index < bytes.len() {
        if bytes[index] == open {
            depth += 1;
        } else if bytes[index] == close {
            depth -= 1;
            if depth == 0 {
                return index + 1;
            }
        }
        index += 1;
    }
    bytes.len()
}

/// The part of a complex selector after its last combinator (outside brackets and parentheses).
fn rightmost_compound(selector: &str) -> &str {
    let bytes = selector.trim().as_bytes();
    let trimmed = selector.trim();
    let mut depth = 0i32;
    let mut last = 0;
    for (at, byte) in bytes.iter().enumerate() {
        match byte {
            b'(' | b'[' => depth += 1,
            b')' | b']' => depth -= 1,
            b' ' | b'\t' | b'\n' | b'\r' | b'>' | b'+' | b'~' if depth <= 0 => last = at + 1,
            _ => {}
        }
    }
    &trimmed[last..]
}

/// Declaration bytes times the elements they can reach, summed over the rules of `css`. Comments
/// and strings are skipped like a CSS parser does; at-rules that do not hold rules for elements
/// (`@font-face`, `@keyframes`, ...) cost nothing and a nested rule is counted as reaching every
/// element.
pub fn copy_cost(css: &str, census: &Census) -> u64 {
    let bytes = css.as_bytes();
    let mut total = 0u64;
    let mut prelude_start = 0;
    let mut at = 0;
    while at < bytes.len() {
        match bytes[at] {
            b'/' if bytes.get(at + 1) == Some(&b'*') => {
                at = css[at + 2..]
                    .find("*/")
                    .map_or(bytes.len(), |end| at + 2 + end + 2);
            }
            b'"' | b'\'' => at = skip_string(bytes, at),
            b'\\' => at += 2,
            b';' | b'}' => {
                at += 1;
                prelude_start = at;
            }
            b'{' => {
                let prelude = strip_comments(&css[prelude_start..at]);
                let prelude = prelude.trim();
                let (end, inner) = block(css, at);
                if let Some(rest) = prelude.strip_prefix('@') {
                    let name = rest
                        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '-'))
                        .next()
                        .unwrap_or("")
                        .to_ascii_lowercase();
                    if CONTAINERS.contains(&name.as_str()) {
                        total = total.saturating_add(copy_cost(inner, census));
                    }
                } else if inner.contains('{') {
                    total =
                        total.saturating_add((inner.len() as u64).saturating_mul(census.elements));
                } else {
                    total = total.saturating_add(
                        (inner.len() as u64).saturating_mul(census.reach_list(prelude)),
                    );
                }
                at = end;
                prelude_start = at;
            }
            _ => at += 1,
        }
    }
    total
}

/// The text of `css` from the `{` at `open` to its matching `}` (exclusive), and the index past
/// the `}`. Strings and comments are skipped; an unclosed block runs to the end.
fn block(css: &str, open: usize) -> (usize, &str) {
    let bytes = css.as_bytes();
    let mut depth = 0i32;
    let mut at = open;
    while at < bytes.len() {
        match bytes[at] {
            b'/' if bytes.get(at + 1) == Some(&b'*') => {
                at = css[at + 2..]
                    .find("*/")
                    .map_or(bytes.len(), |end| at + 2 + end + 2);
                continue;
            }
            b'"' | b'\'' => {
                at = skip_string(bytes, at);
                continue;
            }
            b'\\' => {
                at += 2;
                continue;
            }
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return (at + 1, &css[open + 1..at]);
                }
            }
            _ => {}
        }
        at += 1;
    }
    (bytes.len(), &css[(open + 1).min(css.len())..])
}

fn skip_string(bytes: &[u8], at: usize) -> usize {
    let quote = bytes[at];
    let mut index = at + 1;
    while index < bytes.len() {
        match bytes[index] {
            b'\\' => index += 1,
            byte if byte == quote => return index + 1,
            b'\n' => return index,
            _ => {}
        }
        index += 1;
    }
    bytes.len()
}

fn strip_comments(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("/*") {
        out.push_str(&rest[..start]);
        rest = rest[start + 2..]
            .find("*/")
            .map_or("", |end| &rest[start + 2 + end + 2..]);
    }
    out.push_str(rest);
    out
}

/// The bytes of every `<style>` region of `source` (the text up to the next `</style`, or the
/// end), and the copy cost of those regions. A decoy opener (`<!--<style>-->`, `<styled>`) only
/// makes the regions larger.
pub fn inline_styles(source: &str, census: &Census) -> (u64, u64) {
    let bytes = source.as_bytes();
    let (mut size, mut copy) = (0u64, 0u64);
    let mut at = 0;
    while let Some(found) = find_ignore_case(&bytes[at..], b"<style") {
        let start = at + found + b"<style".len();
        let end =
            find_ignore_case(&bytes[start..], b"</style").map_or(bytes.len(), |end| start + end);
        // Region text starts after the opening tag's `>` when there is one.
        let from = bytes[start..end]
            .iter()
            .position(|b| *b == b'>')
            .map_or(start, |open| start + open + 1);
        size += (end - start) as u64;
        // Cut on character boundaries: the slice bounds come from ASCII matches.
        copy = copy.saturating_add(copy_cost(&source[from..end], census));
        at = end;
        if at >= bytes.len() {
            break;
        }
    }
    (size, copy)
}

pub fn find_ignore_case(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window.eq_ignore_ascii_case(needle))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rare_classes_reach_few_elements_and_bare_tags_reach_all() {
        let census = Census::of("<p class=a>x</p><p class=a>y</p><p class=b>z</p>");
        assert_eq!(census.elements, 3);
        assert_eq!(census.reach(".b"), 1);
        assert_eq!(census.reach("div .a.b"), 1);
        assert_eq!(census.reach(".nothing"), 0);
        assert_eq!(census.reach("*"), 3);
        assert_eq!(census.reach("[a]"), 3);
        assert_eq!(census.reach(r"\31 0"), 3);
        assert_eq!(census.reach_list(".a, .b"), 3);
        assert_eq!(census.reach_list("p:not(.a)"), 3);
    }

    #[test]
    fn a_numeric_reference_makes_every_rule_reach_every_element() {
        let census = Census::of("<p class=&#97;>x</p><i>");
        assert_eq!(census.reach(".a"), 2);
    }

    #[test]
    fn copy_cost_multiplies_declarations_by_reach() {
        let census = Census::of("<i class=a><i class=a><i>");
        // 3 elements; `.a` reaches 2, `i` reaches 3; the font-face costs nothing.
        let css = ".a{x:1}@media print{i{y:22}}@font-face{font-family:q}";
        assert_eq!(copy_cost(css, &census), 3 * 2 + 4 * 3);
    }
}
