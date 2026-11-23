//! Turns a mentioned HTML file into a document the web app may show in a sandboxed frame.
//!
//! The frame inherits the app's CSP, which blocks `<style>` and every network load but allows
//! `style=""` attributes. So linked local stylesheets and `<style>` blocks are inlined into style
//! attributes first (`css-inline`, with no file or network loading of its own: the only stylesheets
//! it sees are the ones [`Sheets`] hands over after the caller's folder and deny-list checks), and
//! the result is sanitized (`ammonia`) with an attribute filter that rebuilds every URL-carrying
//! attribute from scratch:
//!
//! * `<a href>` keeps only a `#fragment`;
//! * `<img src>`, `<img srcset>` and `<picture><source srcset>` keep only image references (relative
//!   with or without `..`, absolute or `file://`, see [`normalize_reference`]), each replaced by the
//!   text `monoptah-asset:<index>` (index into [`Document::assets`]); whether a key may be read is
//!   decided when the asset is requested (inside the project of the document, else its folder); an
//!   `<img src>` may also be an inline raster or svg image `data:image/<type>;base64,<payload>`
//!   (checked by [`data_image`]: allowed type, base64 characters only, size cap per image);
//! * a style attribute loses every declaration that carries a URL other than such an image, any
//!   escape, `expression(`, `@import` or a script scheme, and `url(x.png)` becomes
//!   `url(monoptah-asset:<index>)`;
//! * `<use href>`, `<textPath href>` and SVG paint servers keep only a `#fragment` (an id of the
//!   same document); a presentation attribute or style declaration (`fill`, `marker-end`, ...) may
//!   carry `url(#id)` and no other `url(`;
//! * every other URL attribute is dropped.
//!
//! Inline `<svg>` is kept with a fixed list of drawing elements ([`SVG_TAGS`]) and presentation
//! attributes ([`SVG_ATTRIBUTES`]); `<style>`, `<script>`, `<foreignObject>`, `<image>`, `<feImage>`,
//! animation elements and `on*` attributes are never allowed.
//!
//! Nothing a document says can put a non-placeholder URL in the output.
use std::{
    borrow::Cow,
    collections::HashSet,
    panic::AssertUnwindSafe,
    sync::{Arc, LazyLock, Mutex},
};

use ammonia::{Builder, UrlRelative};
use css_inline::{CSSInliner, StylesheetResolver};
use regex::Regex;

/// Image extensions an asset may have.
pub const IMAGE_EXTENSIONS: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "webp", "svg", "avif", "bmp", "ico",
];

/// Where the stylesheets a document links come from.
pub trait Sheets: Send + Sync {
    /// The text of the stylesheet `href` names, or `None` to leave it out. The implementation
    /// owns every check: this is the only way a file other than the document gets read.
    fn load(&self, href: &str) -> Option<String>;
}

struct Resolver(Arc<dyn Sheets>);

impl StylesheetResolver for Resolver {
    fn retrieve(&self, location: &str) -> css_inline::Result<String> {
        // A refused sheet is an empty one, so a single bad link cannot fail the whole page.
        Ok(self.0.load(location).unwrap_or_default())
    }
}

/// The web-safe document and the relative image paths its placeholders stand for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Document {
    pub html: String,
    pub assets: Vec<String>,
}

/// `source` inlined, sanitized and wrapped as a complete document.
pub fn transform(source: &str, sheets: Arc<dyn Sheets>) -> Document {
    let inlined = std::panic::catch_unwind(AssertUnwindSafe(|| {
        CSSInliner::options()
            .load_remote_stylesheets(true)
            .keep_style_tags(false)
            .keep_link_tags(false)
            .resolver(Arc::new(Resolver(sheets)))
            .build()
            .inline(source)
    }))
    .ok()
    .and_then(Result::ok)
    // Without inlining the styles are lost, never passed on.
    .unwrap_or_else(|| source.to_owned());
    let assets = Arc::new(Refs::default());
    // The sanitizer drops the `<html>` and `<body>` tags, and with them the styles inlined onto
    // them (the page's background, text color, font, padding): they go on a wrapper div, through
    // the same style filter, `<html>` first so `<body>` wins like in a browser. The wrapper
    // fills the frame (a dark page paints all of it) and has no margin of its own, like the
    // outer body, so the page's own `margin` and `padding` keep their meaning.
    let html_style = element_style(&inlined, "html", &assets);
    let body_style = element_style(&inlined, "body", &assets);
    let body = sanitizer(assets.clone()).clean(&inlined).to_string();
    let style: String = ["margin:0;min-height:100vh", &html_style, &body_style]
        .iter()
        .filter(|part| !part.is_empty())
        .map(|part| part.trim_end_matches(';'))
        .collect::<Vec<_>>()
        .join(";");
    let body = format!("<div style=\"{style}\">{body}</div>");
    let assets = std::mem::take(&mut *assets.list.lock().unwrap_or_else(|e| e.into_inner()));
    Document {
        html: format!(
            "<!DOCTYPE html><html><head><meta charset=\"utf-8\"></head><body style=\"margin:0\">{body}</body></html>"
        ),
        assets,
    }
}

/// The safe declarations of the first `<html>` or `<body>` tag's `style` attribute (`name` says
/// which), ready for a double-quoted attribute, or an empty string.
fn element_style(inlined: &str, name: &str, assets: &Refs) -> String {
    element_style_of(inlined, name, assets).unwrap_or_default()
}

