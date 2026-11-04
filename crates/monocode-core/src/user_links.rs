//! The first web link in a user message, following sessions/model/linkPreview.ts.
use std::{ops::Range, sync::LazyLock};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UserLink {
    /// The parsed, normalized address (`URL.href`).
    pub url: String,
    /// Lowercase host without `www.`.
    pub host: String,
    pub display_url: String,
    pub github_work_item: Option<GithubWorkItemLink>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GithubWorkItemLink {
    /// "pr" or "issue".
    pub kind: &'static str,
    pub repo: String,
    pub number: u64,
}

/// parseUserMessageLink: the link, and the bytes of `text` it replaces.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UserMessageLink {
    pub link: UserLink,
    pub range: Range<usize>,
}

/// Find the first web URL and preserve the rest of the user's message.
pub fn parse_user_message_link(text: &str) -> Option<UserMessageLink> {
    find_web_links(text).into_iter().next()
}

/// Every http(s) link in `text`, in order, with the bytes each one occupies.
/// Surrounding punctuation is left out of the range.
pub fn find_web_links(text: &str) -> Vec<UserMessageLink> {
    static URL: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r#"(?i)https?://[^\s<>"']+"#).unwrap());
    URL.find_iter(text)
        .filter_map(|found| {
            let value = trim_url_punctuation(found.as_str());
            let link = parse_http_url(value)?;
            Some(UserMessageLink {
                link,
                range: found.start()..found.start() + value.len(),
            })
        })
        .collect()
}

fn parse_http_url(value: &str) -> Option<UserLink> {
    let parsed = url::Url::parse(value).ok()?;
    if !matches!(parsed.scheme(), "https" | "http")
        || !parsed.username().is_empty()
        || parsed.password().is_some()
    {
        return None;
    }
    let host = parsed
        .host_str()
        .filter(|host| !host.is_empty())?
        .trim_end_matches('.')
        .to_ascii_lowercase();
    let display_host = host.strip_prefix("www.").unwrap_or(&host).to_owned();
    let path = if parsed.path() == "/" {
        ""
    } else {
        parsed.path()
    };
    let search = parsed.query().map(|q| format!("?{q}")).unwrap_or_default();
    let hash = parsed
        .fragment()
        .map(|f| format!("#{f}"))
        .unwrap_or_default();
    Some(UserLink {
        url: parsed.as_str().to_owned(),
        display_url: format!("{display_host}{path}{search}{hash}"),
        github_work_item: github_work_item(&parsed, &display_host),
        host: display_host,
    })
}

fn github_work_item(url: &url::Url, display_host: &str) -> Option<GithubWorkItemLink> {
    if display_host != "github.com" {
        return None;
    }
    let parts: Vec<&str> = url.path().split('/').filter(|p| !p.is_empty()).collect();
    if parts.len() < 4 {
        return None;
    }
    let kind = match parts[2].to_ascii_lowercase().as_str() {
        "pull" => "pr",
        "issues" => "issue",
        _ => return None,
    };
    let digits = parts[3];
    if digits.starts_with('0') || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    // Number.isSafeInteger.
    let number = digits.parse::<u64>().ok().filter(|&n| n < 1 << 53)?;
    let owner = decode_path_part(parts[0])?;
    let name = decode_path_part(parts[1])?;
    let valid =
        |part: &str| !part.is_empty() && !part.chars().any(|c| c.is_whitespace() || c == '/');
    if !valid(&owner) || !valid(&name) {
        return None;
    }
    Some(GithubWorkItemLink {
        kind,
        repo: format!("{owner}/{name}"),
        number,
    })
}

