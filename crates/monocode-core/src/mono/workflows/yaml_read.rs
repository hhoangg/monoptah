//! YAML reader for workflow files: yaml-rust2's event parser, resolved by hand into a `Json`
//! tree with the rules of the npm `yaml` package under `schema: "core"` (what the React app uses).
//!
//! Going through events instead of yaml-rust2's loader is what makes the limits possible: aliases
//! are counted the way `yaml` counts them (`maxAliasCount: 20`), nesting stops at 64 levels, and
//! a node budget stops an alias from expanding into a huge tree.
//!
//! Scalars follow the core schema: `true`/`True`/`TRUE`, `null`/`~`/empty, ints (`0o`, `0x`,
//! signed decimal) and floats (`.inf`, `.nan`, exponents) are typed; everything else (`09:00`,
//! `yes`, `on`) stays text. A quoted or block scalar is always text, unless it carries a core tag.
use super::ordered::{js_number_text, Json, OrderedMap};
use regex::Regex;
use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;
use yaml_rust2::parser::{Event, Parser, Tag};
use yaml_rust2::scanner::TScalarStyle;

/// `yaml`'s `maxAliasCount`: an anchor may be used `count * weight` up to this.
pub const MAX_ALIAS_COUNT: usize = 20;
/// Values nested deeper than this are refused (the root is level 0).
pub const NESTING_MAX: usize = 64;
/// What one file may build, in bytes of memory (roughly): every node, string, key, alias copy
/// and anchor snapshot is charged. A guard on top of the alias count, for anchors that weigh
/// nothing (an empty list) but hold many items, and for anchors nested in anchors, each of which
/// keeps a snapshot of what is inside. A file without aliases costs at most about 20 MB at the
/// 512 KiB input limit, so anything the export writes stays far below it.
const BUDGET: usize = 128 * 1024 * 1024;
/// The cost of one Json node.
const NODE_COST: usize = 48;
const ALIAS_BOMB: &str = "Excessive alias count indicates a resource exhaustion attack";

#[derive(Debug, PartialEq)]
pub enum ReadError {
    /// A value sits deeper than `NESTING_MAX`.
    TooDeep,
    Message(String),
}

fn fail<T>(message: impl Into<String>) -> Result<T, ReadError> {
    Err(ReadError::Message(message.into()))
}

// --- scalars -------------------------------------------------------------------------------

struct CoreRegexes {
    oct: Regex,
    int: Regex,
    hex: Regex,
    nan_inf: Regex,
    float_exp: Regex,
    float: Regex,
}

fn core() -> &'static CoreRegexes {
    static CORE: OnceLock<CoreRegexes> = OnceLock::new();
    CORE.get_or_init(|| {
        let re = |p: &str| Regex::new(p).expect("core schema pattern");
        CoreRegexes {
            oct: re(r"^0o[0-7]+$"),
            int: re(r"^[-+]?[0-9]+$"),
            hex: re(r"^0x[0-9a-fA-F]+$"),
            nan_inf: re(r"^(?:[-+]?\.(?:inf|Inf|INF)|\.nan|\.NaN|\.NAN)$"),
            float_exp: re(r"^[-+]?(?:\.[0-9]+|[0-9]+(?:\.[0-9]*)?)[eE][-+]?[0-9]+$"),
            float: re(r"^[-+]?(?:\.[0-9]+|[0-9]+\.[0-9]*)$"),
        }
    })
}

fn is_null(text: &str) -> bool {
    matches!(text, "" | "~" | "null" | "Null" | "NULL")
}

fn bool_of(text: &str) -> Option<bool> {
    match text {
        "true" | "True" | "TRUE" => Some(true),
        "false" | "False" | "FALSE" => Some(false),
        _ => None,
    }
}

/// `parseInt(digits, radix)` for a power-of-two radix: exact, then correctly rounded.
fn radix_number(digits: &str, radix: u32) -> f64 {
    match u128::from_str_radix(digits, radix) {
        Ok(n) => n as f64,
        Err(_) => digits
            .chars()
            .filter_map(|c| c.to_digit(radix))
            .fold(0.0, |acc, d| acc * radix as f64 + d as f64),
    }
}

