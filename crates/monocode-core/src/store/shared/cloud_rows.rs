//! The cloud row code (`store/cloud_rows.rs`) for the desktop schema, which is what every
//! real launch of the native app opens (`Store::open_monocode`). Rows are not stored here:
//! they are projected from `sessions.blocks_json`, so a row index is a position in
//! `blocks::project(blocks)`. The row semantics match the `native_rows` path exactly.
use super::{blocks, json, sql, HistoryRevision, Store, Value};
use crate::store::cloud_rows::{
    row_digest, AppendOutcome, RowAnchor, DIVERGED, INDEXED_UNSUPPORTED,
};
use crate::transcript::Row;

/// Why the append refuses: a Host message cannot be stored as a block of this conversation
/// and read back as the same row. Nothing is written.
pub const CANNOT_STORE: &str =
    "A cloud message cannot be saved in this conversation yet. Nothing was written.";

/// Why the append refuses a terminal session: its blocks are never stored.
pub const TERMINAL_UNSUPPORTED: &str =
    "Continue in cloud does not support terminal sessions yet. Nothing was changed.";

/// Gives a block built from Host rows the fields that make it project to the same rows again.
/// `merge` builds a plain block from a row group; a qualified Host prompt and a Host plan carry
/// extra markers that the projection needs.
fn restore_host_markers(block: &mut Value) {
    if block["role"] == "user" && block["attachments"].is_array() {
        block["nativeRemoteAttachments"] = json!(true);
    }
    if block["role"] == "remote-plan" {
        let plan: Value = block["text"]
            .as_str()
            .and_then(|raw| serde_json::from_str(raw).ok())
            .unwrap_or(Value::Null);
        if plan.is_object() {
            // A Host plan without an id gets one derived from its content, so that storing the
            // same plan twice reads back the same row (a generated id would differ each time).
            let id = plan["id"].as_str().map(str::to_owned).unwrap_or_else(|| {
                use sha2::{Digest, Sha256};
                let digest = Sha256::digest(block["text"].as_str().unwrap_or("").as_bytes());
                let hex: String = digest.iter().take(8).map(|b| format!("{b:02x}")).collect();
                format!("cloud-plan-{hex}")
            });
            block["id"] = json!(id);
            block["role"] = json!("plan");
            block["nativeRemotePlan"] = json!(true);
            block["text"] = plan["text"].clone();
            block["streaming"] = plan["streaming"].clone();
            block["plan"] = json!({"status": plan["status"]});
        }
    }
}

/// `host` as it reads back once stored: storing rows rebuilds some blocks (task lists, agent
/// panels, plans) with fields the Host did not send, so the stored text of such a row differs
/// from the Host text while staying stable. Every comparison uses these rows. Row count and
/// roles must survive, or the Host row indexes would no longer be local row indexes.
fn normalize(host: &[Row]) -> Result<Vec<Row>, String> {
    let mut stored = blocks::merge(&[], 0, host).map_err(|_| CANNOT_STORE.to_string())?;
    stored.iter_mut().for_each(restore_host_markers);
    let rows = blocks::project(&stored).map_err(|_| CANNOT_STORE.to_string())?;
    if rows.len() != host.len() || rows.iter().zip(host).any(|(a, b)| a.role != b.role) {
        return Err(CANNOT_STORE.into());
    }
    Ok(rows)
}

