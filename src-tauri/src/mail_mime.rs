// Not wired up until the sync task consumes it.
#![allow(dead_code)]

use ammonia::{Builder, UrlRelative};
use mail_parser::{
    Addr, Address, DateTime, HeaderValue, Message, MessageParser, MimeHeaders, PartType,
};
use std::borrow::Cow;
use std::collections::{HashMap, HashSet};

const SNIPPET_CHARS: usize = 180;
/// Before this a `Date` is a broken clock, not a real send time.
const EARLIEST_PLAUSIBLE_DATE: i64 = 631_152_000; // 1990-01-01
const LATEST_PLAUSIBLE_DATE: i64 = 4_102_444_800; // 2100-01-01
const FUTURE_SLACK_SECS: i64 = 86_400;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MailAddress {
    pub name: Option<String>,
    pub address: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DateSource {
    Header,
    /// `Date` was missing or absurd, so `date_utc` is the topmost `Received`.
    Received,
    /// Neither was usable; the caller picks a fallback such as the S3 object time.
    None,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ParsedAttachment {
    pub part_index: usize,
    pub filename: String,
    pub mime_type: String,
    pub size_bytes: usize,
    pub content_id: Option<String>,
    pub is_inline: bool,
}

/// Ids carry no angle brackets, as spec §6 wants for threading.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ParsedMail {
    pub message_id: Option<String>,
    pub in_reply_to: Option<String>,
    /// `In-Reply-To` then `References`, document order, duplicates dropped.
    pub referenced_ids: Vec<String>,
    pub subject: String,
    pub from: Option<MailAddress>,
    pub reply_to: Option<MailAddress>,
    pub to: Vec<MailAddress>,
    pub cc: Vec<MailAddress>,
    pub bcc: Vec<MailAddress>,
    /// Epoch seconds.
    pub date_utc: Option<i64>,
    pub date_source: DateSource,
    /// Epoch seconds of the topmost `Received` header.
    pub received_at: Option<i64>,
    pub snippet: String,
    pub body_text: Option<String>,
    /// Already sanitised.
    pub body_html: Option<String>,
    pub remote_images_blocked: usize,
    pub attachments: Vec<ParsedAttachment>,
    pub size_bytes: usize,
    pub spf: Option<String>,
    pub dkim: Option<String>,
    pub dmarc: Option<String>,
    /// JSON array of `[name, value]` pairs for the raw-source view.
    pub headers_json: String,
}

impl ParsedMail {
    pub fn has_attachments(&self) -> bool {
        self.attachments.iter().any(|a| !a.is_inline)
    }
}

fn user_error(message: &str) -> String {
    message.to_string()
}

pub(crate) fn parse_mail(raw: &[u8]) -> Result<ParsedMail, String> {
    let unreadable = || user_error("This message could not be read: it is not a valid email.");
    let msg = MessageParser::default().parse(raw).ok_or_else(unreadable)?;
    // The parser accepts almost anything and reads junk as a header name, so
    // require at least one header that real mail always has.
    if !msg.headers_raw().any(|(name, _)| is_core_header(name)) {
        return Err(unreadable());
    }

    let received_at = topmost_received(&msg);
    let (date_utc, date_source) = pick_date(msg.date(), received_at);

    let in_reply_to_ids = header_ids(msg.in_reply_to());
    let mut referenced_ids = in_reply_to_ids.clone();
    for id in header_ids(msg.references()) {
        if !referenced_ids.contains(&id) {
            referenced_ids.push(id);
        }
    }

    let (body_text, html_source) = pick_bodies(&msg);
    let (body_html, remote_images_blocked) = match html_source {
        Some(html) if html_exceeds_caps(&html) => (Some(oversized_html_fallback(&body_text)), 0),
        Some(html) => {
            let (clean, blocked) = sanitize_html(&html);
            (Some(clean), blocked)
        }
        None => (None, 0),
    };
    let snippet = match (&body_text, &body_html) {
        (Some(text), _) => snippet_from_text(text),
        (None, Some(html)) => snippet_from_html(html),
        (None, None) => String::new(),
    };

    let (spf, dkim, dmarc) = auth_results(&msg);

    Ok(ParsedMail {
        message_id: msg.message_id().map(trim_id).filter(|s| !s.is_empty()),
        in_reply_to: in_reply_to_ids.into_iter().next(),
        referenced_ids,
        subject: msg.subject().unwrap_or_default().trim().to_string(),
        from: msg.from().and_then(first_address),
        reply_to: msg.reply_to().and_then(first_address),
        to: addresses(msg.all_to()),
        cc: addresses(msg.all_cc()),
        bcc: addresses(msg.all_bcc()),
        date_utc,
        date_source,
        received_at,
        snippet,
        body_text,
        body_html: body_html.filter(|h| !h.is_empty()),
        remote_images_blocked,
        attachments: attachments(&msg),
        size_bytes: raw.len(),
        spf,
        dkim,
        dmarc,
        headers_json: headers_json(&msg),
    })
}

fn is_core_header(name: &str) -> bool {
    const CORE: &[&str] = &[
        "from",
        "to",
        "cc",
        "subject",
        "date",
        "message-id",
        "received",
        "mime-version",
        "content-type",
        "return-path",
        "reply-to",
        "sender",
    ];
    CORE.iter().any(|c| name.eq_ignore_ascii_case(c))
}

fn trim_id(id: &str) -> String {
    id.trim()
        .trim_start_matches('<')
        .trim_end_matches('>')
        .trim()
        .to_string()
}

fn header_ids(value: &HeaderValue) -> Vec<String> {
    let raw: Vec<&str> = match value {
        HeaderValue::Text(t) => vec![t.as_ref()],
        HeaderValue::TextList(list) => list.iter().map(|s| s.as_ref()).collect(),
        _ => Vec::new(),
    };
    raw.into_iter()
        .map(trim_id)
        .filter(|s| !s.is_empty())
        .collect()
}

fn first_address(address: &Address) -> Option<MailAddress> {
    address.iter().find_map(to_mail_address)
}

fn addresses<'a>(all: impl Iterator<Item = &'a Address<'a>>) -> Vec<MailAddress> {
    all.flat_map(|a| a.iter())
        .filter_map(to_mail_address)
        .collect()
}