fn element_style_of(inlined: &str, name: &str, assets: &Refs) -> Option<String> {
    let body = Regex::new(&format!(
        r#"(?is)<{name}(?:\s(?:"[^"]*"|'[^']*'|[^>"'])*)?>"#
    ))
    .ok()?;
    static STYLE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"(?is)(?:^|\s)style\s*=\s*(?:"([^"]*)"|'([^']*)')"#).expect("style pattern")
    });
    let tag = body.find(inlined)?.as_str();
    let captures = STYLE.captures(tag)?;
    let value = captures.get(1).or_else(|| captures.get(2))?.as_str();
    // The raw text is not entity-decoded here (ammonia decodes before its filter): an entity
    // could spell `url(` past the style filter, so such a value is dropped.
    if value.contains('&') {
        return None;
    }
    let kept = sanitize_style(value, assets)?;
    Some(
        kept.replace('"', "&quot;")
            .replace('<', "&lt;")
            .replace('>', "&gt;"),
    )
}

/// Attributes that can carry a URL or load something; each is rebuilt or removed.
const URL_ATTRIBUTES: &[&str] = &[
    "href",
    "src",
    "srcset",
    "cite",
    "background",
    "poster",
    "action",
    "formaction",
    "data",
    "longdesc",
    "usemap",
    "icon",
    "manifest",
    "ping",
    "codebase",
    "classid",
    "archive",
    "profile",
    "xlink:href",
    "dynsrc",
    "lowsrc",
];

/// Inline SVG elements that draw (no scripts, no animation, no `<image>`, no `<foreignObject>`).
/// The names are the camelCase ones the HTML parser produces for SVG elements.
const SVG_TAGS: &[&str] = &[
    "svg",
    "g",
    "defs",
    "symbol",
    "use",
    "marker",
    "path",
    "rect",
    "circle",
    "ellipse",
    "line",
    "polyline",
    "polygon",
    "text",
    "tspan",
    "textPath",
    "linearGradient",
    "radialGradient",
    "stop",
    "clipPath",
    "mask",
    "pattern",
    "filter",
    "feGaussianBlur",
    "feOffset",
    "feDropShadow",
    "feFlood",
    "feMerge",
    "feMergeNode",
    "feColorMatrix",
    "feBlend",
    "feComposite",
];

/// Attributes of the elements in [`SVG_TAGS`]. Names are as the HTML parser spells them
/// (`viewBox`, `markerWidth`, `refX`). URL-carrying ones (`href`) are handled by [`filter`].
const SVG_ATTRIBUTES: &[&str] = &[
    "class",
    "role",
    "aria-label",
    "aria-hidden",
    "aria-labelledby",
    "aria-describedby",
    "viewBox",
    "preserveAspectRatio",
    "width",
    "height",
    "x",
    "y",
    "x1",
    "y1",
    "x2",
    "y2",
    "cx",
    "cy",
    "r",
    "rx",
    "ry",
    "fx",
    "fy",
    "dx",
    "dy",
    "d",
    "points",
    "transform",
    "opacity",
    "visibility",
    "display",
    "overflow",
    "fill",
    "fill-opacity",
    "fill-rule",
    "clip-rule",
    "stroke",
    "stroke-width",
    "stroke-opacity",
    "stroke-dasharray",
    "stroke-dashoffset",
    "stroke-linecap",
    "stroke-linejoin",
    "stroke-miterlimit",
    "paint-order",
    "vector-effect",
    "color",
    "stop-color",
    "stop-opacity",
    "offset",
    "gradientUnits",
    "gradientTransform",
    "spreadMethod",
    "patternUnits",
    "patternContentUnits",
    "patternTransform",
    "clipPathUnits",
    "maskUnits",
    "maskContentUnits",
    "clip-path",
    "mask",
    "filter",
    "marker-start",
    "marker-mid",
    "marker-end",
    "markerWidth",
    "markerHeight",
    "markerUnits",
    "refX",
    "refY",
    "orient",
    "font-family",
    "font-size",
    "font-style",
    "font-weight",
    "font-variant",
    "font-stretch",
    "letter-spacing",
    "word-spacing",
    "text-anchor",
    "text-decoration",
    "dominant-baseline",
    "alignment-baseline",
    "baseline-shift",
    "writing-mode",
    "direction",
    "rotate",
    "textLength",
    "lengthAdjust",
    "startOffset",
    "method",
    "spacing",
    "pathLength",
    "href",
    "in",
    "in2",
    "result",
    "stdDeviation",
    "flood-color",
    "flood-opacity",
    "values",
    "type",
    "mode",
    "operator",
    "k1",
    "k2",
    "k3",
    "k4",
    "filterUnits",
    "primitiveUnits",
];

fn sanitizer(assets: Arc<Refs>) -> Builder<'static> {
    let mut builder = Builder::default();
    for tag in SVG_TAGS {
        builder.add_tags([*tag]);
        builder.add_tag_attributes(*tag, SVG_ATTRIBUTES.iter().copied());
    }
    builder
        .clean_content_tags(HashSet::from([
            "script",
            "style",
            "title",
            "noscript",
            "template",
            "iframe",
            "object",
            "embed",
            "textarea",
            "foreignObject",
            "animate",
            "set",
        ]))
        .add_generic_attributes(["style", "id"])
        .add_tags(["picture", "source", "main"])
        .add_tag_attributes("img", ["srcset"])
        .add_tag_attributes("source", ["srcset"])
        .link_rel(Some("noopener noreferrer"))
        .url_relative(UrlRelative::PassThrough)
        .url_schemes(HashSet::from(["monoptah-asset", "data", "file"]))
        .strip_comments(true)
        .attribute_filter(move |element, attribute, value| {
            filter(&assets, element, attribute, value)
        });
    builder
}

