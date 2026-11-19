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
//! * `<img src>` and `<img srcset>` keep only relative image references that stay inside the folder,
//!   each replaced by the text `monoptah-asset:<index>` (index into [`Document::assets`]);
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
    sync::{Arc, Mutex},
};

use ammonia::{Builder, UrlRelative};
use css_inline::{CSSInliner, StylesheetResolver};

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
    let assets = Arc::new(Mutex::new(Vec::<String>::new()));
    let body = sanitizer(assets.clone()).clean(&inlined).to_string();
    let assets = std::mem::take(&mut *assets.lock().unwrap_or_else(|e| e.into_inner()));
    Document {
        html: format!(
            "<!DOCTYPE html><html><head><meta charset=\"utf-8\"></head><body>{body}</body></html>"
        ),
        assets,
    }
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

fn sanitizer(assets: Arc<Mutex<Vec<String>>>) -> Builder<'static> {
    let mut builder = Builder::default();
    builder
        .clean_content_tags(HashSet::from([
            "script", "style", "title", "noscript", "template", "iframe", "object", "embed",
            "textarea",
        ]))
        .add_generic_attributes(["style", "id"])
        .add_tag_attributes("img", ["srcset"])
        .link_rel(Some("noopener noreferrer"))
        .url_relative(UrlRelative::PassThrough)
        .url_schemes(HashSet::from(["monoptah-asset"]))
        .strip_comments(true)
        .attribute_filter(move |element, attribute, value| {
            filter(&assets, element, attribute, value)
        });
    builder
}

fn filter<'u>(
    assets: &Mutex<Vec<String>>,
    element: &str,
    attribute: &str,
    value: &'u str,
) -> Option<Cow<'u, str>> {
    match (element, attribute) {
        (_, "style") => sanitize_style(value, assets).map(Cow::Owned),
        ("a", "href") => value.starts_with('#').then(|| Cow::Owned(value.to_owned())),
        ("a", "rel") => Some(Cow::Borrowed(value)),
        ("img", "src") => placeholder(assets, value).map(Cow::Owned),
        ("img", "srcset") => srcset(assets, value).map(Cow::Owned),
        (_, name) if URL_ATTRIBUTES.contains(&name) => None,
        _ => Some(Cow::Borrowed(value)),
    }
}

/// `monoptah-asset:<index>` for a relative image reference, or `None` when it may not be an asset.
fn placeholder(assets: &Mutex<Vec<String>>, reference: &str) -> Option<String> {
    let path = reference.split(['?', '#']).next().unwrap_or("");
    let path = normalize_asset(&percent_decode(path))?;
    let mut assets = assets.lock().unwrap_or_else(|e| e.into_inner());
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

fn srcset(assets: &Mutex<Vec<String>>, value: &str) -> Option<String> {
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
fn sanitize_style(value: &str, assets: &Mutex<Vec<String>>) -> Option<String> {
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

fn sanitize_declaration(declaration: &str, assets: &Mutex<Vec<String>>) -> Option<String> {
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
        let id = placeholder(assets, argument)?;
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
        let escaped = (bytes[at] == b'%' && at + 2 < bytes.len())
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
            "<img src=\"data:image/png;base64,AAAA\">",
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
    fn anchors_keep_only_fragments() {
        let doc =
            run("<a href=\"#top\">up</a><a href=\"page.html\">p</a><a href=\"//e.test/\">x</a>");
        assert!(doc.html.contains("href=\"#top\""), "{}", doc.html);
        assert!(!doc.html.contains("page.html"));
        assert!(!doc.html.contains("e.test"));
    }

    #[test]
    fn relative_images_become_placeholders_and_the_rest_vanish() {
        let doc = run(concat!(
            "<img src=\"img/a.png\"><img src=\"./img/a.png?v=2\"><img src=\"b%20c.jpg\">",
            "<img src=\"../x.png\"><img src=\"/abs.png\"><img src=\"%2e%2e/x.png\">",
            "<img src=\"a\\b.png\"><img src=\"notes.txt\"><img src=\"monoptah-asset:7\">",
        ));
        assert_eq!(doc.assets, ["img/a.png", "b c.jpg"]);
        assert!(
            doc.html.contains("src=\"monoptah-asset:0\""),
            "{}",
            doc.html
        );
        assert!(doc.html.contains("src=\"monoptah-asset:1\""));
        assert!(!doc.html.contains("monoptah-asset:7"));
        assert!(!doc.html.contains("x.png") && !doc.html.contains("abs.png"));
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
        assert_eq!(doc.assets, ["b.png"]);
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
