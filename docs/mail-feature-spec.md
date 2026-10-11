# Mail — implementation spec

Fork-local feature. A mailbox inside Monoptah that **receives** by reading the
S3 bucket Amazon SES writes incoming mail into, and **sends** through SES.

Wireframe: `work/mail-mockup.html` (gitignored, local only).

Everything in this document is a decision, not an option, unless it is marked
**Open**. Where a decision was made against an alternative, the alternative and
the reason are recorded, because the reason is the part that gets lost.

---

## 1. Scope

In scope for v1:

- One mail provider, AWS (SES for sending, S3 for receiving), behind a provider
  abstraction so IMAP/Cloudflare can be added without touching the mailbox UI.
- **Many domains.** Several SES regions, several buckets, several AWS key pairs,
  in any combination — see §4.1. One unified mailbox over all of them.
- Mailboxes that map 1:1 to the SES receipt-rule → S3-prefix layout already in
  use: `owner@hhoangg.com` → `s3://bucket/owner/`, `seller@…` → `/seller/`.
- Hourly list-and-reconcile sync, plus a manual sync.
- Thread grouping.
- Filtering by from / to / subject / body / date / mailbox / attachment, and
  saving a filter as a **smart folder**.
- Read, star, archive, delete (with an explicit archive-vs-delete-on-S3 choice).
- Compose, reply, reply all, forward, with attachments.
- Full-text search over subject, body and participants.

Out of scope for v1 (named so they are not accidentally designed out):

- Contacts / address book beyond autocompleting addresses seen in local mail.
- Server-side filters (SES receipt rules are configured in AWS, not here).
- Calendar, labels-as-tags beyond smart folders, snooze, signatures per mailbox.
- Push. SES → SNS → local webhook is not reachable from a desktop app without a
  relay; polling is the design.

---

## 2. Architecture

```
┌─ webview ─────────────────────────────────────────────────────────────┐
│  src/features/mail/ui/MailView.tsx          4-pane mailbox            │
│  src/features/mail/ui/ComposeSheet.tsx      compose / reply (TipTap)  │
│  src/features/mail/ui/SmartFolderSheet.tsx  rule editor               │
│  src/features/settings/ui/MailSettings.tsx  settings page             │
│  src/features/mail/model/*.ts               invoke wrappers, types    │
└──────────────────────────── invoke() ─────────────────────────────────┘
┌─ rust ────────────────────────────────────────────────────────────────┐
│  src-tauri/src/mail.rs           commands, sync loop, thread linking  │
│  src-tauri/src/mail_store.rs     SQL, schema, queries                 │
│  src-tauri/src/mail_mime.rs      parse incoming, build outgoing       │
│  src-tauri/src/aws_s3.rs         S3 calls, behind trait S3Api         │
│  src-tauri/src/aws_ses.rs        SES calls                            │
│  src-tauri/src/aws_sigv4.rs      request signing (reusable)           │
│  src-tauri/src/mail_provider.rs  trait MailProvider + AwsProvider     │
└───────────────────────────────────────────────────────────────────────┘
```

Hard rules that shape this:

- **No credential ever reaches the webview.** The frontend sends them once on
  connect and from then on receives only a status struct. Same shape as
  `clickup_connected`, one notch stricter (keychain, not a JSON file).
- **No AWS traffic from the webview.** `tauri.conf.json` CSP `connect-src` allows
  only IPC and the asset protocol, and it stays that way.
- **The local SQLite DB is the read path.** The mailbox is fully usable offline;
  only sending needs the network.
- **S3 is the source of truth for received mail.** Local rows are a cache plus
  local-only state (read, starred, archived, thread id, folder membership).

### 2.1 Provider seam

```rust
pub(crate) trait MailProvider {
    fn list_objects(&self, prefix: &str, after: Option<&str>) -> Result<ObjectPage>;
    fn get_object(&self, key: &str) -> Result<Vec<u8>>;
    fn delete_objects(&self, keys: &[String]) -> Result<Vec<DeleteOutcome>>;
    fn put_object(&self, key: &str, body: &[u8], content_type: &str) -> Result<()>;
    fn send_raw(&self, from: &str, rcpt: &[String], mime: &[u8]) -> Result<String>;
    fn verified_identities(&self) -> Result<Vec<Identity>>;
}
```

`AwsProvider` is the only implementor in v1. Everything above the trait — sync,
threading, folders, UI — is provider-agnostic. An IMAP provider would map
`list_objects` to `UID SEARCH`/`FETCH` and `send_raw` to SMTP.

---

## 3. Dependencies to add

### Rust crates

`src-tauri/Cargo.toml`. All four are pure Rust and bring no async runtime, which
matters because the existing HTTP code is blocking `ureq` on `spawn_blocking`.

| Crate | Version | Why |
|---|---|---|
| `mail-parser` | `0.11` | MIME parsing. Handles nested multipart, RFC 2047 encoded words (`=?UTF-8?B?SG/DoG5nIFh1w6Ju?=` in the sample mail), quoted-printable, base64, charset conversion, `References` lists, attachments. Hand-rolling this is weeks of edge cases. |
| `mail-builder` | `0.4` | Builds the outgoing RFC 5322 message: multipart/alternative, attachments, correct encoded-word headers. Same author as `mail-parser`. |
| `ammonia` | `4` | HTML allowlist sanitiser (html5ever based). Runs on the Rust side so unsanitised email HTML never crosses into the webview. |
| `hmac` | `0.12` | HMAC-SHA256 for SigV4. `sha2` is already a dependency. |

**Not** adding `aws-sdk-s3` / `aws-sdk-sesv2`: they pull tokio, hyper and a large
dependency tree for six API calls. Hand-rolled SigV4 over the existing `ureq`
is ~150 lines in `aws_sigv4.rs` and keeps the blocking model.

### npm dependencies to add

The install line from the TipTap React guide, plus one extension:

```
npm install @tiptap/react @tiptap/pm @tiptap/starter-kit @tiptap/extension-image
```

| Package | Why |
|---|---|
| `@tiptap/react` | React bindings. `@tiptap/core` is a peer of it and arrives with the install; it does not need its own entry. |
| `@tiptap/pm` | The ProseMirror dependencies. The guide installs it explicitly because the editor does not function without it. |
| `@tiptap/starter-kit` | In v3 this bundles **Link** and **Underline** (both new in v3), plus Bold, Italic, Strike, Code, Blockquote, BulletList, OrderedList, ListItem, ListKeymap, CodeBlock, HardBreak, Heading, HorizontalRule, Paragraph, Text, Document, Dropcursor, Gapcursor, **Undo/Redo** and TrailingNode. Every button in the composer toolbar is covered by it — no separate extension packages, and no history extension to wire up. |
| `@tiptap/extension-image` | The one thing StarterKit does not carry: inline images pasted or dropped into a draft, which become `cid:` parts on send. |

All current at `3.31.x`, all MIT. Pin them to the same minor: TipTap's packages
peer-depend on each other at an *exact* version, so a mixed set fails to
resolve. React 19 is in `@tiptap/react`'s peer range
(`^17.0.0 || ^18.0.0 || ^19.0.0`).

`immediatelyRender: false` is an SSR workaround and is **not** needed here —
this is a client-only Vite app. Do not copy it in from a Next.js example.

Nothing else. There is no rich-text editor, ProseMirror, Lexical, Slate or Quill
in the tree today — the only editors are CodeMirror — so this is the one new
frontend subsystem the feature introduces. It is lazy-loaded (§7.5) so
ProseMirror stays out of the main chunk.

Docs: <https://tiptap.dev/docs/editor/getting-started/install/react> and
<https://tiptap.dev/docs/editor/extensions/functionality/starterkit>.

#### Free TipTap only — a hard rule

**Nothing from the `@tiptap-pro/` scope, ever. If a feature needs a Pro
extension, write the plugin instead.**

This is mechanically checkable rather than a matter of judgement, which is why
it is phrased this way:

- Pro extensions are published under the **`@tiptap-pro/`** npm scope and
  install from TipTap's **private registry**, which needs an account token.
  So the rule reduces to two greps: `package.json` must contain no
  `@tiptap-pro/` dependency, and no `.npmrc` may point a scope at
  `registry.tiptap.dev`. Both are in §14 as a test.
- The four packages above are all plain `@tiptap/*` and all **MIT** (verified
  from each package's `license` field on the registry, and TipTap's own core is
  MIT on GitHub).
- Also avoid the **Tiptap UI Components / "Simple Editor" templates**, pulled
  with `npx @tiptap/cli`. Beyond the registry question they ship their own
  design system, which is the opposite of what §11.1 requires — Mail has to
  look like the rest of the app, not like a TipTap demo.

Nothing this feature needs is Pro. For the record, the Pro extensions a mail
composer might superficially seem to want, and why each is not needed:

| Pro extension | Why we don't need it |
|---|---|
| Drag Handle | Email bodies are short prose. Nothing to reorder by dragging. |
| Export (docx / odt / pdf) | We emit RFC 5322 MIME, not office documents. §7.5's serializer is the export path. |
| Comments, Collaboration, AI | Single-user local drafts. No shared document, no cloud. |
| Table of Contents | A reply is not a document with headings to index. |

**What we do write ourselves**, as ordinary free extensions — TipTap's
extension API is the same API the Pro ones use, so this costs a file, not a
subscription:

- **Paste and drop of files** — a small extension whose
  `addProseMirrorPlugins()` returns one plugin with `handlePaste` and
  `handleDrop`. Images become `Image` nodes bound to a pending attachment;
  anything else goes straight to the attachment strip, bypassing the document.
  ~40 lines.
- **Paste as plain text** (Cmd+Shift+V) — one keyboard shortcut calling
  `insertContent(text)`.
- **Send on Cmd+Enter** — `addKeyboardShortcuts()` on the same extension.
- A **signature node**, if signatures are ever added — a leaf node with a
  `renderHTML`, the standard custom-node recipe.

If something later genuinely needs a Pro extension, that is a conversation to
have before writing code, not a dependency to add quietly.

