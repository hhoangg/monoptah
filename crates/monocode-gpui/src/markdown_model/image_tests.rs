use super::*;

fn document(source: &str) -> Vec<Block> {
    let mut parser = MarkdownParser::new();
    parser.set_images(true);
    parser.set_text(source);
    parser.finish_display();
    parser
        .tree()
        .blocks
        .iter()
        .map(|top| top.block.clone())
        .collect()
}

fn alt_text(runs: &[InlineRun]) -> String {
    runs.iter().map(|run| run.text.as_str()).collect()
}

#[test]
fn chat_parsing_keeps_an_ordinary_image_as_its_alt_text() {
    let mut parser = MarkdownParser::new();
    parser.set_text("![diagram](img/a.png)");
    parser.finish_display();
    let tree = parser.tree().clone();
    assert!(
        tree.blocks
            .iter()
            .all(|top| !matches!(top.block, Block::Image { .. }))
    );
    let Block::Paragraph { runs } = &tree.blocks[0].block else {
        panic!("expected a paragraph");
    };
    assert_eq!(alt_text(runs), "diagram");
    for block in &parse_full("![diagram](img/a.png)").blocks {
        assert!(matches!(block.block, Block::Paragraph { .. }));
    }
}

#[test]
fn document_parsing_places_local_images_and_keeps_the_prose_after_them() {
    let blocks = document("![one](img/a.png)\n![two](/abs/b.svg)\n\nText ![inline](c.png) here\n");
    assert_eq!(
        blocks[..2],
        [
            Block::Image {
                src: "img/a.png".into(),
                alt: "one".into()
            },
            Block::Image {
                src: "/abs/b.svg".into(),
                alt: "two".into()
            },
        ]
    );
    // An image in the middle of prose stays prose in a document too.
    let Block::Paragraph { runs } = &blocks[2] else {
        panic!("expected a paragraph");
    };
    assert_eq!(alt_text(runs), "Text inline here");
}

#[test]
fn document_parsing_never_loads_a_remote_image_and_shows_it_as_a_link() {
    for url in [
        "https://example.com/a.png",
        "HTTP://example.com/a.png",
        "data:image/png;base64,AAAA",
    ] {
        let blocks = document(&format!("![logo]({url})"));
        assert_eq!(blocks.len(), 1);
        let Block::Paragraph { runs } = &blocks[0] else {
            panic!("expected a link paragraph for {url}");
        };
        assert_eq!(alt_text(runs), "logo");
        assert_eq!(runs[0].style.link.as_deref(), Some(url));
    }
}

#[test]
fn a_placed_visual_still_wins_in_a_document() {
    let blocks = document("![chart](visual:abc_1)");
    assert!(matches!(blocks[0], Block::Visual { .. }));
}
