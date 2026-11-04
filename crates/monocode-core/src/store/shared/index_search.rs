//! Transcript search through the persistent cache in `search::index`. A session
//! is decoded only when its cheap fingerprint changed since it was indexed.
use super::{blocks, has_table, sql, Store};
use crate::search::index::{self, Doc, Index, Message};
use std::{
    collections::{HashMap, HashSet},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

#[cfg(test)]
thread_local! {
    /// Sessions decoded from the desktop schema by this thread's index refreshes.
    pub(super) static LOADS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// Sets the flag once this many more sessions have been decoded.
    pub(super) static CANCEL_AFTER: std::cell::RefCell<Option<(usize, Arc<AtomicBool>)>> =
        const { std::cell::RefCell::new(None) };
}

impl Store {
    /// Changes whenever a session's saved transcript changes, without reading
    /// any block content: the edit time, the stored size, and for indexed
    /// Monos the number and last sequence of their blocks.
    fn shared_fingerprints(&self) -> Result<HashMap<String, String>, String> {
        let mut monos: HashMap<String, (i64, i64)> = HashMap::new();
        if has_table(&self.conn, "mono_blocks")? {
            let mut query = sql(self.conn.prepare(
                "SELECT session_id,count(*),max(seq) FROM mono_blocks GROUP BY session_id",
            ))?;
            let mut rows = sql(query.query([]))?;
            while let Some(row) = sql(rows.next())? {
                monos.insert(
                    sql(row.get(0))?,
                    (
                        sql(row.get(1))?,
                        sql(row.get::<_, Option<i64>>(2))?.unwrap_or(0),
                    ),
                );
            }
        }
        let mut query = sql(self
            .conn
            .prepare("SELECT id,ifnull(updated_at,0),octet_length(blocks_json) FROM sessions"))?;
        let mut rows = sql(query.query([]))?;
        let mut out = HashMap::new();
        while let Some(row) = sql(rows.next())? {
            let id: String = sql(row.get(0))?;
            let (count, last) = monos.get(&id).copied().unwrap_or((0, 0));
            out.insert(
                id,
                format!(
                    "{}:{}:{count}:{last}",
                    sql(row.get::<_, i64>(1))?,
                    sql(row.get::<_, Option<i64>>(2))?.unwrap_or(0)
                ),
            );
        }
        Ok(out)
    }

    /// One session's searchable messages, built exactly as the direct scan
    /// rejoins fragment rows and caps each message.
    fn shared_index_messages(&self, id: i64) -> Result<(Vec<Message>, bool), String> {
        #[cfg(test)]
        {
            LOADS.with(|loads| loads.set(loads.get() + 1));
            CANCEL_AFTER.with(|hook| {
                let mut hook = hook.borrow_mut();
                if let Some((left, flag)) = hook.as_mut() {
                    *left -= 1;
                    if *left == 0 {
                        flag.store(true, Ordering::Release);
                    }
                }
            });
        }
        let rows = blocks::project(&self.shared_all_blocks(id)?)?;
        let mut messages = Vec::new();
        let mut capped = false;
        let mut ix = 0;
        while ix < rows.len() {
            let from = ix;
            let (text, next, was_capped) = crate::search::join_message(&rows, from);
            ix = next;
            capped |= was_capped;
            if let Some(text) = crate::search::searchable_text(&rows[from].role, &text) {
                messages.push(Message {
                    row_ix: from,
                    role: rows[from].role.clone(),
                    text,
                });
            }
        }
        Ok((messages, capped))
    }

    /// Message hits and the truncation flag, in session recency order. Errors
    /// mean the cache is unusable and the caller should scan directly.
    pub(super) fn shared_indexed_search(
        &self,
        query: &str,
        cancel: &Arc<AtomicBool>,
        summaries: &[crate::store::Session],
    ) -> Result<(Vec<crate::search::Hit>, bool), String> {
        let index = Index::open(&self.path, cancel.clone())?;
        let fingerprints = self.shared_fingerprints()?;
        let mut docs = index.docs()?;
        let mut keep = HashSet::new();
        let mut searchable = Vec::new();
        for summary in summaries {
            if summary.archived || !summary.has_messages {
                continue;
            }
            let original = self.persisted_session_id(summary.id)?;
            keep.insert(original.clone());
            if cancel.load(Ordering::Acquire) {
                return Ok((Vec::new(), false));
            }
            let fingerprint = fingerprints.get(&original).cloned().unwrap_or_default();
            if docs
                .get(&original)
                .is_none_or(|d| d.fingerprint != fingerprint)
            {
                let (messages, capped) = self.shared_index_messages(summary.id)?;
                let doc = Doc {
                    fingerprint,
                    capped,
                };
                index.replace(&original, &doc, &messages)?;
                docs.insert(original.clone(), doc);
            }
            searchable.push((summary, original));
        }
        index.retain(&keep)?;
        let mut candidates = if index::can_prefilter(query) {
            Some(index.candidates(query)?)
        } else {
            None
        };
        let mut hits = Vec::new();
        let mut truncated = false;
        for (summary, original) in searchable {
            if cancel.load(Ordering::Acquire) {
                return Ok((Vec::new(), false));
            }
            truncated |= docs.get(&original).is_some_and(|d| d.capped);
            let messages: Vec<(usize, Option<String>, i64)> = match &mut candidates {
                Some(map) => map
                    .remove(&original)
                    .unwrap_or_default()
                    .into_iter()
                    .map(|(row, id)| (row, None, id))
                    .collect(),
                None => index
                    .session_messages(&original)?
                    .into_iter()
                    .map(|(row, text)| (row, Some(text), 0))
                    .collect(),
            };
            for (row, text, id) in messages {
                let text = match text {
                    Some(text) => text,
                    None => index.text(id)?,
                };
                if let Some(hit) =
                    crate::search::message_hit_text(summary, row, &text, query, false)
                {
                    hits.push(hit);
                }
                if hits.len() >= 400 {
                    return Ok((hits, true));
                }
            }
        }
        Ok((hits, truncated))
    }
}