fn filter<'u>(
    assets: &Refs,
    element: &str,
    attribute: &str,
    value: &'u str,
) -> Option<Cow<'u, str>> {
    match (element, attribute) {
        (_, "style") => sanitize_style(value, assets).map(Cow::Owned),
        ("a", "href") => value.starts_with('#').then(|| Cow::Owned(value.to_owned())),
        (tag, "href" | "xlink:href") if SVG_TAGS.contains(&tag) => {
            fragment_reference(value).then_some(Cow::Borrowed(value))
        }
        (tag, _) if SVG_TAGS.contains(&tag) => {
            internal_urls_only(value).then_some(Cow::Borrowed(value))
        }
        ("a", "rel") => Some(Cow::Borrowed(value)),
        ("img", "src") => placeholder(assets, value)
            .or_else(|| data_image(value))
            .map(Cow::Owned),
        ("img" | "source", "srcset") => srcset(assets, value).map(Cow::Owned),
        (_, name) if URL_ATTRIBUTES.contains(&name) => None,
        _ => Some(Cow::Borrowed(value)),
    }
}

/// Whether `value` is `#id` (an id of this document) and nothing else.
fn fragment_reference(value: &str) -> bool {
    value
        .trim()
        .strip_prefix('#')
        .is_some_and(|id| !id.is_empty() && id.chars().all(is_id_char))
}

fn is_id_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | ':' | '.')
}

/// Whether the SVG attribute `value` loads nothing: every `url(...)` in it names an id of this
/// document (`url(#id)`), and it carries no escape, comment, `@import`, script scheme or markup.
fn internal_urls_only(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    if FORBIDDEN.iter().any(|bad| lower.contains(bad))
        || LOADING_FUNCTIONS.iter().any(|f| lower.contains(f))
        || lower.contains("data:")
    {
        return false;
    }
    let mut rest = 0;
    while let Some(found) = lower[rest..].find("url(") {
        let args = rest + found + "url(".len();
        let Some(close) = lower[args..].find(')') else {
            return false;
        };
        if internal_url_argument(&value[args..args + close]).is_none() {
            return false;
        }
        rest = args + close + 1;
    }
    true
}

/// `#id` out of the argument of a `url(...)`, quotes allowed, when it is an internal reference.
fn internal_url_argument(argument: &str) -> Option<&str> {
    let argument = argument.trim();
    let argument = argument
        .strip_prefix('"')
        .and_then(|a| a.strip_suffix('"'))
        .or_else(|| {
            argument
                .strip_prefix('\'')
                .and_then(|a| a.strip_suffix('\''))
        })
        .unwrap_or(argument)
        .trim();
    fragment_reference(argument).then_some(argument)
}

/// Image types an inline `data:` image may have.
const DATA_IMAGE_TYPES: &[&str] = &[
    "png", "jpeg", "gif", "webp", "avif", "bmp", "svg+xml", "x-icon",
];

/// What a document names while it is sanitized: its image keys (placeholder order). There is no
/// total for inline images: the transform runs in the sandbox, whose output is bounded as a whole
/// ([`super::sandbox::MAX_OUTPUT_BYTES`]), and the web app loads every image on its own.
#[derive(Default)]
struct Refs {
    list: Mutex<Vec<String>>,
}

/// The most base64 characters one inline image may carry.
pub const MAX_DATA_IMAGE_CHARS: usize = 4 * 1024 * 1024;

/// `value` as `data:image/<type>;base64,<payload>` when it is exactly that: an allowed image type,
/// no parameters, and a payload of base64 characters only (so it cannot close a quote or smuggle
/// markup). Anything else, `data:text/html` and url-encoded payloads included, is `None`.
fn data_image(value: &str) -> Option<String> {
    let value = value.trim();
    let head_len = "data:image/".len();
    if !value.get(..head_len)?.eq_ignore_ascii_case("data:image/") {
        return None;
    }
    let (kind, payload) = value[head_len..].split_once(";base64,")?;
    let kind = kind.to_ascii_lowercase();
    if !DATA_IMAGE_TYPES.contains(&kind.as_str())
        || payload.is_empty()
        || payload.len() > MAX_DATA_IMAGE_CHARS
        || !payload
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'/' | b'='))
    {
        return None;
    }
    Some(format!("data:image/{kind};base64,{payload}"))
}

/// `monoptah-asset:<index>` for a relative image reference, or `None` when it may not be an asset.
fn placeholder(assets: &Refs, reference: &str) -> Option<String> {
    let path = normalize_reference(reference)?;
    let mut assets = assets.list.lock().unwrap_or_else(|e| e.into_inner());
    let index = match assets.iter().position(|known| *known == path) {
        Some(index) => index,
        None => {
            // No count cap: every image the page names is listed. What bounds the list is the document itself (and the
            // sandbox's output limit), and the web app loads the assets a few at a time.
            assets.push(path);
            assets.len() - 1
        }
    };
    Some(format!("monoptah-asset:{index}"))
}

fn srcset(assets: &Refs, value: &str) -> Option<String> {
    let kept: Vec<String> = value
        .split(',')
        .filter_map(|candidate| {
            let mut parts = candidate.split_whitespace();
            let url = placeholder(assets, parts.next()?)?;
            match parts.next() {
                None => Some(url),
                Some(descriptor) if parts.next().is_none() && descriptor_ok(descriptor) => {
                    Some(format!("{url} {descriptor}"))
                }
                Some(_) => None,
            }
        })
        .collect();
    (!kept.is_empty()).then(|| kept.join(", "))
}

fn descriptor_ok(descriptor: &str) -> bool {
    let Some(number) = descriptor
        .strip_suffix('w')
        .or_else(|| descriptor.strip_suffix('x'))
    else {
        return false;
    };
    !number.is_empty()
        && number.len() <= 8
        && number.chars().all(|c| c.is_ascii_digit() || c == '.')
}

/// Declarations of a style attribute that are safe to keep, or `None` when none are.
fn sanitize_style(value: &str, assets: &Refs) -> Option<String> {
    let kept: Vec<String> = split_declarations(value)
        .into_iter()
        .filter_map(|declaration| sanitize_declaration(&declaration, assets))
        .collect();
    (!kept.is_empty()).then(|| kept.join("; "))
}

