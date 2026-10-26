//! A small deterministic YAML writer for the workflow export. It writes what the npm `yaml`
//! package writes for `stringify(doc, { lineWidth: 0 })` (the React app's export): block maps and
//! lists with two-space indent, list items indented under their key, `{}` / `[]` for empty
//! collections, no line folding, and strings left plain only when they read back as the same
//! string. Anything else is double-quoted, or a `|` block when it spans lines.
use super::ordered::{js_number_text, Json, OrderedMap};
use super::yaml_read::{parse_yaml, reads_as_non_string};

/// The document tree the export builds.
#[derive(Clone, Debug, PartialEq)]
pub enum Doc {
    /// Text; the writer picks plain, quoted or block style.
    Str(String),
    /// Text that is always double-quoted (the schedule `time`).
    Quoted(String),
    Num(f64),
    Bool(bool),
    Map(Vec<(String, Doc)>),
    Seq(Vec<Doc>),
}

impl Doc {
    /// The same tree as plain data, for the checks the import also runs.
    pub fn to_json(&self) -> Json {
        match self {
            Doc::Str(s) | Doc::Quoted(s) => Json::String(s.clone()),
            Doc::Num(n) => Json::Number(*n),
            Doc::Bool(b) => Json::Bool(*b),
            Doc::Seq(items) => Json::Array(items.iter().map(Doc::to_json).collect()),
            Doc::Map(entries) => {
                let mut map = OrderedMap::new();
                for (k, v) in entries {
                    map.insert(k.clone(), v.to_json());
                }
                Json::Object(map)
            }
        }
    }
}

const INDENT_STEP: &str = "  ";

/// How a string is being written: the indent its content sits at, and whether it is a map key.
#[derive(Clone, Copy)]
struct Ctx<'a> {
    indent: &'a str,
    key: bool,
}

/// The document as YAML text, ending in a newline.
pub fn stringify(doc: &Doc) -> String {
    let mut out = node(doc, "");
    out.push('\n');
    out
}

fn is_empty_collection(doc: &Doc) -> bool {
    matches!(doc, Doc::Map(e) if e.is_empty()) || matches!(doc, Doc::Seq(i) if i.is_empty())
}

/// A node written at `indent`: where a collection's own lines sit and a block scalar's text hangs.
fn node(doc: &Doc, indent: &str) -> String {
    match doc {
        Doc::Map(entries) if entries.is_empty() => "{}".into(),
        Doc::Seq(items) if items.is_empty() => "[]".into(),
        Doc::Map(entries) => {
            let lines: Vec<String> = entries.iter().map(|(k, v)| pair(k, v, indent)).collect();
            join(lines, indent)
        }
        Doc::Seq(items) => {
            // A list's items sit under the key; each is "- " plus the item written two deeper.
            let item_indent = format!("{indent}{INDENT_STEP}");
            let lines: Vec<String> = items
                .iter()
                .map(|item| format!("- {}", node(item, &item_indent)))
                .collect();
            join(lines, indent)
        }
        Doc::Str(s) => string(s, Ctx { indent, key: false }),
        Doc::Quoted(s) => double_quoted(s, Ctx { indent, key: false }),
        Doc::Num(n) => number(*n),
        Doc::Bool(b) => b.to_string(),
    }
}

fn join(lines: Vec<String>, indent: &str) -> String {
    lines.join(&format!("\n{indent}"))
}

fn number(n: f64) -> String {
    if n == 0.0 && n.is_sign_negative() {
        // `JSON.stringify(-0)` is "0", but the npm package writes `-0`.
        "-0".into()
    } else if n.is_nan() {
        ".nan".into()
    } else if n.is_infinite() {
        if n < 0.0 { "-.inf" } else { ".inf" }.into()
    } else {
        js_number_text(n)
    }
}

/// `key: value`, written in a map whose lines sit at `indent`.
fn pair(key: &str, value: &Doc, indent: &str) -> String {
    let deeper = format!("{indent}{INDENT_STEP}");
    let key_text = string(
        key,
        Ctx {
            indent: &deeper,
            key: true,
        },
    );
    // A key too long to be an implicit key is written with the explicit "? " form.
    let explicit = key_text.encode_utf16().count() > 1024;
    let head = if explicit {
        format!("? {key_text}\n{indent}:")
    } else {
        format!("{key_text}:")
    };
    let body = node(value, &deeper);
    let nested = matches!(value, Doc::Map(_) | Doc::Seq(_)) && !is_empty_collection(value);
    if nested && !explicit {
        format!("{head}\n{deeper}{body}")
    } else if body.is_empty() || body.starts_with('\n') {
        format!("{head}{body}")
    } else {
        format!("{head} {body}")
    }
}

// --- strings -------------------------------------------------------------------------------

fn is_control(c: char) -> bool {
    matches!(c, '\u{0}'..='\u{8}' | '\u{b}'..='\u{1f}' | '\u{7f}'..='\u{9f}')
}

fn string(value: &str, ctx: Ctx) -> String {
    if value.chars().any(is_control) {
        return double_quoted(value, ctx);
    }
    plain(value, ctx)
}