fn to_mail_address(addr: &Addr) -> Option<MailAddress> {
    let address = addr.address()?.trim();
    if address.is_empty() {
        return None;
    }
    Some(MailAddress {
        name: addr
            .name()
            .map(str::trim)
            .filter(|n| !n.is_empty())
            .map(String::from),
        address: address.to_string(),
    })
}

fn topmost_received(msg: &Message) -> Option<i64> {
    // The raw text is read because `Received` parsing in the crate is strict
    // about the clause before the `;` and the date is all that is needed.
    let (_, value) = msg
        .headers_raw()
        .find(|(name, _)| name.eq_ignore_ascii_case("received"))?;
    let date = value.rsplit(';').next()?.trim();
    DateTime::parse_rfc822(date).map(|d| d.to_timestamp())
}

fn pick_date(header: Option<&DateTime>, received: Option<i64>) -> (Option<i64>, DateSource) {
    let usable = header
        .filter(|d| d.is_valid())
        .map(|d| d.to_timestamp())
        .filter(|ts| {
            let ceiling = received.map_or(LATEST_PLAUSIBLE_DATE, |r| r + FUTURE_SLACK_SECS);
            (EARLIEST_PLAUSIBLE_DATE..=ceiling).contains(ts)
        });
    match (usable, received) {
        (Some(ts), _) => (Some(ts), DateSource::Header),
        (None, Some(r)) => (Some(r), DateSource::Received),
        (None, None) => (None, DateSource::None),
    }
}

/// Returns the plain text and the HTML to sanitise. Per spec §7 the HTML is the
/// last real `text/html` part; with none, the plain text is escaped into HTML.
fn pick_bodies(msg: &Message) -> (Option<String>, Option<String>) {
    let texts: Vec<&str> = msg
        .text_body
        .iter()
        .filter_map(|&i| match &msg.parts.get(i as usize)?.body {
            PartType::Text(t) => Some(t.as_ref()),
            _ => None,
        })
        .collect();
    let text = (!texts.is_empty()).then(|| texts.join("\n\n"));

    let html = msg
        .html_body
        .iter()
        .rev()
        .find_map(|&i| match &msg.parts.get(i as usize)?.body {
            PartType::Html(h) => Some(h.to_string()),
            _ => None,
        });
    let html = html.or_else(|| text.as_deref().map(plain_to_html));
    (text, html)
}

fn plain_to_html(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 16);
    for c in text.replace("\r\n", "\n").chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\n' => out.push_str("<br>"),
            c => out.push(c),
        }
    }
    out
}

fn attachments(msg: &Message) -> Vec<ParsedAttachment> {
    msg.attachments
        .iter()
        .filter_map(|&i| {
            let part = msg.parts.get(i as usize)?;
            let content_id = part.content_id().map(trim_id).filter(|s| !s.is_empty());
            let mime_type = part
                .content_type()
                .map(|ct| match ct.subtype() {
                    Some(sub) => format!("{}/{}", ct.ctype(), sub).to_ascii_lowercase(),
                    None => ct.ctype().to_ascii_lowercase(),
                })
                .unwrap_or_else(|| "application/octet-stream".to_string());
            let disposition = part.content_disposition();
            let is_inline = match disposition {
                Some(d) => d.is_inline(),
                None => content_id.is_some(),
            };
            Some(ParsedAttachment {
                part_index: i as usize,
                filename: part
                    .attachment_name()
                    .map(str::to_string)
                    .unwrap_or_else(|| format!("attachment-{i}")),
                mime_type,
                size_bytes: part.len(),
                content_id,
                is_inline,
            })
        })
        .collect()
}

fn headers_json(msg: &Message) -> String {
    let pairs: Vec<[&str; 2]> = msg.headers_raw().map(|(n, v)| [n, v.trim()]).collect();
    serde_json::to_string(&pairs).unwrap_or_else(|_| "[]".to_string())
}

/// RFC 8601: only the header added by the trusted boundary means anything, and
/// that is the topmost one. Any lower header was written by the sender or a
/// relay, so reading it would let a stranger forge `pass`.
fn auth_results(msg: &Message) -> (Option<String>, Option<String>, Option<String>) {
    let Some((_, value)) = msg
        .headers_raw()
        .find(|(name, _)| name.eq_ignore_ascii_case("authentication-results"))
    else {
        return (None, None, None);
    };
    let value = strip_comments(value);
    let mut tokens = value
        .trim_start()
        .split(|c: char| c.is_whitespace() || c == ';');
    // Inbound mail only ever arrives through SES, so any other authserv-id
    // means the header did not come from our boundary.
    if !tokens
        .next()
        .is_some_and(|id| id.eq_ignore_ascii_case(TRUSTED_AUTHSERV_ID))
    {
        return (None, None, None);
    }
    let mut found: [Option<String>; 3] = [None, None, None];
    let mut seen = [false; 3];
    for token in tokens {
        for ((slot, seen), method) in found
            .iter_mut()
            .zip(&mut seen)
            .zip(["spf=", "dkim=", "dmarc="])
        {
            // `get` because a multibyte character may straddle the cut.
            let prefix_matches = token
                .get(..method.len())
                .is_some_and(|p| p.eq_ignore_ascii_case(method));
            if !*seen && prefix_matches {
                *seen = true;
                let verdict: String = token[method.len()..]
                    .chars()
                    .take_while(|c| c.is_ascii_alphabetic())
                    .collect::<String>()
                    .to_ascii_lowercase();
                if !verdict.is_empty() && verdict != "none" {
                    *slot = Some(verdict);
                }
            }
        }
    }
    let [spf, dkim, dmarc] = found;
    (spf, dkim, dmarc)
}

const TRUSTED_AUTHSERV_ID: &str = "amazonses.com";

