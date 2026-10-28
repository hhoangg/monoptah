//! Insertion-ordered maps and an order-preserving JSON tree.
//!
//! serde_json's own maps are not a stable choice here: the app build enables `preserve_order`
//! (an IndexMap), while `cargo test -p monocode-core` builds without it (a BTreeMap, sorted keys).
//! The workflow file keeps keys in the order the author wrote them (inputs, output fields), that
//! order is visible in prompts and panels, and it must be the same in both builds, so `Json`
//! carries its own order.
use serde::de::{Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};
use serde::ser::{Serialize, SerializeMap, SerializeSeq, Serializer};
use serde_json::Value;
use std::{collections::HashMap, fmt};

/// A JSON number the way JS prints it: a whole value without a fraction.
pub fn write_number<S: Serializer>(value: f64, s: S) -> Result<S::Ok, S::Error> {
    if value.fract() == 0.0 && value.abs() < 9e15 {
        s.serialize_i64(value as i64)
    } else {
        s.serialize_f64(value)
    }
}

/// `serialize_with` for an `f64` field.
pub fn serialize_number<S: Serializer>(value: &f64, s: S) -> Result<S::Ok, S::Error> {
    write_number(*value, s)
}

pub fn serialize_opt_number<S: Serializer>(value: &Option<f64>, s: S) -> Result<S::Ok, S::Error> {
    match value {
        Some(v) => write_number(*v, s),
        None => s.serialize_none(),
    }
}

/// A map that keeps insertion order. Equality ignores order, like comparing JS objects.
#[derive(Clone, Debug)]
pub struct OrderedMap<T>(Vec<(String, T)>);

impl<T> Default for OrderedMap<T> {
    fn default() -> Self {
        Self(Vec::new())
    }
}

impl<T> OrderedMap<T> {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn get(&self, key: &str) -> Option<&T> {
        self.0.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }
    pub fn get_mut(&mut self, key: &str) -> Option<&mut T> {
        self.0.iter_mut().find(|(k, _)| k == key).map(|(_, v)| v)
    }
    pub fn contains_key(&self, key: &str) -> bool {
        self.get(key).is_some()
    }
    /// Replaces the value of an existing key in place, else appends.
    pub fn insert(&mut self, key: impl Into<String>, value: T) {
        let key = key.into();
        match self.0.iter_mut().find(|(k, _)| *k == key) {
            Some(slot) => slot.1 = value,
            None => self.0.push((key, value)),
        }
    }
    /// Appends without looking for an existing key, so building a map from keys that are already
    /// unique (a parsed `Json` object, a serde_json map) is linear. The caller guarantees that
    /// `key` is not present yet.
    pub(crate) fn push_unique(&mut self, key: impl Into<String>, value: T) {
        self.0.push((key.into(), value));
    }
    pub fn remove(&mut self, key: &str) -> Option<T> {
        let at = self.0.iter().position(|(k, _)| k == key)?;
        Some(self.0.remove(at).1)
    }
    pub fn iter(&self) -> impl Iterator<Item = (&String, &T)> {
        self.0.iter().map(|(k, v)| (k, v))
    }
    pub fn keys(&self) -> impl Iterator<Item = &String> {
        self.0.iter().map(|(k, _)| k)
    }
    pub fn values(&self) -> impl Iterator<Item = &T> {
        self.0.iter().map(|(_, v)| v)
    }
    pub fn len(&self) -> usize {
        self.0.len()
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl<T: PartialEq> PartialEq for OrderedMap<T> {
    fn eq(&self, other: &Self) -> bool {
        self.len() == other.len() && self.iter().all(|(k, v)| other.get(k) == Some(v))
    }
}

impl<K: Into<String>, T> FromIterator<(K, T)> for OrderedMap<T> {
    fn from_iter<I: IntoIterator<Item = (K, T)>>(iter: I) -> Self {
        let mut map = Self::new();
        for (k, v) in iter {
            map.insert(k, v);
        }
        map
    }
}

impl<T: Serialize> Serialize for OrderedMap<T> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut map = s.serialize_map(Some(self.len()))?;
        for (k, v) in self.iter() {
            map.serialize_entry(k, v)?;
        }
        map.end()
    }
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for OrderedMap<T> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V<T>(std::marker::PhantomData<T>);
        impl<'de, T: Deserialize<'de>> Visitor<'de> for V<T> {
            type Value = OrderedMap<T>;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a map")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut m: A) -> Result<Self::Value, A::Error> {
                let mut out = OrderedMap::new();
                while let Some((k, v)) = m.next_entry::<String, T>()? {
                    out.insert(k, v);
                }
                Ok(out)
            }
        }
        d.deserialize_map(V(std::marker::PhantomData))
    }
}

