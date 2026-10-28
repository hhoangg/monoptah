//! Prompt and command templates for workflows: `{{a.b.c}}` and `{{#if x}}...{{/if}}`.
//!
//! The scanners are hand written over chars (the React app uses regexes) so JS `\s` and `\w`
//! mean exactly what they meant there. Values come from a run scope held as `Json`.
use super::ordered::{js_number_text, Json};
use std::borrow::Cow;

/// JS `\s`: what `RegExp`, `String.trim` and `/\s/` treat as white space.
pub fn js_space(c: char) -> bool {
    matches!(
        c,
        '\t' | '\n' | '\u{b}' | '\u{c}' | '\r' | ' ' | '\u{a0}' | '\u{1680}' | '\u{2000}'
            ..='\u{200a}'
                | '\u{2028}'
                | '\u{2029}'
                | '\u{202f}'
                | '\u{205f}'
                | '\u{3000}'
                | '\u{feff}'
    )
}

/// JS `String.prototype.trim`.
pub fn js_trim(text: &str) -> &str {
    text.trim_matches(js_space)
}

/// JS `\w`.
pub fn js_word(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

fn skip_space(c: &[char], mut i: usize) -> usize {
    while i < c.len() && js_space(c[i]) {
        i += 1;
    }
    i
}

fn starts(c: &[char], i: usize, pat: &str) -> bool {
    pat.chars()
        .enumerate()
        .all(|(k, p)| c.get(i + k) == Some(&p))
}

/// `[^{}\s]+` from `i`: the end of the path, or None when it is empty.
fn path_end(c: &[char], i: usize) -> Option<usize> {
    let mut end = i;
    while end < c.len() && !matches!(c[end], '{' | '}') && !js_space(c[end]) {
        end += 1;
    }
    (end > i).then_some(end)
}

/// `\{\{\s*([^{}\s]+)\s*\}\}` anchored at `i`: the path and where the match ends.
pub(crate) fn var_at(c: &[char], i: usize) -> Option<(String, usize)> {
    if !starts(c, i, "{{") {
        return None;
    }
    let from = skip_space(c, i + 2);
    let end = path_end(c, from)?;
    let close = skip_space(c, end);
    starts(c, close, "}}").then(|| (c[from..end].iter().collect(), close + 2))
}

/// `\{\{#if\s+([^{}\s]+)\s*\}\}` anchored at `i`.
fn if_open_at(c: &[char], i: usize) -> Option<(String, usize)> {
    if !starts(c, i, "{{#if") {
        return None;
    }
    let from = skip_space(c, i + 5);
    if from == i + 5 {
        return None;
    }
    let end = path_end(c, from)?;
    let close = skip_space(c, end);
    starts(c, close, "}}").then(|| (c[from..end].iter().collect(), close + 2))
}

/// Every `{{path}}` in order, as (path, start, end) in chars; a global `VAR` match.
pub(crate) fn scan_vars(c: &[char]) -> Vec<(String, usize, usize)> {
    let mut found = Vec::new();
    let mut i = 0;
    while i < c.len() {
        match var_at(c, i) {
            Some((path, end)) => {
                found.push((path, i, end));
                i = end;
            }
            None => i += 1,
        }
    }
    found
}

/// Every variable path used by the template (including `#if` conditions), once, in order.
/// Paths that start with `/` (a closing `{{/if}}`) are not variables.
pub fn template_vars(text: &str) -> Vec<String> {
    let c: Vec<char> = text.chars().collect();
    let mut seen: Vec<String> = Vec::new();
    let mut i = 0;
    while i < c.len() {
        match if_open_at(&c, i).or_else(|| var_at(&c, i)) {
            Some((path, end)) => {
                if !path.starts_with('/') && !seen.contains(&path) {
                    seen.push(path);
                }
                i = end;
            }
            None => i += 1,
        }
    }
    seen
}

const UNSAFE_MESSAGE: &str = "the command uses shell syntax that cannot be quoted safely around variables; move the variable to a simple argument or pass it differently";
const IF_MESSAGE: &str = "conditionals ({{#if}}) are not supported in command templates";

fn has(c: &[char], pat: &str) -> bool {
    (0..c.len()).any(|i| starts(c, i, pat))
}

fn shell_word_char(c: char) -> bool {
    js_word(c) || matches!(c, '.' | '/' | '-')
}

/// `(^|[^\w./-])(let|declare|typeset|eval)(?![\w./-])`
fn has_re_parsing_word(c: &[char]) -> bool {
    ["let", "declare", "typeset", "eval"].iter().any(|word| {
        let len = word.chars().count();
        (0..c.len()).any(|i| {
            starts(c, i, word)
                && (i == 0 || !shell_word_char(c[i - 1]))
                && c.get(i + len).is_none_or(|n| !shell_word_char(*n))
        })
    })
}

/// Replaces `'[^']*'` and `"(?:\\.|[^"\\])*"` with `''`, like the React app's `QUOTED` replace.
fn blank_quoted(c: &[char]) -> Vec<char> {
    let mut out = Vec::with_capacity(c.len());
    let mut i = 0;
    while i < c.len() {
        let end = match c[i] {
            '\'' => c[i + 1..]
                .iter()
                .position(|ch| *ch == '\'')
                .map(|p| i + 1 + p + 1),
            '"' => {
                let mut j = i + 1;
                let mut closed = None;
                while j < c.len() {
                    match c[j] {
                        '"' => {
                            closed = Some(j + 1);
                            break;
                        }
                        // `.` does not match a line terminator.
                        '\\' => match c.get(j + 1) {
                            Some('\n' | '\r' | '\u{2028}' | '\u{2029}') | None => break,
                            Some(_) => j += 2,
                        },
                        _ => j += 1,
                    }
                }
                closed
            }
            _ => None,
        };
        match end {
            Some(end) => {
                out.extend(['\'', '\'']);
                i = end;
            }
            None => {
                out.push(c[i]);
                i += 1;
            }
        }
    }
    out
}

/// True when the literal text uses shell syntax this check cannot reason about.
fn uses_unquotable_syntax(text: &str) -> bool {
    let c: Vec<char> = text.chars().collect();
    // A backslash would escape the quote the app adds; a `$` would turn it into `$'...'`.
    if has(&c, "\\{{") {
        return true;
    }
    if (0..c.len()).any(|i| c[i] == '$' && starts(&c, skip_space(&c, i + 1), "{{")) {
        return true;
    }
    // Bash deletes backslash-newline before tokenising, which can join `$` and `'`, `<` and `<`.
    if has(&c, "\\\n") || has(&c, "\\\r\n") {
        return true;
    }
    // Parameter expansion nests its own quoting, which the tracker does not follow.
    if has(&c, "${") {
        return true;
    }
    let mut literal: Vec<char> = Vec::with_capacity(c.len());
    let mut at = 0;
    for (_, start, end) in scan_vars(&c) {
        literal.extend_from_slice(&c[at..start]);
        literal.push('X');
        at = end;
    }
    literal.extend_from_slice(&c[at..]);
    for pat in ["`", "$(", "$'", "$[", "<<", "((", "[[", "*(", "?("] {
        if has(&literal, pat) {
            return true;
        }
    }
    if has_re_parsing_word(&literal) {
        return true;
    }
    // A `(` glued to the word before it is a zsh glob qualifier; quoted text is not globbed.
    let plain = blank_quoted(&literal);
    let glued = (0..plain.len().saturating_sub(1)).any(|i| {
        plain[i + 1] == '('
            && !js_space(plain[i])
            && !matches!(plain[i], ';' | '&' | '|' | '(' | ')' | '<' | '>')
    });
    if glued {
        return true;
    }
    // A `#` at the start of a word starts a comment that can swallow quotes.
    (0..literal.len()).any(|i| {
        literal[i] == '#'
            && (i == 0
                || js_space(literal[i - 1])
                || matches!(literal[i - 1], ';' | '&' | '|' | '(' | ')'))
    })
}

/// Problems with using `text` as a shell command template, as readable messages.
///
/// Covers quoting context only. The app single-quotes every value, so a variable written inside
/// the author's own quotes would be wrapped twice and could break out of them. Deliberately
/// conservative: a simple quote tracker, not a shell parser. A variable is accepted only in a
/// clearly bare context; when the text uses syntax the tracker cannot follow, every variable is
/// reported. Conditionals are rejected outright.
pub fn shell_template_problems(text: &str) -> Vec<String> {
    let c: Vec<char> = text.chars().collect();
    let every_var = |message: &str| -> Vec<String> {
        scan_vars(&c)
            .into_iter()
            .filter(|(path, ..)| !path.starts_with('/'))
            .map(|(path, ..)| format!("{{{{{path}}}}}: {message}"))
            .collect()
    };
    if text.contains("{{#if") || text.contains("{{/if}}") {
        let mut problems = vec![IF_MESSAGE.to_string()];
        problems.extend(every_var(IF_MESSAGE));
        return problems;
    }
    if uses_unquotable_syntax(text) {
        return every_var(UNSAFE_MESSAGE);
    }
    let mut problems = Vec::new();
    let mut quote: Option<char> = None;
    let mut i = 0;
    while i < c.len() {
        if starts(&c, i, "{{") {
            if let Some((path, end)) = var_at(&c, i) {
                if let Some(q) = quote {
                    let name = if q == '\'' {
                        "single quotes"
                    } else {
                        "double quotes"
                    };
                    problems.push(format!(
                        "{{{{{path}}}}} is inside {name}; write it bare, the app quotes it for the shell"
                    ));
                }
                i = end;
                continue;
            }
        }
        let ch = c[i];
        if ch == '\\' && quote != Some('\'') {
            // Never let the escape consume the start of a `{{` token.
            i += if starts(&c, i + 1, "{{") { 1 } else { 2 };
            continue;
        }
        match quote {
            None => {
                if ch == '\'' || ch == '"' {
                    quote = Some(ch);
                }
            }
            Some(q) if ch == q => quote = None,
            Some(_) => {}
        }
        i += 1;
    }
    problems
}

/// POSIX single-quote quoting: the value always reaches the shell as one literal word.
/// Quoting only protects against word splitting, expansion and operators. A value that a command
/// later feeds to eval, bash -c, (( )), [[ ]], let, declare -i or an array subscript is still
/// interpreted by the shell, so `shell_template_problems` rejects the syntax forms of these. A
/// value that starts with `-` can still be read as an option; `shell_dash_problems` covers that.
pub fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// A JS array index key: canonical decimal digits, no sign, no leading zero, below 2^32 - 1.
fn array_index(key: &str) -> Option<usize> {
    let canonical = !key.is_empty()
        && key.bytes().all(|b| b.is_ascii_digit())
        && (key == "0" || !key.starts_with('0'));
    canonical
        .then(|| key.parse::<u32>().ok().filter(|i| *i != u32::MAX))
        .flatten()
        .map(|i| i as usize)
}

fn step<'a>(value: &'a Json, key: &str) -> Option<Cow<'a, Json>> {
    match value {
        Json::Object(map) => map.get(key).map(Cow::Borrowed),
        // `hasOwnProperty` is true for an array's indices and for `length`.
        Json::Array(items) if key == "length" => Some(Cow::Owned(Json::Number(items.len() as f64))),
        Json::Array(items) => array_index(key)
            .and_then(|i| items.get(i))
            .map(Cow::Borrowed),
        _ => None,
    }
}