impl Store {
    /// Whether the session keeps its history in the paged desktop tables. Their row indexes
    /// are relative to a per-store window, so an absolute `moved_at_row` is not stable.
    fn shared_refuse_indexed(&self, id: i64) -> Result<(), String> {
        // A terminal session never stores blocks, so an append would report rows it lost.
        if id >= 0 && self.shared_surface(id)?.is_tui() {
            return Err(TERMINAL_UNSUPPORTED.into());
        }
        let original = self.persisted_session_id(id)?;
        if self.shared_indexed(&original)? {
            return Err(INDEXED_UNSUPPORTED.into());
        }
        Ok(())
    }
    /// Reads only. It must not call `shared_load`: that would replace the conflict snapshot of
    /// the open conversation and mask a change made by another window.
    pub(in crate::store) fn shared_row_anchor(&self, session: i64) -> Result<RowAnchor, String> {
        self.shared_refuse_indexed(session)?;
        let rows = blocks::project(&self.shared_blocks(session)?)?;
        Ok(RowAnchor {
            count: rows.len(),
            last: rows.last().map(|row| {
                row_digest(
                    &row.role,
                    &row.text,
                    row.first,
                    row.sent_at.and_then(|at| i64::try_from(at).ok()),
                )
            }),
        })
    }
    /// The shared counterpart of the native append, after the checks they have in common.
    ///
    /// Rows before `base` (`moved_at_row`) are never changed: the blocks that hold them are
    /// kept as stored. The first block boundary at or after `base` that is not past the
    /// common prefix is the cut. Blocks after it are replaced by new blocks built from the
    /// Host rows (`blocks::merge` as a pure append), exactly as the runtime write does.
    pub(in crate::store) fn shared_append_projected_rows(
        &mut self,
        local_session: i64,
        environment: &str,
        host_session: &str,
        revision: u64,
        host: &[Row],
        base: usize,
    ) -> Result<AppendOutcome, String> {
        self.shared_writable()?;
        self.shared_refuse_indexed(local_session)?;
        let first_sync = self
            .cloud_synced_revision(environment, host_session)?
            .is_none();
        let tx = sql(self.conn.unchecked_transaction())?;
        let current = self.shared_blocks(local_session)?;
        let local = blocks::project(&current)?;
        if local.len() < base || (first_sync && local.len() != base) {
            return Err(DIVERGED.into());
        }
        // From here on `host` means the Host rows as they read back once stored.
        let normalized = normalize(host)?;
        let host = &normalized[..];
        let tail = &local[base..];
        let prefix = tail
            .iter()
            .zip(host)
            .take_while(|(a, b)| a.role == b.role && a.text == b.text)
            .count();
        let truncated = tail.len().saturating_sub(host.len());
        let outcome = AppendOutcome {
            start: base + prefix,
            written: host.len() - prefix,
            truncated,
        };
        let target = base + prefix;
        // Row index at the start of each block, and the total at the end.
        let mut starts = Vec::with_capacity(current.len() + 1);
        let mut total = 0;
        for block in &current {
            starts.push(total);
            total += blocks::project(std::slice::from_ref(block))?.len();
        }
        starts.push(total);
        // The largest boundary in `base..=target` whose Host row starts a message. Empty
        // blocks that sit on it stay (they are before the cut), so a draft is never dropped.
        let mut cut = None;
        for (kept, &row) in starts.iter().enumerate() {
            if row >= base && row <= target && host.get(row - base).is_none_or(|first| first.first)
            {
                cut = Some((kept, row));
            }
        }
        if !starts.contains(&base) {
            return Err(DIVERGED.into());
        }
        let Some((kept, cut_row)) = cut else {
            return Err(DIVERGED.into());
        };
        let unchanged = prefix == host.len() && truncated == 0;
        let next = if unchanged {
            current.clone()
        } else {
            let mut next = blocks::merge(&current[..kept], cut_row, &host[cut_row - base..])?;
            next.iter_mut().skip(kept).for_each(restore_host_markers);
            // Fail closed: what is stored must read back as exactly the Host rows, and the
            // rows before the move point must not have moved.
            let stored = blocks::project(&next)?;
            if next[..kept] != current[..kept]
                || stored.len() != base + host.len()
                || stored[..base] != local[..base]
                || stored[base..]
                    .iter()
                    .zip(host)
                    .any(|(a, b)| a.role != b.role || a.text != b.text)
            {
                return Err(CANNOT_STORE.into());
            }
            next
        };
        // The cursor is saved BEFORE the blocks. Both live in different databases, so no
        // transaction covers them. If the blocks then fail, the next sync is a non-first
        // sync and the prefix compare repairs it; the reverse order could leave rows written
        // without a cursor, which a later first-sync check would report as DIVERGED forever.
        self.save_setting(
            &Self::synced_key(environment, host_session),
            &revision.to_string(),
        )?;
        if next != current {
            let shared = self.shared.as_ref().unwrap();
            let was_current = shared
                .snapshots
                .borrow()
                .get(&local_session)
                .is_some_and(|old| old.matches(&current));
            self.shared_store_blocks(local_session, &next)?;
            self.shared_patch(
                local_session,
                &[("updated_at", json!(crate::time::now_ms()))],
            )?;
            sql(tx.commit())?;
            // A stale view stays stale: only a snapshot that matched what was stored moves on.
            if was_current {
                shared
                    .snapshots
                    .borrow_mut()
                    .insert(local_session, HistoryRevision::of(&next));
            }
        } else {
            sql(tx.commit())?;
        }
        // Only now that the rows are stored: a failure marker must outlive a failed append.
        self.save_setting(&Self::sync_error_key(local_session), "")?;
        Ok(outcome)
    }
}

#[cfg(test)]
mod tests;