/// Removes RFC 5322 comments (nested parentheses), so text inside one cannot
/// pose as a method result while `spf=pass(note)` still reads as a verdict.
fn strip_comments(value: &str) -> String {
    let mut depth = 0usize;
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            c if depth == 0 => out.push(c),
            _ => {}
        }
    }
    out
}

fn snippet_from_text(text: &str) -> String {
    collapse_and_cut(text)
}

fn snippet_from_html(sanitised: &str) -> String {
    collapse_and_cut(&unescape(&strip_tags(sanitised)))
}

fn collapse_and_cut(text: &str) -> String {
    let mut out = String::new();
    for word in text.split_whitespace() {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(word);
        if out.chars().count() >= SNIPPET_CHARS {
            break;
        }
    }
    out.chars().take(SNIPPET_CHARS).collect()
}

/// Only ever fed ammonia output, where text has no raw `<` and attribute values
/// are always double-quoted.
fn strip_tags(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut in_tag = false;
    let mut in_quote = false;
    for c in html.chars() {
        match (in_tag, c) {
            (false, '<') => {
                in_tag = true;
                out.push(' ');
            }
            (false, c) => out.push(c),
            (true, '"') => in_quote = !in_quote,
            (true, '>') if !in_quote => in_tag = false,
            (true, _) => {}
        }
    }
    out
}

fn unescape(text: &str) -> String {
    text.replace("&nbsp;", " ")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&amp;", "&")
}

const ALLOWED_TAGS: &[&str] = &[
    "a",
    "abbr",
    "address",
    "article",
    "aside",
    "b",
    "bdi",
    "bdo",
    "big",
    "blockquote",
    "br",
    "caption",
    "center",
    "cite",
    "code",
    "col",
    "colgroup",
    "dd",
    "del",
    "dfn",
    "div",
    "dl",
    "dt",
    "em",
    "figcaption",
    "figure",
    "font",
    "footer",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "header",
    "hr",
    "i",
    "img",
    "ins",
    "kbd",
    "li",
    "main",
    "mark",
    "nav",
    "ol",
    "p",
    "pre",
    "q",
    "s",
    "samp",
    "section",
    "small",
    "span",
    "strike",
    "strong",
    "sub",
    "sup",
    "table",
    "tbody",
    "td",
    "tfoot",
    "th",
    "thead",
    "time",
    "tr",
    "tt",
    "u",
    "ul",
    "var",
    "wbr",
];

// `title` is not in spec §7's list: a full HTML document would otherwise leak
// its <title> text into the body.
const REMOVED_WITH_CONTENT: &[&str] = &[
    "script", "style", "iframe", "object", "embed", "applet", "form", "input", "button", "link",
    "meta", "base", "svg", "title",
];

const TABLE_CELL_ATTRS: &[&str] = &[
    "width", "height", "align", "valign", "bgcolor", "colspan", "rowspan", "border",
];

/// What the reader sees instead of HTML that was refused before sanitising.
const OVERSIZED_HTML_NOTICE: &str =
    "This message's HTML was too large or too deeply nested to display safely; use View source.";

/// html5ever's tree builder is quadratic in nesting depth (measured with
/// ammonia 4.2: 100k nested divs took ~15 s, 1M would take about 26 minutes),
/// so one hostile mail would pin a sync thread for that long. Both caps sit far
/// above any real mail: 512 is the DOM depth browsers themselves flatten at,
/// and real newsletters and long reply chains nest a few dozen levels (at 512
/// the parser spends well under a millisecond); 2 MiB is about twenty times the
/// 102 KB at which Gmail clips a message, and flat HTML of that size is linear
/// (a second or two in a debug build).
const MAX_HTML_BYTES: usize = 2 * 1024 * 1024;
const MAX_HTML_DEPTH: usize = 512;

/// Elements that never have content, so they never nest.
const VOID_TAGS: &[&str] = &[
    "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param", "source",
    "track", "wbr",
];

/// Elements the HTML parser closes by itself when the next sibling starts, and
/// that senders habitually leave open. Counting them would flag a long, sloppy
/// but harmless list; each one nests only through an element that is counted.
const IMPLIED_END_TAGS: &[&str] = &[
    "html", "head", "body", "p", "li", "dt", "dd", "tr", "td", "th", "thead", "tbody", "tfoot",
    "colgroup", "option", "optgroup", "rt", "rp",
];

/// A cheap tag scan, not a parse. It errs towards over-counting (no comment,
/// quote or raw-text awareness, so markup inside those still counts), because
/// the only costly mistake is reading a deep document as shallow. An end tag
/// pops only when it matches the innermost open element: popping further, as a
/// browser does, would let `<div></x>` repeated hide a growing stack.
fn html_exceeds_caps(html: &str) -> bool {
    if html.len() > MAX_HTML_BYTES {
        return true;
    }
    let bytes = html.as_bytes();
    let mut stack: Vec<&str> = Vec::new();
    let mut i = 0;
    while let Some(offset) = bytes[i..].iter().position(|&b| b == b'<') {
        i += offset + 1;
        let closing = bytes.get(i) == Some(&b'/');
        let name_start = if closing { i + 1 } else { i };
        if !bytes.get(name_start).is_some_and(u8::is_ascii_alphabetic) {
            continue;
        }
        let name_len = bytes[name_start..]
            .iter()
            .position(|&b| b.is_ascii_whitespace() || b == b'/' || b == b'>')
            .unwrap_or(bytes.len() - name_start);
        let name_end = name_start + name_len;
        // The delimiters are ASCII, so both ends are char boundaries; `get`
        // only spares an unwrap.
        let Some(name) = html.get(name_start..name_end) else {
            continue;
        };
        i = name_end;
        let counted = !VOID_TAGS.iter().any(|t| name.eq_ignore_ascii_case(t))
            && !IMPLIED_END_TAGS
                .iter()
                .any(|t| name.eq_ignore_ascii_case(t));
        if !counted {
            continue;
        }
        if closing {
            if stack
                .last()
                .is_some_and(|top| top.eq_ignore_ascii_case(name))
            {
                stack.pop();
            }
        } else {
            stack.push(name);
            if stack.len() > MAX_HTML_DEPTH {
                return true;
            }
        }
    }
    false
}

