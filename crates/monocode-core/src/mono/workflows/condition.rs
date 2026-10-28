//! Conditions for If nodes and loops: one comparison over `{{path}}`, string, number and boolean
//! operands. `parse_condition` is the grammar; `eval_condition` runs it against a run scope.
use super::ordered::Json;
use super::template::{js_space, js_trim, lookup, var_at};
use serde_json::Value;
use std::borrow::Cow;

/// One side of a comparison.
#[derive(Clone, Debug, PartialEq)]
pub enum Operand {
    /// `{{a.b}}`, looked up in the scope when evaluated.
    Path(String),
    /// A double-quoted string, a number, true or false.
    Literal(Value),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Parsed {
    /// `==`, `!=`, `contains`, `empty` or `not empty`.
    pub operator: String,
    pub left: Operand,
    pub right: Option<Operand>,
}

fn skip_space(c: &[char], mut pos: usize) -> usize {
    while pos < c.len() && js_space(c[pos]) {
        pos += 1;
    }
    pos
}

/// `(?=\s|$)` at `pos`.
fn boundary(c: &[char], pos: usize) -> bool {
    c.get(pos).is_none_or(|ch| js_space(*ch))
}

fn word_at(c: &[char], pos: usize, word: &str) -> Option<usize> {
    let end = pos + word.chars().count();
    (c.get(pos..end)? == word.chars().collect::<Vec<_>>().as_slice()).then_some(end)
}

/// A double-quoted string starting at `pos`, honouring backslash escapes.
fn read_string(c: &[char], pos: usize) -> Result<(Operand, usize), &'static str> {
    let mut i = pos + 1;
    while i < c.len() && c[i] != '"' {
        i += if c[i] == '\\' { 2 } else { 1 };
    }
    if i >= c.len() {
        return Err("unterminated string");
    }
    let literal: String = c[pos..=i].iter().collect();
    match serde_json::from_str::<String>(&literal) {
        Ok(value) => Ok((Operand::Literal(Value::String(value)), i + 1)),
        Err(_) => Err("invalid string escape"),
    }
}

/// `-?\d+(?:\.\d+)?(?=\s|$)`
fn read_number(c: &[char], pos: usize) -> Option<(f64, usize)> {
    let digits = |from: usize| {
        let mut end = from;
        while c.get(end).is_some_and(|ch| ch.is_ascii_digit()) {
            end += 1;
        }
        end
    };
    let int_start = if c.get(pos) == Some(&'-') {
        pos + 1
    } else {
        pos
    };
    let int_end = digits(int_start);
    if int_end == int_start {
        return None;
    }
    let mut end = int_end;
    if c.get(end) == Some(&'.') {
        let frac_end = digits(end + 1);
        if frac_end > end + 1 && boundary(c, frac_end) {
            end = frac_end;
        }
    }
    if !boundary(c, end) {
        return None;
    }
    let text: String = c[pos..end].iter().collect();
    Some((text.parse().ok()?, end))
}

fn number_operand(value: f64) -> Operand {
    let number = if value.fract() == 0.0 && value.abs() < 9e15 {
        Value::from(value as i64)
    } else {
        serde_json::Number::from_f64(value).map_or(Value::Null, Value::Number)
    };
    Operand::Literal(number)
}

fn read_operand(c: &[char], pos: usize) -> Result<(Operand, usize), &'static str> {
    if c.get(pos) == Some(&'"') {
        return read_string(c, pos);
    }
    if let Some((path, end)) = var_at(c, pos) {
        return Ok((Operand::Path(path), end));
    }
    if let Some((value, end)) = read_number(c, pos) {
        return Ok((number_operand(value), end));
    }
    for (word, value) in [("true", true), ("false", false)] {
        if let Some(end) = word_at(c, pos, word).filter(|end| boundary(c, *end)) {
            return Ok((Operand::Literal(Value::Bool(value)), end));
        }
    }
    Err("expected {{path}}, a double-quoted string, a number, true or false")
}

/// `(==|!=|contains|not\s+empty|empty)(?=\s|$)`: the operator (spaces collapsed) and its end.
fn read_operator(c: &[char], pos: usize) -> Option<(&'static str, usize)> {
    for op in ["==", "!=", "contains"] {
        if let Some(end) = word_at(c, pos, op).filter(|end| boundary(c, *end)) {
            return Some((op, end));
        }
    }
    if let Some(after_not) = word_at(c, pos, "not") {
        let from = skip_space(c, after_not);
        if from > after_not {
            if let Some(end) = word_at(c, from, "empty").filter(|end| boundary(c, *end)) {
                return Some(("not empty", end));
            }
        }
    }
    word_at(c, pos, "empty")
        .filter(|end| boundary(c, *end))
        .map(|end| ("empty", end))
}