FTS5 needs no new feature flag: `libsqlite3-sys`' bundled build compiles
`-DSQLITE_ENABLE_FTS5` unconditionally (verified in its `build.rs`), and
`rusqlite` is already `features = ["bundled", "hooks"]`.

---

## 4. Database schema

One new module `src-tauri/src/mail_store.rs` exposing
`pub(crate) fn ensure_tables(conn: &Connection) -> rusqlite::Result<()>`, called
from `session_store.rs::migrate()` alongside the other
`ensure_*` helpers (next to `mono_transcript::ensure_tables`, around line 1005).

This follows the newer module-owned, idempotent `CREATE TABLE IF NOT EXISTS`
style rather than adding a block to the versioned ladder, so the feature stays
in files upstream does not own and merges cleanly. Later column additions use
the existing `ensure_column` helper.

### 4.1 Three levels, because three different things vary

A naive "one row per domain" model breaks the first time two domains share a
bucket, or one domain is verified in two SES regions. What actually varies is
not the domain — it is these three, independently:

| Varies | By | Table |
|---|---|---|
| Access key / secret | AWS account | `mail_credential` |
| SES region, bucket, S3 region | where mail lands | `mail_account` ("source") |
| Address → S3 prefix | SES receipt rule | `mail_mailbox` |

So the hierarchy is **credential → source → mailbox**, and a *domain* is not a
table at all: it is parsed from `mail_mailbox.address` and used only for
grouping in the UI. That one decision is what makes every real layout fall out
for free:

```
A. Two domains, one AWS account, one bucket, one region
   cred "personal" ─ source "byptah-mail / us-east-1"
                       ├── owner@hhoangg.com   → hhoangg/owner/
                       ├── seller@hhoangg.com  → hhoangg/seller/
                       ├── owner@byptah.com    → byptah/owner/
                       └── support@byptah.com  → byptah/support/

B. Two domains, one AWS account, a bucket each
   cred "personal" ─┬ source "hhoangg-mail / us-east-1"
                    │   ├── owner@hhoangg.com  → owner/
                    │   └── seller@hhoangg.com → seller/
                    └ source "byptah-mail / ap-southeast-1"
                        └── owner@byptah.com   → owner/

C. Two separate AWS accounts, two separate buckets
   cred "personal" ─ source "byptah-mail / us-east-1"
                       ├── owner@hhoangg.com   → hhoangg/owner/
                       └── owner@byptah.com    → byptah/owner/
   cred "acme-ltd" ─ source "acme-inbound / eu-west-1"
                       ├── hello@acme.com      → in/
                       └── billing@acme.com    → billing/
```

Layout A costs one credential and one source. Layout C needs a second
credential and never mixes the two key pairs. Nothing in the sync, threading or
folder code knows the difference.

**The mailbox table belongs to the source.** This is the part worth stating
plainly, because the settings page makes it look otherwise at a glance: the
address → prefix rows in a source card are scoped to that card's bucket. A
second AWS account is therefore just a second card — its own credential, SES
region, bucket, S3 region, prefixes and error state, sharing nothing with the
first. The sync interval is the one thing that is deliberately *not* per
source: one app-wide setting, because mail is small and three different clocks
would be three things to get wrong for no benefit. Two sources can even use *different* key pairs for
sending and for reading (`credential_id` vs `s3_credential_id`), so a
read-only S3 key can be shared across sources while each keeps its own SES
sending key.

Everything above the source level is unified regardless: one thread list, one
set of smart folders, one From picker, one unread badge. A source is a place
mail comes from, not a partition of the mailbox.

A **source is identified by what it points at**, not by a name:
`UNIQUE (credential_id, ses_region, bucket, s3_region)`. Adding a second domain
to an existing bucket therefore cannot accidentally create a duplicate source —
it adds mailboxes to the one that is already there.

A **mailbox address is globally unique** across every source. SES delivers one
address to exactly one place, so two rows for `owner@hhoangg.com` would make
"which prefix do I sync?" and "which source do I reply from?" ambiguous. The
settings form rejects the duplicate and names the source that already has it,
rather than silently accepting a configuration that would double-thread mail.

