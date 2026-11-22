//! What a step can do to the user's accounts, worked out from words on the page, and what must
//! never be typed by a recipe. Phase 3 (write recipes).
//!
//! Two jobs, both deliberately conservative (a false positive costs one approval card, a miss
//! can send a message):
//!
//! * [`label_class`]: the risk of acting on an element from its role and accessible name. The
//!   stored recipe is only a floor: the runner calls this again on the element it found on the
//!   live page, so a button that now says "Send" is a send even if the recipe recorded it as
//!   "Save".
//! * [`secret_field`] and [`secret_value`]: fields and values a recipe never types. Passwords,
//!   one-time codes and card data are not stored, defaulted or replayed.
//!
//! The approval rules per trust level are not here: they are `gate::approval_required`, the same
//! function the gate's own rules text follows.
use super::model::Risk;
use crate::mono::browser::gate::{contains_card_number, Category};
use regex::Regex;
use serde_json::json;
use std::collections::BTreeSet;

/// Risk and the approval category that goes with it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Class {
    pub risk: Risk,
    pub category: Category,
}
impl Class {
    pub const READ: Class = Class {
        risk: Risk::Read,
        category: Category::Other,
    };
    pub const WRITE: Class = Class {
        risk: Risk::Write,
        category: Category::Other,
    };
    pub fn send() -> Class {
        Class {
            risk: Risk::Send,
            category: Category::SendMessage,
        }
    }
    pub fn post() -> Class {
        Class {
            risk: Risk::Send,
            category: Category::PostPublish,
        }
    }
    pub fn money() -> Class {
        Class {
            risk: Risk::Money,
            category: Category::Payment,
        }
    }
    /// The stronger of two classes (the higher risk wins; on a tie `self` stays).
    pub fn max(self, other: Class) -> Class {
        if other.risk > self.risk {
            other
        } else {
            self
        }
    }
}

fn words(text: &str) -> BTreeSet<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_owned)
        .collect()
}

const MONEY: [&str; 31] = [
    "pay",
    "paying",
    "payment",
    "payments",
    "purchase",
    "buy",
    "checkout",
    "subscribe",
    "subscription",
    "donate",
    "donation",
    "transfer",
    "withdraw",
    "deposit",
    "upgrade",
    "renew",
    "tip",
    "book",
    "reserve",
    "bid",
    "sell",
    "trade",
    "rent",
    "money",
    "cash",
    "invest",
    "redeem",
    "wire",
    "fund",
    "reorder",
    "ordering",
];
const MONEY_PHRASES: [&str; 4] = [
    "place order",
    "complete order",
    "confirm order",
    "add to cart",
];
const POST: [&str; 9] = [
    "post", "comment", "reply", "publish", "tweet", "review", "share", "repost", "announce",
];
const SEND: [&str; 6] = ["send", "submit", "invite", "forward", "dm", "transmit"];
/// Removing things: no category of its own in the gate, so it is a write that asks like any
/// other risky action at Level 2.
const DESTRUCTIVE: [&str; 9] = [
    "delete",
    "remove",
    "erase",
    "destroy",
    "unsubscribe",
    "deactivate",
    "revoke",
    "terminate",
    "wipe",
];

/// The class a name suggests, or `None` when it names nothing risky. Money outranks sending,
/// sending outranks removing.
pub fn name_class(name: &str) -> Option<Class> {
    let w = words(name);
    let lower = name.to_lowercase();
    // "Order" is money unless it only sorts something ("Sort order", "Order by").
    let flat = format!(
        " {} ",
        lower
            .split(|c: char| !c.is_alphanumeric())
            .filter(|t| !t.is_empty())
            .collect::<Vec<_>>()
            .join(" ")
    )
    .replace(" sort order ", " ")
    .replace(" order by ", " ");
    let orders = flat.contains(" order ");
    static CURRENCY: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    let amount = regex_once(
        &CURRENCY,
        r"(?i)[$€£¥₫₹]\s*\d|\d\s*[$€£¥₫₹]|\d\s*(usd|eur|gbp|vnd|jpy|dollars?|euros?)\b",
    )
    .is_match(name);
    // Money beats sending ("Send money", "Send $50").
    if orders
        || amount
        || MONEY.iter().any(|k| w.contains(*k))
        || MONEY_PHRASES.iter().any(|p| lower.contains(p))
    {
        return Some(Class::money());
    }
    if SEND.iter().any(|k| w.contains(*k)) {
        return Some(Class::send());
    }
    if POST.iter().any(|k| w.contains(*k)) {
        return Some(Class::post());
    }
    if DESTRUCTIVE.iter().any(|k| w.contains(*k)) {
        return Some(Class::WRITE);
    }
    None
}

/// The class of acting on a live element: what its role and name say, never lower than a plain
/// write. Roles that only hold text (a textbox the user types into) are not keyword checked: a
/// box named "Send message" is where the text goes, the button sends it.
pub fn label_class(role: &str, name: &str) -> Class {
    if matches!(role, "textbox" | "searchbox" | "spinbutton") {
        return Class::WRITE;
    }
    name_class(name).unwrap_or(Class::WRITE).max(Class::WRITE)
}