/// The declarations of `value`, split on `;` outside quotes and parentheses.
fn split_declarations(value: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let (mut depth, mut quote) = (0usize, None::<char>);
    for c in value.chars() {
        match quote {
            Some(q) => {
                if c == q {
                    quote = None;
                }
            }
            None => match c {
                '"' | '\'' => quote = Some(c),
                '(' => depth += 1,
                ')' => depth = depth.saturating_sub(1),
                ';' if depth == 0 => {
                    out.push(std::mem::take(&mut current));
                    continue;
                }
                _ => {}
            },
        }
        current.push(c);
    }
    out.push(current);
    out
}

/// Functions that load a resource other than through `url(`.
const LOADING_FUNCTIONS: &[&str] = &[
    "image-set(",
    "src(",
    "image(",
    "cross-fade(",
    "element(",
    "paint(",
    "-webkit-image-set(",
];

/// Substrings that make a declaration unsafe wherever they occur (compared in lowercase).
const FORBIDDEN: &[&str] = &[
    "expression",
    "@import",
    "javascript:",
    "vbscript:",
    "-moz-binding",
    "behavior",
    "<",
    "\\",
    "/*",
];

/// Properties that may name an id of this document with `url(#id)`.
const PAINT_PROPERTIES: &[&str] = &[
    "fill",
    "stroke",
    "marker",
    "marker-start",
    "marker-mid",
    "marker-end",
    "clip-path",
    "mask",
    "filter",
];

fn sanitize_declaration(declaration: &str, assets: &Refs) -> Option<String> {
    let declaration = declaration.trim();
    let (property, value) = declaration.split_once(':')?;
    let property = property.trim();
    let value = value.trim();
    if property.is_empty()
        || !property
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        || value.is_empty()
    {
        return None;
    }
    let lower = value.to_ascii_lowercase();
    let lower_property = property.to_ascii_lowercase();
    if FORBIDDEN
        .iter()
        .any(|bad| lower.contains(bad) || lower_property.contains(bad))
        || LOADING_FUNCTIONS.iter().any(|f| lower.contains(f))
    {
        return None;
    }
    // Every `url(...)` must name an image of the folder; it becomes a placeholder.
    let mut out = String::new();
    let mut rest = 0;
    while let Some(found) = lower[rest..].find("url(") {
        let start = rest + found;
        let args = start + "url(".len();
        let end = args + lower[args..].find(')')?;
        let argument = value[args..end].trim();
        let argument = argument
            .strip_prefix('"')
            .and_then(|a| a.strip_suffix('"'))
            .or_else(|| {
                argument
                    .strip_prefix('\'')
                    .and_then(|a| a.strip_suffix('\''))
            })
            .unwrap_or(argument)
            .trim();
        let id = match internal_url_argument(argument) {
            Some(fragment) if PAINT_PROPERTIES.contains(&lower_property.as_str()) => {
                fragment.to_owned()
            }
            Some(_) => return None,
            None => placeholder(assets, argument).or_else(|| data_image(argument))?,
        };
        out.push_str(&value[rest..start]);
        out.push_str(&format!("url({id})"));
        rest = end + 1;
    }
    out.push_str(&value[rest..]);
    Some(format!("{property}: {out}"))
}

/// A path as the folder-relative `a/b.c` it names, or `None` when the rules refuse it: relative
/// only, forward slashes, no `..`, no scheme or drive (`:`), no `~`, no control characters and
/// no empty name. `.` and empty parts are dropped.
pub fn normalize_relative(path: &str) -> Option<String> {
    if path.is_empty()
        || path.chars().count() > 1024
        || path.starts_with('/')
        || path.starts_with('~')
        || path.contains(['\\', ':'])
        || path.chars().any(char::is_control)
    {
        return None;
    }
    let mut parts = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => return None,
            other => parts.push(other),
        }
    }
    (!parts.is_empty()).then(|| parts.join("/"))
}

/// The lowercase extension of the last component of `path`.
pub fn extension(path: &str) -> Option<String> {
    let name = path.rsplit('/').next()?;
    Some(name.rsplit_once('.')?.1.to_ascii_lowercase())
}

/// The key of an image reference as the document names it: relative (`a/b.png`, `../a.png`),
/// absolute (`/home/me/a.png`) or `file://` (percent-decoded, an empty or `localhost` host).
/// `?query` and `#fragment` are cut. Whether the key may be read is decided on the desktop at
/// request time ([`super::serve_asset`]): this only refuses what can never be an image path
/// (another scheme, `~`, backslashes, control characters, no image extension).
pub fn normalize_reference(reference: &str) -> Option<String> {
    let reference = reference.trim();
    let reference = reference.split(['?', '#']).next().unwrap_or("");
    let decoded = percent_decode(reference);
    let lower = decoded.get(..7).map(str::to_ascii_lowercase);
    let path = if lower.as_deref() == Some("file://") {
        let rest = &decoded[7..];
        let rest = match rest.get(..10).map(str::to_ascii_lowercase).as_deref() {
            Some("localhost/") => &rest[9..],
            _ => rest,
        };
        if !rest.starts_with('/') {
            return None;
        }
        rest.to_owned()
    } else {
        decoded
    };
    normalize_key(&path)
}

