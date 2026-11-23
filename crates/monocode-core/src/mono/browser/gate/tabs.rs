//! Tab bookkeeping of one Mono's browser: which tabs the agent made, which were used last, and
//! when an address is already open.
//!
//! Why: the Playwright Bridge extension makes a tab group per connection and the connection is
//! the only owner of the tabs it made or was handed. A new connection (the app restarted the
//! server, the extension reconnected) never adopts the tabs of an earlier one: the old group is
//! greyed and left (`docs/superpowers/specs/2026-11-22-browser-all-tabs-research.md`; only a drag
//! into the live group transfers a tab, `TabGroupManager._onTabChangedGroup`). The extension has
//! no message to claim a group by its name, and `@playwright/mcp` 0.0.86 only sends the name and
//! color for the new group (`--session-name`, `--session-color`). So re-adoption after a restart
//! is not possible from this side; the server therefore lives for hours (see
//! [`super::super::server::IDLE_TIMEOUT`]) and the gate makes the Mono reuse what is open now.
//!
//! The book is a mirror of the last `browser_tabs` list, kept by index (the only identity the
//! tool gives) and by exact address. It fails safe: a tab counts as the agent's only while its
//! identity is certain (the same address at the same position, the tab the agent is working in,
//! or an address that is unique in both lists). Anything else (a tab the user opened, dragged in
//! or swapped, a popup, any list that changed in a way the agent did not cause) counts as the
//! user's: the gate never closes it and never navigates it away.
use serde_json::Value;

/// Tabs one Mono keeps open at most. Past it the least recently used tab the agent made is closed
/// before the agent opens another.
pub const MAX_TABS: usize = 8;
/// The argument a Mono adds to `browser_tabs` `new` to open a second tab to a site that is already
/// open. The gate removes it before the call goes on.
pub const DUPLICATE_FLAG: &str = "allow_duplicate_tab";

/// One line of a `browser_tabs` list: `- 1: (current) [Title](https://host/path)`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tab {
    pub current: bool,
    pub url: String,
}

/// A candidate address is usable when it parses as a URL with a scheme and has no spaces.
fn plausible_url(url: &str) -> bool {
    !url.is_empty() && !url.contains(char::is_whitespace) && url::Url::parse(url).is_ok()
}

/// The address of one list line after `[`: the text up to the `)` that is followed by nothing or
/// by a marker (` [crashed]`, ` [isolatedContext: name]`), for each place a title could end
/// (`](`). A title and an address are both written by the page, so a line with more than one
/// reading that gives a usable address (an address holding `](`) is ambiguous: `None`.
fn address_of(rest: &str) -> Option<String> {
    let rest = rest.trim_end();
    let mut found: Option<String> = None;
    for (at, _) in rest.match_indices("](") {
        let after = &rest[at + 2..];
        let end = after
            .match_indices(')')
            .map(|(i, _)| i)
            .find(|&i| after[i + 1..].is_empty() || after[i + 1..].starts_with(" ["));
        let Some(end) = end else { continue };
        let url = &after[..end];
        if plausible_url(url) {
            if found.is_some() {
                return None;
            }
            found = Some(url.to_owned());
        }
    }
    found
}

/// The tabs a tool result lists, or none when it holds no list or one that is not exactly well
/// formed. Lines that are not tab lines (`- 3: ...`) and anything inside a code fence are ignored
/// (a result can add a page snapshot or the same list twice, under `### Result` and
/// `### Open tabs`; every list in it must be the same). A tab line that does not parse, indexes
/// that do not run 0, 1, 2 ... in order, or an ambiguous address give no list at all, never a
/// partial or spoofed one.
pub fn parse_tabs(text: &str) -> Vec<Tab> {
    let mut runs: Vec<Vec<Tab>> = Vec::new();
    let mut fenced = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with("```") {
            fenced = !fenced;
            continue;
        }
        if fenced {
            continue;
        }
        let Some(rest) = line.strip_prefix("- ") else {
            continue;
        };
        let Some((index, rest)) = rest.split_once(':') else {
            continue;
        };
        let Ok(index) = index.trim().parse::<usize>() else {
            continue;
        };
        if index == 0 {
            runs.push(Vec::new());
        }
        let Some(run) = runs.last_mut().filter(|run| run.len() == index) else {
            return Vec::new();
        };
        let rest = rest.trim_start();
        let (current, rest) = match rest.strip_prefix("(current)") {
            Some(tail) => (true, tail.trim_start()),
            None => (false, rest),
        };
        let Some(url) = rest.strip_prefix('[').and_then(address_of) else {
            return Vec::new();
        };
        run.push(Tab { current, url });
    }
    match runs.split_first() {
        Some((first, others)) if others.iter().all(|run| run == first) => first.clone(),
        _ => Vec::new(),
    }
}

/// `scheme://host[:port]` of an http or https address.
pub fn origin_of(url: &str) -> Option<String> {
    let parsed = url::Url::parse(url.trim()).ok()?;
    matches!(parsed.scheme(), "http" | "https").then_some(())?;
    parsed.host_str()?;
    Some(parsed.origin().ascii_serialization())
}

/// An address without its fragment and a trailing slash, to tell "the same page" from "the same
/// site".
pub fn same_page(a: &str, b: &str) -> bool {
    let norm = |url: &str| {
        let mut parsed = url::Url::parse(url.trim()).ok()?;
        parsed.set_fragment(None);
        Some(parsed.as_str().trim_end_matches('/').to_owned())
    };
    matches!((norm(a), norm(b)), (Some(a), Some(b)) if a == b)
}