/// The notice, then the message's text part when there is one. The text is
/// escaped here rather than sanitised: it is the cheap path that cannot nest.
fn oversized_html_fallback(text: &Option<String>) -> String {
    let mut out = format!("<p><em>{OVERSIZED_HTML_NOTICE}</em></p>");
    if let Some(text) = text {
        let mut end = text.len().min(MAX_HTML_BYTES);
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        out.push_str(&plain_to_html(&text[..end]));
    }
    out
}

/// Sanitises untrusted email HTML. Returns the HTML and the number of remote
/// images that were blocked.
///
/// Remote `img src` cannot be moved to `data-mono-src` from ammonia's attribute
/// filter (it can change a value, not add an attribute). The filter instead
/// swaps the URL for one under a one-off scheme, and the serialised output is
/// rewritten from that.
///
/// Safety rests on the nonce, not on the allowlist: the scheme embeds a fresh
/// uuid per call that never appears in the output, so sender content cannot
/// spell the needle `src="<marker>` (text nodes do not escape `"`). As a second
/// wall `filter_attribute` drops any sender `img src` that starts with the
/// marker, and attribute values escape `"`, so an extracted URL cannot break
/// out of `data-mono-src="..."`. `data-mono-src` is not allowlisted either.
fn sanitize_html(html: &str) -> (String, usize) {
    let scheme = format!("monoblk-{}", uuid::Uuid::new_v4().simple());
    let marker = format!("{scheme}:");

    let mut tag_attributes: HashMap<&str, HashSet<&str>> = HashMap::new();
    tag_attributes.insert("a", HashSet::from(["href", "title"]));
    tag_attributes.insert(
        "img",
        HashSet::from(["src", "alt", "title", "width", "height"]),
    );
    tag_attributes.insert("font", HashSet::from(["color", "face", "size"]));
    for tag in ["table", "tbody", "thead", "tfoot", "tr", "td", "th"] {
        let mut attrs: HashSet<&str> = TABLE_CELL_ATTRS.iter().copied().collect();
        if tag == "table" {
            attrs.extend(["cellpadding", "cellspacing"]);
        }
        tag_attributes.insert(tag, attrs);
    }
    tag_attributes.insert("col", HashSet::from(["span", "width"]));
    tag_attributes.insert("colgroup", HashSet::from(["span", "width"]));
    tag_attributes.insert("ol", HashSet::from(["start", "type"]));
    tag_attributes.insert("li", HashSet::from(["value"]));
    tag_attributes.insert("blockquote", HashSet::from(["cite"]));
    tag_attributes.insert("q", HashSet::from(["cite"]));
    tag_attributes.insert("div", HashSet::from(["align"]));
    tag_attributes.insert("p", HashSet::from(["align"]));
    tag_attributes.insert("hr", HashSet::from(["align", "width", "size"]));

    let filter_marker = marker.clone();
    let cleaned = Builder::empty()
        .tags(ALLOWED_TAGS.iter().copied().collect())
        .clean_content_tags(REMOVED_WITH_CONTENT.iter().copied().collect())
        .tag_attributes(tag_attributes)
        .generic_attributes(HashSet::from(["style", "dir", "lang"]))
        .url_schemes(HashSet::from([
            "http",
            "https",
            "mailto",
            "cid",
            "data",
            scheme.as_str(),
        ]))
        // Relative URLs must reach `filter_attribute`: under Deny ammonia drops them
        // first, so a `//host/x.gif` image would vanish without being counted.
        .url_relative(UrlRelative::PassThrough)
        .link_rel(Some("noopener noreferrer nofollow"))
        .filter_style_properties(ALLOWED_STYLE_PROPERTIES.iter().copied().collect())
        .attribute_filter(move |element, attribute, value| {
            filter_attribute(&filter_marker, element, attribute, value)
        })
        .clean(html)
        .to_string();

    rewrite_blocked_images(&cleaned, &marker)
}

/// Owns the decision for every URL-bearing attribute, because relative URLs
/// are passed through by ammonia and `cite` is not checked by it at all.
fn filter_attribute<'u>(
    marker: &str,
    element: &str,
    attribute: &str,
    value: &'u str,
) -> Option<Cow<'u, str>> {
    let trimmed = value.trim();
    let lower = trimmed.to_ascii_lowercase();
    let is_relative = || url::Url::parse(trimmed) == Err(url::ParseError::RelativeUrlWithoutBase);
    match (element, attribute) {
        (_, "style") => filter_style(value).map(Cow::Owned),
        ("img", "src") => {
            if lower.starts_with(&marker.to_ascii_lowercase()) {
                None
            } else if lower.starts_with("cid:")
                || (lower.starts_with("data:image/") && !lower.starts_with("data:image/svg"))
            {
                Some(Cow::Borrowed(trimmed))
            } else if lower.starts_with("http://") || lower.starts_with("https://") {
                Some(Cow::Owned(format!("{marker}{trimmed}")))
            } else if trimmed.starts_with("//") {
                Some(Cow::Owned(format!("{marker}https:{trimmed}")))
            } else if is_relative() {
                // No base to resolve against, but it is still a remote reference
                // the sender wrote, so it is blocked and counted like the rest.
                Some(Cow::Owned(format!("{marker}{trimmed}")))
            } else {
                None
            }
        }
        ("a", "href") => {
            if lower.starts_with("data:") || lower.starts_with(&marker.to_ascii_lowercase()) {
                None
            } else if is_relative() {
                // A relative link in an email has no base and is broken anyway.
                None
            } else {
                Some(Cow::Borrowed(value))
            }
        }
        (_, "cite") => {
            let ok = ["http://", "https://", "mailto:"]
                .iter()
                .any(|scheme| lower.starts_with(scheme));
            ok.then_some(Cow::Borrowed(value))
        }
        _ => Some(Cow::Borrowed(value)),
    }
}