```sql
-- ── an AWS key pair, reusable across sources ───────────────────────
CREATE TABLE IF NOT EXISTS mail_credential (
  id             INTEGER PRIMARY KEY,
  provider       TEXT NOT NULL,              -- 'aws'
  label          TEXT NOT NULL,              -- 'personal', 'acme-ltd'
  access_key_masked TEXT NOT NULL,           -- 'AKIA••••3K7Q', display only
  default_region TEXT,
  created_at     INTEGER NOT NULL,
  updated_at     INTEGER NOT NULL,
  UNIQUE (provider, label)
);
-- the secret itself is NOT here; see §12

-- ── a source: one bucket + one SES region, under one credential ────
CREATE TABLE IF NOT EXISTS mail_account (
  id             INTEGER PRIMARY KEY,
  provider       TEXT NOT NULL,              -- 'aws'
  credential_id  INTEGER NOT NULL REFERENCES mail_credential(id) ON DELETE RESTRICT,
  s3_credential_id INTEGER REFERENCES mail_credential(id) ON DELETE RESTRICT,
                                             -- NULL = reuse credential_id
  label          TEXT NOT NULL,              -- 'byptah-mail (us-east-1)'
  ses_region     TEXT NOT NULL,
  bucket         TEXT NOT NULL,
  s3_region      TEXT NOT NULL,
  default_from   TEXT,                       -- 'Name <owner@byptah.com>'
  attach_policy  TEXT NOT NULL DEFAULT 'on_demand',  -- on_demand | always
  image_policy   TEXT NOT NULL DEFAULT 'ask',        -- ask | always | never
  enabled        INTEGER NOT NULL DEFAULT 1,
  sort_order     INTEGER NOT NULL DEFAULT 0,
  identities_refreshed_at INTEGER,           -- millis; NULL = never refreshed, so
                                             -- is_sendable is unknown, not false
  created_at     INTEGER NOT NULL,
  updated_at     INTEGER NOT NULL,
  UNIQUE (credential_id, ses_region, bucket, s3_region)
);

-- ── SES verified identities, cached per source ─────────────────────
-- Verification is per AWS-account AND per region, so the same domain can be
-- verified in one source and not another. This is why is_sendable cannot be
-- derived from the address alone.
CREATE TABLE IF NOT EXISTS mail_identity (
  account_id   INTEGER NOT NULL REFERENCES mail_account(id) ON DELETE CASCADE,
  identity     TEXT NOT NULL,                -- 'byptah.com' or 'owner@byptah.com'
  kind         TEXT NOT NULL,                -- domain | email
  status       TEXT NOT NULL,                -- verified | pending | failed
  refreshed_at INTEGER NOT NULL,
  PRIMARY KEY (account_id, identity)
);

-- ── one row per SES receipt rule / S3 prefix ───────────────────────
CREATE TABLE IF NOT EXISTS mail_mailbox (
  id           INTEGER PRIMARY KEY,
  account_id   INTEGER NOT NULL REFERENCES mail_account(id) ON DELETE CASCADE,
  address      TEXT NOT NULL,                -- 'owner@hhoangg.com'
  domain       TEXT NOT NULL,                -- 'hhoangg.com', lowercased,
                                             -- derived on write, for grouping
  s3_prefix    TEXT NOT NULL,                -- 'owner/' or 'hhoangg/owner/'
  display_name TEXT NOT NULL,                -- 'Owner'
  color        TEXT,
  sort_order   INTEGER NOT NULL DEFAULT 0,
  is_sendable  INTEGER NOT NULL DEFAULT 1,   -- resolved against mail_identity
  UNIQUE (address),
  UNIQUE (account_id, s3_prefix)
);
CREATE INDEX IF NOT EXISTS mail_mailbox_domain ON mail_mailbox(domain, sort_order);

-- ── sync ledger: what we have seen in the bucket ───────────────────
CREATE TABLE IF NOT EXISTS mail_object (
  id           INTEGER PRIMARY KEY,
  mailbox_id   INTEGER NOT NULL REFERENCES mail_mailbox(id) ON DELETE CASCADE,
  s3_key       TEXT NOT NULL,
  etag         TEXT,
  size_bytes   INTEGER,
  s3_modified  INTEGER,                      -- epoch SECONDS, from LastModified
  first_seen_at INTEGER NOT NULL,            -- MILLISECONDS, like every other
  last_seen_at INTEGER NOT NULL,             -- bookkeeping column. This table
                                             -- mixes the two units on purpose:
                                             -- s3_modified feeds date_utc, which
                                             -- is seconds. Never compare across.
  state        TEXT NOT NULL,                -- pending | parsed | gone | error
                                             -- | deleted. `deleted` is a
                                             -- tombstone for a confirmed hard
                                             -- delete: `gone` gets re-ingested
                                             -- on the next listing and `parsed`
                                             -- gets flipped to `gone`, so
                                             -- neither can hold the line while
                                             -- S3's listing is still catching
                                             -- up. Its cost is that an object
                                             -- S3 reported deleted but kept
                                             -- becomes invisible to us while
                                             -- still sitting in the bucket.
  parse_error  TEXT,
  parse_attempts INTEGER NOT NULL DEFAULT 0, -- §5 step 4 caps retries at 3

  UNIQUE (mailbox_id, s3_key)
);
CREATE INDEX IF NOT EXISTS mail_object_state ON mail_object(state, mailbox_id);

-- ── threads ────────────────────────────────────────────────────────
CREATE TABLE IF NOT EXISTS mail_thread (
  id               INTEGER PRIMARY KEY,
  account_id       INTEGER NOT NULL REFERENCES mail_account(id) ON DELETE CASCADE,
  mailbox_id       INTEGER NOT NULL REFERENCES mail_mailbox(id) ON DELETE CASCADE,
  subject          TEXT NOT NULL,            -- subject of the first message
  subject_key      TEXT NOT NULL,            -- normalised, see §6
  participants_json TEXT NOT NULL,           -- [{name,addr}] deduped, ordered
  first_message_at INTEGER NOT NULL,
  last_message_at  INTEGER NOT NULL,
  message_count    INTEGER NOT NULL DEFAULT 0,
  unread_count     INTEGER NOT NULL DEFAULT 0,
  has_attachments  INTEGER NOT NULL DEFAULT 0,
  is_starred       INTEGER NOT NULL DEFAULT 0,
  state            TEXT NOT NULL DEFAULT 'active',  -- active | archived
  archived_at      INTEGER
);
CREATE INDEX IF NOT EXISTS mail_thread_box ON mail_thread(mailbox_id, state, last_message_at DESC);
CREATE INDEX IF NOT EXISTS mail_thread_skey ON mail_thread(mailbox_id, subject_key, last_message_at DESC);

-- ── messages ───────────────────────────────────────────────────────
CREATE TABLE IF NOT EXISTS mail_message (
  id              INTEGER PRIMARY KEY,
  mailbox_id      INTEGER NOT NULL REFERENCES mail_mailbox(id) ON DELETE CASCADE,
  thread_id       INTEGER NOT NULL REFERENCES mail_thread(id) ON DELETE CASCADE,
  object_id       INTEGER REFERENCES mail_object(id) ON DELETE SET NULL,
  direction       TEXT NOT NULL,             -- inbound | outbound
  message_id_hdr  TEXT,                      -- '<...@mail.gmail.com>' normalised
  in_reply_to     TEXT,
  subject         TEXT NOT NULL DEFAULT '',
  subject_key     TEXT NOT NULL DEFAULT '',
  from_name       TEXT,
  from_addr       TEXT NOT NULL DEFAULT '',
  reply_to_addr   TEXT,
  to_json         TEXT NOT NULL DEFAULT '[]',
  cc_json         TEXT NOT NULL DEFAULT '[]',
  bcc_json        TEXT NOT NULL DEFAULT '[]',
  date_utc        INTEGER NOT NULL,          -- Date: header, epoch seconds
  received_at     INTEGER NOT NULL,          -- SES Received: timestamp
  snippet         TEXT NOT NULL DEFAULT '',  -- first ~180 chars of text
  body_text       TEXT,
  body_html       TEXT,                      -- ALREADY SANITISED, see §7
  body_html_raw_key TEXT,                    -- NULL; raw stays in S3
  has_attachments INTEGER NOT NULL DEFAULT 0,
  size_bytes      INTEGER,
  spf             TEXT, dkim TEXT, dmarc TEXT,   -- verdict as written, or NULL
                                                 -- 'pass' | 'fail' | 'softfail' |
                                                 -- 'neutral' | 'permerror' | …;
                                                 -- 'none' is stored as NULL. Kept
                                                 -- raw because a softfail badge is
                                                 -- more honest than calling it fail.
                                                 -- Only the topmost
                                                 -- Authentication-Results header is
                                                 -- read: a lower one is the sender's.
  headers_json    TEXT,                      -- full header list for the raw view
  is_read         INTEGER NOT NULL DEFAULT 0,
  is_starred      INTEGER NOT NULL DEFAULT 0,
  state           TEXT NOT NULL DEFAULT 'active',  -- active | archived | gone
  UNIQUE (mailbox_id, message_id_hdr)
);
CREATE INDEX IF NOT EXISTS mail_message_thread ON mail_message(thread_id, date_utc);
CREATE INDEX IF NOT EXISTS mail_message_from ON mail_message(from_addr);

-- ── flattened References/In-Reply-To, the primary threading signal ─
CREATE TABLE IF NOT EXISTS mail_message_ref (
  message_id   INTEGER NOT NULL REFERENCES mail_message(id) ON DELETE CASCADE,
  ref_hdr      TEXT NOT NULL,                -- a Message-ID this one references
  PRIMARY KEY (message_id, ref_hdr)
);
CREATE INDEX IF NOT EXISTS mail_message_ref_hdr ON mail_message_ref(ref_hdr);

CREATE TABLE IF NOT EXISTS mail_attachment (
  id          INTEGER PRIMARY KEY,
  message_id  INTEGER NOT NULL REFERENCES mail_message(id) ON DELETE CASCADE,
  part_index  INTEGER NOT NULL,
  filename    TEXT NOT NULL,
  mime_type   TEXT NOT NULL,
  size_bytes  INTEGER NOT NULL,
  content_id  TEXT,                          -- for cid: inline images
  is_inline   INTEGER NOT NULL DEFAULT 0,
  local_path  TEXT,                          -- NULL until downloaded
  UNIQUE (message_id, part_index)
);

-- ── smart folders ──────────────────────────────────────────────────
-- Folders are GLOBAL, not per source: "Invoices" should collect invoices from
-- every domain. Narrowing to one domain or source is expressed as a rule
-- (field 'domain' / 'account'), not as ownership, so it stays the user's
-- choice instead of the schema's.
CREATE TABLE IF NOT EXISTS mail_folder (
  id          INTEGER PRIMARY KEY,
  name        TEXT NOT NULL,
  color       TEXT,
  sort_order  INTEGER NOT NULL DEFAULT 0,
  match_mode  TEXT NOT NULL DEFAULT 'all',   -- all | any
  hide_from_mailbox INTEGER NOT NULL DEFAULT 0,
  count_unread INTEGER NOT NULL DEFAULT 1,
  created_at  INTEGER NOT NULL,
  updated_at  INTEGER NOT NULL,
  UNIQUE (name)
);

CREATE TABLE IF NOT EXISTS mail_folder_rule (
  id         INTEGER PRIMARY KEY,
  folder_id  INTEGER NOT NULL REFERENCES mail_folder(id) ON DELETE CASCADE,
  position   INTEGER NOT NULL DEFAULT 0,
  field      TEXT NOT NULL,   -- from|to|cc|recipient|subject|body|mailbox
                              -- |domain|account|has_attachment|date
                              -- |is_unread|is_starred
  op         TEXT NOT NULL,   -- is|contains|contains_any|starts_with|ends_with
                              -- |matches|before|after|within_days|true
  value      TEXT NOT NULL DEFAULT '',       -- scalar, or JSON array for *_any
  negate     INTEGER NOT NULL DEFAULT 0
);

-- materialised membership so sidebar counts are O(1)
CREATE TABLE IF NOT EXISTS mail_folder_member (
  folder_id  INTEGER NOT NULL REFERENCES mail_folder(id) ON DELETE CASCADE,
  thread_id  INTEGER NOT NULL REFERENCES mail_thread(id) ON DELETE CASCADE,
  PRIMARY KEY (folder_id, thread_id)
);
CREATE INDEX IF NOT EXISTS mail_folder_member_thread ON mail_folder_member(thread_id);

-- ── outbox ─────────────────────────────────────────────────────────
CREATE TABLE IF NOT EXISTS mail_outbox (
  id            INTEGER PRIMARY KEY,
  account_id    INTEGER NOT NULL REFERENCES mail_account(id) ON DELETE CASCADE,
  mailbox_id    INTEGER REFERENCES mail_mailbox(id) ON DELETE SET NULL,
  thread_id     INTEGER REFERENCES mail_thread(id) ON DELETE SET NULL,
  reply_to_message_id INTEGER REFERENCES mail_message(id) ON DELETE SET NULL,
  kind          TEXT NOT NULL,               -- new | reply | reply_all | forward
  from_addr     TEXT NOT NULL,
  to_json       TEXT NOT NULL,
  cc_json       TEXT NOT NULL DEFAULT '[]',
  bcc_json      TEXT NOT NULL DEFAULT '[]',
  subject       TEXT NOT NULL DEFAULT '',
  body_text     TEXT,                          -- plain-text alternative, derived
  body_html     TEXT,                          -- email HTML, derived at send time
  body_json     TEXT,                          -- TipTap document, the source of truth
                                               -- for a draft; HTML is lossy to re-parse
  quoted_html   TEXT,                          -- the original being replied to, kept
                                               -- outside the editor; see §7.5
  attachments_json TEXT NOT NULL DEFAULT '[]',  -- [{path,filename,mime,cid?}]
  in_reply_to   TEXT,
  references_json TEXT NOT NULL DEFAULT '[]',
  state         TEXT NOT NULL DEFAULT 'draft',  -- draft|queued|sending|sent|failed
  attempts      INTEGER NOT NULL DEFAULT 0,
  last_error    TEXT,
  ses_message_id TEXT,
  created_at    INTEGER NOT NULL,
  updated_at    INTEGER NOT NULL,
  sent_at       INTEGER
);

-- ── sync bookkeeping ───────────────────────────────────────────────
CREATE TABLE IF NOT EXISTS mail_sync_state (
  mailbox_id        INTEGER PRIMARY KEY REFERENCES mail_mailbox(id) ON DELETE CASCADE,
  last_started_at   INTEGER,
  last_finished_at  INTEGER,
  last_error        TEXT,
  objects_seen      INTEGER NOT NULL DEFAULT 0,
  objects_added     INTEGER NOT NULL DEFAULT 0,
  objects_gone      INTEGER NOT NULL DEFAULT 0
);

-- ── full-text search ───────────────────────────────────────────────
CREATE VIRTUAL TABLE IF NOT EXISTS mail_message_fts USING fts5 (
  subject, body, people,                     -- 'Name <addr>' of from+to+cc
  content = '',                              -- contentless: we own the rows
  tokenize = "unicode61 remove_diacritics 2"
);
-- rowid of mail_message_fts == mail_message.id, written by the same
-- transaction that inserts the message. Deletes use the
-- `INSERT INTO mail_message_fts(mail_message_fts, rowid, ...) VALUES('delete', ...)`
-- form required by contentless tables.
```

`remove_diacritics 2` matters here: it makes `hoang` find `Hoàng`.

### Where local state lives vs S3