/// decodeURIComponent, which rejects malformed escapes and invalid UTF-8.
fn decode_path_part(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = value.get(i + 1..i + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

fn trim_url_punctuation(value: &str) -> &str {
    let mut value = value.trim_end_matches(['.', ',', '!', ';', ':']);
    for (opening, closing) in [('(', ')'), ('[', ']'), ('{', '}')] {
        while value.ends_with(closing)
            && value.matches(closing).count() > value.matches(opening).count()
        {
            value = &value[..value.len() - 1];
        }
    }
    value
}

/// UserLinkPreview's plainTextSummary: a GitHub body as one run of prose.
pub fn plain_text_summary(value: &str) -> String {
    static STEPS: LazyLock<[(regex::Regex, &str); 5]> = LazyLock::new(|| {
        [
            (r"!\[[^\]]*\]\([^)]*\)", ""),
            (r"\[([^\]]+)\]\([^)]*\)", "$1"),
            (r"<[^>]+>", " "),
            (r"[`*_>#~|-]+", " "),
            (r"\s+", " "),
        ]
        .map(|(re, with)| (regex::Regex::new(re).unwrap(), with))
    });
    let mut text = value.to_owned();
    for (re, with) in STEPS.iter() {
        text = re.replace_all(&text, *with).into_owned();
    }
    text.trim().to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spans(text: &str) -> Vec<&str> {
        find_web_links(text)
            .iter()
            .map(|found| &text[found.range.clone()])
            .collect()
    }

    #[test]
    fn every_link_in_a_text_is_found_in_order() {
        let text = "a https://a.com/x then http://b.org, and https://c.net/y?q=1!";
        assert_eq!(
            spans(text),
            ["https://a.com/x", "http://b.org", "https://c.net/y?q=1"]
        );
        assert_eq!(find_web_links("no links here"), []);
    }

    #[test]
    fn links_drop_trailing_punctuation() {
        for punct in [".", ",", "!", ";", ":"] {
            let text = format!("see https://a.com/x{punct} next");
            assert_eq!(spans(&text), ["https://a.com/x"], "{punct}");
        }
    }

    #[test]
    fn parentheses_are_balanced_around_links() {
        assert_eq!(spans("(see https://a.com/x)"), ["https://a.com/x"]);
        assert_eq!(
            spans("https://en.wikipedia.org/wiki/Foo_(bar)"),
            ["https://en.wikipedia.org/wiki/Foo_(bar)"]
        );
        assert_eq!(
            spans("(https://en.wikipedia.org/wiki/Foo_(bar))."),
            ["https://en.wikipedia.org/wiki/Foo_(bar)"]
        );
    }

    #[test]
    fn only_http_and_https_are_links() {
        for text in [
            "ftp://a.com/x",
            "mailto:me@a.com",
            "javascript:alert(1)",
            "file:///etc/hosts",
        ] {
            assert_eq!(spans(text), Vec::<&str>::new(), "{text}");
        }
        assert_eq!(spans("HTTPS://A.com/X"), ["HTTPS://A.com/X"]);
    }

    #[test]
    fn several_pull_request_urls_are_each_found_in_every_layout() {
        let pr = |n: u32| format!("https://github.com/a/b/pull/{n}");
        assert_eq!(
            spans(&format!("{} {}", pr(1), pr(2))),
            [pr(1).as_str(), pr(2).as_str()]
        );
        assert_eq!(
            spans(&format!("{}\n{}", pr(1), pr(2))),
            [pr(1).as_str(), pr(2).as_str()]
        );
        assert_eq!(
            spans("see https://github.com/a/b/pull/1, and https://github.com/a/b/pull/2."),
            [
                "https://github.com/a/b/pull/1",
                "https://github.com/a/b/pull/2"
            ]
        );
        let text = "https://github.com/a/b/pull/3/ https://github.com/a/b/pull/4/files";
        assert_eq!(
            spans(text),
            [
                "https://github.com/a/b/pull/3/",
                "https://github.com/a/b/pull/4/files"
            ]
        );
        let found = find_web_links(text);
        assert!(found.iter().all(|f| f.link.github_work_item.is_some()));
        let repos = find_web_links("https://github.com/a/b/pull/1 https://github.com/c/d/issues/2");
        assert_eq!(repos.len(), 2);
        assert_eq!(repos[1].link.github_work_item.as_ref().unwrap().repo, "c/d");
        assert_eq!(spans("https://example.com/x"), ["https://example.com/x"]);
        assert_eq!(spans("ftp://x mailto:a@b.c"), Vec::<&str>::new());
    }

    #[test]
    fn the_first_link_is_the_first_found_link() {
        let text = "x https://a.com y https://b.com";
        assert_eq!(
            parse_user_message_link(text),
            find_web_links(text).into_iter().next()
        );
    }

    #[test]
    fn the_first_link_keeps_the_text_around_it() {
        let text = "hey lets review this pr\n\nhttps://github.com/hhoangg/monoptah/pull/1142 its for the native one";
        let found = parse_user_message_link(text).unwrap();
        assert_eq!(&text[..found.range.start], "hey lets review this pr\n\n");
        assert_eq!(&text[found.range.end..], " its for the native one");
        assert_eq!(
            found.link.github_work_item,
            Some(GithubWorkItemLink {
                kind: "pr",
                repo: "hhoangg/monoptah".into(),
                number: 1142,
            })
        );
        assert_eq!(found.link.host, "github.com");
    }

    #[test]
    fn trailing_punctuation_and_unbalanced_brackets_stay_text() {
        let text = "(see https://www.Example.com/a_(b)/c?q=1#x).";
        let found = parse_user_message_link(text).unwrap();
        assert_eq!(
            &text[found.range.clone()],
            "https://www.Example.com/a_(b)/c?q=1#x"
        );
        assert_eq!(found.link.host, "example.com");
        assert_eq!(found.link.display_url, "example.com/a_(b)/c?q=1#x");
        assert_eq!(found.link.github_work_item, None);
    }

    #[test]
    fn only_numbered_github_issues_and_pulls_are_work_items() {
        let item = |text: &str| parse_user_message_link(text).and_then(|f| f.link.github_work_item);
        assert_eq!(
            item("https://GitHub.com/a/b/Issues/7/").map(|i| (i.kind, i.number)),
            Some(("issue", 7))
        );
        assert_eq!(
            item("https://github.com/a/b/pull/12/files").map(|i| i.repo),
            Some("a/b".into())
        );
        for text in [
            "https://github.com/a/b/pull/0",
            "https://github.com/a/b/pull/012",
            "https://github.com/a/b/pull/12abc",
            "https://github.com/a/b/commit/12",
            "https://github.com/a/b/pull",
            "https://gist.github.com/a/b/pull/1",
            "https://github.com/a%2Fx/b/pull/1",
            "https://github.com/a%ZZ/b/pull/1",
        ] {
            assert_eq!(item(text), None, "{text}");
        }
        assert_eq!(
            parse_user_message_link("https://user:pw@github.com/a/b/pull/1"),
            None
        );
        assert_eq!(parse_user_message_link("no links here"), None);
    }

    #[test]
    fn summaries_drop_markdown_syntax() {
        assert_eq!(
            plain_text_summary(
                "## Summary\n\n![shot](a.png) Fixes [the bug](https://x.y) in `foo_bar`.\n- <b>one</b>\n> two"
            ),
            "Summary Fixes the bug in foo bar . one two"
        );
    }
}