fn regex_once(cell: &'static std::sync::OnceLock<Regex>, pattern: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(pattern).expect("risk pattern"))
}

/// Whether a field is a place for a credential: a password, a one-time or security code, card or
/// bank data, a secret or a token. A recipe never types into one, and refuses to be recorded
/// with one, whatever the value.
pub fn secret_field(name: &str) -> bool {
    static RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    regex_once(
        &RE,
        r"(?i)pass\s*(word|code|phrase)|passwd|\bpwd\b|one[\s-]*time|\botp\b|\btotp\b|verification\s+code|security\s+code|confirmation\s+code|login\s+code|auth(entication|enticator)?\s+code|\b2fa\b|\bmfa\b|two[\s-]*factor|\bcvv\b|\bcvc\b|card\s*(number|no\b|num\b)|credit\s+card|debit\s+card|expir(y|ation)|\biban\b|account\s+number|routing\s+number|\bssn\b|social\s+security|\bpin\b|\bcard\b|\bcc[\s_-]*(number|num|csc|exp|name)|cc-(number|csc|exp)|secret|api[\s_-]*key|access[\s_-]*token|\btoken\b",
    )
    .is_match(name)
}

/// What a value is when it must never be typed by a recipe: a payment card number (Luhn, the gate's
/// own check), a private key, a JWT, a well-known token shape or one long unbroken token. The
/// answer is the reason, for the refusal.
pub fn secret_value(text: &str) -> Option<&'static str> {
    if contains_card_number(&json!(text)) {
        return Some("a payment card number");
    }
    static SHAPES: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    let shapes = regex_once(
        &SHAPES,
        r"-----BEGIN [A-Z ]*PRIVATE KEY|^eyJ[A-Za-z0-9_-]{5,}\.[A-Za-z0-9_-]{5,}\.|\bsk-[A-Za-z0-9_-]{16,}|\bgh[pousr]_[A-Za-z0-9]{20,}|github_pat_[A-Za-z0-9_]{20,}|\bxox[abprs]-[A-Za-z0-9-]{10,}|\bAKIA[0-9A-Z]{16}\b|(?i)\bbearer\s+[A-Za-z0-9._~+/=-]{8,}",
    );
    if shapes.is_match(text) {
        return Some("a key or token");
    }
    static LONG: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    if regex_once(&LONG, r"^[A-Za-z0-9+/=_-]{40,}$").is_match(text.trim()) {
        return Some("a long token");
    }
    None
}

/// A short numeric code (4 to 8 digits): what a one-time code looks like. Only refused where the
/// field is a code field or the parameter has no business holding a number.
pub fn otp_like(text: &str) -> bool {
    let t = text.trim();
    (4..=8).contains(&t.len()) && t.bytes().all(|b| b.is_ascii_digit())
}

/// Replaces every occurrence of a typed value in `text` (an error message, an observation) so a
/// value never reaches a log or a result by way of the browser's own echo.
pub fn scrub(text: &str, secrets: &[String]) -> String {
    let mut variants: Vec<String> = Vec::new();
    for value in secrets.iter().filter(|v| v.chars().count() >= 2) {
        variants.push(value.clone());
        // The browser echoes values escaped for JavaScript or JSON.
        let json = serde_json::to_string(value).unwrap_or_default();
        if json.len() >= 2 {
            variants.push(json[1..json.len() - 1].to_owned());
        }
        variants.push(
            value
                .replace('\\', "\\\\")
                .replace('\'', "\\'")
                .replace('"', "\\\"")
                .replace('\n', "\\n"),
        );
        variants.push(value.escape_unicode().to_string());
        let ascii: String = value
            .chars()
            .map(|c| {
                if c.is_ascii() {
                    c.to_string()
                } else {
                    c.encode_utf16(&mut [0; 2])
                        .iter()
                        .map(|u| format!("\\u{u:04x}"))
                        .collect()
                }
            })
            .collect();
        variants.push(ascii);
        // Quotes, angle brackets and the like written as \u0022 (some JSON writers do).
        let specials: String = value
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == ' ' {
                    c.to_string()
                } else {
                    c.encode_utf16(&mut [0; 2])
                        .iter()
                        .map(|u| format!("\\u{u:04x}"))
                        .collect()
                }
            })
            .collect();
        variants.push(specials);
    }
    variants.sort_by_key(|v| std::cmp::Reverse(v.len()));
    variants.dedup();
    let mut out = text.to_owned();
    for variant in variants.iter().filter(|v| !v.is_empty()) {
        out = out.replace(variant.as_str(), "[value hidden]");
    }
    out
}

/// Whether the digits of these values, written one after the other (a card number split over
/// several fields), hold a payment card number. Every contiguous slice of the values is checked.
pub fn split_card(values: &[&str]) -> bool {
    use crate::mono::browser::gate::digits_hold_card;
    (0..values.len()).any(|start| {
        (start + 1..=values.len()).any(|end| {
            digits_hold_card(&values[start..end].join(" "))
                || digits_hold_card(&values[start..end].concat())
        })
    })
}