fn number_of(text: &str) -> Option<f64> {
    let re = core();
    if re.oct.is_match(text) {
        Some(radix_number(&text[2..], 8))
    } else if re.int.is_match(text) || re.float_exp.is_match(text) || re.float.is_match(text) {
        text.parse().ok()
    } else if re.hex.is_match(text) {
        Some(radix_number(&text[2..], 16))
    } else if re.nan_inf.is_match(text) {
        Some(if text.ends_with(['n', 'N']) {
            f64::NAN
        } else if text.starts_with('-') {
            f64::NEG_INFINITY
        } else {
            f64::INFINITY
        })
    } else {
        None
    }
}

fn is_int_text(text: &str) -> bool {
    let re = core();
    re.oct.is_match(text) || re.int.is_match(text) || re.hex.is_match(text)
}

fn is_float_text(text: &str) -> bool {
    let re = core();
    re.nan_inf.is_match(text) || re.float_exp.is_match(text) || re.float.is_match(text)
}

/// Whether a plain string would be read back as something other than text: what the emitter
/// quotes (`yaml`'s `actualString` check against the core schema).
pub fn reads_as_non_string(text: &str) -> bool {
    is_null(text) || bool_of(text).is_some() || number_of(text).is_some()
}

/// A plain scalar with no tag, as the core schema resolves it.
fn resolve_plain(text: &str) -> Json {
    if is_null(text) {
        Json::Null
    } else if let Some(b) = bool_of(text) {
        Json::Bool(b)
    } else if let Some(n) = number_of(text) {
        Json::Number(n)
    } else {
        Json::String(text.to_string())
    }
}

const YAML_TAG: &str = "tag:yaml.org,2002:";

/// A scalar with an explicit tag: a core tag resolves the text when it fits (whatever the
/// quoting), every other tag, known or not, leaves text.
fn resolve_tagged(text: String, tag: &Tag) -> Json {
    let name = format!("{}{}", tag.handle, tag.suffix);
    let kind = name.strip_prefix(YAML_TAG).unwrap_or("");
    match kind {
        "null" if is_null(&text) => Json::Null,
        "bool" => bool_of(&text).map_or(Json::String(text), Json::Bool),
        "int" if is_int_text(&text) => number_of(&text).map_or(Json::String(text), Json::Number),
        "float" if is_float_text(&text) => {
            number_of(&text).map_or(Json::String(text), Json::Number)
        }
        _ => Json::String(text),
    }
}

fn resolve_scalar(text: String, style: TScalarStyle, tag: Option<&Tag>) -> Json {
    match tag {
        Some(tag) => resolve_tagged(text, tag),
        None if style == TScalarStyle::Plain => resolve_plain(&text),
        None => Json::String(text),
    }
}

// --- keys ----------------------------------------------------------------------------------

/// What makes two keys the same (`yaml` compares resolved scalar values).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum KeyId {
    Null,
    Bool(bool),
    Number(u64),
    Text(String),
    /// A list or map used as a key: never equal to another.
    Complex(usize),
}

/// The property name the key becomes (`String(key)` in JS; null is the empty name).
fn key_of(node: &Json, serial: usize) -> (String, KeyId) {
    match node {
        Json::Null => (String::new(), KeyId::Null),
        Json::Bool(b) => (b.to_string(), KeyId::Bool(*b)),
        // 0 and -0 are one key; NaN never equals itself, so repeated `.nan` keys are all kept.
        Json::Number(n) if n.is_nan() => (js_number_text(*n), KeyId::Complex(serial)),
        Json::Number(n) => (js_number_text(*n), KeyId::Number((*n + 0.0).to_bits())),
        Json::String(s) => (s.clone(), KeyId::Text(s.clone())),
        other => (other.stringify(false), KeyId::Complex(serial)),
    }
}

// --- aliases -------------------------------------------------------------------------------

/// What an anchored node holds, enough to weigh an alias to it the way `yaml` does: a scalar
/// weighs 1, an empty collection 0, a collection the heaviest thing inside, an alias the
/// current `count * weight` of its anchor.
#[derive(Clone, Default)]
struct Weight {
    scalar: bool,
    aliases: Vec<usize>,
}

impl Weight {
    fn absorb(&mut self, other: Weight) {
        self.scalar |= other.scalar;
        self.aliases.extend(other.aliases);
    }
}