| State | Lives in | Note |
|---|---|---|
| Raw MIME | S3 only | Never copied into SQLite. Re-fetched for the raw view. |
| Parsed headers/body/snippet | SQLite | Cheap, and the offline read path. |
| Attachment bytes | App data dir, `mail/attachments/<message_id>/<idx>-<name>` | On demand by default. |
| read / starred / archived / thread id / folder membership | SQLite only | Not representable in S3; a bucket re-sync must never clobber them. |
| Sent mail | SQLite **and** `s3://bucket/sent/` | See §8.4. |

---

## 5. Sync

`mail::init(app)` spawns one `std::thread` in `.setup()` (the `reminders.rs`
pattern), which wakes every 60 s and syncs any mailbox whose
`last_finished_at` is older than the app-wide sync interval (one setting, one
hour by default, in `monocode.mail.syncIntervalSeconds`). A `mail_sync_now`
command triggers it immediately.

With several sources this needs three rules it would not need with one:

- **One mutex per `credential_id`, not one global mutex.** Sources that share a
  key pair are serialised, so a manual sync of one does not race a scheduled
  sync of another and double-insert; sources on *different* AWS accounts run
  concurrently, because they cannot throttle each other.
- **A source failing is isolated.** An `AccessDenied` on `acme-inbound` sets
  `last_error` on that source only. The other sources finish normally, the
  unified mailbox keeps working, and the sidebar shows the error on that
  domain group alone — not as an app-wide banner.
- **Sync order is by staleness, not by id**, so one large bucket cannot starve
  the rest.

Per mailbox:

1. `ListObjectsV2` with `prefix = s3_prefix`, paginating on
   `NextContinuationToken`, 1000 keys per page. Collect `(key, etag, size,
   LastModified)`.
2. For every listed key: `INSERT … ON CONFLICT(mailbox_id, s3_key) DO UPDATE SET
   last_seen_at = ?, etag = ?` with `state='pending'` on insert. An etag change
   on an existing key is treated as a new body: re-parse and overwrite the
   message row (SES does not rewrite objects, so this is a safety net, not a
   normal path).
3. For every `mail_object` row in this mailbox whose `last_seen_at < sync_start`
   and `state != 'gone'`: set `state='gone'`, and set the linked
   `mail_message.state='gone'` too — **not** `'archived'`. The two must stay
   distinguishable, because §10's no-resurrect rule is precisely "a re-listed key
   must not undo what the *user* archived", and collapsing them into one state
   makes that rule unimplementable. Listing hides both. **Never delete the row** —
   the user's read state, thread links and folder membership are the only copy.
   A thread whose every message is `gone` or `archived` gets
   `mail_thread.state='archived'`.
4. For each `state='pending'` object, in batches: `GetObject`, parse (§6/§7),
   insert the message, link the thread, update FTS, re-evaluate smart folders,
   then `state='parsed'`. A parse failure sets `state='error'` with
   `parse_error`; it is retried on the next sync at most 3 times, then left
   alone and surfaced in the sidebar footer.
5. Write `mail_sync_state`, emit `mail://sync-state` to the webview, carrying
   the `account_id` so the UI updates one domain group rather than re-rendering
   every mailbox.

Once per source per day — not per sync — SES `ListEmailIdentities` refreshes
`mail_identity` and recomputes `mail_mailbox.is_sendable`. An identity that is
verified in `us-east-1` but not in `eu-west-1` makes the same domain sendable
from one source and not another, and the From picker has to know.

Everything from step 2 onward for one page runs in a single transaction so a
crash mid-sync leaves no half-threaded message.

Progress is emitted as a Tauri event rather than polled, so the UI shows "42 of
214" during a first sync without a round trip per object.

---

## 6. Thread grouping

Gmail's own rule is not published. The design below is the standard
References-first algorithm (the one JWZ wrote up for Netscape, which every mail
client descends from) plus the subject fallback that makes it behave the way
users expect when a client strips `References`.

### Subject normalisation → `subject_key`

Applied repeatedly until the string stops shrinking:

1. Trim.
2. Strip a leading mailing-list tag: `^\[[^\]]{1,40}\]\s*`.
3. Strip a leading reply/forward prefix, case-insensitive, with optional
   count and optional colon-space: `^(re|aw|sv|antw|res|fwd|fw|vs|tr|rv|trả lời|chuyển tiếp)\s*(\[\d+\])?\s*:\s*`.
4. Collapse internal whitespace to single spaces.
5. Lowercase, then Unicode NFKC normalise (the crate
   `unicode-normalization` is already a dependency).

`"Re: Re: FW: [sales]  Order #A-1182"` → `"order #a-1182"`.

### Linking a new message

```
refs = normalise_ids(In-Reply-To ∪ References)      # keep <...> content only

candidates = SELECT DISTINCT m.thread_id
             FROM mail_message m
             WHERE m.mailbox_id = ?
               AND (m.message_id_hdr IN refs
                    OR m.id IN (SELECT message_id FROM mail_message_ref
                                WHERE ref_hdr = this.message_id_hdr))

if candidates is not empty:
    thread = min(candidates)
    if |candidates| > 1: merge_threads(candidates -> thread)   # see below
else:
    thread = SELECT id FROM mail_thread
             WHERE mailbox_id = ?
               AND subject_key = this.subject_key
               AND subject_key <> ''
               AND last_message_at >= this.date_utc - 30 days
               AND participants ∩ {from, to, cc} <> ∅
             ORDER BY last_message_at DESC LIMIT 1
    if none: thread = new mail_thread(...)

insert message with thread_id = thread
insert one mail_message_ref row per ref       # lets a later message link back
recompute thread aggregates (count, unread, last_message_at, participants, has_attachments)
```

The three guards on the subject fallback are the whole point, and each exists
for a named failure:

- **`subject_key <> ''`** — otherwise every no-subject message merges into one
  giant thread.
- **30-day window** — otherwise a yearly `"invoice"` from the same vendor forms
  one 40-message thread.
- **participant overlap** — otherwise two unrelated strangers who both send
  `"hello"` (exactly the sample mail) land in the same thread.

### Threads are scoped per mailbox

`owner@` and `seller@` never merge, even for the same `References` chain. They
are different inboxes from the user's point of view, and a message addressed to
both is stored twice by SES anyway — once per prefix — so each copy threads
inside its own mailbox.

The same rule, unchanged, is what keeps domains apart: `owner@hhoangg.com` and
`owner@byptah.com` are two mailboxes, so a sender who writes to both gets two
threads. That is the right answer — they are two conversations, with two
reply-from addresses — and it falls out of the existing scope without a special
case for domains.

### Out-of-order arrival and merging

S3 listing order is lexicographic by key, not chronological, so a reply can be
parsed before the message it answers. That is why `mail_message_ref` is a table
rather than a derived value: when the parent arrives later, its `Message-ID`
matches an existing `ref_hdr` row, both candidate threads surface, and
`merge_threads` reparents every message of the losing threads onto the lowest
thread id, moves `mail_folder_member` rows, recomputes aggregates and deletes
the empty threads. Merging is idempotent and runs inside the insert transaction.

### Outbound mail

A sent message generates its own `Message-ID`
(`<uuid-v4@{sending-domain}>`), carries `In-Reply-To` = the message replied to
and `References` = that message's `References` + its `Message-ID` (capped at 20
ids, keeping the first and the last 19, as mail clients conventionally do). It
is inserted with `direction='outbound'` into the same thread. This is why
replies thread correctly in the recipient's client too.

---

## 7. Rendering an email body

Two layers, both required.

### Layer 1 — sanitise in Rust, at parse time

`mail_mime.rs` picks the best body part: the last `text/html` part of the
top-level `multipart/alternative`, else the `text/plain` part converted to HTML
with escaped text and `<br>`, else empty.

`ammonia` then runs with an explicit allowlist:

- **Tags allowed**: the structural and inline set plus `table`/`thead`/`tbody`/
  `tr`/`td`/`th`/`colgroup`/`col` (email is table layout) and `img`.
- **Tags removed with contents**: `script`, `style`, `iframe`, `object`,
  `embed`, `applet`, `form`, `input`, `button`, `link`, `meta`, `base`, `svg`.
  `style` is dropped entirely rather than parsed — a CSS sanitiser is a second
  attack surface and inline `style` attributes carry most email styling anyway.
- **Attributes**: a per-tag allowlist. `on*` is impossible because it is not on
  it. `style` is allowed but passed through a small property filter that keeps
  colour/font/spacing/border/width/align and drops `position`, `z-index`,
  `fixed`, `behavior`, `expression`, `-moz-binding`, and anything containing
  `url(` that is not `data:` or `cid:`.
- **URLs**: schemes limited to `http`, `https`, `mailto`, `cid`, `data`
  (`data:` only for `img`). `rel="noopener noreferrer nofollow"` forced on
  links. `target` is stripped: links open through `tauri-plugin-opener`, not
  in the webview.
- **Remote images**: every `img` whose `src` is remote is rewritten to
  `src=""` with the original kept in `data-mono-src`, and a counter is returned
  alongside the HTML. That is what drives the "3 remote images blocked" banner.
  "Show images" asks Rust to fetch each one and hand back bytes, exactly as
  `fetch_inbox_media` already does for the Inbox. **This does not hide the
  user's IP** — Rust runs on the user's machine, so the moment they choose to
  load remote images the sender learns their address and roughly when the mail
  was read. What fetching in Rust actually buys: no cookies, no `Referer`, no
  webview cache or storage touched, no access to the app origin, and a guard
  that refuses `127.0.0.1`, RFC1918, `169.254.169.254` and `*.local`, which a
  webview fetch would happily reach. Say that in the UI, not the IP claim.
- **`cid:` inline images**: resolved at render time against
  `mail_attachment.content_id` and swapped for a blob URL.

The sanitised result is what goes into `mail_message.body_html`. The raw MIME is
never stored, so there is no path by which unsanitised HTML can be served from
the DB later.

### Layer 2 — isolate in the webview

Render into:

```html
<iframe
  sandbox="allow-same-origin"
  srcdoc="<meta http-equiv='Content-Security-Policy'
            content=&quot;default-src 'none'; img-src data: blob:;
                     style-src 'unsafe-inline'&quot;>…">
```

`font-src` is deliberately absent. `<style>` and `<link>` are stripped with
their contents in layer 1 and a `style` **attribute** cannot hold an at-rule, so
`@font-face` is unreachable and the frame can make no font request. If that
strip ever regressed, `style-src 'unsafe-inline'` would make `@font-face`
reachable — and then `default-src 'none'` is the fallback you want standing
there, not a permissive `font-src`.

- `sandbox` **without** `allow-scripts` — the email cannot run JavaScript even
  though scripts were already stripped. Defence in depth.
- `allow-same-origin` **is** included, for exactly two reasons, and both must be
  named or a later cleanup will delete one as unexplained: it lets the host read
  `contentDocument.body.scrollHeight` to size the iframe to its content, and it
  lets the host intercept link clicks inside the frame so they open through
  `tauri-plugin-opener` rather than navigating it. Without `allow-scripts` this
  grants the email nothing, because nothing in it can execute to use the
  same-origin privilege. The two flags are one decision: with both present the
  sandbox is fully escapable, and since the app CSP now carries
  `frame-src 'self'`, that would be a path to a same-origin frame able to call
  `invoke`. The §14 test asserting no `allow-scripts` is what holds this up.
- The inner CSP is a second wall: `default-src 'none'` means even a sanitiser
  bug that leaves a `<script src>` or a tracking pixel in place results in a
  blocked request, not a leak.
- No `allow-popups`, no `allow-forms`, no `allow-top-navigation`.

**This requires one change to `src-tauri/tauri.conf.json`**: `frame-src 'none'`
goes to `frame-src 'self'` in **both** `security.csp` and `security.devCsp` —
they are two separate full CSP strings and it is easy to fix only one and then
wonder why the dev build differs. That is the only CSP relaxation in this
feature, scoped to frames of the app's own origin. `connect-src` is untouched,
so the webview still cannot reach AWS.

Alternative considered and rejected: `dangerouslySetInnerHTML` into a scoped
container, which needs no CSP change. Rejected because email CSS then shares a
cascade with the app — one `td { display:flex }` in a newsletter reflows the
reading pane — and because there is no second wall if `ammonia`'s allowlist is
ever mis-specified.

A "Plain text" toggle renders `body_text` in a plain `<pre>`-ish block with
linkified URLs and no iframe at all. That is the safe path for anything
suspicious, and it is what the keyboard-driven reader will mostly use.

A "View raw source" action re-fetches the object from S3 and shows it in the
existing CodeMirror viewer — no new dependency, and it makes the sample
`Received:`/`DKIM-Signature` wall inspectable.

---

## 7.5 The composer — TipTap

**TipTap is for writing only. It never renders received mail.** Those are two
opposite jobs: §7 must faithfully display arbitrary hostile HTML, and TipTap
would silently drop everything outside its schema — a marketing email put
through it comes out mangled. Incoming HTML goes to the sanitised iframe;
TipTap only ever holds text the user typed.

### Extensions

`StarterKit` plus `Image`. In TipTap v3 StarterKit already carries bold,
italic, underline, strike, code, **link**, bullet and ordered lists, the list
keymap, blockquote, code block, hard break, horizontal rule, heading and
undo/redo, so the toolbar in the mockup needs no extra packages and no history
extension.

```ts
const extensions = [
  StarterKit.configure({
    heading: { levels: [1, 2, 3] },
    link: { openOnClick: false, autolink: true, protocols: ["http", "https", "mailto"] },
  }),
  Image.configure({ inline: false, allowBase64: false }),
];
```

StarterKit v3 also brings `TrailingNode`, which keeps an empty paragraph after
the last block — that is what lets you click below a quoted block or an image
and keep typing, so leave it on.

`openOnClick: false` because a click inside a draft must place the caret, not
navigate. `allowBase64: false` because an inline image belongs in
`mail_attachment` as a `cid:` part, not inlined into the body — base64 in the
HTML would bloat every draft row and break the 40 MB SES check in a way that is
hard to explain to the user.

### Output: three forms, one source

A draft's source of truth is the **TipTap JSON** in `mail_outbox.body_json`.
HTML is derived, and re-parsing HTML back into the editor is lossy, so a draft
reopened a week later restores from JSON.

On send, `src/features/mail/model/mailCompose.ts` derives both MIME parts:

1. **`text/html`** — `editor.getHTML()` is web HTML with classes and no inline
   styles, which many mail clients strip or ignore. A table-driven transform
   walks the known node set and emits inline styles (`p` →
   `margin:0 0 1em`, `blockquote` → `margin:0 0 1em;padding-left:12px;
   border-left:2px solid #d0d0d0`, `code` → monospace + a tint, `ul`/`ol` →
   explicit padding, and so on), then wraps the result in a minimal
   `<html><body style="font-family:…;font-size:14px;line-height:1.5">`.
   This is ~80 lines because the schema is small and ours; it is **not** a
   general CSS inliner, so no `juice`-style dependency is needed.
2. **`text/plain`** — `editor.getText({ blockSeparator: "\n\n" })` alone loses
   list markers and link targets, so the same walk emits `- ` / `1. ` prefixes,
   `> ` for quotes, and appends `(https://…)` after a link whose text is not the
   URL. A real text part is required: `multipart/alternative` with a stub text
   part is what gets a message scored as spam.
3. **`quoted_html`** is appended after both, unchanged.

### The quoted original

The message being replied to is kept **outside** the editor, as a non-editable
collapsed block under it, and appended at send time. The reason is the same one
as above: pouring arbitrary received HTML through TipTap's schema would rewrite
it. A **Quote inline** action inserts the *plain-text* rendering into the
editor instead, for people who answer point by point.

### Attachments and inline images

Paste or drop of an image inserts an `Image` node whose `src` is a blob URL and
whose `data-mono-att` is the pending attachment id. At send time each one
becomes a `multipart/related` part with a generated `Content-ID`, and the `src`
is rewritten to `cid:…`. Non-image files skip the editor entirely and go
straight to the attachment strip.

### Keyboard

StarterKit gives Cmd/Ctrl+B, I, U, the list keymap and undo/redo for free. The composer
adds Cmd+Enter to send and Escape to close. This does not fight the app's
global shortcuts: `App.tsx:12291` already excludes
`[contenteditable="true"]` from global shortcut handling, which is exactly what
a ProseMirror surface is. **Cmd+Shift+V** maps to
`editor.commands.insertContent(plainText)` for paste-as-plain-text.

### Styling

TipTap renders a bare `contenteditable` div; all of its appearance is ours. It
is styled with the app's own tokens through `editorProps`, not a TipTap theme
stylesheet:

```ts
editorProps: {
  attributes: {
    class:
      "min-h-[180px] px-3 py-2.5 text-[13px] leading-relaxed text-content " +
      "outline-none [&_p]:my-0 [&_p+p]:mt-3 " +
      "[&_a]:text-accent [&_a]:underline " +
      "[&_blockquote]:border-l-2 [&_blockquote]:border-content/20 [&_blockquote]:pl-3 " +
      "[&_code]:rounded [&_code]:bg-content/8 [&_code]:px-1 [&_code]:py-0.5",
  },
}
```

### Loading

The composer is behind `lazySurface` (`src/shared/ui/lazySurface.tsx`), the
same helper `App.tsx` uses for `AutomationsView`, with a `.preload` fired when
the Mail view mounts. ProseMirror is a few hundred KB and nobody who only reads
mail should pay for it.

---

## 8. Sending

### 8.1 Signing

`aws_sigv4.rs` implements SigV4 for `ureq`: canonical request, SHA-256 payload
hash, `AWS4-HMAC-SHA256` string to sign, scope `date/region/service/aws4_request`,
`Authorization` header. Used by both `s3` and `ses` services, with
`x-amz-content-sha256` on S3 requests. Clock skew beyond 5 minutes produces a
specific error message, because `RequestTimeTooSkewed` is otherwise baffling.

### 8.2 Calls used

| Purpose | Service | Call |
|---|---|---|
| List a prefix | S3 | `GET /?list-type=2&prefix=&continuation-token=` |
| Read a message | S3 | `GET /{key}` |
| Delete messages | S3 | `POST /?delete` (`DeleteObjects`, up to 1000 keys) |
| Store a sent copy | S3 | `PUT /sent/{uuid}` |
| Discover prefixes | S3 | `GET /?list-type=2&delimiter=/` → `CommonPrefixes` |
| Send | SES v2 | `POST /v2/email/outbound-emails` with `{"FromEmailAddress":…,"Destination":{…},"Content":{"Raw":{"Data":"<base64 MIME>"}}}` |
| Verified identities | SES v2 | `GET /v2/email/identities` |
| Quota / sandbox check | SES v2 | `GET /v2/email/account` |

Raw send is used rather than `Simple` content because reply threading needs
`In-Reply-To` and `References` headers, which the simple form cannot set.

Every call is made **with the credential and region of the source that owns the
From mailbox**. This is the one place multi-domain can go wrong quietly: sending
`owner@byptah.com` through the `acme-ltd` key in `eu-west-1` fails with
`MessageRejected: Email address is not verified`, which reads like a DNS problem
and is not. So the From address picks the source, never the other way round, and
`mail_outbox` stores `account_id` at draft time so a later retry uses the same
one.

Replying defaults to the mailbox the message arrived in. The user may switch to
any other sendable mailbox; if the chosen address is not `verified` in that
source's `mail_identity`, the compose sheet blocks Send with the reason rather
than letting SES reject it after the fact.

### 8.3 Build

`mail-builder` produces `multipart/mixed( multipart/alternative(text/plain,
text/html), attachment* )`. Headers set explicitly: `From`, `To`, `Cc`, `Bcc`
(stripped from the MIME, passed only in `Destination`), `Subject`, `Date`,
`Message-ID`, `In-Reply-To`, `References`, `MIME-Version`.

