//! A parser for the accessibility snapshot text of `@playwright/mcp` (YAML-like lines such as
//! `- link "Releases" [ref=e12] [cursor=pointer]:` with `- /url: ...` properties and
//! `- text: ...` leaves), and the matching and field reading the recipe extractors need.
//!
//! The runner only parses what a scoped probe returned; the text never goes to the model.
use super::model::{Each, Field, Source};
use regex::Regex;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Node {
    pub role: String,
    pub name: String,
    /// `ref=e12`, `level=2`, `checked`, ... without the brackets.
    pub attrs: Vec<String>,
    /// Text after the colon (`- paragraph [ref=e3]: Some text`) or of a `- text:` leaf.
    pub text: String,
    /// The `/url:` property of a link.
    pub url: Option<String>,
    pub children: Vec<Node>,
}

impl Node {
    pub fn reference(&self) -> Option<&str> {
        self.attrs.iter().find_map(|a| a.strip_prefix("ref="))
    }
    /// All the text of the node: its own, else its children's joined by spaces, else its name.
    pub fn text_content(&self) -> String {
        if !self.text.is_empty() {
            return self.text.clone();
        }
        let inner: Vec<String> = self
            .children
            .iter()
            .map(Node::text_content)
            .filter(|t| !t.is_empty())
            .collect();
        if inner.is_empty() {
            self.name.clone()
        } else {
            inner.join(" ")
        }
    }
    /// This node and every node below it, depth first.
    pub fn walk<'a>(&'a self, out: &mut Vec<&'a Node>) {
        out.push(self);
        for child in &self.children {
            child.walk(out);
        }
    }
}

/// The page address and title a tool result reports.
pub fn page_header(text: &str) -> (String, String) {
    let find = |key: &str| {
        text.lines()
            .find_map(|line| {
                line.trim()
                    .trim_start_matches("- ")
                    .strip_prefix(key)
                    .map(|v| v.trim().to_owned())
            })
            .unwrap_or_default()
    };
    (find("Page URL:"), find("Page Title:"))
}

/// The snapshot body of a tool result: the fenced `yaml` block, or the whole text when there is
/// no fence (a saved file).
pub fn snapshot_body(text: &str) -> &str {
    if let Some(start) = text.find("```yaml") {
        let rest = &text[start + "```yaml".len()..];
        let rest = rest.strip_prefix('\n').unwrap_or(rest);
        return rest.split("\n```").next().unwrap_or(rest);
    }
    text
}

enum Line {
    Node(Node),
    Prop(String, String),
}

fn unquote(text: &str) -> String {
    let text = text.trim();
    if text.len() >= 2 && text.starts_with('"') && text.ends_with('"') {
        return serde_json::from_str::<String>(text)
            .unwrap_or_else(|_| text[1..text.len() - 1].to_owned());
    }
    if text.len() >= 2 && text.starts_with('\'') && text.ends_with('\'') {
        return text[1..text.len() - 1].replace("''", "'");
    }
    text.to_owned()
}

/// Reads a JSON-style quoted string at the start of `text`: its value and what follows.
fn quoted(text: &str) -> Option<(String, &str)> {
    let mut escaped = false;
    for (index, c) in text.char_indices().skip(1) {
        match c {
            '\\' if !escaped => escaped = true,
            '"' if !escaped => {
                let value = serde_json::from_str::<String>(&text[..=index])
                    .unwrap_or_else(|_| text[1..index].to_owned());
                return Some((value, &text[index + 1..]));
            }
            _ => escaped = false,
        }
    }
    None
}

fn parse_line(content: &str) -> Line {
    if let Some(prop) = content.strip_prefix('/') {
        let (key, value) = prop.split_once(':').unwrap_or((prop, ""));
        return Line::Prop(key.trim().to_owned(), unquote(value));
    }
    if let Some(rest) = content.strip_prefix("text:") {
        return Line::Node(Node {
            role: "text".into(),
            text: unquote(rest),
            ..Node::default()
        });
    }
    let role_end = content.find([' ', ':', '[', '"']).unwrap_or(content.len());
    let mut node = Node {
        role: content[..role_end].to_owned(),
        ..Node::default()
    };
    let mut rest = content[role_end..].trim_start();
    if rest.starts_with('"') {
        if let Some((name, after)) = quoted(rest) {
            node.name = name;
            rest = after.trim_start();
        }
    }
    while rest.starts_with('[') {
        let Some(end) = rest.find(']') else { break };
        node.attrs.push(rest[1..end].to_owned());
        rest = rest[end + 1..].trim_start();
    }
    if let Some(inline) = rest.strip_prefix(':') {
        node.text = unquote(inline);
    }
    Line::Node(node)
}