/// `/^(%|---|\.\.\.)/m`
fn has_document_marker(value: &str) -> bool {
    value
        .split('\n')
        .any(|line| line.starts_with('%') || line.starts_with("---") || line.starts_with("..."))
}

/// `/^[\n\t ,[\]{}#&*!|>'"%@`]|^[?-]$|^[?-][ \t]|[\n:][ \t]|[ \t]\n|[\n\t ]#|[\n\t :]$/`
fn plain_hazard(value: &str) -> bool {
    let c: Vec<char> = value.chars().collect();
    let Some(&first) = c.first() else {
        return false;
    };
    let ws = |ch: char| matches!(ch, '\n' | '\t' | ' ');
    if ws(first) || ",[]{}#&*!|>'\"%@`".contains(first) {
        return true;
    }
    if (first == '?' || first == '-') && (c.len() == 1 || matches!(c[1], ' ' | '\t')) {
        return true;
    }
    if c.windows(2).any(|w| {
        (matches!(w[0], '\n' | ':') && matches!(w[1], ' ' | '\t'))
            || (matches!(w[0], ' ' | '\t') && w[1] == '\n')
            || (ws(w[0]) && w[1] == '#')
    }) {
        return true;
    }
    c.last().is_some_and(|&l| ws(l) || l == ':')
}

fn plain(value: &str, ctx: Ctx) -> String {
    if ctx.key && value.contains('\n') {
        return quoted(value, ctx);
    }
    if plain_hazard(value) {
        return if ctx.key || !value.contains('\n') {
            quoted(value, ctx)
        } else {
            block(value, ctx)
        };
    }
    // The type is not set explicitly, so a multi-line string prefers the block style.
    if !ctx.key && value.contains('\n') {
        return block(value, ctx);
    }
    if has_document_marker(value) {
        if ctx.indent.is_empty() {
            return block(value, ctx);
        } else if ctx.key && ctx.indent == INDENT_STEP {
            return quoted(value, ctx);
        }
    }
    if reads_as_non_string(value) {
        return quoted(value, ctx);
    }
    value.to_string()
}

fn quoted(value: &str, ctx: Ctx) -> String {
    let (double, single) = (value.contains('"'), value.contains('\''));
    if double && !single {
        single_quoted(value, ctx)
    } else {
        double_quoted(value, ctx)
    }
}

fn indent_of(value: &str, ctx: Ctx) -> String {
    if !ctx.indent.is_empty() {
        ctx.indent.to_string()
    } else if has_document_marker(value) {
        INDENT_STEP.to_string()
    } else {
        String::new()
    }
}

fn single_quoted(value: &str, ctx: Ctx) -> String {
    let around_break = {
        let c: Vec<char> = value.chars().collect();
        c.windows(2).any(|w| {
            (matches!(w[0], ' ' | '\t') && w[1] == '\n')
                || (w[0] == '\n' && matches!(w[1], ' ' | '\t'))
        })
    };
    if (ctx.key && value.contains('\n')) || around_break {
        return double_quoted(value, ctx);
    }
    let indent = indent_of(value, ctx);
    let mut out = String::from("'");
    let mut run = false;
    for ch in value.chars() {
        if ch == '\n' {
            out.push('\n');
            run = true;
            continue;
        }
        if run {
            // Each run of line breaks gains one more break, then the indent.
            out.push('\n');
            out.push_str(&indent);
            run = false;
        }
        if ch == '\'' {
            out.push_str("''");
        } else {
            out.push(ch);
        }
    }
    if run {
        out.push('\n');
        out.push_str(&indent);
    }
    out.push('\'');
    out
}

/// `JSON.stringify` for a string.
fn json_string(value: &str) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "\"\"".into())
}