The 40 MB SES raw limit is checked before the call and reported as a form error,
not an API failure. Base64 inflates by 4/3, so the check is on the encoded size.

### 8.4 After a successful send

Three writes, one transaction:

1. `mail_outbox.state='sent'`, `ses_message_id`, `sent_at`.
2. `mail_message` row with `direction='outbound'`, threaded by §6, FTS updated,
   smart folders re-evaluated. The message appears in the thread immediately —
   it is not waited for from S3.
3. `PutObject` of the raw MIME to the **sending source's** own
   `s3://bucket/sent/{message_id}.eml`. Each source keeps its own `sent/`
   prefix, so sent mail stays next to the inbound mail of the same domain, and
   a source whose key has no `s3:PutObject` degrades to local-only sent mail
   without affecting the others.

Step 3 is what keeps S3 the source of truth: a second machine syncing the same
bucket sees sent mail too, and `sent/` is registered as a mailbox with
`is_sendable = 0` so the normal sync path parses it. Insert in step 2 is keyed on
`UNIQUE(mailbox_id, message_id_hdr)`, so when the sync later lists that object
it updates rather than duplicates.

A send failure leaves `state='failed'` with `last_error`; the thread shows the
message with a retry affordance. Sends are not retried automatically — a
half-delivered email resent silently is worse than a visible failure.

---

## 9. Filters and smart folders

### Rule model

One `mail_folder` + N `mail_folder_rule`. The same model backs the ad-hoc filter
chips in the thread list, which is why "Save as folder" is a pure data copy and
not a re-implementation.

Fields: `from`, `to`, `cc`, `recipient` (to ∪ cc), `subject`, `body`, `mailbox`,
`domain`, `account`, `has_attachment`, `date`, `is_unread`, `is_starred`.
`domain` and `account` are what scope a folder to one or some of the configured
sources; with neither, the folder spans every domain.
Operators: `is`, `contains`, `contains_any`, `starts_with`, `ends_with`,
`matches` (SQLite `GLOB`, not a regex — no regex engine in the DB and no
appetite for a user-supplied one), `before`, `after`, `within_days`, `true`.
Each rule has a `negate` flag; `match_mode` is `all` or `any`.

### Evaluation

Rules compile to a parameterised `SELECT mail_thread.id` with:

- scalar fields → `LIKE`/`GLOB`/`=` on the message columns, wrapped in
  `EXISTS (SELECT 1 FROM mail_message m WHERE m.thread_id = t.id AND …)` so a
  thread matches when **any** of its messages does;
- `subject`/`body` `contains*` → an FTS5 `MATCH` subquery, which is what makes
  body search fast on thousands of messages;
- `mailbox` → `t.mailbox_id = ?`.

No string interpolation of user values, ever. Values are bound parameters; the
only thing the rule controls is which operator template is chosen.

Membership is materialised into `mail_folder_member`:

- on message insert, evaluate that one thread against every folder (cheap:
  N folders × 1 thread), regardless of which source it came from;
- on folder create or rule edit, re-evaluate every thread and replace the
  folder's rows ("Apply to mail already stored");
- on thread merge, union the members of the merged threads.

`hide_from_mailbox = 0` (the default) makes a folder behave like a Gmail label:
the thread stays in its mailbox *and* shows in the folder, so a thread can be in
several folders at once. `= 1` makes it behave like a real folder.

### Search box

`from:`, `to:`, `cc:`, `subject:`, `body:`, `has:attachment`, `is:unread`,
`is:starred`, `in:owner@…`, `domain:byptah.com`, `source:byptah-mail`,
`after:`/`before:`, bare words → `body` contains.
The parser's output is a `Vec<Rule>` — the identical type the rule editor and
`mail_folder_rule` use. One grammar, one evaluator, three entry points.

---

## 10. Delete and archive

Both the toolbar and the context menu open the dialog in screen 5 of the
mockup. There is no silent destructive default.

- **Archive locally** — `mail_message.state='archived'` for the thread's
  messages, `mail_thread.state='archived'`, `archived_at` set. S3 untouched.
  The next sync still lists those objects, so step 3 of §5 must not resurrect
  them: a `gone` object archives a message, but an already-`archived` message is
  never set back to `active` by a sync. Reversible from the Archive folder.
- **Delete on S3 as well** — requires typing `delete`. Issues `DeleteObjects` for
  every `mail_object.s3_key` in the thread, then deletes the local rows
  (cascade clears refs, attachments, folder membership, FTS). The dialog states
  plainly that the S3 object is the only copy of the original MIME unless bucket
  versioning is on. Partial failures (per-key `Error` entries in the
  `DeleteObjects` response) are reported and the corresponding local rows are
  kept.
- `mail_attachment.local_path` files are removed on either path.
- "Remember my choice for this session" is session-scoped only, never persisted.
  A persisted "always hard delete" is a foot-gun that outlives the intent.

---

## 11. Frontend wiring

### 11.1 Design system: reuse, do not invent

The app has no component library — no shadcn, no Radix, no Headless UI. It is
Tailwind v4 utility classes inline, over the custom tokens in
`src/styles/index.css`, with a thin set of shared components. Mail must look
like it shipped with the app, so every piece below comes from what already
exists.

**Shared components that cover most of the feature** (`src/shared/ui/`):

| Need | Use | Notes |
|---|---|---|
| Any button | `SecondaryButton.tsx` | The only shared button. Has a `danger` prop — that is the Delete-on-S3 button. |
| Compose / smart-folder / delete dialogs | `Modal.tsx` (`Modal`, `ModalPanel`) | Portals to `document.body` at `LAYER.dialog`, Escape handler, backdrop close. Sizes `sm` (420px) and `md` (560px); compose needs a wider panel, so pass `className` rather than adding a third size. |
| From picker, mailbox menu, row context menu | `Popover.tsx` | Anchors to an element, ref, rect **or a bare point** — the point form is what makes it a context menu. `role="menu"` + hand-styled `role="menuitem"` buttons, as `MonoRailSection.tsx:318` does. |
| Region / credential / rule-field dropdowns | `SearchableSelect.tsx` | Has a `variant` of `field \| transparent \| row \| panel \| pill`. |
| Mailbox colour, smart-folder colour | `ColorPickerPopover.tsx` (`ColorPickerPopover`, `ColorSwatchRow`) | Already exactly the swatch row in the mockup. |
| `date` rule values | `DateTimePicker.tsx` | Plus its `toLocalDateTime` / `parseLocalDateTime` helpers. |
| Resizable thread-list pane | `useDragResize` (`src/shared/hooks/useDragResize.ts`) | What `InboxView`, `Sidebar`, `NotesView` already use. Writes width straight to the DOM. The **folder column is fixed at 232px**, as the mockup pins it, so `monocode.mail.listWidth` is the only width key and there is no folder-width key to add. |
| Search-match highlighting in the thread list | `MatchText.tsx` | |
| Lazy-loading `MailView` and the composer | `lazySurface.tsx` | |

**Things that do not exist and must not be invented either.** The house
convention is inline classes, and copying it keeps Mail from looking like a
different app:

- **No Tooltip.** Hover hints are native `title=`.
- **No Badge/Pill/Chip.** The mockup's pills are inline classes. Unread counts,
  the `owner@` chip and the `verified` tag are each a `<span>` with
  `rounded-full border border-content/10 bg-content/5 px-1.5 text-[10px]`.
- **No Spinner.** `<Loader className="size-3 animate-spin" />` from `icons.tsx`.
- **No EmptyState.** `<p className="px-3 py-2 text-[12px] text-content/50">`.
- **No Tabs.** Hand-built `role="tab"` buttons, as `SurfaceTabs.tsx:358` does.
- **No primary/ghost/icon button.** Inline classes. Copy `SecondaryButton`'s
  shape: `rounded-md border border-content/10 px-2.5 py-1 text-[12px]
  text-content/70 hover:bg-content/10 hover:text-content
  focus-visible:outline-2 focus-visible:outline-accent`.
- **Danger styling is raw Tailwind**, not a token: `text-red-400` for a quiet
  danger button, `bg-red-500/20 text-red-300` for the confirm action. See
  `src/features/monos/ui/ConfirmReset.tsx` — the delete dialog should be built
  from that file almost line for line.

**Tokens.** Use `text-content` with opacity steps for hierarchy
(`/45` secondary, `/50` muted, `/70` on controls), `border-content/10` for
control borders, `border-stroke` for structural separators, `bg-content/3` for
card fills, `bg-selection` for a selected row, and `accent` for the active
state. Font sizes are arbitrary values by convention: `text-[11px]`,
`text-[12px]`, `text-[13px]`.

**Settings primitives need extracting first.** `Group`, `Row`, `Toggle`,
`Select`, `Segmented` and `Slider` all live inside
`src/features/settings/ui/SettingsView.tsx` and are **file-local, not
exported** (`Group` at ~L4238, `Row` at ~L4283, `Toggle` at ~L4511, `Select` at
~L4547). The Mail settings page cannot render natively without them.

Move them to `src/features/settings/ui/primitives.tsx` and import them back
into `SettingsView.tsx`. That is a mechanical edit to an upstream-owned file —
a real merge cost — but the alternative is copying them, which guarantees the
Mail page drifts the first time upstream restyles a row. Pay the small cost
once.

**The thread list is not virtualized, because nothing here is.**
`package.json` has no react-window, virtua or tanstack-virtual. The pattern is
progressive windowing: `src/shared/lib/listWindow.ts` exports
`LIST_PAGE_SIZE = 32` and `listWindowSize()`, and `InboxView.tsx:906-915` grows
the window with an `IntersectionObserver` on a sentinel, rooted at the scroll
container with `rootMargin: "240px"`. Follow it exactly.

This is the one place Mail may outgrow the house pattern: an inbox of 10k+
threads scrolled to the bottom means 10k live DOM rows. Ship the windowed
version in phase 2, and only reach for a virtualizer when a real mailbox
proves it is needed — adding one speculatively means a dependency and a
scroll-behaviour difference from every other list in the app.