/// A JSON tree that keeps object key order. Numbers are f64, like JS.
#[derive(Clone, Debug, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Number(f64),
    String(String),
    Array(Vec<Json>),
    Object(OrderedMap<Json>),
}

impl Json {
    pub fn parse(text: &str) -> Option<Json> {
        serde_json::from_str(text).ok()
    }
    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Object(map) => map.get(key),
            _ => None,
        }
    }
    pub fn as_object(&self) -> Option<&OrderedMap<Json>> {
        match self {
            Json::Object(map) => Some(map),
            _ => None,
        }
    }
    pub fn as_array(&self) -> Option<&Vec<Json>> {
        match self {
            Json::Array(items) => Some(items),
            _ => None,
        }
    }
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Json::String(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Json::Number(n) if n.is_finite() => Some(*n),
            _ => None,
        }
    }
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Json::Bool(b) => Some(*b),
            _ => None,
        }
    }
    /// The same tree as a serde_json value. Key order follows the serde_json build: insertion
    /// order with `preserve_order` (the app), sorted without it (`-p monocode-core` tests).
    pub fn to_value(&self) -> Value {
        match self {
            Json::Null => Value::Null,
            Json::Bool(b) => Value::Bool(*b),
            Json::Number(n) => number_value(*n),
            Json::String(s) => Value::String(s.clone()),
            Json::Array(items) => Value::Array(items.iter().map(Json::to_value).collect()),
            Json::Object(map) => {
                Value::Object(map.iter().map(|(k, v)| (k.clone(), v.to_value())).collect())
            }
        }
    }
}

/// `String(n)` in JS: shortest round-trip digits, an exponent only below 1e-6 and from 1e21.
pub fn js_number_text(n: f64) -> String {
    if n.is_nan() {
        return "NaN".into();
    }
    if n == 0.0 {
        return "0".into();
    }
    if n.is_infinite() {
        return if n < 0.0 { "-Infinity" } else { "Infinity" }.into();
    }
    // `{:e}` prints the shortest round-trip digits as `d.ddde<exp>`. When two candidates of that
    // length are equally close JS takes the even one where Rust takes the upper, so the digit
    // count is reused with an exact, correctly rounded print.
    let shortest = format!("{:e}", n.abs());
    let places = shortest
        .split_once('e')
        .map_or(0, |(m, _)| m.len().saturating_sub(2));
    let exact = format!("{:.places$e}", n.abs());
    let sci = if exact.parse::<f64>().ok() == Some(n.abs()) {
        exact
    } else {
        shortest
    };
    let (mantissa, exp) = sci.split_once('e').unwrap_or((&sci, "0"));
    let digits: String = mantissa.chars().filter(|c| *c != '.').collect();
    let k = digits.len() as i32;
    // The value is 0.<digits> * 10^point.
    let point = exp.parse::<i32>().unwrap_or(0) + 1;
    let body = if k <= point && point <= 21 {
        format!("{digits}{}", "0".repeat((point - k) as usize))
    } else if 0 < point && point <= 21 {
        format!(
            "{}.{}",
            &digits[..point as usize],
            &digits[point as usize..]
        )
    } else if -6 < point && point <= 0 {
        format!("0.{}{digits}", "0".repeat((-point) as usize))
    } else {
        let e = point - 1;
        let sign = if e < 0 { '-' } else { '+' };
        let head = &digits[..1];
        if k == 1 {
            format!("{head}e{sign}{}", e.abs())
        } else {
            format!("{head}.{}e{sign}{}", &digits[1..], e.abs())
        }
    };
    if n < 0.0 {
        format!("-{body}")
    } else {
        body
    }
}

impl Json {
    /// `JSON.stringify(self)` (compact) or `JSON.stringify(self, null, 2)` (`pretty`): keys in
    /// the tree's order, numbers as JS prints them, a non-finite number as `null`.
    pub fn stringify(&self, pretty: bool) -> String {
        let mut out = String::new();
        self.write_js(&mut out, pretty, 0);
        out
    }

