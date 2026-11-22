//! The action overlay: before the gate lets a click, a typed value or another targeted action
//! reach the page, it shows the user where the action lands (a highlight box and a cursor moving
//! to the element), holds for a moment, acts, and removes the drawing.
//!
//! The drawing is the gate's, never the agent's. It runs through `browser_evaluate` of the
//! Playwright server, which the agent can not call without an approval card, so this module keeps
//! that door shut from the other side:
//!
//! * The function is a Rust constant ([`draw_script`], [`remove_script`]). The only variable part
//!   is a random hex token the gate made itself, used as a data attribute name. No text from the
//!   agent, the page or a recipe is ever part of the code.
//! * The element is passed as the call's `target` argument, which Playwright reads as data. For
//!   an agent call it must be a snapshot ref (`e12`, `f1e3`): a CSS selector or a locator
//!   expression from the agent never reaches this path. A recipe step passes the very selector
//!   the step is about to act on, which the action call would resolve anyway.
//! * The overlay is one closed shadow root under an `aria-hidden`, `pointer-events: none` host
//!   with the highest z-index. It sets styles through the CSSOM only (no `<style>`, no inline
//!   attribute), so a strict CSP does not stop it. The page removes nothing it needs to read: the
//!   host is removed after the action and also by a timer in the page.
//! * Drawing is best effort. Every failure (no element, an iframe, a CSP, a closed tab) is
//!   swallowed and the action still runs.
use serde_json::{json, Value};

/// The tools whose target is drawn, with the argument that names the element.
const TARGET_TOOLS: [&str; 7] = [
    "browser_click",
    "browser_hover",
    "browser_type",
    "browser_select_option",
    "browser_drop",
    "browser_drag",
    "browser_fill_form",
];
/// The longest selector a recipe step may pass for drawing.
const MAX_SELECTOR_CHARS: usize = 600;
/// How long the box and the cursor stay, in the page, if the gate never removes them.
const SELF_REMOVE_MS: u32 = 4000;
/// The default hold between drawing and acting.
pub const DEFAULT_HOLD_MS: u64 = 500;
/// How long one overlay call to the server may take before the gate goes on without it.
pub const CALL_TIMEOUT_MS: u64 = 2000;
/// What the draw function answers when it drew, as the server prints a string result. The code
/// the server echoes back holds `'ok-' + String(1)`, never this exact text.
const DREW: &str = "\"ok-1\"";

/// Whether the text of an evaluate reply says the box was drawn: the `### Result` block only, not
/// the code or page text that follows it.
pub fn drew(reply: &str) -> bool {
    let Some((_, after)) = reply.split_once("### Result") else {
        return false;
    };
    let block = after.split("\n### ").next().unwrap_or("");
    block.trim() == DREW
}

const DRAW: &str = r#"(el) => {
  try {
    const doc = el.ownerDocument;
    const win = doc.defaultView;
    const attr = 'data-__TOKEN__';
    for (const old of doc.querySelectorAll('[' + attr + ']')) old.remove();
    let r = el.getBoundingClientRect();
    if (!(r.width > 0 && r.height > 0)) return 'ok-' + String(0);
    if (r.bottom < 0 || r.top > win.innerHeight || r.right < 0 || r.left > win.innerWidth) {
      el.scrollIntoView({ block: 'center', inline: 'center' });
      r = el.getBoundingClientRect();
    }
    const host = doc.createElement('div');
    host.setAttribute(attr, '');
    host.setAttribute('aria-hidden', 'true');
    Object.assign(host.style, { position: 'fixed', left: '0', top: '0', width: '0', height: '0', margin: '0', padding: '0', border: '0', pointerEvents: 'none', zIndex: '2147483647' });
    const root = host.attachShadow({ mode: 'closed' });
    const box = doc.createElement('div');
    Object.assign(box.style, { position: 'fixed', left: (r.left - 3) + 'px', top: (r.top - 3) + 'px', width: (r.width + 6) + 'px', height: (r.height + 6) + 'px', boxSizing: 'border-box', border: '2px solid #6d5efc', borderRadius: '4px', background: 'rgba(109, 94, 252, 0.12)', boxShadow: '0 0 0 2px rgba(255, 255, 255, 0.85)', pointerEvents: 'none' });
    const ns = 'http://www.w3.org/2000/svg';
    const cursor = doc.createElementNS(ns, 'svg');
    cursor.setAttribute('width', '22');
    cursor.setAttribute('height', '22');
    cursor.setAttribute('viewBox', '0 0 24 24');
    const path = doc.createElementNS(ns, 'path');
    path.setAttribute('d', 'M3 2l7 18 2.6-7.4L20 10z');
    path.setAttribute('fill', '#111');
    path.setAttribute('stroke', '#fff');
    path.setAttribute('stroke-width', '1.5');
    cursor.appendChild(path);
    Object.assign(cursor.style, { position: 'fixed', left: '0', top: '0', pointerEvents: 'none' });
    root.appendChild(box);
    root.appendChild(cursor);
    doc.documentElement.appendChild(host);
    const x = r.left + r.width / 2;
    const y = r.top + r.height / 2;
    cursor.animate([{ transform: 'translate(' + (x - 90) + 'px, ' + (y - 70) + 'px)' }, { transform: 'translate(' + x + 'px, ' + y + 'px)' }], { duration: 350, easing: 'ease-out', fill: 'forwards' });
    win.setTimeout(() => host.remove(), __SELF_REMOVE_MS__);
    return 'ok-' + String(1);
  } catch (e) {
    return 'ok-' + String(0);
  }
}"#;