### Icons

`src/shared/ui/icons.tsx` wraps Hugeicons (`@hugeicons/react ^1.1.10`,
`@hugeicons/core-free-icons ^4.3.0`). Already there and reusable: `Inbox`,
`Archive`, `Search`, `Trash2`, `Star`, `Folder`, `ListFilter`, `Plus`, `X`,
`Check`, `RefreshCw`, `Loader`, `MoreHorizontal`, `ExternalLink`, `Lock`,
`Clock`, `Pencil`, `ChevronDown`, `ChevronRight`, `ImagePlus`, `ListBullet`,
`Undo2`.

Add the rest in the same file, in the same style — **deep imports**, because
the file's own header warns that the barrel pulls in a 5 MB catalog, and
`wrap()` which defaults `strokeWidth` to `1.75`:

```tsx
import Mail01Icon from "@hugeicons/core-free-icons/Mail01Icon";
import MailSend01Icon from "@hugeicons/core-free-icons/MailSend01Icon";
import MailReply01Icon from "@hugeicons/core-free-icons/MailReply01Icon";
import MailReplyAll01Icon from "@hugeicons/core-free-icons/MailReplyAll01Icon";
import Attachment01Icon from "@hugeicons/core-free-icons/Attachment01Icon";
import TextBoldIcon from "@hugeicons/core-free-icons/TextBoldIcon";
import TextItalicIcon from "@hugeicons/core-free-icons/TextItalicIcon";
import TextUnderlineIcon from "@hugeicons/core-free-icons/TextUnderlineIcon";
import TextStrikethroughIcon from "@hugeicons/core-free-icons/TextStrikethroughIcon";
import LeftToRightListNumberIcon from "@hugeicons/core-free-icons/LeftToRightListNumberIcon";
import QuoteDownIcon from "@hugeicons/core-free-icons/QuoteDownIcon";
import Link01Icon from "@hugeicons/core-free-icons/Link01Icon";
import CodeXmlIcon from "@hugeicons/core-free-icons/CodeXmlIcon";
import CloudIcon from "@hugeicons/core-free-icons/CloudIcon";
import Key01Icon from "@hugeicons/core-free-icons/Key01Icon";

export const Mail = wrap(Mail01Icon, "Mail");
export const Send = wrap(MailSend01Icon, "Send");
export const Reply = wrap(MailReply01Icon, "Reply");
export const ReplyAll = wrap(MailReplyAll01Icon, "ReplyAll");
export const Paperclip = wrap(Attachment01Icon, "Paperclip");
export const Bold = wrap(TextBoldIcon, "Bold");
export const Italic = wrap(TextItalicIcon, "Italic");
export const Underline = wrap(TextUnderlineIcon, "Underline");
export const Strikethrough = wrap(TextStrikethroughIcon, "Strikethrough");
export const ListOrdered = wrap(LeftToRightListNumberIcon, "ListOrdered");
export const Quote = wrap(QuoteDownIcon, "Quote");
export const Link = wrap(Link01Icon, "Link");
export const Code = wrap(CodeXmlIcon, "Code");
export const Cloud = wrap(CloudIcon, "Cloud");
export const Key = wrap(Key01Icon, "Key");
```

`ListBullet` already wraps `LeftToRightListBulletIcon`, so `ListOrdered` taking
`LeftToRightListNumberIcon` keeps the pair from the same glyph family.
Forward reuses the existing `CornerDownRight`, or add `ArrowTurnForwardIcon`
under a `Forward` alias.

Usage is `<Mail className="size-3.5" />`; `strokeWidth` can be omitted since
`wrap()` already defaults it to the house value.

### New files

```
src/features/mail/model/mailTypes.ts        shared types mirroring the Rust structs
src/features/mail/model/mail.ts             invoke wrappers
src/features/mail/model/mailQuery.ts        search-string ↔ Rule[] parser  (+ .test.ts)
src/features/mail/model/mailCompose.ts      TipTap doc → email HTML + text  (+ .test.ts)
src/features/mail/model/mailPrefs.ts        localStorage prefs + change events
src/features/mail/ui/MailView.tsx           4-pane shell
src/features/mail/ui/MailFolderList.tsx
src/features/mail/ui/MailThreadList.tsx
src/features/mail/ui/MailThreadRead.tsx
src/features/mail/ui/MailMessageBody.tsx    the sandboxed iframe + height sync
src/features/mail/ui/ComposeSheet.tsx        the Modal, address fields, send
src/features/mail/ui/MailEditor.tsx         the TipTap instance, lazy-loaded
src/features/mail/ui/MailEditorToolbar.tsx  formatting buttons
src/features/mail/ui/SmartFolderSheet.tsx
src/features/mail/ui/DeleteMailDialog.tsx
src/features/settings/ui/MailSettings.tsx        provider list + source list
src/features/settings/ui/MailCredentialRow.tsx   one AWS key pair
src/features/settings/ui/MailSourceCard.tsx      one bucket/region + its mailboxes
```

### Sidebar with several domains

```
▸ All mail                 12      ← unified, every source
  hhoangg.com               7      ← group header, derived from address
    owner@                  5
    seller@                 2
  byptah.com                5  ⚠   ← per-source error badge
    owner@                  5
    support@                0
  acme.com                  0
    hello@                  0
▸ Smart folders                    ← global, span every domain
    Invoices               12
    Buyers                  3
```

- Group headers come from `mail_mailbox.domain`, collapse, and remember their
  state in `monocode.mail.collapsedDomains`.
- A domain served by two sources (different regions) shows one group; the
  source is a detail on the mailbox row, not another level of nesting. Three
  levels of tree for a mail sidebar is one too many.
- The unread badge on the rail is the sum over enabled sources.
- The thread list shows a domain chip next to the mailbox chip whenever the
  current scope spans more than one domain, and hides it when it does not.

### Existing files that must change

| File | Change |
|---|---|
| `src/shared/ui/icons.tsx` | Add the icons listed in §11.1, deep-imported, via `wrap()`. `Archive`, `Folder`, `ListFilter`, `Star`, `Trash2`, `Inbox`, `ImagePlus`, `ListBullet`, `Loader` already exist. |
| `package.json` | The five TipTap packages in §3. |
| `src/app/shell/Sidebar.tsx` | Add `onOpenMail` / `mailActive` to the props type, the destructure and the three pass-throughs; add an `IconButton` in the expanded row (~L2407-2475) and a `CompactRailAction` in the 48px rail (~L2682-2715); include `mailActive` in the two "a non-workspace view is open" checks (~L795, ~L2583). Use the Automations entry as the template. |
| `src/app/App.tsx` | `mailViewOpen` state + ref mirror (~L1261); `onOpenMail` callback (~L11866 pattern); `setMailViewOpen(false)` in **every** other view opener — grep `setAutomationsViewOpen` and match each site; `lazySurface` import of `MailView` (~L838) and its `.preload` (~L1567); render branch (~L13399); the layout/visibility conditions that currently test `automationsViewOpen` (L1514, 1521, 2192, 5577, 12004, 12301, 12384, 12849, 12971, 13037-13053, 13279, 13444). |
| `src/features/settings/model/settings.ts` | `"mail"` in the `SettingsSectionId` union (L20-32); a `SETTINGS_SECTIONS` entry in group `workspace` (after `inbox`); `SETTINGS_INDEX` rows for the searchable controls; `loadMailEnabled` / `saveMailEnabled` next to `loadNotesEnabled` (L791-802). |
| `src/app/shell/SettingsRail.tsx` | `mail: Mail` in `SECTION_ICONS` (L19-32). It is a `Record`, so `tsc --noEmit` fails until this is added. |
| `src/features/settings/ui/SettingsView.tsx` | `{section === "mail" ? <MailSettings /> : null}` next to the existing branches (L595-639). Plus **extract** `Group`, `Row`, `Toggle`, `Select`, `Segmented`, `Slider` into `primitives.tsx` and import them back — see §11.1. |
| `src-tauri/src/lib.rs` | `mod mail;` and friends; the commands in `generate_handler!` (~L297+); `mail::init(app.handle())` in `.setup()` (~L268). |
| `src-tauri/src/session_store.rs` | One line in `migrate()` (~L1005): `crate::mail_store::ensure_tables(conn)?;` |
| `src-tauri/tauri.conf.json` | `frame-src 'none'` → `'self'` in both `security.csp` and `security.devCsp`. Nothing else. |
| `src-tauri/Cargo.toml` | The four crates in §3. |

Everything else is new files, which keeps the merge surface against upstream
small — the pattern this fork already follows.

### State

No state library in this repo, so: local `useState` inside `MailView` for
selection and pane state, `useSyncExternalStore` over `localStorage` for prefs
(`flagStore()` in `displayPrefs.ts` is the template), and Tauri events for
server-pushed change.