fn rewrite_blocked_images(cleaned: &str, marker: &str) -> (String, usize) {
    let needle = format!("src=\"{marker}");
    let mut out = String::with_capacity(cleaned.len());
    let mut rest = cleaned;
    let mut blocked = 0;
    while let Some(start) = rest.find(&needle) {
        let url_start = start + needle.len();
        // Ammonia escapes `"` inside attribute values, so the next one closes it.
        let Some(len) = rest[url_start..].find('"') else {
            break;
        };
        out.push_str(&rest[..start]);
        out.push_str("src=\"\" data-mono-src=\"");
        out.push_str(&rest[url_start..url_start + len]);
        out.push('"');
        rest = &rest[url_start + len + 1..];
        blocked += 1;
    }
    out.push_str(rest);
    (out, blocked)
}

const ALLOWED_STYLE_PROPERTIES: &[&str] = &[
    "color",
    "background",
    "background-color",
    "font",
    "font-family",
    "font-size",
    "font-style",
    "font-weight",
    "font-variant",
    "line-height",
    "letter-spacing",
    "word-spacing",
    "text-align",
    "text-decoration",
    "text-indent",
    "text-transform",
    "vertical-align",
    "white-space",
    "direction",
    "width",
    "height",
    "min-width",
    "max-width",
    "min-height",
    "max-height",
    "margin",
    "margin-top",
    "margin-right",
    "margin-bottom",
    "margin-left",
    "padding",
    "padding-top",
    "padding-right",
    "padding-bottom",
    "padding-left",
    "border",
    "border-top",
    "border-right",
    "border-bottom",
    "border-left",
    "border-color",
    "border-style",
    "border-width",
    "border-collapse",
    "border-spacing",
    "border-radius",
    "list-style-type",
];

const FORBIDDEN_VALUE_WORDS: &[&str] = &[
    "expression",
    "behavior",
    "-moz-binding",
    "javascript:",
    "vbscript:",
    "fixed",
    "@import",
    "image-set",
    "\\",
    "/*",
];

/// Keeps only allowlisted properties with safe values. `None` if nothing is left.
fn filter_style(style: &str) -> Option<String> {
    let kept: Vec<String> = split_declarations(style)
        .into_iter()
        .filter_map(|decl| {
            let (property, value) = decl.split_once(':')?;
            let property = property.trim().to_ascii_lowercase();
            let value = value.trim();
            if !ALLOWED_STYLE_PROPERTIES.contains(&property.as_str()) || value.is_empty() {
                return None;
            }
            let lower = value.to_ascii_lowercase();
            if FORBIDDEN_VALUE_WORDS.iter().any(|w| lower.contains(w)) || !urls_are_inline(&lower) {
                return None;
            }
            Some(format!("{property}: {value}"))
        })
        .collect();
    (!kept.is_empty()).then(|| kept.join("; "))
}

/// True when every `url(...)` points at `data:` or `cid:`.
fn urls_are_inline(lower_value: &str) -> bool {
    let mut rest = lower_value;
    while let Some(pos) = rest.find("url(") {
        let arg = rest[pos + 4..].trim_start().trim_start_matches(['"', '\'']);
        if !(arg.starts_with("data:") || arg.starts_with("cid:")) {
            return false;
        }
        rest = &rest[pos + 4..];
    }
    true
}

