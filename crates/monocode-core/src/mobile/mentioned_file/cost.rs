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
//!
//! This is a fast pre-filter, not the guarantee: the transform runs in an isolated child process
//! with a time, output and memory limit (see [`super::sandbox`]). The document's census comes
//! from the real parse ([`super::prepass`]: clones included, entities decoded), and the style text
//! is the concatenation of every `<style>` and linked sheet in the order `css-inline` joins them.
//!
//! Matching cost: a rule costs one check per element plus, for each element its rightmost
//! compound can reach, the work of the rest of the selector. A descendant combinator walks the
//! ancestors (up to the depth of the document, and backtracks), `~`, `:nth-*` and `*-of-type`
//! walk the siblings (up to the widest node), and `:has(` walks the descendants (up to every
//! element); each such feature in a selector (also inside `:not(`, `:is(`, `:where(`) multiplies
//! the reach by that bound. A rule with no declarations costs matching all the same.
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
    /// Bounds of the ancestors and the siblings a combinator can walk.
    pub depth: u64,
    pub siblings: u64,
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
            // Without a parse the walk bounds are the element count.
            depth: 0,
            siblings: 0,
            elements: bytes
                .windows(2)
                .filter(|pair| pair[0] == b'<' && pair[1].is_ascii_alphabetic())
                .count() as u64,
            opaque: entity_in_a_tag(bytes),
            ..Self::default()
        };
        census.depth = census.elements;
        census.siblings = census.elements;
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

    /// The census of a parsed document: every count is the true one, entities decoded.
    pub fn from_shape(shape: &super::prepass::Shape) -> Self {
        Self {
            elements: shape.elements,
            depth: shape.max_depth.max(1),
            siblings: shape.max_children.max(1),
            words: shape.words.clone(),
            opaque: shape.opaque,
        }
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

    /// An upper bound of the elements a rule with this selector list can match. The list is cut
    /// at EVERY comma, also inside parentheses, like `css-inline` does (an over-count at worst).
    fn reach_list(&self, prelude: &str) -> u64 {
        let total = prelude
            .split(',')
            .fold(0u64, |total, piece| total.saturating_add(self.reach(piece)));
        total
            .min(
                self.elements
                    .saturating_mul(prelude.matches(',').count() as u64 + 1),
            )
            .min(self.elements)
    }

    /// The matching cost of a rule with this selector list (see the module docs).
    fn match_list(&self, prelude: &str) -> u64 {
        prelude.split(',').fold(0u64, |total, piece| {
            let walk = (0..walks(piece, WalkKind::Ancestors))
                .fold(1u64, |a, _| a.saturating_mul(self.depth.max(1)))
                .saturating_mul(
                    (0..walks(piece, WalkKind::Siblings))
                        .fold(1u64, |a, _| a.saturating_mul(self.siblings.max(1))),
                )
                .saturating_mul(
                    (0..walks(piece, WalkKind::Descendants))
                        .fold(1u64, |a, _| a.saturating_mul(self.elements.max(1))),
                );
            total
                .saturating_add(self.elements)
                .saturating_add(self.reach(piece).saturating_mul(walk))
        })
    }
}

#[derive(Clone, Copy)]
enum WalkKind {
    Ancestors,
    Siblings,
    Descendants,
}

/// How many walks of `kind` one selector piece makes.
fn walks(piece: &str, kind: WalkKind) -> u32 {
    let lower = piece.to_ascii_lowercase();
    match kind {
        WalkKind::Descendants => lower.matches(":has(").count() as u32,
        WalkKind::Siblings => {
            (lower.matches('~').count()
                + lower.matches(":nth-").count()
                + lower.matches("-of-type").count()) as u32
        }
        WalkKind::Ancestors => {
            let bytes = lower.as_bytes();
            let mut count = 0;
            let mut at = 0;
            let mut square = 0i32;
            while at < bytes.len() {
                match bytes[at] {
                    b'[' => square += 1,
                    b']' => square -= 1,
                    b'"' | b'\'' => {
                        at = skip_string(bytes, at);
                        continue;
                    }
                    byte if byte.is_ascii_whitespace() && square <= 0 => {
                        let before = bytes[..at].iter().rev().find(|b| !b.is_ascii_whitespace());
                        while at < bytes.len() && bytes[at].is_ascii_whitespace() {
                            at += 1;
                        }
                        let after = bytes.get(at);
                        let joins = |b: Option<&u8>| {
                            b.is_none_or(|b| matches!(b, b'>' | b'+' | b'~' | b'(' | b')' | b','))
                        };
                        if !(joins(before) || joins(after)) {
                            count += 1;
                        }
                        continue;
                    }
                    _ => {}
                }
                at += 1;
            }
            // Child combinators (`>`) walk one step but chain like the others.
            count + lower.matches('>').count() as u32
        }
    }
}

