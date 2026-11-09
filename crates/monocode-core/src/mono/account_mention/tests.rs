use super::*;

fn book() -> (Book, Account, Account, Account) {
    let mut book = Book::default();
    let can = book.add(Kind::Claude, "can.le").unwrap();
    let work = book.add(Kind::Claude, "Work account").unwrap();
    let codex = book.add(Kind::Codex, "hoangxuan2402").unwrap();
    (book, can, work, codex)
}
fn none(_: &str) -> Option<ClaudeCredential> {
    None
}

#[test]
fn mention_text_quotes_labels_outside_the_plain_set() {
    assert_eq!(mention_text("claude", "can.le"), "@claude/can.le");
    assert_eq!(mention_text("codex", "a_b-1"), "@codex/a_b-1");
    assert_eq!(
        mention_text("claude", "Work account"),
        "@\"claude/Work account\""
    );
    assert_eq!(mention_text("claude", "Café"), "@\"claude/Café\"");
    assert_eq!(mention_text("claude", "a\"b"), "@\"claude/a\\\"b\"");
}

#[test]
fn mentions_resolve_labels_ids_and_quoted_labels() {
    let (book, can, work, codex) = book();
    let ids = |text: &str| -> Vec<String> {
        mentioned_accounts(text, &book)
            .into_iter()
            .map(|(_, a)| a.id)
            .collect()
    };
    assert_eq!(ids("use @claude/can.le now"), [can.id.clone()]);
    assert_eq!(ids("use @claude/CAN.LE now"), [can.id.clone()]);
    assert_eq!(ids("use @\"claude/Work account\" now"), [work.id.clone()]);
    assert_eq!(ids(&format!("@claude/{}", work.id)), [work.id.clone()]);
    assert_eq!(ids("@codex/hoangxuan2402"), [codex.id.clone()]);
    assert_eq!(ids("@claude/default"), ["default"]);
    // Trailing punctuation does not belong to the label.
    assert_eq!(
        ids("ask @claude/can.le, then @codex/hoangxuan2402."),
        [can.id.clone(), codex.id.clone()]
    );
    // Once each, in order.
    assert_eq!(
        ids("@codex/hoangxuan2402 @claude/can.le @codex/hoangxuan2402"),
        [codex.id.clone(), can.id.clone()]
    );
}

#[test]
fn only_resolving_account_mentions_count() {
    let (book, _, _, _) = book();
    let ids = |text: &str| mentioned_accounts(text, &book).len();
    // A real folder named like a provider stays a file mention.
    assert_eq!(ids("@claude/src"), 0);
    assert_eq!(ids("@claude/src/main.rs"), 0);
    // No prefix matching, and another provider's label is not this provider's.
    assert_eq!(ids("@claude/can"), 0);
    assert_eq!(ids("@claude/hoangxuan2402"), 0);
    assert_eq!(ids("@codex/can.le"), 0);
    // Providers without account support, bare words, mid-word and quoted lines.
    assert_eq!(ids("@cursor/default"), 0);
    assert_eq!(ids("@claude"), 0);
    assert_eq!(ids("@claude/"), 0);
    assert_eq!(ids("mail@claude/can.le"), 0);
    assert_eq!(ids("> @claude/can.le"), 0);
}

#[test]
fn the_context_block_carries_the_id_and_nothing_secret() {
    let (mut book, _, work, _) = book();
    book.set_preferred(Kind::Claude, &work.id).unwrap();
    let kinds = |id: &str| (id == work.id).then_some(ClaudeCredential::SetupToken);
    let prompt = "Review it with @\"claude/Work account\" please";
    let out = attach_accounts(prompt, prompt, &book, &kinds);
    assert!(out.starts_with(&format!("{prompt}\n\n---\nReferenced provider account")));
    assert!(out.contains("pass account:\"<id>\" with harness:\"<provider>\""));
    assert!(out.contains("cloud sessions cannot take an account"));
    let json = out.lines().last().unwrap();
    let value: serde_json::Value = serde_json::from_str(json).unwrap();
    assert_eq!(value["provider"], "claude");
    assert_eq!(value["id"], work.id.as_str());
    assert_eq!(value["label"], "Work account");
    assert_eq!(value["authKind"], "setupToken");
    assert_eq!(value.as_object().unwrap().len(), 4);
    // No mention, no change; a mention only in the expanded prompt does not count.
    assert_eq!(attach_accounts("plain", "plain", &book, &none), "plain");
    assert_eq!(
        attach_accounts("x @claude/default", "x", &book, &none),
        "x @claude/default"
    );
}