const REMOVE: &str = r#"(el) => {
  try {
    const attr = 'data-__TOKEN__';
    const docs = [document];
    if (el && el.ownerDocument && el.ownerDocument !== document) docs.push(el.ownerDocument);
    for (const doc of docs) for (const old of doc.querySelectorAll('[' + attr + ']')) old.remove();
  } catch (e) {}
  return 'ok-' + String(0);
}"#;

/// A new random token for the overlay's attribute name: lowercase hex, so it is safe in code.
pub fn new_token() -> String {
    format!("mp-{:032x}", rand::random::<u128>())
}

fn fill(template: &str, token: &str) -> String {
    template
        .replace("__TOKEN__", token)
        .replace("__SELF_REMOVE_MS__", &SELF_REMOVE_MS.to_string())
}

/// The function that draws the box and the cursor around the element it is given.
pub fn draw_script(token: &str) -> String {
    fill(DRAW, token)
}

/// The function that removes the drawing (from the element's document and the page's).
pub fn remove_script(token: &str) -> String {
    fill(REMOVE, token)
}

/// `browser_evaluate` arguments that draw around `target`.
pub fn draw_args(token: &str, target: &str) -> Value {
    json!({"function": draw_script(token), "target": target})
}

/// `browser_evaluate` arguments that remove the drawing, with the element when there is one.
pub fn remove_args(token: &str, target: Option<&str>) -> Value {
    match target {
        Some(target) => json!({"function": remove_script(token), "target": target}),
        None => json!({"function": remove_script(token)}),
    }
}

/// Whether `text` is a snapshot ref of `@playwright/mcp` 0.0.86: `e12`, or `f1e3` inside a frame.
pub fn is_ref(text: &str) -> bool {
    let rest = match text.strip_prefix('f') {
        Some(rest) => {
            let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
            if digits == 0 {
                return false;
            }
            &rest[digits..]
        }
        None => text,
    };
    let Some(number) = rest.strip_prefix('e') else {
        return false;
    };
    text.len() <= 24 && !number.is_empty() && number.bytes().all(|b| b.is_ascii_digit())
}

/// The element a call acts on, when it is drawn: only for the targeted tools, and only a ref
/// (`trusted` false, an agent call) or a ref or a bounded plain selector (`trusted` true, a step
/// the recipe runner is about to run). `None` draws nothing.
pub fn target_of(tool: &str, args: &Value, trusted: bool) -> Option<String> {
    if !TARGET_TOOLS.contains(&tool) {
        return None;
    }
    let target = match tool {
        "browser_drag" => args["startTarget"].as_str(),
        // A form fill draws the first field.
        "browser_fill_form" => args["fields"][0]["target"].as_str(),
        _ => args["target"].as_str(),
    }?;
    let ok = is_ref(target)
        || (trusted
            && !target.is_empty()
            && target.chars().count() <= MAX_SELECTOR_CHARS
            && !target.chars().any(char::is_control));
    ok.then(|| target.to_owned())
}