/// The nodes at `indent` (with their children), and the `/url` property found among them.
fn build(lines: &[(usize, Line)], pos: &mut usize, indent: usize) -> (Vec<Node>, Option<String>) {
    let mut out = Vec::new();
    let mut url = None;
    while *pos < lines.len() {
        let level = lines[*pos].0;
        if level < indent {
            break;
        }
        match &lines[*pos].1 {
            Line::Prop(key, value) => {
                if key == "url" {
                    url = Some(value.clone());
                }
                *pos += 1;
            }
            Line::Node(node) => {
                let mut node = node.clone();
                *pos += 1;
                if *pos < lines.len() && lines[*pos].0 > level {
                    let child_indent = lines[*pos].0;
                    let (children, own_url) = build(lines, pos, child_indent);
                    node.children = children;
                    node.url = own_url;
                }
                out.push(node);
            }
        }
    }
    (out, url)
}

/// Parses the lines of a snapshot into a forest.
pub fn parse(body: &str) -> Vec<Node> {
    let lines: Vec<(usize, Line)> = body
        .lines()
        .filter_map(|raw| {
            let trimmed = raw.trim_start();
            let content = trimmed.strip_prefix("- ")?;
            let indent = raw.len() - trimmed.len();
            Some((indent, parse_line(content.trim_end())))
        })
        .collect();
    let mut pos = 0;
    let first = lines.first().map_or(0, |l| l.0);
    build(&lines, &mut pos, first).0
}

/// Every node of a forest, depth first.
pub fn all(nodes: &[Node]) -> Vec<&Node> {
    let mut out = Vec::new();
    for node in nodes {
        node.walk(&mut out);
    }
    out
}

// ---- Extraction ----

fn matches_text(node: &Node, pattern: &Regex) -> bool {
    pattern.is_match(&node.name)
        || pattern.is_match(&node.text)
        || pattern.is_match(&node.text_content())
}

/// The nodes `each` picks out of a forest. No `each`: the top nodes.
pub fn items<'a>(
    forest: &'a [Node],
    each: Option<&Each>,
    values: &std::collections::BTreeMap<String, String>,
) -> Result<Vec<&'a Node>, String> {
    let Some(each) = each else {
        return Ok(forest.iter().collect());
    };
    let compile = |p: &str| {
        let filled = super::model::substitute(p, values, super::model::Encode::Regex);
        regex::RegexBuilder::new(&filled)
            .size_limit(1 << 20)
            .build()
            .map_err(|_| "A pattern in the extractor is not valid.".to_owned())
    };
    let has = each
        .has
        .as_ref()
        .map(|h| compile(&h.matches).map(|re| (h.role.clone(), re)))
        .transpose()?;
    let not_name = each.not_name_matches.as_deref().map(compile).transpose()?;
    let mut matched: Vec<&Node> = all(forest)
        .into_iter()
        .filter(|node| each.role.as_deref().is_none_or(|r| node.role == r))
        .filter(|node| {
            has.as_ref().is_none_or(|(role, re)| {
                let mut inside = Vec::new();
                for child in &node.children {
                    child.walk(&mut inside);
                }
                inside
                    .into_iter()
                    .any(|n| role.as_deref().is_none_or(|r| n.role == r) && matches_text(n, re))
            })
        })
        .filter(|node| not_name.as_ref().is_none_or(|re| !re.is_match(&node.name)))
        .collect();
    if each.innermost {
        let snapshot = matched.clone();
        matched.retain(|node| {
            !snapshot.iter().any(|other| {
                !std::ptr::eq(*other, *node) && {
                    let mut below = Vec::new();
                    for child in &node.children {
                        child.walk(&mut below);
                    }
                    below.iter().any(|n| std::ptr::eq(*n, *other))
                }
            })
        });
    }
    Ok(matched)
}

/// One field of one item, cut at `max` characters.
pub fn field_value(item: &Node, field: &Field, max: usize) -> Result<Option<String>, String> {
    let mut nodes = Vec::new();
    item.walk(&mut nodes);
    let target = match &field.role {
        None => Some(item),
        Some(role) => nodes
            .into_iter()
            .filter(|n| &n.role == role)
            .nth(field.nth as usize),
    };
    let Some(target) = target else {
        return Ok(None);
    };
    let raw = match field.from {
        Source::Name => target.name.clone(),
        Source::Text => target.text_content(),
        Source::Url => target.url.clone().unwrap_or_default(),
    };
    let raw = match &field.regex {
        Some(pattern) => {
            let re = regex::RegexBuilder::new(pattern)
                .size_limit(1 << 20)
                .build()
                .map_err(|_| "A field pattern is not valid.".to_owned())?;
            match re.captures(&raw) {
                Some(caps) => caps
                    .get(1)
                    .or_else(|| caps.get(0))
                    .map_or(String::new(), |m| m.as_str().to_owned()),
                None => String::new(),
            }
        }
        None => raw,
    };
    let collapsed = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    Ok(Some(collapsed.chars().take(max).collect()))
}