#[derive(Clone, Debug)]
struct Rec {
    /// The agent opened this tab and its identity is still certain.
    agent: bool,
    used: u64,
    url: String,
}

/// A tab an address can be shown in instead of a new one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Reuse {
    pub index: usize,
    /// The agent made it, so it may be navigated. A user's tab may only be switched to, and only
    /// when it already shows that page.
    pub agent: bool,
}

#[derive(Default)]
pub struct TabBook {
    recs: Vec<Rec>,
    clock: u64,
    current: Option<usize>,
}
impl TabBook {
    /// Forgets everything: a new connection owns none of the tabs of an earlier one.
    pub fn reset(&mut self) {
        *self = Self::default();
    }
    pub fn tab_count(&self) -> usize {
        self.recs.len()
    }
    /// How many tabs the book would close on its own (made by the agent).
    pub fn agent_tabs(&self) -> usize {
        self.recs.iter().filter(|r| r.agent).count()
    }
    pub fn current(&self) -> Option<usize> {
        self.current
    }
    /// The address the book remembers for the tab at `index`.
    pub fn url_at(&self, index: usize) -> Option<&str> {
        self.recs.get(index).map(|r| r.url.as_str())
    }

    /// Aligns the book with a fresh list. The agent mark is kept for a tab only when its identity
    /// is certain: with the list the same length, the same address at the same position, or the
    /// tab the agent is working in (it navigates there itself) still on the same site; with a
    /// different length that the agent did not cause, only an address that is unique in the old
    /// and the new list. Everything else becomes the user's.
    pub fn sync(&mut self, list: &[Tab]) {
        let same_len = list.len() == self.recs.len();
        let count = |url: &str| list.iter().filter(|t| t.url == url).count();
        let old_count = |url: &str| self.recs.iter().filter(|r| r.url == url).count();
        let mut next = Vec::with_capacity(list.len());
        for (i, tab) in list.iter().enumerate() {
            let kept = if same_len {
                let old = &self.recs[i];
                let same_site = tab.current
                    && origin_of(&old.url).is_some()
                    && origin_of(&old.url) == origin_of(&tab.url);
                (old.url == tab.url || same_site).then(|| old.clone())
            } else if count(&tab.url) == 1 && old_count(&tab.url) == 1 {
                self.recs.iter().find(|r| r.url == tab.url).cloned()
            } else {
                None
            };
            next.push(match kept {
                Some(old) => Rec {
                    url: tab.url.clone(),
                    ..old
                },
                None => Rec {
                    agent: false,
                    used: 0,
                    url: tab.url.clone(),
                },
            });
        }
        self.recs = next;
        self.current = list.iter().position(|t| t.current);
        self.touch_current();
    }
    /// Aligns the book with the list that followed the agent's own `new` call for `requested`
    /// (empty: a blank tab). The one new last tab is the agent's only when exactly one tab
    /// appeared and it shows what was asked for; a popup or any other change is the user's.
    pub fn opened(&mut self, list: &[Tab], requested: &str) {
        let before = self.recs.len();
        if list.len() != before + 1 {
            return self.sync(list);
        }
        self.sync(&list[..before]);
        let last = &list[before];
        let asked = if requested.trim().is_empty() {
            last.url.is_empty() || last.url == "about:blank"
        } else {
            origin_of(requested).is_some() && origin_of(requested) == origin_of(&last.url)
        };
        self.clock += 1;
        self.recs.push(Rec {
            agent: asked,
            used: self.clock,
            url: last.url.clone(),
        });
        self.current = list.iter().position(|t| t.current);
    }
    /// Marks the current tab as used now.
    pub fn touch_current(&mut self) {
        if let Some(rec) = self.current.and_then(|i| self.recs.get_mut(i)) {
            self.clock += 1;
            rec.used = self.clock;
        }
    }
    /// The tab at `index` was closed (call before [`TabBook::sync`] with the list that followed).
    pub fn closed(&mut self, index: usize) {
        if index < self.recs.len() {
            self.recs.remove(index);
        }
    }
    /// The tab to show `url` in instead of a new one: a tab on the same origin, the agent's before
    /// the user's and the current one before the others.
    pub fn reusable(&self, list: &[Tab], url: &str) -> Option<Reuse> {
        let target = origin_of(url)?;
        (0..list.len())
            .filter(|&i| origin_of(&list[i].url).as_deref() == Some(target.as_str()))
            .map(|index| Reuse {
                index,
                agent: self.recs.get(index).is_some_and(|r| r.agent),
            })
            .min_by_key(|r| (!r.agent, !list[r.index].current, r.index))
    }
    /// The agent-made tab used longest ago.
    pub fn victim(&self) -> Option<usize> {
        self.recs
            .iter()
            .enumerate()
            .filter(|(_, r)| r.agent)
            .min_by_key(|(_, r)| r.used)
            .map(|(i, _)| i)
    }
}

/// A note for the agent that the gate reused a tab.
pub fn reuse_note(index: usize, url: &str) -> String {
    format!(
        "Monoptah reused the already open tab {index} ({url}) instead of opening a new one. Open a second tab to the same site only when the user asks for it: call browser_tabs with action new and {DUPLICATE_FLAG} true."
    )
}

/// The index a `browser_tabs` `close` call means: the one it names, else the current tab.
pub fn close_index(args: &Value, book: &TabBook) -> Option<usize> {
    match args["index"].as_u64() {
        Some(i) => usize::try_from(i).ok(),
        None => book.current(),
    }
}
