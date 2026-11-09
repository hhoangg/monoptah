//! `@<provider>/<label>` in a Mono composer: names a provider account so the message carries its
//! id. A mention attaches context and never starts anything; the Mono passes the id to
//! `sessions.start` when the user's request asks for a session.
use crate::accounts::{self, Account, Book, ClaudeCredential};
use crate::provider::Kind;

const INSTRUCTION: &str = "Referenced provider account (context only: when the user's request starts a local session for it, pass account:\"<id>\" with harness:\"<provider>\" to sessions.start; cloud sessions cannot take an account):";

/// The text a composer inserts for an account: `@claude/can.le`, or `@"claude/Work account"` when
/// the label has a character outside `[A-Za-z0-9._-]`.
pub fn mention_text(provider: &str, label: &str) -> String {
    if !label.is_empty()
        && label
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'-'))
    {
        format!("@{provider}/{label}")
    } else {
        format!("@\"{provider}/{}\"", label.replace('"', "\\\""))
    }
}

/// The account a mention token names, if `provider` has accounts and the rest resolves to one.
/// `token` is the text after `@` with quotes already removed.
fn account_of(book: &Book, token: &str) -> Option<(Kind, Account)> {
    let (provider, rest) = token.split_once('/')?;
    let kind = Kind::parse(provider).filter(|kind| accounts::supports_accounts(*kind))?;
    let rest = rest.replace("\\\"", "\"");
    let stripped = rest.trim_end_matches([')', ']', '}', '.', '!', '?', ';', ',', ':']);
    let found = [rest.as_str(), stripped]
        .into_iter()
        .filter(|candidate| !candidate.trim().is_empty())
        .find_map(|candidate| {
            let id = accounts::resolve(book, kind, candidate).ok()?;
            book.list(kind).into_iter().find(|a| a.id == id)
        });
    found.map(|account| (kind, account))
}

/// The accounts named by `@<provider>/<label>` mentions in what the user typed, once each, in
/// order. A token whose provider has no accounts, or whose rest names no account of it, is not an
/// account mention (it stays a file mention). Quoted lines are not the user's own words.
pub fn mentioned_accounts(text: &str, book: &Book) -> Vec<(Kind, Account)> {
    let mut out: Vec<(Kind, Account)> = Vec::new();
    for token in crate::composer::mentions(text) {
        if let Some((kind, account)) = account_of(book, &token) {
            if !out.iter().any(|(k, a)| *k == kind && a.id == account.id) {
                out.push((kind, account));
            }
        }
    }
    out
}

/// `prompt` plus a context block for each account `typed` names. `typed` is what the user wrote,
/// not `prompt`. The block holds the provider, id, label and auth kind, never anything secret.
pub fn attach_accounts(
    prompt: &str,
    typed: &str,
    book: &Book,
    claude_credential: &dyn Fn(&str) -> Option<ClaudeCredential>,
) -> String {
    let rows: Vec<String> = mentioned_accounts(typed, book)
        .into_iter()
        .map(|(kind, account)| {
            serde_json::json!({
                "provider": kind.id(),
                "id": account.id,
                "label": account.label,
                "authKind": accounts::auth_kind(kind, claude_credential(&account.id)),
            })
            .to_string()
        })
        .collect();
    if rows.is_empty() {
        return prompt.to_owned();
    }
    format!("{prompt}\n\n---\n{INSTRUCTION}\n{}", rows.join("\n"))
}

#[cfg(test)]
mod tests;