/// [`normalize_reference`] for a path that is already decoded (an `asset` the client sends back):
/// the same key comes out, so the request is held to exactly what the document named.
pub fn normalize_key(path: &str) -> Option<String> {
    if path.is_empty()
        || path.chars().count() > 1024
        || path.starts_with('~')
        || path.contains(['\\', ':'])
        || path.chars().any(char::is_control)
    {
        return None;
    }
    let absolute = path.starts_with('/');
    let parts: Vec<&str> = path
        .split('/')
        .filter(|part| !part.is_empty() && *part != ".")
        .collect();
    if parts.is_empty() {
        return None;
    }
    let key = parts.join("/");
    let extension = extension(&key)?;
    if !IMAGE_EXTENSIONS.contains(&extension.as_str()) {
        return None;
    }
    Some(if absolute { format!("/{key}") } else { key })
}

/// [`normalize_relative`] for a path that must be an image.
pub fn normalize_asset(path: &str) -> Option<String> {
    let path = normalize_relative(path)?;
    let extension = extension(&path)?;
    IMAGE_EXTENSIONS
        .contains(&extension.as_str())
        .then_some(path)
}

pub(super) fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        // `%XX` needs two hex digits and may end the text (`at + 2` is its last byte).
        let escaped = (bytes[at] == b'%'
            && at + 2 < bytes.len()
            && bytes[at + 1].is_ascii_hexdigit()
            && bytes[at + 2].is_ascii_hexdigit())
        .then(|| std::str::from_utf8(&bytes[at + 1..at + 3]).ok())
        .flatten()
        .and_then(|hex| u8::from_str_radix(hex, 16).ok());
        match escaped {
            Some(byte) => {
                out.push(byte);
                at += 3;
            }
            None => {
                out.push(bytes[at]);
                at += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    struct Fixed(HashMap<&'static str, &'static str>);
    impl Sheets for Fixed {
        fn load(&self, href: &str) -> Option<String> {
            self.0.get(href).map(|css| (*css).to_owned())
        }
    }

    fn run(source: &str) -> Document {
        transform(source, Arc::new(Fixed(HashMap::new())))
    }

    const SVG_PAGE: &str = concat!(
        "<style>:root{--c:#0969da}svg.d{display:block;color:var(--c)}",
        "svg.d .b{fill:var(--c);stroke:#333;stroke-width:1.2}svg.d text{font:12px sans-serif}</style>",
        "<main><svg class=\"d\" viewBox=\"0 0 100 50\" role=\"img\" aria-label=\"Diagram\">",
        "<defs><marker id=\"ar\" viewBox=\"0 0 10 10\" refX=\"9\" refY=\"5\" markerWidth=\"7\" markerHeight=\"7\" orient=\"auto\">",
        "<path d=\"M0,0 L10,5 L0,10 z\" fill=\"currentColor\"/></marker>",
        "<linearGradient id=\"g\"><stop offset=\"0\" stop-color=\"#fff\"/></linearGradient></defs>",
        "<rect class=\"b\" x=\"1\" y=\"2\" width=\"30\" height=\"20\" fill=\"url(#g)\" stroke-dasharray=\"4 3\"/>",
        "<g id=\"n\"><text x=\"5\" y=\"9\" text-anchor=\"middle\" dominant-baseline=\"middle\" font-weight=\"700\">Hi</text></g>",
        "<use href=\"#n\"/><path d=\"M1 1 L9 9\" marker-end=\"url(#ar)\" style=\"stroke:url(#g)\"/></svg></main>",
    );

    #[test]
    fn inline_svg_keeps_shapes_camel_case_attributes_styles_and_internal_references() {
        let doc = run(SVG_PAGE);
        for kept in [
            "<svg",
            "viewBox=\"0 0 100 50\"",
            "<defs",
            "<marker id=\"ar\"",
            "refX=\"9\"",
            "markerWidth=\"7\"",
            "markerHeight=\"7\"",
            "<linearGradient id=\"g\"",
            "<stop",
            "<rect",
            "<text",
            "text-anchor=\"middle\"",
            "dominant-baseline=\"middle\"",
            "font-weight=\"700\"",
            "stroke-dasharray=\"4 3\"",
            "fill=\"url(#g)\"",
            "marker-end=\"url(#ar)\"",
            "<use href=\"#n\"",
            "stroke: url(#g)",
            "<main",
            // Custom properties go to the wrapper and the svg's own rules onto the elements.
            "--c: #0969da",
            "fill: var(--c)",
            "font: 12px sans-serif",
        ] {
            assert!(doc.html.contains(kept), "{kept} lost: {}", doc.html);
        }
        assert!(!doc.html.contains("<style"), "{}", doc.html);
        assert!(doc.assets.is_empty());
    }

    #[test]
    fn svg_reference_tricks_and_foreign_content_are_dropped() {
        let doc = run(concat!(
            "<svg><rect fill=\"url(\\23 x)\"/><rect fill=\"url(#a)/**/\"/><rect FILL=\"URL(HTTP://x.test/p)\"/>",
            "<foreignObject><p>secret</p><b>inner</b></foreignObject>",
            "<animate attributeName=\"x\"><text>anim</text></animate><set attributeName=\"y\"><text>setc</text></set>",
            "<desc>described</desc>",
            "<use xlink:href=\"#n\"/><rect id=\"n\" style=\"background:url(#x);fill:url(#ok)\"/></svg>",
        ));
        let lower = doc.html.to_ascii_lowercase();
        for bad in [
            "\\23", "/**/", "http", "secret", "inner", "anim", "setc", "url(#x)", "<desc",
        ] {
            assert!(!lower.contains(bad), "{bad} survived: {}", doc.html);
        }
        assert!(doc.html.contains("fill: url(#ok)"), "{}", doc.html);
        assert!(doc.html.contains("href=\"#n\""), "{}", doc.html);
    }

    #[test]
    fn inline_svg_loads_nothing_and_runs_nothing() {
        let doc = run(concat!(
            "<svg viewBox=\"0 0 10 10\" onload=\"x()\">",
            "<script>alert(1)</script><style>@import url(https://e.test/a.css);rect{fill:url(https://e.test/p)}</style>",
            "<rect onclick=\"x()\" fill=\"url(https://e.test/a.svg#p)\" stroke=\"url(//e.test/p)\" ",
            "marker-end=\"url(data:image/svg+xml;base64,AAAA)\" clip-path=\"url('http://e.test/c')\" ",
            "style=\"fill:url(https://e.test/p);stroke:url(//e.test/p);filter:url(http://e.test/f)\"/>",
            "<a href=\"javascript:alert(1)\"><text>j</text></a>",
            "<use href=\"https://e.test/a.svg#x\"/><use xlink:href=\"//e.test/a.svg#x\"/>",
            "<use href=\"data:image/svg+xml;base64,AAAA\"/><use href=\"javascript:alert(1)\"/>",
            "<image href=\"https://e.test/a.png\"/><feImage href=\"https://e.test/a.png\"/>",
            "<animate attributeName=\"href\" to=\"javascript:alert(1)\"/><set attributeName=\"onclick\" to=\"x()\"/>",
            "<foreignObject><div>html</div><iframe src=\"https://e.test/\"></iframe></foreignObject>",
            "<text style=\"fill:url(#ok)\">kept</text></svg>",
        ));
        let lower = doc.html.to_ascii_lowercase();
        for bad in [
            "<script",
            "alert",
            "onload",
            "onclick",
            "@import",
            "<style",
            "https://",
            "http://",
            "//e.test",
            "e.test",
            "javascript:",
            "data:",
            "<image",
            "<feimage",
            "<animate",
            "<set",
            "<foreignobject",
            "<iframe",
        ] {
            assert!(!lower.contains(bad), "{bad} survived: {}", doc.html);
        }
        assert!(doc.html.contains("<svg"), "{}", doc.html);
        assert!(doc.html.contains("<rect"), "{}", doc.html);
        assert!(doc.html.contains("fill: url(#ok)"), "{}", doc.html);
        assert!(doc.assets.is_empty());
    }

    #[test]
    fn scripts_handlers_frames_forms_and_remote_loads_are_removed() {
        let doc = run(concat!(
            "<h1 onclick=\"x()\">Title</h1><script>alert(1)</script>",
            "<iframe src=\"https://e.test/\"></iframe><object data=\"a.swf\"></object>",
            "<form action=\"https://e.test/\"><input name=a></form>",
            "<img src=\"https://e.test/a.png\" onerror=\"x()\">",
            "<img src=\"data:text/html;base64,AAAA\">",
            "<a href=\"javascript:alert(1)\">j</a><a href=\"https://e.test/\">r</a>",
            "<base href=\"https://e.test/\"><meta http-equiv=\"refresh\" content=\"0;url=https://e.test/\">",
            "<link rel=\"stylesheet\" href=\"https://e.test/a.css\">",
        ));
        let lower = doc.html.to_ascii_lowercase();
        for bad in [
            "<script",
            "alert",
            "<iframe",
            "<object",
            "<form",
            "<input",
            "onclick",
            "onerror",
            "https://",
            "javascript:",
            "data:",
            "<base",
            "http-equiv",
            "<link",
            "<style",
        ] {
            assert!(!lower.contains(bad), "{bad} survived: {}", doc.html);
        }
        assert!(doc.html.contains("Title"));
        assert!(doc.assets.is_empty());
    }

    #[test]
    fn inline_data_images_are_kept_and_nothing_else_with_a_data_scheme() {
        let png = "data:image/png;base64,iVBORw0KGgo=";
        let doc = run(&format!(
            "<img id=a src=\"{png}\"><img id=b src=\"DATA:image/SVG+xml;base64,PHN2Zz48L3N2Zz4=\">"
        ));
        assert!(doc.html.contains(&format!("src=\"{png}\"")), "{}", doc.html);
        assert!(
            doc.html
                .contains("data:image/svg+xml;base64,PHN2Zz48L3N2Zz4="),
            "{}",
            doc.html
        );
        assert!(doc.assets.is_empty());
        for bad in [
            "data:text/html;base64,PGgxPg==",
            "data:image/png;charset=utf-8;base64,AAAA",
            "data:image/png,%3Csvg%3E",
            "data:image/png;base64,AA AA\"onerror=\"x()",
            "data:image/png;base64,",
            "data:image/tiff;base64,AAAA",
            "data:application/javascript;base64,AAAA",
        ] {
            let doc = run(&format!("<img src='{bad}'><p>kept</p>"));
            assert!(
                !doc.html.to_ascii_lowercase().contains("data:"),
                "{bad} survived: {}",
                doc.html
            );
            assert!(!doc.html.contains("onerror"), "{}", doc.html);
            assert!(doc.html.contains("kept"));
        }
        // Only <img src>: other places that take a URL still lose a data: image.
        let doc = run(&format!(
            "<a href=\"{png}\">a</a><img srcset=\"{png} 1x\"><div background=\"{png}\">e</div>"
        ));
        assert!(!doc.html.contains("data:"), "{}", doc.html);
        // A css url() takes the same inline images, and srcset or a picture source takes references.
        let doc = run(&format!(
            "<div style=\"background:url({png})\">d</div><picture><source srcset=\"../a.png 1x, b.webp 2x\"><img src=\"c.png\"></picture><div style=\"background:url(../bg.png)\">e</div>"
        ));
        assert!(doc.html.contains(&format!("url({png})")), "{}", doc.html);
        assert_eq!(doc.assets, ["../a.png", "b.webp", "c.png", "../bg.png"]);
        assert!(doc.html.contains("<source"), "{}", doc.html);
        // The size cap.
        let big = format!(
            "data:image/png;base64,{}",
            "A".repeat(MAX_DATA_IMAGE_CHARS + 4)
        );
        assert!(!run(&format!("<img src=\"{big}\">")).html.contains("data:"));
    }

    #[test]
    fn inline_images_have_no_document_total() {
        // One 3 MiB image copied by a rule into 4 elements: every copy stays (12 MiB in all, over the old 8 MiB total).
        let payload = "A".repeat(3 * 1024 * 1024);
        let uri = format!("data:image/png;base64,{payload}");
        struct Owned(String);
        impl Sheets for Owned {
            fn load(&self, _href: &str) -> Option<String> {
                Some(self.0.clone())
            }
        }
        let sheets = Owned(format!(".c {{ background: url({uri}) }}"));
        let doc = transform(
            "<link rel=stylesheet href=t.css><p class=c>1</p><p class=c>2</p><p class=c>3</p><p class=c>4</p>",
            Arc::new(sheets),
        );
        let kept = doc.html.matches("data:image/png;base64,").count();
        assert_eq!(kept, 4, "{}", &doc.html[..200.min(doc.html.len())]);
        for n in 1..=4 {
            assert!(doc.html.contains(&format!(">{n}</p>")));
        }
    }

    #[test]
    fn a_page_may_name_any_number_of_images_and_none_is_dropped() {
        // More than the old 256 distinct images: every one is listed and keeps a placeholder.
        let page: String = (0..300)
            .map(|n| format!("<img id=i{n} src=\"img/{n}.png\">"))
            .collect();
        let doc = run(&page);
        assert_eq!(doc.assets.len(), 300);
        assert_eq!(doc.assets[299], "img/299.png");
        assert!(
            doc.html.contains("src=\"monoptah-asset:299\""),
            "{}",
            doc.html
        );
        assert_eq!(doc.html.matches("monoptah-asset:").count(), 300);
        let reply = super::super::sandbox::encode_reply(&doc);
        assert_eq!(
            super::super::sandbox::decode_reply(&reply).map(|d| d.assets.len()),
            Some(300)
        );
    }

    #[test]
    fn anchors_keep_only_fragments() {
        let doc =
            run("<a href=\"#top\">up</a><a href=\"page.html\">p</a><a href=\"//e.test/\">x</a>");
        assert!(doc.html.contains("href=\"#top\""), "{}", doc.html);
        assert!(!doc.html.contains("page.html"));
        assert!(!doc.html.contains("e.test"));
    }

    #[test]
    fn image_references_become_placeholders_and_the_rest_vanish() {
        let doc = run(concat!(
            "<img src=\"img/a.png\"><img src=\"./img/a.png?v=2\"><img src=\"b%20c.jpg\">",
            "<img src=\"../x.png\"><img src=\"/abs.png\"><img src=\"%2e%2e/x.png\">",
            "<img src=\"file:///home/me/p.png#f\"><img src=\"file://host/p.png\">",
            "<img src=\"a\\b.png\"><img src=\"notes.txt\"><img src=\"monoptah-asset:7\">",
            "<img src=\"~/a.png\"><img src=\"https://e.test/q.png\">",
        ));
        assert_eq!(
            doc.assets,
            [
                "img/a.png",
                "b c.jpg",
                "../x.png",
                "/abs.png",
                "/home/me/p.png"
            ]
        );
        assert!(
            doc.html.contains("src=\"monoptah-asset:0\""),
            "{}",
            doc.html
        );
        assert!(doc.html.contains("src=\"monoptah-asset:4\""));
        assert!(!doc.html.contains("monoptah-asset:7"));
        assert!(!doc.html.contains("x.png") && !doc.html.contains("abs.png"));
        assert!(!doc.html.contains("q.png") && !doc.html.contains("p.png"));
        assert!(!doc.html.contains("notes.txt"));
    }

    #[test]
    fn srcset_keeps_only_asset_candidates() {
        let doc = run("<img srcset=\"a.png 1x, https://e.test/b.png 2x, c.png 2x\">");
        assert_eq!(doc.assets, ["a.png", "c.png"]);
        assert!(
            doc.html
                .contains("srcset=\"monoptah-asset:0 1x, monoptah-asset:1 2x\""),
            "{}",
            doc.html
        );
    }

    #[test]
    fn style_blocks_and_linked_sheets_are_inlined_into_style_attributes() {
        let sheets = Fixed(HashMap::from([(
            "theme.css",
            "h1 { color: red; background: url(bg.png) }",
        )]));
        let doc = transform(
            concat!(
                "<html><head><style>p { margin: 4px }</style>",
                "<link rel=\"stylesheet\" href=\"theme.css\">",
                "<link rel=\"stylesheet\" href=\"https://e.test/x.css\"></head>",
                "<body><h1>T</h1><p>x</p></body></html>",
            ),
            Arc::new(sheets),
        );
        assert!(
            !doc.html.contains("<style") && !doc.html.contains("<link"),
            "{}",
            doc.html
        );
        assert!(doc.html.contains("color: red"), "{}", doc.html);
        assert!(doc.html.contains("margin: 4px"), "{}", doc.html);
        assert!(doc.html.contains("url(monoptah-asset:0)"), "{}", doc.html);
        assert_eq!(doc.assets, ["bg.png"]);
        assert!(!doc.html.contains("e.test"));
    }

    #[test]
    fn dangerous_declarations_are_dropped_from_style_attributes() {
        let doc = run(concat!(
            "<p style=\"color: blue; background: url(https://e.test/a.png); width: expression(alert(1));",
            " content: 'x'; background-image: image-set('a.png' 1x); behavior: url(x.htc);",
            " list-style: url('b.png'); margin: 1px; background: u\\72l(c.png)\">t</p>",
            "<p style=\"background: url('/etc/passwd.png')\">u</p>",
        ));
        assert!(doc.html.contains("color: blue"), "{}", doc.html);
        assert!(doc.html.contains("margin: 1px"));
        assert!(doc.html.contains("list-style: url(monoptah-asset:0)"));
        for bad in [
            "e.test",
            "expression",
            "image-set",
            "behavior",
            "c.png",
            "passwd",
            "u\\72l",
        ] {
            assert!(!doc.html.contains(bad), "{bad} survived: {}", doc.html);
        }
        // An absolute path is only a key: reading it is refused at request time unless it is inside the root.
        assert_eq!(doc.assets, ["b.png", "/etc/passwd.png"]);
    }

    #[test]
    fn a_hostile_sheet_cannot_bring_in_a_remote_url() {
        let sheets = Fixed(HashMap::from([(
            "a.css",
            "@import url(https://e.test/i.css); p { background: url(//e.test/p.png); color: green }",
        )]));
        let doc = transform("<link rel=stylesheet href=a.css><p>x</p>", Arc::new(sheets));
        assert!(!doc.html.contains("e.test"), "{}", doc.html);
        assert!(doc.html.contains("color: green"), "{}", doc.html);
    }

    #[test]
    fn the_body_style_moves_to_a_wrapper_div_through_the_style_filter() {
        let doc = run(concat!(
            "<style>body{background:#123456;color:white}</style>",
            "<body><p>hi</p></body>"
        ));
        assert!(
            doc.html.contains("<div style=\"") && doc.html.contains("background: #123456"),
            "{}",
            doc.html
        );
        assert!(doc.html.contains("<p>hi</p>"));
        // The same rules as any style attribute: no remote URL, no entity that spells one.
        for style in [
            "background:url(https://e.test/a.png);color:red",
            "background:&#117;rl(https://e.test/a.png)",
        ] {
            let doc = run(&format!("<body style=\"{style}\"><p>x</p></body>"));
            assert!(!doc.html.contains("e.test"), "{style}: {}", doc.html);
        }
        let doc = run("<body style=\"background:url(a.png)\"><p>x</p></body>");
        assert!(doc.html.contains("url(monoptah-asset:0)"), "{}", doc.html);
        assert_eq!(doc.assets, ["a.png"]);
        // Every page gets the wrapper that fills the frame, with its own margin removed.
        let plain = run("<p>x</p>").html;
        assert!(
            plain.contains("<div style=\"margin:0;min-height:100vh\"><p>x</p></div>"),
            "{plain}"
        );
        assert!(plain.contains("<body style=\"margin:0\">"), "{plain}");
    }

    #[test]
    fn the_html_background_goes_on_the_wrapper_and_the_body_wins() {
        let doc = run(concat!(
            "<style>html{background:#111111;color:white}body{background:#222222;padding:12px}</style>",
            "<p>x</p>"
        ));
        let wrapper = doc.html.split("<div style=\"").nth(1).unwrap();
        let wrapper = wrapper.split('"').next().unwrap();
        let html = wrapper.find("#111111").expect(wrapper);
        let body = wrapper.find("#222222").expect(wrapper);
        assert!(html < body, "{wrapper}");
        assert!(wrapper.contains("padding: 12px"), "{wrapper}");
        assert!(
            wrapper.starts_with("margin:0;min-height:100vh;"),
            "{wrapper}"
        );
    }

    #[test]
    fn the_title_text_and_comments_do_not_leak_into_the_page() {
        let doc = run("<head><title>Secret title</title></head><!-- note --><p>hi</p>");
        assert!(
            !doc.html.contains("Secret title") && !doc.html.contains("note"),
            "{}",
            doc.html
        );
        assert!(doc.html.contains("<p>hi</p>"));
        assert!(doc.html.starts_with("<!DOCTYPE html>"));
    }

    #[test]
    fn percent_decoding_handles_the_end_of_the_text_and_bad_escapes() {
        assert_eq!(percent_decode("a%2e"), "a.");
        assert_eq!(percent_decode("%2E%2e/x.png"), "../x.png");
        assert_eq!(percent_decode("a%2"), "a%2");
        assert_eq!(percent_decode("a%"), "a%");
        assert_eq!(percent_decode("a%+1b"), "a%+1b");
        assert_eq!(percent_decode("100%zz"), "100%zz");
    }

    #[test]
    fn reference_rules() {
        for (given, key) in [
            ("a.png", "a.png"),
            ("./a//b.PNG?x#y", "a/b.PNG"),
            ("../a.png", "../a.png"),
            ("/abs/a.png", "/abs/a.png"),
            ("file:///abs/a%20b.png", "/abs/a b.png"),
            ("FILE://localhost/abs/a.png", "/abs/a.png"),
        ] {
            assert_eq!(normalize_reference(given).as_deref(), Some(key), "{given}");
            assert_eq!(normalize_key(key).as_deref(), Some(key), "{key}");
        }
        for bad in [
            "",
            "~/a.png",
            "a\\b.png",
            "a.txt",
            "dir/",
            "file://host/a.png",
            "http://x/a.png",
            "data:image/png;base64,AA",
            "C:/a.png",
            "a\0.png",
            "/",
        ] {
            assert!(normalize_reference(bad).is_none(), "{bad:?}");
        }
    }

    #[test]
    fn asset_rules() {
        for good in ["a.png", "img/a.JPG", "./img/a.svg", "a b/c.webp"] {
            assert!(normalize_asset(good).is_some(), "{good}");
        }
        for bad in [
            "",
            "/a.png",
            "~/a.png",
            "../a.png",
            "a/../b.png",
            "a\\b.png",
            "http://x/a.png",
            "C:a.png",
            "a.txt",
            "dir/",
            "a\0.png",
            "noext",
        ] {
            assert!(normalize_asset(bad).is_none(), "{bad:?}");
        }
        assert_eq!(
            normalize_asset("./img//a.png").as_deref(),
            Some("img/a.png")
        );
    }
}