/// What a finished node costs and how tall it is.
#[derive(Clone, Copy)]
struct Measure {
    /// Charged to the budget.
    cost: usize,
    /// Levels below it: 0 for a scalar or an empty collection.
    height: usize,
}

struct Anchor {
    value: Json,
    measure: Measure,
    /// Starts at 1 for the definition itself, like `yaml`.
    count: usize,
    weight_cache: usize,
    inside: Weight,
}

fn weight_of(inside: &Weight, anchors: &HashMap<usize, Anchor>) -> usize {
    let nested = inside
        .aliases
        .iter()
        .filter_map(|id| anchors.get(id))
        .map(|a| a.count * a.weight_cache)
        .max()
        .unwrap_or(0);
    nested.max(usize::from(inside.scalar))
}

// --- the reader ----------------------------------------------------------------------------

enum Frame {
    Seq {
        items: Vec<Json>,
    },
    Map {
        entries: Vec<(String, Json)>,
        at: HashMap<String, usize>,
        seen: HashSet<KeyId>,
        /// The key waiting for its value, with where it was read.
        key: Option<(String, (usize, usize))>,
    },
}

struct Open {
    frame: Frame,
    anchor: usize,
    measure: Measure,
    inside: Weight,
}

struct Reader {
    stack: Vec<Open>,
    anchors: HashMap<usize, Anchor>,
    root: Option<Json>,
    spent: usize,
    serial: usize,
}

impl Reader {
    fn spend(&mut self, n: usize) -> Result<(), ReadError> {
        self.spent = self.spent.saturating_add(n);
        if self.spent > BUDGET {
            return fail(ALIAS_BOMB);
        }
        Ok(())
    }

    fn enter(&self) -> Result<(), ReadError> {
        if self.stack.len() > NESTING_MAX {
            return Err(ReadError::TooDeep);
        }
        Ok(())
    }

    /// A finished node: into its parent, or the root.
    fn finish(
        &mut self,
        node: Json,
        anchor: usize,
        measure: Measure,
        inside: Weight,
        at: (usize, usize),
    ) -> Result<(), ReadError> {
        if anchor != 0 {
            // The snapshot is a copy of everything inside, so it is charged before it is made.
            self.spend(measure.cost)?;
            self.anchors.insert(
                anchor,
                Anchor {
                    value: node.clone(),
                    measure,
                    count: 1,
                    weight_cache: 0,
                    inside: inside.clone(),
                },
            );
        }
        let serial = self.serial;
        self.serial += 1;
        let Some(parent) = self.stack.last_mut() else {
            self.root = Some(node);
            return Ok(());
        };
        parent.measure.cost += measure.cost;
        parent.measure.height = parent.measure.height.max(measure.height + 1);
        parent.inside.absorb(inside);
        match &mut parent.frame {
            Frame::Seq { items } => items.push(node),
            Frame::Map {
                entries,
                at: index,
                seen,
                key,
            } => match key.take() {
                None => {
                    let (name, id) = key_of(&node, serial);
                    if !seen.insert(id) {
                        return fail(format!(
                            "Map keys must be unique at line {}, column {}:",
                            at.0, at.1
                        ));
                    }
                    *key = Some((name, at));
                }
                Some((name, _)) => {
                    // The name lives in the entry, the index and the duplicate check.
                    let cost = NODE_COST + 3 * name.len();
                    self.spent = self.spent.saturating_add(cost);
                    parent.measure.cost += cost;
                    if self.spent > BUDGET {
                        return fail(ALIAS_BOMB);
                    }
                    match index.get(&name) {
                        // Two keys that read as the same text (`1` and `"1"`): the last value wins
                        // at the first position, like assigning to a JS object.
                        Some(&i) => entries[i].1 = node,
                        None => {
                            index.insert(name.clone(), entries.len());
                            entries.push((name, node));
                        }
                    }
                }
            },
        }
        Ok(())
    }