/// Own-property path lookup (`a.b.c`); never reads the prototype chain. None is JS `undefined`.
pub fn lookup<'a>(scope: &'a Json, path: &str) -> Option<Cow<'a, Json>> {
    let mut current = Cow::Borrowed(scope);
    for key in path.split('.') {
        current = match current {
            Cow::Borrowed(value) => step(value, key)?,
            // Only an array's `length` is owned, and a number has no properties.
            Cow::Owned(_) => return None,
        };
    }
    Some(current)
}

/// The text a value renders as: nothing for missing and null, pretty JSON for lists and objects.
fn stringify(value: Option<&Json>) -> String {
    match value {
        None | Some(Json::Null) => String::new(),
        Some(Json::String(s)) => s.clone(),
        Some(Json::Number(n)) => js_number_text(*n),
        Some(Json::Bool(b)) => b.to_string(),
        Some(other) => other.stringify(true),
    }
}

/// Problems with rendered values that start with `-`: a command that gets such a value as the
/// start of a word reads it as an option (`ssh {{host}}` with `-oProxyCommand=...`). It looks at
/// the word as the shell will see it, so an empty value or an empty `''` in front of the
/// variable does not hide it (`ssh {{a}}{{b}}`). A value is fine after a standalone `--` earlier
/// in the same simple command. A variable that follows other text in its word (`--flag={{x}}`,
/// `a-{{x}}`) cannot become an option, so it is not reported.
pub fn shell_dash_problems(text: &str, scope: &Json) -> Vec<String> {
    let c: Vec<char> = text.chars().collect();
    let mut problems = Vec::new();
    let mut word = String::new(); // the current word as rendered so far
    let mut in_word = false;
    let mut plain = true; // no variable in the word yet
    let mut end_of_options = false;
    let mut quote: Option<char> = None;
    let mut i = 0;
    while i < c.len() {
        if let Some((path, end)) = var_at(&c, i) {
            i = end;
            if path.starts_with('/') {
                continue;
            }
            let value = stringify(lookup(scope, &path).as_deref());
            if word.is_empty() && value.starts_with('-') && !end_of_options {
                problems.push(format!(
                    "{{{{{path}}}}}: the value starts with \"-\" and would be read as an option; put \"--\" before it, or use the --option={{{{{path}}}}} form"
                ));
            }
            word.push_str(&value);
            in_word = true;
            plain = false;
            continue;
        }
        let ch = c[i];
        let escaped_token = starts(&c, i + 1, "{{");
        match quote {
            None => {
                if ch == '\\' {
                    if !escaped_token {
                        if let Some(next) = c.get(i + 1) {
                            word.push(*next);
                        }
                    }
                    in_word = true;
                    i += if escaped_token { 1 } else { 2 };
                    continue;
                }
                if ch == '\'' || ch == '"' {
                    quote = Some(ch);
                    in_word = true;
                } else if js_space(ch) || ";&|()<>".contains(ch) {
                    if in_word && plain && word == "--" {
                        end_of_options = true;
                    }
                    word.clear();
                    in_word = false;
                    plain = true;
                    if ch == '\n' || ";&|(".contains(ch) {
                        end_of_options = false;
                    }
                } else {
                    word.push(ch);
                    in_word = true;
                }
            }
            Some(q) if ch == q => quote = None,
            Some('"') if ch == '\\' && !escaped_token => {
                if let Some(next) = c.get(i + 1) {
                    word.push(*next);
                }
                in_word = true;
                i += 2;
                continue;
            }
            Some(_) => {
                word.push(ch);
                in_word = true;
            }
        }
        i += 1;
    }
    problems
}