    fn write_js(&self, out: &mut String, pretty: bool, depth: usize) {
        let line = |out: &mut String, depth: usize| {
            if pretty {
                out.push('\n');
                out.extend(std::iter::repeat_n("  ", depth));
            }
        };
        match self {
            Json::Null => out.push_str("null"),
            Json::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            Json::Number(n) if n.is_finite() => out.push_str(&js_number_text(*n)),
            Json::Number(_) => out.push_str("null"),
            Json::String(s) => write_js_string(out, s),
            Json::Array(items) if items.is_empty() => out.push_str("[]"),
            Json::Array(items) => {
                out.push('[');
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    line(out, depth + 1);
                    item.write_js(out, pretty, depth + 1);
                }
                line(out, depth);
                out.push(']');
            }
            Json::Object(map) if map.is_empty() => out.push_str("{}"),
            Json::Object(map) => {
                out.push('{');
                for (i, (key, value)) in map.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    line(out, depth + 1);
                    write_js_string(out, key);
                    out.push_str(if pretty { ": " } else { ":" });
                    value.write_js(out, pretty, depth + 1);
                }
                line(out, depth);
                out.push('}');
            }
        }
    }
}

/// A JSON string literal as `JSON.stringify` writes it (serde_json escapes the same set, and
/// both leave U+2028 and U+2029 alone).
fn write_js_string(out: &mut String, text: &str) {
    out.push_str(&serde_json::to_string(text).unwrap_or_else(|_| "\"\"".into()));
}

fn number_value(n: f64) -> Value {
    if n.fract() == 0.0 && n.abs() < 9e15 {
        Value::from(n as i64)
    } else {
        serde_json::Number::from_f64(n).map_or(Value::Null, Value::Number)
    }
}

impl From<&Value> for Json {
    fn from(value: &Value) -> Self {
        match value {
            Value::Null => Json::Null,
            Value::Bool(b) => Json::Bool(*b),
            Value::Number(n) => Json::Number(n.as_f64().unwrap_or(0.0)),
            Value::String(s) => Json::String(s.clone()),
            Value::Array(items) => Json::Array(items.iter().map(Json::from).collect()),
            // serde_json keys are unique already.
            Value::Object(map) => Json::Object(OrderedMap(
                map.iter()
                    .map(|(k, v)| (k.clone(), Json::from(v)))
                    .collect(),
            )),
        }
    }
}

impl Serialize for Json {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Json::Null => s.serialize_unit(),
            Json::Bool(b) => s.serialize_bool(*b),
            Json::Number(n) => write_number(*n, s),
            Json::String(v) => s.serialize_str(v),
            Json::Array(items) => {
                let mut seq = s.serialize_seq(Some(items.len()))?;
                for item in items {
                    seq.serialize_element(item)?;
                }
                seq.end()
            }
            Json::Object(map) => map.serialize(s),
        }
    }
}

impl<'de> Deserialize<'de> for Json {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Json;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("any JSON value")
            }
            fn visit_unit<E>(self) -> Result<Json, E> {
                Ok(Json::Null)
            }
            fn visit_none<E>(self) -> Result<Json, E> {
                Ok(Json::Null)
            }
            fn visit_bool<E>(self, v: bool) -> Result<Json, E> {
                Ok(Json::Bool(v))
            }
            fn visit_i64<E>(self, v: i64) -> Result<Json, E> {
                Ok(Json::Number(v as f64))
            }
            fn visit_u64<E>(self, v: u64) -> Result<Json, E> {
                Ok(Json::Number(v as f64))
            }
            fn visit_f64<E>(self, v: f64) -> Result<Json, E> {
                Ok(Json::Number(v))
            }
            fn visit_str<E>(self, v: &str) -> Result<Json, E> {
                Ok(Json::String(v.to_string()))
            }
            fn visit_string<E>(self, v: String) -> Result<Json, E> {
                Ok(Json::String(v))
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Json, A::Error> {
                let mut items = Vec::new();
                while let Some(item) = seq.next_element()? {
                    items.push(item);
                }
                Ok(Json::Array(items))
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Json, A::Error> {
                // A repeated key keeps the last value at the first position, like JSON.parse.
                let mut out: Vec<(String, Json)> = Vec::new();
                let mut at: HashMap<String, usize> = HashMap::new();
                while let Some((k, v)) = map.next_entry::<String, Json>()? {
                    match at.get(&k) {
                        Some(&i) => out[i].1 = v,
                        None => {
                            at.insert(k.clone(), out.len());
                            out.push((k, v));
                        }
                    }
                }
                Ok(Json::Object(OrderedMap(out)))
            }
        }
        d.deserialize_any(V)
    }
}
