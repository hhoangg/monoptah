use super::*;

/// Every (text, link) pair in a tree's first block, flattening lists.
fn links(source: &str) -> Vec<(String, Option<String>)> {
    fn walk(block: &Block, out: &mut Vec<(String, Option<String>)>) {
        let push = |runs: &[InlineRun], out: &mut Vec<_>| {
            for run in runs {
                out.push((run.text.clone(), run.style.link.clone()));
            }
        };
        match block {
            Block::Paragraph { runs } | Block::Heading { runs, .. } => push(runs, out),
            Block::List { items, .. } => items.iter().flatten().for_each(|b| walk(b, out)),
            Block::BlockQuote { children } => children.iter().for_each(|b| walk(b, out)),
            Block::Table { header, rows, .. } => {
                header.iter().for_each(|c| push(c, out));
                rows.iter().flatten().for_each(|c| push(c, out));
            }
            _ => {}
        }
    }
    let tree = parse_full(source);
    let mut out = Vec::new();
    tree.blocks.iter().for_each(|b| walk(&b.block, &mut out));
    out
}

fn linked(source: &str) -> Vec<String> {
    links(source).into_iter().filter_map(|(_, l)| l).collect()
}

#[test]
fn explicit_markdown_links_and_autolinks_are_links() {
    assert_eq!(
        linked("Read [docs](https://example.com/a)."),
        ["https://example.com/a"]
    );
    assert_eq!(
        linked("Read <https://example.com/b> now"),
        ["https://example.com/b"]
    );
}

#[test]
fn bare_urls_in_prose_are_links() {
    assert_eq!(
        linked("See https://github.com/acme/app/pull/12 for details."),
        ["https://github.com/acme/app/pull/12"]
    );
}

#[test]
fn bare_urls_in_every_block_kind_are_links() {
    for source in [
        "- item https://a.com/x here",
        "1. item https://a.com/x here",
        "| h | i |\n| - | - |\n| https://a.com/x | y |",
        "> quoted https://a.com/x",
        "# Title https://a.com/x",
        "- [x] done https://a.com/x",
    ] {
        assert_eq!(linked(source), ["https://a.com/x"], "{source}");
    }
}

#[test]
fn a_bare_url_keeps_the_text_around_it_and_drops_trailing_punctuation() {
    assert_eq!(
        links("see https://a.com/x, then"),
        [
            ("see ".to_owned(), None),
            (
                "https://a.com/x".to_owned(),
                Some("https://a.com/x".to_owned())
            ),
            (", then".to_owned(), None),
        ]
    );
    assert_eq!(linked("end https://a.com/x."), ["https://a.com/x"]);
    assert_eq!(linked("(https://a.com/x)"), ["https://a.com/x"]);
}

#[test]
fn two_bare_urls_in_one_paragraph_are_two_links() {
    assert_eq!(
        linked("https://a.com/1 and https://b.org/2\nhttps://c.net/3"),
        ["https://a.com/1", "https://b.org/2", "https://c.net/3"]
    );
}

#[test]
fn urls_in_code_stay_code() {
    assert_eq!(linked("run `https://a.com/x` now"), Vec::<String>::new());
    assert_eq!(linked("```\nhttps://a.com/x\n```"), Vec::<String>::new());
    assert_eq!(linked("math $https://a.com/x$ here"), Vec::<String>::new());
}

#[test]
fn a_url_next_to_bold_keeps_the_bold_and_is_a_link() {
    let runs = links("**bold** https://a.com/x and *https://b.org/y*");
    let urls: Vec<_> = runs.iter().filter_map(|(_, l)| l.clone()).collect();
    assert_eq!(urls, ["https://a.com/x", "https://b.org/y"]);
    let tree = parse_full("**bold** https://a.com/x and *https://b.org/y*");
    let Block::Paragraph { runs } = &tree.blocks[0].block else {
        panic!("paragraph");
    };
    assert!(runs[0].style.bold && runs[0].style.link.is_none());
    let italic = runs.iter().find(|r| r.text == "https://b.org/y").unwrap();
    assert!(italic.style.italic);
}

#[test]
fn explicit_link_text_that_is_a_url_is_not_split_again() {
    assert_eq!(
        links("[https://a.com/x](https://a.com/x) after"),
        [
            (
                "https://a.com/x".to_owned(),
                Some("https://a.com/x".to_owned())
            ),
            (" after".to_owned(), None),
        ]
    );
    assert_eq!(
        linked("[see https://a.com/x](https://b.org/y)"),
        ["https://b.org/y"]
    );
}

#[test]
fn other_schemes_stay_plain() {
    assert_eq!(linked("get ftp://a.com/x now"), Vec::<String>::new());
    assert_eq!(linked("mail mailto:a@b.c now"), Vec::<String>::new());
}

#[test]
fn an_uppercase_scheme_is_a_link_like_in_user_messages() {
    assert_eq!(
        linked("See HTTPS://Example.com/x now"),
        ["https://example.com/x"]
    );
}

#[test]
fn a_streaming_link_label_is_not_linked_before_its_target_arrives() {
    let mut parser = MarkdownParser::new();
    parser.set_text("[see https://a.com/x");
    let tree = parser.display_tree();
    let Block::Paragraph { runs } = &tree.blocks[0].block else {
        panic!("expected paragraph")
    };
    assert!(runs.iter().all(|run| run.style.link.is_none()), "{runs:?}");
    parser.set_text("[see https://a.com/x](https://b.org/y)");
    let tree = parser.display_tree();
    let Block::Paragraph { runs } = &tree.blocks[0].block else {
        panic!("expected paragraph")
    };
    let targets: Vec<_> = runs.iter().filter_map(|r| r.style.link.clone()).collect();
    assert_eq!(targets, ["https://b.org/y"]);
}