/// Reads `condition`: the grammar and the operands, but not the comparison itself. The error is
/// the full message, `Cannot parse condition "...": <reason>`.
pub fn parse_condition(condition: &str) -> Result<Parsed, String> {
    let fail = |reason: &str| format!("Cannot parse condition \"{condition}\": {reason}");
    let text: Vec<char> = js_trim(condition).chars().collect();
    if text.is_empty() {
        return Err(fail("the condition is empty"));
    }
    let (left, end) = read_operand(&text, 0).map_err(fail)?;
    let pos = skip_space(&text, end);
    let Some((operator, end)) = read_operator(&text, pos) else {
        return Err(fail("expected ==, !=, contains, empty or not empty"));
    };
    let pos = skip_space(&text, end);
    if operator == "empty" || operator == "not empty" {
        if pos < text.len() {
            return Err(fail(
                "unexpected text after the operator (and/or are not supported)",
            ));
        }
        return Ok(Parsed {
            operator: operator.into(),
            left,
            right: None,
        });
    }
    let (right, end) = read_operand(&text, pos).map_err(fail)?;
    if skip_space(&text, end) < text.len() {
        return Err(fail(
            "unexpected text after the second operand (and/or are not supported)",
        ));
    }
    Ok(Parsed {
        operator: operator.into(),
        left,
        right: Some(right),
    })
}

/// What is wrong with the way `condition` is written, if anything: the grammar only, not whether
/// it can be true. Used to refuse a workflow whose condition would fail when the run reaches it.
pub fn condition_syntax_problem(condition: &str) -> Option<String> {
    parse_condition(condition).err()
}

/// An operand's value in `scope`; None is JS `undefined` (a path that is not there).
fn resolve<'a>(operand: &Operand, scope: &'a Json) -> Option<Cow<'a, Json>> {
    match operand {
        Operand::Path(path) => lookup(scope, path),
        Operand::Literal(value) => Some(Cow::Owned(Json::from(value))),
    }
}

/// JS `a === b`, except that two objects (or lists) are equal when their JSON text is.
fn same_value(a: Option<&Json>, b: Option<&Json>) -> bool {
    match (a, b) {
        (None, None) | (Some(Json::Null), Some(Json::Null)) => true,
        (
            Some(a @ (Json::Array(_) | Json::Object(_))),
            Some(b @ (Json::Array(_) | Json::Object(_))),
        ) => a.stringify(false) == b.stringify(false),
        (Some(Json::Bool(a)), Some(Json::Bool(b))) => a == b,
        (Some(Json::Number(a)), Some(Json::Number(b))) => a == b,
        (Some(Json::String(a)), Some(Json::String(b))) => a == b,
        _ => false,
    }
}

fn is_empty(value: Option<&Json>) -> bool {
    match value {
        None | Some(Json::Null) => true,
        Some(Json::String(s)) => s.is_empty(),
        Some(Json::Array(items)) => items.is_empty(),
        Some(Json::Object(map)) => map.is_empty(),
        Some(_) => false,
    }
}

/// Evaluates a single-comparison condition against `scope`. Err is the message to show: the
/// parse error, or `contains` applied to something it cannot search.
pub fn eval_condition(condition: &str, scope: &Json) -> Result<bool, String> {
    let parsed = parse_condition(condition)?;
    let a = resolve(&parsed.left, scope);
    let b = parsed
        .right
        .as_ref()
        .and_then(|right| resolve(right, scope));
    let (a, b) = (a.as_deref(), b.as_deref());
    let fail = |reason: &str| format!("Cannot parse condition \"{condition}\": {reason}");
    match parsed.operator.as_str() {
        "empty" => Ok(is_empty(a)),
        "not empty" => Ok(!is_empty(a)),
        "==" => Ok(same_value(a, b)),
        "!=" => Ok(!same_value(a, b)),
        _ => match a {
            None | Some(Json::Null) => Ok(false),
            Some(Json::String(text)) => match b {
                Some(Json::String(needle)) => Ok(text.contains(needle.as_str())),
                _ => Err(fail("contains on a string needs a string on the right")),
            },
            Some(Json::Array(items)) => Ok(items.iter().any(|item| same_value(Some(item), b))),
            Some(_) => Err(fail("contains needs a string or a list on the left")),
        },
    }
}