fn is_truthy(value: Option<&Json>) -> bool {
    match value {
        None | Some(Json::Null) => false,
        Some(Json::Bool(b)) => *b,
        Some(Json::Number(n)) => *n != 0.0 && !n.is_nan(),
        Some(Json::String(s)) => !s.is_empty(),
        Some(Json::Array(items)) => !items.is_empty(),
        Some(Json::Object(_)) => true,
    }
}

/// Where `pat` first occurs at or after `from`.
fn find(c: &[char], from: usize, pat: &str) -> Option<usize> {
    (from..c.len()).find(|i| starts(c, *i, pat))
}

/// Renders `text` against `scope`. Missing values render as an empty string. With `shell` every
/// interpolated value is single-quoted; the template's own text is not.
///
/// `{{#if path}}...{{/if}}` blocks go first: the body is kept when the value is truthy (a
/// non-empty list or any other JS-truthy value). They do not nest and have no else; the first
/// `{{/if}}` closes the block, and an opener with no closer stays as text.
pub fn render_template(text: &str, scope: &Json, shell: bool) -> String {
    let c: Vec<char> = text.chars().collect();
    let mut without_ifs: Vec<char> = Vec::with_capacity(c.len());
    // An opener after the last `{{/if}}` has no closer: it stays text, without a rescan each time.
    let last_close = (0..c.len()).rev().find(|i| starts(&c, *i, "{{/if}}"));
    let mut i = 0;
    while i < c.len() {
        if let Some((path, body)) = if_open_at(&c, i) {
            if let Some(close) = last_close
                .filter(|last| *last >= body)
                .and_then(|_| find(&c, body, "{{/if}}"))
            {
                if is_truthy(lookup(scope, &path).as_deref()) {
                    without_ifs.extend_from_slice(&c[body..close]);
                }
                i = close + "{{/if}}".len();
                continue;
            }
        }
        without_ifs.push(c[i]);
        i += 1;
    }
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < without_ifs.len() {
        match var_at(&without_ifs, i) {
            Some((path, end)) => {
                let rendered = stringify(lookup(scope, &path).as_deref());
                out.push_str(&if shell {
                    shell_quote(&rendered)
                } else {
                    rendered
                });
                i = end;
            }
            None => {
                out.push(without_ifs[i]);
                i += 1;
            }
        }
    }
    out
}