localStorage keys keep the mandated `monocode.` prefix (CLAUDE.md, "Never
rename these"):

```
monocode.mail.enabled
monocode.mail.selectedMailbox
monocode.mail.selectedFolder
monocode.mail.collapsedDomains    // JSON array of domain names
monocode.mail.syncIntervalSeconds // app-wide, applies to every source
monocode.mail.listWidth
monocode.mail.readingPaneSide     // right | bottom | off
monocode.mail.density             // comfortable | compact
// No showImages pref. The image policy lives in mail_account.image_policy and
// Rust enforces it, so a localStorage mirror would be a second source of truth
// that the enforcement point ignores. The settings control writes the DB.
```

Events, following the existing `monocode:<thing>change` convention:
`monocode:mail-enabled-change`. Rust → webview events use the `mail://` prefix:
`mail://sync-state`, `mail://messages-changed`, `mail://outbox-changed`.

### Tauri commands

```
mail_status() -> MailStatus                       // every source, counts, per-source last sync + error

mail_credentials() -> Vec<CredentialRow>          // masked key ids only
mail_credential_upsert(label, keys) -> CredentialRow   // writes keychain, validates
mail_credential_delete(id)                        // refused while a source uses it

mail_sources() -> Vec<SourceRow>
mail_source_upsert(src) -> SourceRow              // credential + region + bucket
mail_source_delete(id)
mail_source_test(id) -> TestReport                // S3 list + SES identities + sandbox state
mail_source_detect_prefixes(id) -> Vec<String>
mail_mailboxes_upsert(account_id, rows)           // rejects a duplicate address, names the holder
mail_mailboxes_delete(id)

mail_sync_now(scope: SyncScope) -> Vec<SyncReport>  // all | source(id) | mailbox(id)
                                   // mailbox(id) syncs that mailbox's whole
                                   // source: the scheduler works per source.
mail_sync_interval_set(secs)       // Rust cannot read localStorage, so the
                                   // webview MUST push the interval at startup
                                   // and on every settings change. Without it
                                   // the interval stays at the 3600 default
                                   // whatever the user sets.
mail_threads(query: ThreadQuery) -> Page<ThreadRow>        // mailbox|folder|rules, cursor
mail_thread(thread_id) -> ThreadDetail                     // messages + attachments
mail_message_raw(message_id) -> String                     // re-fetch from S3
mail_mark(thread_ids, read: bool) / mail_star(...)
mail_archive(thread_ids)
mail_delete(thread_ids, hard: bool) -> DeleteReport
mail_attachment_open(attachment_id) -> PathBuf             // downloads if needed
mail_attachment_bytes(attachment_id) -> Vec<u8>            // for inline images
mail_remote_image(message_id, url) -> Vec<u8>              // proxied, never webview fetch

mail_folders() -> Vec<FolderRow>
mail_folder_upsert(folder, rules) -> FolderRow
mail_folder_delete(id)
mail_folder_preview(rules) -> PreviewCount                 // the live count in the editor

mail_outbox_save(draft) -> i64
mail_outbox_send(id) -> SendReport
mail_outbox_list() / mail_outbox_delete(id)
mail_identities(account_id: Option<i64>) -> Vec<Identity>
mail_sendable_mailboxes() -> Vec<SendableMailbox>  // the From picker, grouped by domain
```

---

## 12. Credentials

- Stored with `keyring::Entry`, the helpers in `codex_mono_store.rs:288-304`
  being the existing precedent. Service `com.monoptah.desktop`, keys
  `mail.aws.cred.<credential_id>.access_key_id` and `….secret_access_key`.
  Chosen over the `clickup-config.json` file pattern because this is an IAM key
  pair, not a scoped app token.
- **Keyed by credential, not by source.** Two domains in the same AWS account
  share one keychain entry, so rotating that key is one edit and not one per
  domain. A source referencing a credential keeps only its id; deleting a
  credential still in use is refused (`ON DELETE RESTRICT`) and the error names
  the sources holding it.
- Separate S3 credentials are supported — the S3 key for reading mail can be a
  different, read-only key from the SES sending key. It is just another
  `mail_credential` row pointed at by `mail_account.s3_credential_id`, with
  "Same as SES" (NULL) the default in the UI. Nothing special, which is the
  point: the same mechanism covers "one read-only S3 key shared by every
  domain".
- Non-secret fields (region, bucket, prefixes, from address) live in
  `mail_credential` / `mail_account` / `mail_mailbox` in SQLite, where the UI
  can read them.
- `MailStatus` returns each access key id **masked** (`AKIA••••3K7Q`) and never
  a secret. There is no command that reads a secret back out.
- Settings copy states the minimum IAM policy, and the error path names the
  missing action *and the source it failed on* (screen 7 of the mockup):
  `s3:ListBucket` on the bucket, `s3:GetObject`/`s3:PutObject` on `bucket/*`,
  `s3:DeleteObject` only if hard delete is to be used, `ses:SendEmail`,
  `ses:ListEmailIdentities`, `ses:GetAccount`. With several sources the policy
  shown is per source, because each bucket needs its own resource ARN.
- `mail_source_test(account_id)` tests one source; the settings page runs
  it for each and shows a per-row result. A single aggregate "connected" light
  would hide exactly the case this feature introduces — three domains fine, one
  broken.

---

## 13. Branding

New user-visible copy says **Monoptah**, never MonoCode, and any link points at
`hhoangg/monoptah`. Run `./scripts/check-fork-branding.sh` before calling the
work done — it gates on exactly this.

---

## 14. Tests

Rust, `cargo test`, using `SessionStore::open_in_memory()` (`notes.rs:627` is
the template):

- `subject_key` normalisation: `Re:`, `RE:`, `Fwd:`, `[list]`, `Re: Re:`,
  Vietnamese `Trả lời:`, count forms `Re[2]:`, whitespace, diacritics.
- Threading: reply links by `In-Reply-To`; links by a mid-chain `References` id;
  **out-of-order** insert (reply first, parent second) merges into one thread;
  two different senders both sending `"hello"` do **not** merge; same subject
  107 days apart does not merge; same chain in `owner/` and `seller/` stays two
  threads; the same chain reaching `owner@hhoangg.com` and `owner@byptah.com`
  stays two threads; a 3-way merge reparents all messages and leaves no orphan thread.
- Multi-source: two sources sharing one credential serialise, two on different
  credentials do not; one source failing leaves the others' `last_error` NULL
  and their threads readable; a duplicate mailbox address across sources is
  rejected and the error names the existing source; deleting a credential still
  referenced is refused; the same domain verified in one region and not another
  yields `is_sendable` true for one mailbox and false for the other; a sent
  message lands in the sending source's `sent/` prefix and not another's.
- Sync reconcile: new key inserts; vanished key archives and does not delete;
  re-listed key does not un-archive a user-archived thread; etag change
  re-parses; `DeleteObjects` partial failure keeps the local row.
- MIME parse of `work/` fixtures including the real sample mail: encoded-word
  `From`, `multipart/alternative`, `Date` vs `Received` divergence, SES
  `Authentication-Results` → spf/dkim/dmarc columns.
- Sanitiser: `<script>` removed; `onerror=` removed; `javascript:` href removed;
  `<style>` removed; remote `img` rewritten and counted; `cid:` preserved;
  `position:fixed` dropped from `style`; a table layout survives.
- Rule compilation: every field × operator pair produces valid SQL with bound
  parameters; a value containing `%`, `_`, `'` and `*` matches literally;
  `negate` and `any`/`all` combine correctly.
- Outgoing MIME: `References` capping at 20; `Bcc` absent from the MIME but
  present in `Destination`; attachment base64 round-trips.
- SigV4: the AWS published test-suite vectors for the canonical request and
  string-to-sign.

Vitest:

- `mailQuery` parse ↔ serialise round-trip, and that it produces the same
  `Rule[]` the editor produces for the equivalent chip set.
- No paid TipTap: `package.json` declares no dependency under the
  `@tiptap-pro/` scope, and no `.npmrc` in the repo maps a scope to
  `registry.tiptap.dev`. A two-line test, and the only thing that reliably
  stops a Pro package arriving as a transitive "just this once".
- `mailCompose`: a TipTap document containing every supported node round-trips
  to HTML with inline styles on each one; the plain-text alternative keeps
  `- ` / `1. ` list markers, `> ` quote prefixes and appends a link's URL when
  the link text is not the URL; an inline image becomes a `cid:` reference plus
  a matching attachment entry; `Bcc` never appears in the generated body;
  reopening a draft from `body_json` reproduces the same HTML — the regression
  test for anyone who later "simplifies" drafts to store HTML only.
- Thread-list grouping/sorting and relative date formatting.
- `MailMessageBody` renders an iframe with no `allow-scripts` in its `sandbox`
  attribute — a regression test for the one thing that must never be relaxed.
- `MailEditor` is never handed a received message's HTML: the compose surface
  only ever mounts with `body_json` or empty content.

Verify with `npm run check > /tmp/check.log 2>&1; echo $?`. Never through a
pipe (CLAUDE.md: `tail`'s exit status is always 0).

---

## 15. Phases

Each phase ends green on `npm run check` and is independently useful.

1. **Store + sync.** `mail_store.rs`, `aws_sigv4.rs`, S3 list/get,
   `mail_mime.rs` parse, threading, the hourly thread. No UI beyond a settings
   form and a sync button. This is where the risk is, so it goes first.
   Build it multi-source from the start — `credential → source → mailbox` is
   cheap to write now, and later it is a migration plus a rewrite of the sync
   loop, the From picker and the folder scope.
2. **Read UI.** Rail item, `MailView`, folder/mailbox/thread list, reading pane,
   sanitised body in the sandboxed iframe, attachments on demand, read/star.
3. **Filters and smart folders.** Chips, search grammar, rule editor,
   materialised membership, FTS.
4. **Archive and delete,** with the two-way dialog and the no-resurrect rule.
5. **Send.** The TipTap composer (§7.5) and its HTML/text serializer, SES raw
   send, reply / reply-all / forward, outbox, `sent/` prefix write-back. The
   serializer is the risky half, not the editor — write its tests first.
6. **Polish.** Keyboard map (`j`/`k`, `Enter`, `r`, `a`, `e`, `#`, `/`), density,
   reading-pane position, unread badge on the rail, empty and error states.

**Open questions** — each has a working default, so none of them blocks a start:

- Does the bucket have versioning on? It decides whether hard delete is truly
  irreversible, and the dialog copy should say which. Default: assume no.
- `sent/` as a real S3 prefix needs an IAM `PutObject`. If the key is read-only,
  phase 5 falls back to local-only sent mail and the setting is disabled with a
  reason.
- SES sandbox mode only sends to verified addresses, and it is per AWS account
  **and** region, so one source can be in sandbox while another is not. Phase
  1's `mail_source_test` surfaces it per source, and the compose sheet warns
  before a send that would be rejected.
- How many domains, and do they share one AWS account? If they all do, the
  whole thing is one credential and one or two sources, and the settings page
  looks almost like the single-domain mockup. The schema does not care either
  way; this only decides how much of the settings UI is worth polishing in
  phase 1.