    fn alias(&mut self, id: usize, at: (usize, usize)) -> Result<(), ReadError> {
        let Some(anchor) = self.anchors.get_mut(&id) else {
            return fail("Unresolved alias (the anchor must be set before the alias)");
        };
        anchor.count += 1;
        if anchor.weight_cache == 0 {
            let inside = anchor.inside.clone();
            let weight = weight_of(&inside, &self.anchors);
            if let Some(a) = self.anchors.get_mut(&id) {
                a.weight_cache = weight;
            }
        }
        let anchor = &self.anchors[&id];
        if anchor.count * anchor.weight_cache > MAX_ALIAS_COUNT {
            return fail(ALIAS_BOMB);
        }
        let measure = anchor.measure;
        // What the anchored tree adds to the depth of this spot counts, as when `yaml` builds it.
        if self.stack.len() + measure.height > NESTING_MAX {
            return Err(ReadError::TooDeep);
        }
        self.spend(measure.cost)?;
        let value = self.anchors[&id].value.clone();
        let inside = Weight {
            scalar: false,
            aliases: vec![id],
        };
        self.finish(value, 0, measure, inside, at)
    }

    fn open(&mut self, frame: Frame, anchor: usize) -> Result<(), ReadError> {
        self.enter()?;
        self.spend(1)?;
        self.stack.push(Open {
            frame,
            anchor,
            measure: Measure {
                cost: NODE_COST,
                height: 0,
            },
            inside: Weight::default(),
        });
        Ok(())
    }

    fn close(&mut self, at: (usize, usize)) -> Result<(), ReadError> {
        let Some(open) = self.stack.pop() else {
            return fail("unbalanced end of a collection");
        };
        let node = match open.frame {
            Frame::Seq { items } => Json::Array(items),
            Frame::Map { entries, .. } => {
                let mut map = OrderedMap::new();
                for (name, value) in entries {
                    map.push_unique(name, value);
                }
                Json::Object(map)
            }
        };
        self.finish(node, open.anchor, open.measure, open.inside, at)
    }
}

/// The parser's own limits and unknown anchors, in the terms of this file's errors.
fn parse_failure(message: &str) -> ReadError {
    if message.starts_with("recursion limit exceeded") {
        ReadError::TooDeep
    } else if message.contains("unknown anchor") {
        ReadError::Message("Unresolved alias (the anchor must be set before the alias)".into())
    } else {
        ReadError::Message(message.to_string())
    }
}

/// Reads one YAML document into a `Json` tree. An empty text or a comment is `null`; more than
/// one document is an error, like `yaml`'s `parse`.
pub fn parse_yaml(text: &str) -> Result<Json, ReadError> {
    // A byte order mark is not part of the first key.
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut parser = Parser::new_from_str(text);
    let mut reader = Reader {
        stack: Vec::new(),
        anchors: HashMap::new(),
        root: None,
        spent: 0,
        serial: 0,
    };
    let mut documents = 0;
    loop {
        let (event, mark) = match parser.next_token() {
            Ok(next) => next,
            Err(err) => return Err(parse_failure(&err.to_string())),
        };
        let at = (mark.line(), mark.col() + 1);
        match event {
            Event::Nothing | Event::StreamStart | Event::DocumentEnd => {}
            Event::StreamEnd => break,
            Event::DocumentStart => {
                documents += 1;
                if documents > 1 {
                    return fail(format!(
                        "Source contains multiple documents; please use YAML.parseAllDocuments() at line {}, column {}:",
                        at.0, at.1
                    ));
                }
            }
            Event::Scalar(text, style, anchor, tag) => {
                reader.enter()?;
                let cost = NODE_COST + text.len();
                reader.spend(cost)?;
                let node = resolve_scalar(text, style, tag.as_ref());
                let inside = Weight {
                    scalar: true,
                    aliases: Vec::new(),
                };
                let measure = Measure { cost, height: 0 };
                reader.finish(node, anchor, measure, inside, at)?;
            }
            Event::Alias(id) => {
                reader.enter()?;
                reader.alias(id, at)?;
            }
            Event::SequenceStart(anchor, _) => {
                reader.open(Frame::Seq { items: Vec::new() }, anchor)?;
            }
            Event::MappingStart(anchor, _) => {
                reader.open(
                    Frame::Map {
                        entries: Vec::new(),
                        at: HashMap::new(),
                        seen: HashSet::new(),
                        key: None,
                    },
                    anchor,
                )?;
            }
            Event::SequenceEnd | Event::MappingEnd => reader.close(at)?,
        }
    }
    Ok(reader.root.unwrap_or(Json::Null))
}