/// An entity (`&` followed by a letter, a digit or `#`) inside a tag, where it can spell a class
/// name, an id or a tag name without writing it. Quotes are not tracked: any quote opens a quoted
/// stretch, so the scan stays inside a tag at least as long as the parser does.
fn entity_in_a_tag(bytes: &[u8]) -> bool {
    let (mut in_tag, mut quote) = (false, 0u8);
    for (at, byte) in bytes.iter().enumerate() {
        if !in_tag {
            in_tag = *byte == b'<' && bytes.get(at + 1).is_some_and(u8::is_ascii_alphabetic);
            continue;
        }
        if *byte == b'&'
            && bytes
                .get(at + 1)
                .is_some_and(|next| next.is_ascii_alphanumeric() || *next == b'#')
        {
            return true;
        }
        if quote != 0 {
            if *byte == quote {
                quote = 0;
            }
        } else if matches!(byte, b'"' | b'\'') {
            quote = *byte;
        } else if *byte == b'>' {
            in_tag = false;
        }
    }
    false
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

/// What inlining a stylesheet costs, in abstract units (see the module docs).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Cost {
    /// Declaration bytes times the elements they can reach.
    pub copy: u64,
    /// Selector checks, with the walks of combinators and structural pseudo classes.
    pub matching: u64,
}

impl Cost {
    fn add(&mut self, other: Cost) {
        self.copy = self.copy.saturating_add(other.copy);
        self.matching = self.matching.saturating_add(other.matching);
    }
}

/// The copy cost of `css` (see [`assess`]).
pub fn copy_cost(css: &str, census: &Census) -> u64 {
    assess(css, census).copy
}

/// Declaration bytes times the elements they can reach, and the matching cost, summed over the
/// rules of `css`. Comments and strings are skipped like a CSS parser does; at-rules that do not
/// hold rules for elements (`@font-face`, `@keyframes`, ...) cost nothing and a nested rule is
/// counted as reaching every element.
pub fn assess(css: &str, census: &Census) -> Cost {
    let bytes = css.as_bytes();
    let mut total = Cost::default();
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
                        total.add(assess(inner, census));
                    }
                } else if inner.contains('{') {
                    total.add(Cost {
                        copy: (inner.len() as u64).saturating_mul(census.elements),
                        matching: census.elements.saturating_mul(census.elements),
                    });
                } else {
                    total.add(Cost {
                        copy: (inner.len() as u64).saturating_mul(census.reach_list(prelude)),
                        matching: census.match_list(prelude),
                    });
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

    #[test]
    fn every_comma_splits_a_selector_list_like_the_inliner_does() {
        let census = Census::of("<p class=a>x</p><p class=b>y</p><p>z</p>");
        // `:is(.a, p)` is cut into `p:is(.a` and `p)`: the second reaches every `p`.
        assert_eq!(census.reach_list("i:is(.a, p)"), 3);
        assert_eq!(census.reach_list(".a, .b"), 2);
    }

    #[test]
    fn an_entity_inside_a_tag_hides_names_but_one_in_text_does_not() {
        let hidden = Census::of("<p class=&Aacute;x>x</p><i>");
        assert_eq!(hidden.reach(".y"), 2);
        let named = Census::of("<p class='a&#98;'>x</p><i>");
        assert_eq!(named.reach(".b"), 2);
        let text = Census::of("<p class=a>AT&T &amp; co</p><i>");
        assert_eq!(text.reach(".b"), 0);
    }

    #[test]
    fn walking_selectors_cost_more_than_plain_ones_even_without_declarations() {
        let mut census = Census::of(&"<i>x</i>".repeat(1000));
        census.depth = 100;
        census.siblings = 500;
        let plain = assess("i{}", &census).matching;
        let descendant = assess("i i{}", &census).matching;
        let sibling = assess("i ~ i{}", &census).matching;
        let nth = assess("i:nth-child(2n){}", &census).matching;
        let has = assess("i:has(b){}", &census).matching;
        assert!(descendant >= 100 * plain / 2, "{descendant} {plain}");
        assert!(sibling >= 500 * plain / 2);
        assert!(nth >= 500 * plain / 2);
        assert!(has >= 1000 * plain / 2);
        // Chained walks multiply.
        assert!(assess("i i i i{}", &census).matching > 100 * descendant);
    }
}