/// The double-quoted form, with `yaml`'s escapes: `\0 \a \v \e \xNN` instead of `\u00NN`, a
/// space before a line break escaped, and a long text's breaks written as real breaks.
fn double_quoted(value: &str, ctx: Ctx) -> String {
    const MIN_MULTI_LINE: usize = 40;
    let json: Vec<char> = json_string(value).chars().collect();
    let json_len = json.iter().map(|c| c.len_utf16()).sum::<usize>();
    let indent = indent_of(value, ctx);
    let at = |i: usize| json.get(i).copied();
    let slice =
        |from: usize, to: usize| -> String { json[from..to.min(json.len())].iter().collect() };
    let mut out = String::new();
    let mut start = 0;
    let mut i = 0;
    while i < json.len() {
        let mut ch = json[i];
        if ch == ' ' && at(i + 1) == Some('\\') && at(i + 2) == Some('n') {
            // A space before a line break must not be folded away.
            out.push_str(&slice(start, i));
            out.push_str("\\ ");
            i += 1;
            start = i;
            ch = '\\';
        }
        if ch == '\\' {
            match at(i + 1) {
                Some('u') => {
                    out.push_str(&slice(start, i));
                    let code = slice(i + 2, i + 6);
                    match code.as_str() {
                        "0000" => out.push_str("\\0"),
                        "0007" => out.push_str("\\a"),
                        "000b" => out.push_str("\\v"),
                        "001b" => out.push_str("\\e"),
                        "0085" => out.push_str("\\N"),
                        "00a0" => out.push_str("\\_"),
                        "2028" => out.push_str("\\L"),
                        "2029" => out.push_str("\\P"),
                        c if c.starts_with("00") => {
                            out.push_str("\\x");
                            out.push_str(&c[2..]);
                        }
                        _ => out.push_str(&slice(i, i + 6)),
                    }
                    i += 5;
                    start = i + 1;
                }
                Some('n') => {
                    if ctx.key || at(i + 2) == Some('"') || json_len < MIN_MULTI_LINE {
                        i += 1;
                    } else {
                        // Folding eats the first break, so a break is written as two.
                        out.push_str(&slice(start, i));
                        out.push_str("\n\n");
                        while at(i + 2) == Some('\\')
                            && at(i + 3) == Some('n')
                            && at(i + 4) != Some('"')
                        {
                            out.push('\n');
                            i += 2;
                        }
                        out.push_str(&indent);
                        // A space at the start of a line is escaped so it is not folded away.
                        // The npm package escapes it a second time when the space is also the
                        // end of its line (a line of one space), which reads back as a
                        // backslash; here the escape in front of the line break is enough.
                        let lone_space_line = at(i + 3) == Some('\\') && at(i + 4) == Some('n');
                        if at(i + 2) == Some(' ') && !lone_space_line {
                            out.push('\\');
                        }
                        i += 1;
                        start = i + 1;
                    }
                }
                _ => i += 1,
            }
        }
        i += 1;
    }
    if start > 0 {
        out.push_str(&slice(start, json.len()));
        out
    } else {
        json.iter().collect()
    }
}

/// A `|` block: the text on lines of its own, indented under the key. A text that ends in
/// spaces after a break cannot be a block and is quoted instead. A text of nothing but white
/// space is written as the npm package writes it only when that reads back as the same text:
/// for some of them the npm block loses the spaces, or cannot be read at all, and they are quoted.
fn block(value: &str, ctx: Ctx) -> String {
    let text = block_text(value, ctx);
    let blank = value.contains(' ') && value.chars().all(|c| matches!(c, ' ' | '\t' | '\n'));
    if blank && text.starts_with('|') {
        let pad = " ".repeat(ctx.indent.len().saturating_sub(INDENT_STEP.len()));
        let reads_back = matches!(
            parse_yaml(&format!("{pad}k: {text}\n")),
            Ok(Json::Object(map)) if map.get("k") == Some(&Json::String(value.to_string()))
        );
        if !reads_back {
            return quoted(value, ctx);
        }
    }
    text
}

fn block_text(value: &str, ctx: Ctx) -> String {
    let trimmed = value.trim_end_matches(['\t', ' ']);
    if trimmed.len() < value.len() && trimmed.ends_with('\n') {
        return quoted(value, ctx);
    }
    let indent = indent_of(value, ctx);
    if value.is_empty() {
        return "|\n".into();
    }
    // Chomping, from the white space at the end of the text.
    let end_start = value.trim_end_matches(['\n', '\t', ' ']).len();
    let (body, tail) = value.split_at(end_start);
    let chomp = match tail.find('\n') {
        None => "-",
        Some(nl) if body.is_empty() || nl != tail.len() - 1 => "+",
        Some(_) => "",
    };
    let mut end = tail.to_string();
    if !end.is_empty() {
        if end.ends_with('\n') {
            end.pop();
        }
        end = indent_line_runs(&end, &indent);
    }
    let mut value = body.to_string();
    // An indentation indicator when the text starts with a space (before its first letter).
    let mut start_with_space = false;
    let mut start_len = 0;
    for (i, ch) in value.char_indices() {
        match ch {
            ' ' => start_with_space = true,
            '\n' => start_len = i + 1,
            _ => break,
        }
    }
    let mut start = value[..start_len].to_string();
    if !start.is_empty() {
        value = value[start_len..].to_string();
        start = after_runs(&start, &indent);
    }
    let indicator = if indent.is_empty() { "1" } else { "2" };
    let header = format!("{}{chomp}", if start_with_space { indicator } else { "" });
    let value = after_runs(&value, &indent);
    format!("|{header}\n{indent}{start}{value}{end}")
}

/// `text.replace(/\n+/g, "$&" + indent)`: the indent follows every run of line breaks.
fn after_runs(text: &str, indent: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut run = false;
    for ch in text.chars() {
        if ch == '\n' {
            run = true;
        } else if run {
            out.push_str(indent);
            run = false;
        }
        out.push(ch);
    }
    if run {
        out.push_str(indent);
    }
    out
}

/// `end.replace(/(^|(?<!\n))\n+(?!\n|$)/g, "$&" + indent)`: like `after_runs`, but a run at the
/// very end of the text is left alone.
fn indent_line_runs(text: &str, indent: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut run = false;
    for ch in text.chars() {
        if ch == '\n' {
            run = true;
        } else if run {
            out.push_str(indent);
            run = false;
        }
        out.push(ch);
    }
    out
}
