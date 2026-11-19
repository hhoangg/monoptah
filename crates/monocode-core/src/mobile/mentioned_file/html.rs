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
//!   (checked by [`data_image`]: allowed type, base64 characters only, size cap);
//! * a style attribute loses every declaration that carries a URL other than such an image, any
//!   escape, `expression(`, `@import` or a script scheme, and `url(x.png)` becomes
//!   `url(monoptah-asset:<index>)`;
//! * every other URL attribute is dropped.
//!
//! Nothing a document says can put a non-placeholder URL in the output.
use std::{
    borrow::Cow,
    collections::HashSet,
    panic::AssertUnwindSafe,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, LazyLock, Mutex,
    },
};

use ammonia::{Builder, UrlRelative};
use css_inline::{CSSInliner, StylesheetResolver};
use regex::Regex;

/// The most distinct assets one document may name; later references are removed.
pub const MAX_ASSETS: usize = 256;

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

fn sanitizer(assets: Arc<Refs>) -> Builder<'static> {
    let mut builder = Builder::default();
    builder
        .clean_content_tags(HashSet::from([
            "script", "style", "title", "noscript", "template", "iframe", "object", "embed",
            "textarea",
        ]))
        .add_generic_attributes(["style", "id"])
        .add_tags(["picture", "source"])
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
        ("a", "rel") => Some(Cow::Borrowed(value)),
        ("img", "src") => placeholder(assets, value)
            .or_else(|| data_image(assets, value))
            .map(Cow::Owned),
        ("img" | "source", "srcset") => srcset(assets, value).map(Cow::Owned),
        (_, name) if URL_ATTRIBUTES.contains(&name) => None,
        _ => Some(Cow::Borrowed(value)),
    }
}

/// Image types an inline `data:` image may have.
const DATA_IMAGE_TYPES: &[&str] = &[
    "png", "jpeg", "gif", "webp", "avif", "bmp", "svg+xml", "x-icon",
];

/// The most base64 characters all the inline images of one document may carry together.
pub const MAX_DATA_IMAGE_TOTAL_CHARS: usize = 8 * 1024 * 1024;

/// What a document names while it is sanitized: its image keys (placeholder order) and the
/// characters of inline images kept so far.
#[derive(Default)]
struct Refs {
    list: Mutex<Vec<String>>,
    data: AtomicUsize,
}

/// The most base64 characters one inline image may carry.
pub const MAX_DATA_IMAGE_CHARS: usize = 4 * 1024 * 1024;

/// `value` as `data:image/<type>;base64,<payload>` when it is exactly that: an allowed image type,
/// no parameters, and a payload of base64 characters only (so it cannot close a quote or smuggle
/// markup). Anything else, `data:text/html` and url-encoded payloads included, is `None`.
fn data_image(refs: &Refs, value: &str) -> Option<String> {
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
    // A stylesheet image is copied by the inliner into every element its rule matches: the document
    // keeps inline images only up to a total, later ones are dropped (the page still renders).
    let total = refs.data.fetch_add(payload.len(), Ordering::Relaxed) + payload.len();
    if total > MAX_DATA_IMAGE_TOTAL_CHARS {
        refs.data.fetch_sub(payload.len(), Ordering::Relaxed);
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
        None if assets.len() < MAX_ASSETS => {
            assets.push(path);
            assets.len() - 1
        }
        None => return None,
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
        let id = placeholder(assets, argument).or_else(|| data_image(assets, argument))?;
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
    fn inline_images_stop_at_a_document_total_and_the_page_still_renders() {
        // One 3 MiB image copied by a rule into 4 elements: the first two fit under 8 MiB, the rest go.
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
        assert_eq!(kept, 2, "{}", &doc.html[..200.min(doc.html.len())]);
        assert!(doc.html.len() < MAX_DATA_IMAGE_TOTAL_CHARS + 4096);
        for n in 1..=4 {
            assert!(doc.html.contains(&format!(">{n}</p>")));
        }
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
