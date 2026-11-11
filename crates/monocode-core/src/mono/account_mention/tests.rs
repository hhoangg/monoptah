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
    let mut book = Book::default();
    let plain = book.add(Kind::Claude, "can.le").unwrap();
    let spaced = book.add(Kind::Claude, "Work account").unwrap();
    let accent = book.add(Kind::Claude, "Café").unwrap();
    let text = |a: &Account| mention_text(&book, Kind::Claude, a);
    assert_eq!(text(&plain), "@claude/can.le");
    assert_eq!(text(&spaced), "@\"claude/Work account\"");
    assert_eq!(text(&accent), "@\"claude/Café\"");
    // The default account answers to its own label.
    let default = book.list(Kind::Claude).remove(0);
    assert_eq!(text(&default), "@\"claude/CLI login\"");
}

#[test]
fn a_label_that_would_pick_another_credential_inserts_the_id() {
    let mut book = Book::default();
    let first = book.add(Kind::Claude, "Team").unwrap();
    let second = book.add(Kind::Claude, "team").unwrap();
    let literal = book.add(Kind::Claude, "default").unwrap();
    let named = book.add(Kind::Claude, "CLI login").unwrap();
    let quoted = book.add(Kind::Claude, "say \"hi\"").unwrap();
    for account in [&first, &second, &literal, &named, &quoted] {
        let text = mention_text(&book, Kind::Claude, account);
        assert_eq!(text, format!("@claude/{}", account.id));
        // Insert then resolve lands on the same account.
        let found = mentioned_accounts(&text, &book);
        assert_eq!(found.len(), 1, "{text}");
        assert_eq!(found[0].1.id, account.id);
    }
    // The real default account keeps working through the literal.
    let default = book.list(Kind::Claude).remove(0);
    let text = mention_text(&book, Kind::Claude, &default);
    assert_eq!(text, "@claude/default");
    assert_eq!(mentioned_accounts(&text, &book)[0].1.id, "default");
}

#[test]
fn inserted_mentions_round_trip_through_the_parser() {
    let mut book = Book::default();
    for label in ["can.le", "Work account", "Café", "a,b", "x."] {
        book.add(Kind::Claude, label).unwrap();
    }
    for account in book.list(Kind::Claude) {
        let text = format!(
            "start with {} please",
            mention_text(&book, Kind::Claude, &account)
        );
        let found = mentioned_accounts(&text, &book);
        assert_eq!(found.len(), 1, "{text}");
        assert_eq!(found[0].1.id, account.id, "{text}");
    }
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
    assert_eq!(ids("use @claude/can.le now"), vec![can.id.clone()]);
    assert_eq!(ids("use @claude/CAN.LE now"), vec![can.id.clone()]);
    assert_eq!(
        ids("use @\"claude/Work account\" now"),
        vec![work.id.clone()]
    );
    assert_eq!(ids(&format!("@claude/{}", work.id)), vec![work.id.clone()]);
    assert_eq!(ids("@codex/hoangxuan2402"), vec![codex.id.clone()]);
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