/// Splits on `;` outside quotes and parentheses, because `url(data:...;base64,...)`
/// contains a semicolon of its own.
fn split_declarations(style: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let (mut depth, mut quote, mut start) = (0usize, None::<char>, 0usize);
    for (i, c) in style.char_indices() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), _) => {}
            (None, '"' | '\'') => quote = Some(c),
            (None, '(') => depth += 1,
            (None, ')') => depth = depth.saturating_sub(1),
            (None, ';') if depth == 0 => {
                parts.push(&style[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    parts.push(&style[start..]);
    parts.into_iter().filter(|p| !p.trim().is_empty()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &[u8] = include_bytes!("../tests/fixtures/mail/ses-hello.eml");

    fn html_mail(body: &str) -> ParsedMail {
        let raw = format!(
            "From: a@example.com\r\nTo: b@example.com\r\nSubject: t\r\n\
             Content-Type: text/html; charset=UTF-8\r\n\r\n{body}"
        );
        parse_mail(raw.as_bytes()).unwrap()
    }

    fn sanitised(body: &str) -> String {
        html_mail(body).body_html.unwrap_or_default()
    }

    fn alternative_mail(text: &str, html: &str) -> ParsedMail {
        let raw = format!(
            "From: a@example.com\r\nTo: b@example.com\r\nSubject: t\r\n\
             Content-Type: multipart/alternative; boundary=\"b\"\r\n\r\n\
             --b\r\nContent-Type: text/plain\r\n\r\n{text}\r\n\
             --b\r\nContent-Type: text/html\r\n\r\n{html}\r\n--b--\r\n"
        );
        parse_mail(raw.as_bytes()).unwrap()
    }

    #[test]
    fn deeply_nested_html_falls_back_to_text_without_running_the_sanitiser() {
        let html = "<div>".repeat(200_000);
        let start = std::time::Instant::now();
        let m = alternative_mail("the plain part", &html);
        assert!(
            start.elapsed() < std::time::Duration::from_secs(1),
            "took {:?}",
            start.elapsed()
        );
        let shown = m.body_html.unwrap();
        assert!(shown.contains(OVERSIZED_HTML_NOTICE));
        assert!(shown.contains("the plain part"));
        assert_eq!(m.body_text.as_deref(), Some("the plain part"));
    }

    #[test]
    fn deeply_nested_html_only_mail_keeps_a_notice_body() {
        let m = html_mail(&"<div><span>".repeat(100_000));
        assert_eq!(
            m.body_html.as_deref(),
            Some(format!("<p><em>{OVERSIZED_HTML_NOTICE}</em></p>").as_str())
        );
    }

    #[test]
    fn oversized_flat_html_falls_back() {
        let m = html_mail(&format!("<p>{}</p>", "x".repeat(MAX_HTML_BYTES)));
        assert!(m.body_html.unwrap().contains(OVERSIZED_HTML_NOTICE));
    }

    #[test]
    fn mismatched_end_tags_cannot_hide_depth() {
        assert!(html_exceeds_caps(&"<div></x>".repeat(MAX_HTML_DEPTH + 1)));
    }

    #[test]
    fn ordinary_html_is_unchanged_by_the_caps() {
        // Sloppy but real: unclosed paragraphs and list items, nested tables,
        // void tags, a long reply chain.
        let sloppy = format!(
            "{}<ul>{}</ul>{}<br><img src=\"cid:a\"><table><tr><td><table><tr><td>x</table></table>",
            "<p>para".repeat(2_000),
            "<li>item".repeat(2_000),
            "<blockquote><div>reply ".repeat(100) + &"</div></blockquote>".repeat(100),
        );
        assert!(!html_exceeds_caps(&sloppy));
        let body = "<p>Hello <b>world</b></p><table><tr><td>cell</td></tr></table>";
        assert_eq!(
            sanitised(body),
            "<p>Hello <b>world</b></p><table><tbody><tr><td>cell</td></tr></tbody></table>"
        );
        assert!(!sanitised(body).contains(OVERSIZED_HTML_NOTICE));
        assert!(!html_exceeds_caps(&"<div>".repeat(MAX_HTML_DEPTH)));
    }

    #[test]
    fn ses_fixture_parses() {
        let m = parse_mail(FIXTURE).unwrap();
        let from = m.from.as_ref().unwrap();
        assert_eq!(from.name.as_deref(), Some("Hoàng Xuân"));
        assert_eq!(from.address, "hoangxuan2402@gmail.com");
        assert_eq!(m.to.len(), 1);
        assert_eq!(m.to[0].address, "owner@byptah.com");
        assert_eq!(m.subject, "hello");
        assert_eq!(
            m.message_id.as_deref(),
            Some("CAJT68EYO51mq=D-_T3SQT2m3Yh4R6raprw7FwN0+ZsJ7_RCxxg@mail.gmail.com")
        );
        assert_eq!(m.body_text.as_deref().map(str::trim), Some("hello"));
        assert!(m.body_html.as_deref().unwrap().contains("hello"));
        assert_eq!(m.snippet, "hello");
        assert_eq!(m.remote_images_blocked, 0);
        assert!(m.attachments.is_empty() && !m.has_attachments());
        assert_eq!(m.size_bytes, FIXTURE.len());
        assert_eq!(m.spf.as_deref(), Some("pass"));
        assert_eq!(m.dkim.as_deref(), Some("pass"));
        assert_eq!(m.dmarc.as_deref(), Some("pass"));
        assert!(m.headers_json.contains("X-SES-RECEIPT"));
    }

    #[test]
    fn date_header_wins_over_received_and_divergence_is_recorded() {
        let m = parse_mail(FIXTURE).unwrap();
        // Date: Sun, 11 Oct 2026 00:57:56 +0700 = 2026-10-10T17:57:56Z
        assert_eq!(m.date_source, DateSource::Header);
        assert_eq!(m.date_utc, Some(1_791_655_076));
        // Topmost Received: Sat, 10 Oct 2026 18:29:07 +0000
        assert_eq!(m.received_at, Some(1_791_656_947));
        assert_ne!(m.date_utc, m.received_at);
    }

    #[test]
    fn missing_or_absurd_date_falls_back_to_received() {
        let tail = "Received: by x; Sat, 10 Oct 2026 18:29:07 +0000\r\n";
        for date in [
            "",
            "Date: Mon, 01 Jan 1970 00:00:00 +0000\r\n",
            "Date: nonsense\r\n",
        ] {
            let raw = format!("{tail}From: a@example.com\r\n{date}Subject: s\r\n\r\nbody");
            let m = parse_mail(raw.as_bytes()).unwrap();
            assert_eq!(m.date_source, DateSource::Received, "{date:?}");
            assert_eq!(m.date_utc, Some(1_791_656_947));
        }
        let raw = "From: a@example.com\r\nSubject: s\r\n\r\nbody";
        assert_eq!(
            parse_mail(raw.as_bytes()).unwrap().date_source,
            DateSource::None
        );
    }

    #[test]
    fn in_reply_to_is_first_then_references_without_duplicates() {
        let raw = "From: a@example.com\r\nMessage-ID: <c@x>\r\n\
                   In-Reply-To: <b@x>\r\n\
                   References: <a@x>\r\n <b@x> <z@y>\r\n\r\nhi";
        let m = parse_mail(raw.as_bytes()).unwrap();
        assert_eq!(m.in_reply_to.as_deref(), Some("b@x"));
        assert_eq!(m.referenced_ids, ["b@x", "a@x", "z@y"]);
    }

    #[test]
    fn encoded_word_names_and_groups_decode() {
        let raw = "From: =?UTF-8?B?SG/DoG5nIFh1w6Ju?= <h@example.com>\r\n\
                   To: =?UTF-8?Q?J=C3=BCrgen?= <j@example.com>, k@example.com\r\n\
                   Cc: team: p@example.com, q@example.com;\r\n\
                   Reply-To: r@example.com\r\n\
                   Subject: =?UTF-8?B?SG/DoG5nIFh1w6Ju?=\r\n\r\nhi";
        let m = parse_mail(raw.as_bytes()).unwrap();
        assert_eq!(m.subject, "Hoàng Xuân");
        assert_eq!(m.to[0].name.as_deref(), Some("Jürgen"));
        assert_eq!(m.to[1].name, None);
        assert_eq!(m.cc.len(), 2);
        assert_eq!(m.reply_to.unwrap().address, "r@example.com");
    }

    #[test]
    fn auth_results_ignore_comments_and_none() {
        let raw = "Authentication-Results: amazonses.com; spf=softfail (note: dkim=pass here) \
                   smtp.mailfrom=a; dkim=none; dmarc=fail header.from=x\r\n\
                   From: a@example.com\r\n\r\nhi";
        let m = parse_mail(raw.as_bytes()).unwrap();
        assert_eq!(m.spf.as_deref(), Some("softfail"));
        assert_eq!(m.dkim, None);
        assert_eq!(m.dmarc.as_deref(), Some("fail"));
        let raw =
            "Authentication-Results: amazonses.com; héé=1 dké=2 spf=pass\r\nFrom: a@example.com\r\n\r\nhi";
        assert_eq!(
            parse_mail(raw.as_bytes()).unwrap().spf.as_deref(),
            Some("pass")
        );
    }

    #[test]
    fn only_the_topmost_auth_header_counts() {
        let raw = "Authentication-Results: amazonses.com; spf=pass; dkim=none; dmarc=none\r\n\
                   Authentication-Results: x; dkim=pass; dmarc=pass; spf=fail\r\n\
                   From: a@example.com\r\n\r\nhi";
        let m = parse_mail(raw.as_bytes()).unwrap();
        assert_eq!(m.spf.as_deref(), Some("pass"));
        assert_eq!((m.dkim, m.dmarc), (None, None));
    }

    #[test]
    fn auth_header_from_another_authserv_is_ignored() {
        let raw = "Authentication-Results: evil.example; spf=pass; dkim=pass\r\n\
                   From: a@example.com\r\n\r\nhi";
        let m = parse_mail(raw.as_bytes()).unwrap();
        assert_eq!((m.spf, m.dkim, m.dmarc), (None, None, None));
    }

    #[test]
    fn auth_verdict_directly_before_a_comment_is_read() {
        let raw =
            "Authentication-Results: amazonses.com; spf=pass(ok) dkim=pass (c; dmarc=pass)\r\n\
                   From: a@example.com\r\n\r\nhi";
        let m = parse_mail(raw.as_bytes()).unwrap();
        assert_eq!(m.spf.as_deref(), Some("pass"));
        assert_eq!(m.dkim.as_deref(), Some("pass"));
        assert_eq!(m.dmarc, None);
    }

    #[test]
    fn sender_written_marker_scheme_is_dropped() {
        let m = html_mail(r#"<img src="monoblk-00000000000000000000000000000000:http://evil/x">"#);
        let out = m.body_html.unwrap();
        assert!(
            !out.contains("evil") && !out.contains("data-mono-src"),
            "{out}"
        );
        assert_eq!(m.remote_images_blocked, 0);
        // A text node may carry the needle; with a fresh nonce it cannot match.
        let m = html_mail(r#"<p>src="monoblk-0:http://evil/x"</p>"#);
        assert_eq!(m.remote_images_blocked, 0);
    }

    #[test]
    fn relative_images_are_blocked_and_counted_but_relative_links_vanish() {
        let m = html_mail(
            r#"<img src="//t.example/c.gif"><img src="/rel.png"><a href="/x">l</a><a href="page.html">m</a>"#,
        );
        let out = m.body_html.unwrap();
        assert_eq!(m.remote_images_blocked, 2, "{out}");
        assert!(
            out.contains(r#"data-mono-src="https://t.example/c.gif""#),
            "{out}"
        );
        assert!(!out.contains("href"), "{out}");
    }

    #[test]
    fn cite_urls_are_scheme_checked() {
        let out = sanitised(
            r#"<blockquote cite="javascript:x">a</blockquote><q cite="https://e.com/p">b</q>"#,
        );
        assert!(!out.contains("javascript"), "{out}");
        assert!(out.contains(r#"cite="https://e.com/p""#), "{out}");
    }

    #[test]
    fn plain_only_becomes_escaped_html() {
        let raw =
            "From: a@example.com\r\nContent-Type: text/plain\r\n\r\n1 < 2 & <b>x</b>\r\nline2";
        let m = parse_mail(raw.as_bytes()).unwrap();
        assert_eq!(
            m.body_html.as_deref(),
            Some("1 &lt; 2 &amp; &lt;b&gt;x&lt;/b&gt;<br>line2")
        );
        assert!(m.body_text.unwrap().contains("<b>x</b>"));
    }

    #[test]
    fn html_only_gets_snippet_without_style_or_markup() {
        let m = html_mail("<style>p{color:red}</style><p>Hello</p><p>a &amp; b</p>");
        assert_eq!(m.body_text, None);
        assert_eq!(m.snippet, "Hello a & b");
    }

    #[test]
    fn empty_body_has_no_html() {
        let raw = "From: a@example.com\r\nSubject: s\r\n\r\n";
        let m = parse_mail(raw.as_bytes()).unwrap();
        assert!(m.body_html.is_none() && m.snippet.is_empty());
    }

    #[test]
    fn attachments_record_metadata_and_inline_flag() {
        let raw = "From: a@example.com\r\nMIME-Version: 1.0\r\n\
            Content-Type: multipart/mixed; boundary=\"B\"\r\n\r\n\
            --B\r\nContent-Type: multipart/related; boundary=\"R\"\r\n\r\n\
            --R\r\nContent-Type: text/html\r\n\r\n<img src=\"cid:logo@x\">\r\n\
            --R\r\nContent-Type: image/png\r\nContent-ID: <logo@x>\r\n\
            Content-Transfer-Encoding: base64\r\n\r\niVBORw0KGgo=\r\n--R--\r\n\
            --B\r\nContent-Type: application/pdf; name=\"inv.pdf\"\r\n\
            Content-Disposition: attachment; filename=\"inv.pdf\"\r\n\
            Content-Transfer-Encoding: base64\r\n\r\nJVBERi0xLjQ=\r\n--B--\r\n";
        let m = parse_mail(raw.as_bytes()).unwrap();
        assert_eq!(m.attachments.len(), 2);
        let logo = &m.attachments[0];
        assert_eq!(logo.content_id.as_deref(), Some("logo@x"));
        assert!(logo.is_inline);
        assert_eq!(logo.mime_type, "image/png");
        let pdf = &m.attachments[1];
        assert_eq!(pdf.filename, "inv.pdf");
        assert_eq!(pdf.mime_type, "application/pdf");
        assert_eq!(pdf.size_bytes, 8);
        assert!(!pdf.is_inline && m.has_attachments());
        assert!(m.body_html.unwrap().contains("cid:logo@x"));
    }

    #[test]
    fn garbage_and_empty_input_return_an_error() {
        for bad in [
            &b""[..],
            b"\x00\x01\x02\xff\xfe",
            b"just some words with no headers",
        ] {
            let err = parse_mail(bad).unwrap_err();
            assert!(err.ends_with('.') && !err.is_empty());
        }
        // Truncated mid-header and mid-boundary must not panic either way.
        for cut in [10, 200, 900] {
            let _ = parse_mail(&FIXTURE[..cut.min(FIXTURE.len())]);
        }
    }

    #[test]
    fn script_removed_with_contents() {
        let out = sanitised("<p>a</p><script>alert(1)</script><p>b</p>");
        assert!(!out.contains("script") && !out.contains("alert"), "{out}");
        assert!(out.contains("<p>a</p>") && out.contains("<p>b</p>"));
    }

    #[test]
    fn event_handlers_removed() {
        let out =
            sanitised(r#"<img src="cid:a" onerror="alert(1)" onload=x><div onclick="x()">t</div>"#);
        assert!(
            !out.contains("onerror") && !out.contains("onclick") && !out.contains("onload"),
            "{out}"
        );
        assert!(out.contains("cid:a") && out.contains("t</div>"));
    }

    #[test]
    fn javascript_href_removed() {
        for href in [
            "javascript:alert(1)",
            " JaVa\tScript:alert(1)",
            "data:text/html,<b>x</b>",
            "/relative",
        ] {
            let out = sanitised(&format!(r#"<a href="{href}">x</a>"#));
            assert!(!out.contains("href"), "{href}: {out}");
        }
        let out = sanitised(r#"<a href="https://e.com/p?a=1&b=2" target="_blank">x</a>"#);
        assert!(
            out.contains(r#"href="https://e.com/p?a=1&amp;b=2""#),
            "{out}"
        );
        assert!(
            out.contains(r#"rel="noopener noreferrer nofollow""#),
            "{out}"
        );
        assert!(!out.contains("target"), "{out}");
        assert!(sanitised(r#"<a href="mailto:a@b.c">x</a>"#).contains("mailto:a@b.c"));
    }

    #[test]
    fn style_svg_form_and_friends_removed_with_contents() {
        let out = sanitised(
            "<style>body{background:url(http://t/x)}</style><svg><circle/><text>SVGTEXT</text></svg>\
             <form action=x><input name=q><button>BTN</button></form><iframe src=x>IFR</iframe>\
             <object data=x>OBJ</object><embed src=x><link rel=stylesheet href=http://t/c>\
             <meta http-equiv=refresh content=0><base href=http://t/><p>keep</p>",
        );
        for gone in [
            "style", "SVGTEXT", "svg", "form", "input", "BTN", "IFR", "OBJ", "embed", "link",
            "meta", "base", "http://t",
        ] {
            assert!(!out.contains(gone), "{gone} survived: {out}");
        }
        assert!(out.contains("<p>keep</p>"));
    }

    #[test]
    fn remote_images_are_rewritten_and_counted() {
        let m = html_mail(
            r#"<img src="https://t.example/a.png?x=1&y=2" alt="A"><img src="HTTP://t.example/b.gif">
               <img src="cid:keep@x"><img src="data:image/png;base64,AAAA">
               <img src="data:image/svg+xml,<svg/>"><img src="javascript:x"><img src="/rel.png">"#,
        );
        let out = m.body_html.unwrap();
        assert_eq!(m.remote_images_blocked, 3, "{out}");
        assert!(
            out.contains(r#"src="" data-mono-src="https://t.example/a.png?x=1&amp;y=2""#),
            "{out}"
        );
        assert!(
            out.contains(r#"data-mono-src="HTTP://t.example/b.gif""#),
            "{out}"
        );
        assert!(out.contains(r#"src="cid:keep@x""#), "{out}");
        assert!(out.contains(r#"src="data:image/png;base64,AAAA""#), "{out}");
        assert!(
            !out.contains("svg")
                && !out.contains("javascript")
                && out.contains(r#"src="" data-mono-src="/rel.png""#),
            "{out}"
        );
        assert!(!out.contains("monoblk"), "{out}");
    }

    #[test]
    fn sender_cannot_forge_data_mono_src() {
        let out = sanitised(r#"<img src="cid:a" data-mono-src="http://evil/x">"#);
        assert!(
            !out.contains("data-mono-src") && !out.contains("evil"),
            "{out}"
        );
    }

    #[test]
    fn style_attribute_keeps_only_safe_properties() {
        let out = sanitised(
            r#"<p style="color:#333; position:fixed; z-index:9; font-size:14px; width:expression(alert(1));
               behavior:url(x.htc); -moz-binding:url(x); margin:0 auto; display:flex;
               background:url(http://t/x.png); border:1px solid red">t</p>"#,
        );
        assert!(out.contains("color:#333"), "{out}");
        assert!(
            out.contains("font-size:14px") && out.contains("margin:0 auto"),
            "{out}"
        );
        assert!(out.contains("border:1px solid red"), "{out}");
        for gone in [
            "position",
            "z-index",
            "expression",
            "behavior",
            "binding",
            "display",
            "http://t",
        ] {
            assert!(!out.contains(gone), "{gone} survived: {out}");
        }
    }

    #[test]
    fn style_attribute_allows_inline_urls_and_blocks_escapes() {
        let out = sanitised(
            r#"<p style="background:url(data:image/png;base64,AAAA) no-repeat; color:\72 ed; width:50%">t</p>"#,
        );
        assert!(out.contains("data:image/png;base64,AAAA"), "{out}");
        assert!(out.contains("width:50%") && !out.contains("72 ed"), "{out}");
        let out = sanitised(r#"<p style="position:fixed">t</p>"#);
        assert!(!out.contains("style"), "{out}");
    }

    #[test]
    fn table_layout_survives() {
        let src = r##"<table width="600" cellpadding="0" cellspacing="0" border="0" bgcolor="#fff"><colgroup><col width="300"></colgroup><thead><tr><th align="left">H</th></tr></thead><tbody><tr><td colspan="2" valign="top" style="padding:8px">C</td></tr></tbody></table>"##;
        let out = sanitised(src);
        assert_eq!(out, src);
    }
}
