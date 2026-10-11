import { invoke } from "@tauri-apps/api/core";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import type { Editor } from "@tiptap/react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  attachmentsFromFiles,
  attachmentsFromPaths,
  isVisionImage,
  pickAttachments,
} from "../../sessions/model/attachments";
import type { Attachment } from "../../sessions/model/session";
import {
  ChevronDown,
  Loader,
  Paperclip,
  Plus,
  Send,
  Trash2,
  X,
} from "../../../shared/ui/icons";
import { Modal } from "../../../shared/ui/Modal";
import { Popover } from "../../../shared/ui/Popover";
import { SecondaryButton } from "../../../shared/ui/SecondaryButton";
import { lazySurface } from "../../../shared/ui/lazySurface";
import {
  mailIdentities,
  mailOutboxCheck,
  mailOutboxDelete,
  mailOutboxList,
  mailOutboxRearm,
  mailOutboxSave,
  mailOutboxSend,
  mailSendableMailboxes,
  mailStatus,
  mailThread,
} from "../model/mail";
import {
  contentIdFor,
  serializeCompose,
  type TipTapNode,
} from "../model/mailCompose";
import {
  SENDER_PROBLEM_LABEL,
  addressLabel,
  buildQuote,
  defaultFrom,
  parseRecipients,
  prefixedSubject,
  recipientsFor,
  senderGroups,
  senderOk,
  sendMode,
  type ComposeRequest,
  type Quote,
  type SenderEntry,
  type SenderGroup,
} from "../model/mailReply";
import type {
  MailAddress,
  OutboxAttachment,
  OutboxKind,
  OutboxRow,
  OutboxState,
  SendProblem,
  SendReport,
} from "../model/mailTypes";
import type { MailEditorProps } from "./MailEditor";

/**
 * TipTap is a few hundred KB of ProseMirror that someone who only reads mail
 * should not download, so the editor is its own chunk.
 */
const LazyMailEditor = lazySurface<MailEditorProps>(
  () => import("./MailEditor"),
);
export const preloadMailEditor = () => LazyMailEditor.preload();

const AUTOSAVE_MS = 1500;
const FROM_NOTE = "Choose a From address before this can be saved.";
const DISCARD_NOTE = `${FROM_NOTE} Close again to discard it.`;

const TITLE: Record<OutboxKind, string> = {
  new: "New message",
  reply: "Reply",
  reply_all: "Reply all",
  forward: "Forward",
};

/** A file waiting to go out. `inline` ones are referenced from the body by id. */
type Pending = {
  id: string;
  name: string;
  mime: string;
  path: string;
  size: number;
  inline: boolean;
  /** Object URL shown by the editor's Image node. */
  previewUrl: string | null;
};

type Recipients = { to: MailAddress[]; cc: MailAddress[]; bcc: MailAddress[] };
type Field = keyof Recipients;

const message = (err: unknown): string =>
  typeof err === "string"
    ? err
    : err instanceof Error
      ? err.message
      : typeof (err as { message?: unknown } | null)?.message === "string"
        ? (err as { message: string }).message
        : String(err);

const sizeLabel = (bytes: number) =>
  bytes < 1024
    ? `${bytes} B`
    : bytes < 1024 * 1024
      ? `${Math.round(bytes / 1024)} KB`
      : `${(bytes / (1024 * 1024)).toFixed(1)} MB`;

const asDoc = (value: unknown): TipTapNode | null => {
  let v = value;
  if (typeof v === "string") {
    try {
      v = JSON.parse(v);
    } catch {
      return null;
    }
  }
  return v && typeof v === "object" && (v as TipTapNode).type === "doc"
    ? (v as TipTapNode)
    : null;
};

function mapImages(
  node: TipTapNode,
  fn: (n: TipTapNode) => TipTapNode,
): TipTapNode {
  const next = node.type === "image" ? fn(node) : node;
  return next.content
    ? { ...next, content: next.content.map((c) => mapImages(c, fn)) }
    : next;
}

function base64Blob(data: string, mime: string): Blob {
  return new Blob([Uint8Array.from(atob(data), (c) => c.charCodeAt(0))], {
    type: mime,
  });
}

/** Gives a picked, pasted or dropped file a path on disk and, for images, a preview. */
async function stage(
  file: Attachment,
  inline: boolean,
): Promise<Pending | null> {
  const path =
    file.path ??
    (file.data
      ? await invoke<string>("mail_stage_attachment", {
          name: file.name,
          data: file.data,
        })
      : null);
  if (!path) return null;
  const wantsPreview = inline && isVisionImage(file.mimeType);
  const previewUrl = !wantsPreview
    ? null
    : (file.previewUrl ??
      (file.data
        ? URL.createObjectURL(base64Blob(file.data, file.mimeType))
        : null));
  return {
    id: file.id,
    name: file.name,
    mime: file.mimeType,
    path,
    size: file.size,
    inline: inline && previewUrl !== null,
    previewUrl,
  };
}

const toAttachment = (
  p: Pending,
  cid: string | null = null,
): OutboxAttachment => ({
  path: p.path,
  filename: p.name,
  mime: p.mime,
  cid,
});

// ---------- From picker ----------

const PILL = "rounded-full border px-1.5 text-[10px] leading-4";
const PROBLEM_TONE: Record<string, string> = {
  sourceFailing: "border-red-400/40 text-red-400",
  default: "border-amber-400/40 text-amber-400",
};

export function FromPicker({
  groups,
  value,
  onChange,
}: {
  groups: readonly SenderGroup[];
  value: string;
  onChange: (address: string) => void;
}) {
  const [open, setOpen] = useState(false);
  const trigger = useRef<HTMLButtonElement>(null);
  const current = groups
    .flatMap((g) => g.entries)
    .find((e) => e.address.toLowerCase() === value.toLowerCase());
  return (
    <div className="relative min-w-0">
      <button
        ref={trigger}
        type="button"
        aria-label={`From: ${value || "none"}`}
        aria-haspopup="listbox"
        aria-expanded={open}
        onClick={() => setOpen((o) => !o)}
        className="flex h-8 w-full items-center gap-2 rounded-md border border-content/10 px-2 text-left text-[12px] text-content outline-none hover:border-content/20 focus-visible:border-accent/50"
      >
        <span className="min-w-0 truncate">
          {value || "No mailbox can send"}
        </span>
        {current ? (
          <span className="shrink-0 text-[11px] text-content/45">
            {current.sourceLabel} {"·"} {current.region}
          </span>
        ) : null}
        <ChevronDown className="ml-auto size-3 shrink-0 text-content/45" />
      </button>
      {open ? (
        <Popover
          anchor={trigger}
          side="bottom"
          align="start"
          gap={4}
          width={420}
          maxHeight={300}
          role="dialog"
          aria-label="From options"
          data-dialog-popover
          onDismiss={() => setOpen(false)}
          className="flex flex-col overflow-hidden"
        >
          <div
            role="listbox"
            aria-label="From"
            className="min-h-0 flex-1 overflow-y-auto p-1"
          >
            {groups.map((group) => (
              <div key={group.domain} role="group" aria-label={group.domain}>
                <div className="flex items-baseline gap-2 px-2 pt-1.5 pb-0.5 text-[11px]">
                  <span className="font-medium text-content/70">
                    {group.domain}
                  </span>
                  <span className="truncate text-content/40">
                    {group.sources.join(", ")}
                  </span>
                </div>
                {group.entries.map((entry) => (
                  <SenderRow
                    key={entry.address}
                    entry={entry}
                    selected={
                      entry.address.toLowerCase() === value.toLowerCase()
                    }
                    onPick={() => {
                      onChange(entry.address);
                      setOpen(false);
                    }}
                  />
                ))}
              </div>
            ))}
          </div>
        </Popover>
      ) : null}
    </div>
  );
}

function SenderRow({
  entry,
  selected,
  onPick,
}: {
  entry: SenderEntry;
  selected: boolean;
  onPick: () => void;
}) {
  const ok = senderOk(entry);
  return (
    <button
      type="button"
      role="option"
      aria-selected={selected}
      aria-disabled={!ok}
      // Not `disabled`: a greyed entry still says why, to a keyboard too.
      onClick={ok ? onPick : undefined}
      title={entry.problem ? SENDER_PROBLEM_LABEL[entry.problem] : undefined}
      className={`flex h-8 w-full items-center gap-2 rounded-md px-2 text-left text-[12px] ${
        ok
          ? "text-content/80 hover:bg-content/5 hover:text-content"
          : "cursor-default text-content/40"
      } ${selected ? "bg-selection text-content" : ""}`}
    >
      <span
        className="size-2 shrink-0 rounded-full bg-content/30"
        style={entry.color ? { backgroundColor: entry.color } : undefined}
      />
      <span className="min-w-0 flex-1 truncate">{entry.address}</span>
      {entry.problem ? (
        <span
          className={`${PILL} ${PROBLEM_TONE[entry.problem] ?? PROBLEM_TONE.default}`}
        >
          {SENDER_PROBLEM_LABEL[entry.problem]}
        </span>
      ) : (
        <span className={`${PILL} border-emerald-400/40 text-emerald-400`}>
          verified
        </span>
      )}
    </button>
  );
}

// ---------- recipients ----------

function RecipientField({
  label,
  value,
  text,
  invalid,
  onRemove,
  onText,
  onCommit,
}: {
  label: string;
  value: MailAddress[];
  text: string;
  invalid: boolean;
  onRemove: (index: number) => void;
  onText: (text: string) => void;
  onCommit: (text: string) => void;
}) {
  return (
    <div className="grid grid-cols-[64px_1fr] items-center gap-2 border-b border-stroke py-1.5">
      <label className="text-[11px] text-content/50">{label}</label>
      <div
        className={`flex min-h-7 min-w-0 flex-wrap items-center gap-1 rounded-md border px-1.5 py-0.5 focus-within:border-accent/50 ${
          invalid ? "border-red-400/60" : "border-content/10"
        }`}
      >
        {value.map((a, i) => (
          <span
            key={`${a.addr}-${i}`}
            className="flex items-center gap-1 rounded-full border border-content/10 bg-content/5 px-1.5 text-[11px] text-content/80"
          >
            {addressLabel(a)}
            <button
              type="button"
              aria-label={`Remove ${a.addr}`}
              onClick={() => onRemove(i)}
              className="text-content/45 hover:text-content"
            >
              <X className="size-3" />
            </button>
          </span>
        ))}
        <input
          aria-label={label}
          value={text}
          aria-invalid={invalid}
          autoComplete="off"
          spellCheck={false}
          onChange={(event) => {
            const next = event.target.value;
            onText(next);
            // A typed comma or semicolon ends the address.
            if (/[,;]$/.test(next)) onCommit(next);
          }}
          onBlur={() => onCommit(text)}
          onKeyDown={(event) => {
            if (event.key === "Enter") {
              event.preventDefault();
              onCommit(text);
            }
          }}
          className="h-6 min-w-24 flex-1 bg-transparent text-[12px] text-content outline-none"
        />
      </div>
    </div>
  );
}

// ---------- the sheet ----------

type Props = {
  request: ComposeRequest;
  onClose: () => void;
  /** Fired once a send went through, with the report. */
  onSent?: (report: SendReport) => void;
};

type Init = {
  doc: TipTapNode | null;
  recipients: Recipients;
  subject: string;
  from: string;
  quote: Quote | null;
  pending: Pending[];
};

export function ComposeSheet({ request, onClose, onSent }: Props) {
  const draftRow = request.kind === "draft" ? request.row : null;
  const kind: OutboxKind =
    request.kind === "draft" ? request.row.kind : request.kind;

  const [groups, setGroups] = useState<SenderGroup[] | null>(null);
  const [init, setInit] = useState<Init | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);

  const [from, setFrom] = useState("");
  const [recipients, setRecipients] = useState<Recipients>({
    to: [],
    cc: [],
    bcc: [],
  });
  const [texts, setTexts] = useState<Record<Field, string>>({
    to: "",
    cc: "",
    bcc: "",
  });
  const [badField, setBadField] = useState<Field | null>(null);
  const [subject, setSubject] = useState("");
  const [quote, setQuote] = useState<Quote | null>(null);
  const [pending, setPending] = useState<Pending[]>([]);

  const [rowState, setRowState] = useState<OutboxState | null>(
    draftRow?.state ?? null,
  );
  const [rowError, setRowError] = useState<string | null>(
    draftRow?.lastError ?? null,
  );
  const [busy, setBusy] = useState<"send" | "rearm" | null>(null);
  const [staging, setStaging] = useState(0);
  const [confirmRearm, setConfirmRearm] = useState(false);
  const [problems, setProblems] = useState<SendProblem[]>([]);
  const [notice, setNotice] = useState<string | null>(null);
  const [saveError, setSaveError] = useState<string | null>(null);
  const [savedAt, setSavedAt] = useState<Date | null>(null);
  const [report, setReport] = useState<SendReport | null>(null);
  const [missingPaths, setMissingPaths] = useState<string[]>([]);
  // Ids of the staged files the message would carry now; null until the body
  // is first edited, when everything loaded with the draft still counts.
  const [attachedIds, setAttachedIds] = useState<string[] | null>(null);
  const [fromNote, setFromNote] = useState<string | null>(null);
  const mode = sendMode(rowState);

  const editorRef = useRef<Editor | null>(null);
  const setEditor = useCallback((e: Editor | null) => {
    editorRef.current = e;
  }, []);
  const idRef = useRef<number | null>(draftRow?.id ?? null);
  const dirty = useRef(false);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const queue = useRef<Promise<unknown>>(Promise.resolve());
  // `busy` is the value of the render that made the handler, so two clicks in
  // one tick both read it as null; only a ref is seen by the second click.
  const sending = useRef(false);
  const forceClose = useRef(false);
  const alive = useRef(true);
  // While a second close would discard, the note must keep saying so.
  const fromNoteText = () => (forceClose.current ? DISCARD_NOTE : FROM_NOTE);

  // Read by the async save closures, which outlive the render that made them.
  const latest = useRef({ from, recipients, subject, quote, pending });
  latest.current = { from, recipients, subject, quote, pending };

  const enqueue = useCallback(<T,>(fn: () => Promise<T>): Promise<T> => {
    const run = queue.current.then(fn);
    queue.current = run.catch(() => undefined);
    return run;
  }, []);

  // ----- load senders, then work out the starting fields -----
  useEffect(() => {
    alive.current = true;
    let stale = false;
    (async () => {
      const [status, sendable, identities] = await Promise.all([
        mailStatus(),
        mailSendableMailboxes(),
        mailIdentities(),
      ]);
      const g = senderGroups(status.sources, sendable, identities);
      const own = status.sources.flatMap((s) =>
        s.mailboxes.map((m) => m.address),
      );
      let next: Init;
      if (request.kind === "draft") {
        const row = request.row;
        const doc = asDoc(row.bodyJson);
        const staged = await rebind(row, doc);
        // A reopened reply rebuilds its plain-text quote from the stored
        // original; only the sanitised HTML is kept on the row.
        let quote: Quote | null = null;
        if (row.quotedHtml) {
          const original = await originalOf(row);
          const rebuilt = original ? buildQuote(row.kind, original) : null;
          quote = {
            attribution: rebuilt?.attribution ?? "Quoted message",
            html: row.quotedHtml,
            text: rebuilt?.text ?? "",
            preview: rebuilt?.preview ?? "",
            plain: rebuilt?.plain ?? "",
          };
        }
        next = {
          doc: staged.doc,
          recipients: { to: row.to, cc: row.cc, bcc: row.bcc },
          subject: row.subject,
          from: row.fromAddr,
          quote,
          pending: staged.pending,
        };
      } else if (request.kind === "new") {
        next = {
          doc: null,
          recipients: { to: [], cc: [], bcc: [] },
          subject: "",
          from: defaultFrom(g, null, null),
          quote: null,
          pending: [],
        };
      } else {
        const m = request.message;
        const from = defaultFrom(g, m, request.thread);
        const r = recipientsFor(request.kind, m, [...own, from]);
        next = {
          doc: null,
          recipients: { ...r, bcc: [] },
          subject: prefixedSubject(m.subject, request.kind),
          from,
          quote: buildQuote(request.kind, m),
          pending: [],
        };
      }
      if (stale) return;
      setGroups(g);
      setFrom(next.from);
      setRecipients(next.recipients);
      setSubject(next.subject);
      setQuote(next.quote);
      setPending(next.pending);
      setInit(next);
      if (request.kind === "draft") {
        const gone = await missingFiles(next.pending);
        if (!stale) setMissingPaths(gone);
      }
    })().catch((err) => {
      if (!stale) setLoadError(message(err));
    });
    return () => {
      stale = true;
      alive.current = false;
    };
    // The request is fixed for the life of a sheet; MailView remounts per opening.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(
    () => () => {
      if (timer.current) clearTimeout(timer.current);
      for (const p of latest.current.pending) {
        if (p.previewUrl) URL.revokeObjectURL(p.previewUrl);
      }
    },
    [],
  );

  // ----- saving -----

  const persist = useCallback(
    async (recipientsOverride?: Recipients): Promise<number> => {
      const editor = editorRef.current;
      if (!editor) throw new Error("The editor is not ready yet.");
      const s = latest.current;
      const recips = recipientsOverride ?? s.recipients;
      const doc = editor.getJSON() as TipTapNode;
      const plain = s.pending
        .filter((p) => !p.inline)
        .map((p) => toAttachment(p));
      const draft = {
        kind,
        fromAddr: s.from,
        replyToMessageId:
          request.kind === "draft"
            ? request.row.replyToMessageId
            : request.kind === "new"
              ? null
              : request.message.id,
        ...recips,
        subject: s.subject,
        bodyJson: doc,
        quotedHtml: s.quote?.html ?? null,
      };
      let id = idRef.current;
      if (id === null) {
        // The id is needed first: it salts the content ids of inline images.
        id = await mailOutboxSave({ ...draft, id: null, attachments: plain });
        idRef.current = id;
      }
      const { out, inline } = compose(s.pending, s.quote, doc, String(id));
      await mailOutboxSave({
        ...draft,
        id,
        attachments: [
          ...plain,
          ...inline.map(({ p, cid }) => toAttachment(p, cid)),
        ],
        bodyHtml: out.html,
        bodyText: out.text,
      });
      return id;
    },
    [kind, request],
  );

  const refreshRow = useCallback(
    async (id: number): Promise<OutboxRow | null> => {
      try {
        const row = (await mailOutboxList()).find((r) => r.id === id) ?? null;
        if (row && alive.current) {
          setRowState(row.state);
          setRowError(row.lastError);
        }
        return row;
      } catch {
        return null;
      }
    },
    [],
  );

  const save = useCallback(
    () =>
      enqueue(async () => {
        try {
          const id = await persist();
          dirty.current = false;
          if (alive.current) {
            setSaveError(null);
            setSavedAt(new Date());
          }
          return id;
        } catch (err) {
          // A sending or sent row refuses the save; the person must hear it,
          // or they keep editing text that is not being stored.
          if (alive.current) setSaveError(message(err));
          // "Already sent" is the row moving under the composer; what may be
          // offered next comes from the row, not from this refusal.
          if (idRef.current !== null) void refreshRow(idRef.current);
          throw err;
        }
      }),
    [enqueue, persist, refreshRow],
  );

  const touch = useCallback(() => {
    dirty.current = true;
    if (timer.current) clearTimeout(timer.current);
    timer.current = setTimeout(() => {
      // No From means nothing can be stored (Rust rejects an unknown mailbox);
      // say what to do instead of failing every pause.
      if (!latest.current.from) return setFromNote(fromNoteText());
      void save().catch(() => undefined);
    }, AUTOSAVE_MS);
  }, [save]);

  // ----- fields -----

  const commit = (field: Field, text: string) => {
    const { valid, invalid } = parseRecipients(text);
    if (valid.length) {
      setRecipients((r) => ({ ...r, [field]: [...r[field], ...valid] }));
      touch();
    }
    setTexts((t) => ({ ...t, [field]: invalid.join(", ") }));
    setBadField(invalid.length ? field : null);
  };

  /** Folds typed-but-uncommitted text into the lists; returns what is left invalid. */
  const settleTexts = (): {
    recipients: Recipients;
    invalid: { field: Field; token: string } | null;
  } => {
    const out: Recipients = { ...latest.current.recipients };
    const left: Record<Field, string> = { to: "", cc: "", bcc: "" };
    let invalid: { field: Field; token: string } | null = null;
    for (const f of ["to", "cc", "bcc"] as const) {
      const r = parseRecipients(texts[f]);
      out[f] = [...out[f], ...r.valid];
      left[f] = r.invalid.join(", ");
      if (!invalid && r.invalid.length)
        invalid = { field: f, token: r.invalid[0] };
    }
    latest.current.recipients = out;
    setRecipients(out);
    setTexts(left);
    setBadField(invalid?.field ?? null);
    return { recipients: out, invalid };
  };

  // ----- files -----

  const insertImage = (p: Pending) =>
    editorRef.current
      ?.chain()
      .focus()
      .insertContent({
        type: "image",
        attrs: { src: p.previewUrl, alt: p.name, "data-mono-att": p.id },
      })
      .run();

  const addFiles = async (files: Attachment[], preferInline: boolean) => {
    setStaging((n) => n + 1);
    try {
      const staged: Pending[] = [];
      for (const f of files) {
        const p = await stage(f, preferInline && f.kind === "image");
        if (p) staged.push(p);
        else setNotice(`Could not read ${f.name}.`);
      }
      if (!alive.current) return;
      setPending((prev) => [...prev, ...staged]);
      // They were just read, so an older "missing" verdict on the same path is
      // stale and would claim a present file is gone.
      setMissingPaths((prev) =>
        prev.filter((x) => !staged.some((p) => p.path === x)),
      );
      latest.current.pending = [...latest.current.pending, ...staged];
      for (const p of staged) if (p.inline) insertImage(p);
      if (staged.length) touch();
    } catch (err) {
      setNotice(message(err));
    } finally {
      if (alive.current) setStaging((n) => n - 1);
    }
  };

  const pick = async (inline: boolean) => {
    try {
      const files = await pickAttachments();
      if (files.length) await addFiles(files, inline);
    } catch (err) {
      setNotice(message(err));
    }
  };

  const onFiles = (files: File[]) => {
    void attachmentsFromFiles(files)
      .then((list) => addFiles(list, true))
      .catch((err) => setNotice(message(err)));
  };

  // A dropped file reaches the webview as a path through Tauri, not as a DOM
  // drop, so the editor's own handleDrop never fires for files from outside.
  useEffect(() => {
    let stop: (() => void) | undefined;
    let gone = false;
    void getCurrentWebview()
      .onDragDropEvent((event) => {
        if (event.payload.type !== "drop") return;
        void attachmentsFromPaths(event.payload.paths)
          .then((list) => addFiles(list, false))
          .catch((err) => setNotice(message(err)));
      })
      .then((unlisten) => {
        if (gone) unlisten();
        else stop = unlisten;
      })
      .catch(() => undefined);
    return () => {
      gone = true;
      stop?.();
    };
    // addFiles reads refs and stable setters only.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const syncAttached = () => {
    const editor = editorRef.current;
    if (!editor) return;
    const { pending: files, quote: q } = latest.current;
    const ids = compose(files, q, editor.getJSON() as TipTapNode, "0").attached
      .map((p) => p.id)
      .sort();
    setAttachedIds((prev) =>
      prev && prev.length === ids.length && prev.every((x, i) => x === ids[i])
        ? prev
        : ids,
    );
  };

  const removePending = (id: string) => {
    setPending((prev) => prev.filter((p) => p.id !== id));
    touch();
  };

  const quoteInline = () => {
    const editor = editorRef.current;
    // An empty original (a reopened forward) would be replaced by nothing.
    if (!quote || !editor || quote.plain.trim() === "") return;
    const lines = quote.plain.split(/\r\n?|\n/);
    editor
      .chain()
      .focus("end")
      .insertContent([
        {
          type: "paragraph",
          content: [{ type: "text", text: quote.attribution }],
        },
        {
          type: "blockquote",
          content: lines.map((l) =>
            l
              ? { type: "paragraph", content: [{ type: "text", text: l }] }
              : { type: "paragraph" },
          ),
        },
      ])
      .run();
    // The text now lives in the document; keeping the block too would send it twice.
    setQuote(null);
    latest.current.quote = null;
    touch();
  };

  // ----- send -----

  const blocked = (): string | null => {
    const s = latest.current;
    const entry = groups
      ?.flatMap((g) => g.entries)
      .find((e) => e.address.toLowerCase() === s.from.toLowerCase());
    if (!s.from) return "Choose a From address that can send.";
    if (entry && !senderOk(entry)) {
      return `${entry.address} cannot send: ${SENDER_PROBLEM_LABEL[entry.problem!]}.`;
    }
    if (staging > 0) return "Wait for the attachments to finish loading.";
    return null;
  };

  const send = async () => {
    // Cmd+Enter reaches here with the editor still mounted, so the row's state
    // is enforced here and not only by hiding the button.
    if (sending.current || busy || mode.kind !== "send") return;
    sending.current = true;
    try {
      await sendOnce();
    } finally {
      sending.current = false;
    }
  };

  const sendOnce = async () => {
    setProblems([]);
    setNotice(null);
    const editor = editorRef.current;
    const s = latest.current;
    // Only what the message would carry can block it: an inline image deleted
    // from the body is no longer part of it.
    const { attached } = compose(
      s.pending,
      s.quote,
      editor ? (editor.getJSON() as TipTapNode) : { type: "doc" },
      String(idRef.current ?? 0),
    );
    const gone = await missingFiles(attached);
    setMissingPaths(gone);
    const settled = settleTexts();
    // Everything that blocks is shown at once, not one problem per attempt.
    const reasons = [
      blocked(),
      settled.invalid
        ? `"${settled.invalid.token}" is not a valid email address.`
        : settled.recipients.to.length === 0
          ? "Add at least one recipient in To."
          : null,
    ].filter((r): r is string => r !== null);
    if (reasons.length) setNotice(reasons.join(" "));
    if (reasons.length || gone.length) return;
    setBusy("send");
    let id = idRef.current;
    try {
      if (timer.current) clearTimeout(timer.current);
      // Order is fixed: save for the id, serialize with it, save the result, send.
      id = await enqueue(() => persist(settled.recipients));
      dirty.current = false;
      setSaveError(null);
      const found = await mailOutboxCheck(id);
      if (found.length) return setProblems(found);
      const sent = await mailOutboxSend(id);
      setRowState("sent");
      setReport(sent);
      onSent?.(sent);
      if (!sent.bookkeepingError && sent.archive.stored) onClose();
    } catch (err) {
      setNotice(message(err));
      // The row, not the error, says what may be offered next. If it cannot be
      // read, a call that may have reached SES fails closed.
      const row = id === null ? null : await refreshRow(id);
      const e = err as { kind?: string; retrySafe?: boolean } | null;
      if (!row && (e?.kind === "unconfirmed" || e?.retrySafe === false)) {
        setRowState("unconfirmed");
      }
    } finally {
      if (alive.current) setBusy(null);
    }
  };

  const rearm = async () => {
    const id = idRef.current;
    if (id === null) return;
    setBusy("rearm");
    try {
      await mailOutboxRearm(id);
      setConfirmRearm(false);
      setNotice(null);
      await refreshRow(id);
    } catch (err) {
      setNotice(message(err));
    } finally {
      setBusy(null);
    }
  };

  const saveDraft = () => {
    settleTexts();
    if (!latest.current.from) return setFromNote(fromNoteText());
    void save().catch(() => undefined);
  };

  const discard = async () => {
    try {
      if (timer.current) clearTimeout(timer.current);
      if (idRef.current !== null) await mailOutboxDelete(idRef.current);
      dirty.current = false;
      onClose();
    } catch (err) {
      setNotice(message(err));
    }
  };

  const requestClose = async () => {
    if (timer.current) clearTimeout(timer.current);
    if (dirty.current && !forceClose.current && !report) {
      if (!latest.current.from) {
        // Nothing can be saved without a From; closing now would drop the text.
        forceClose.current = true;
        setFromNote(DISCARD_NOTE);
        return;
      }
      try {
        await save();
      } catch {
        // The banner says why; closing again leaves anyway.
        forceClose.current = true;
        return;
      }
    }
    onClose();
  };

  // ----- render -----

  // Only files the message still carries count: removing one, or deleting an
  // inline image from the body, clears its warning.
  const missing = pending.filter(
    (p) =>
      missingPaths.includes(p.path) &&
      (!p.inline || attachedIds === null || attachedIds.includes(p.id)),
  );
  const entry = groups
    ?.flatMap((g) => g.entries)
    .find((e) => e.address.toLowerCase() === from.toLowerCase());
  const attachments = useMemo(
    () => pending.filter((p) => !p.inline),
    [pending],
  );

  return (
    <Modal
      title={TITLE[kind]}
      width="w-[min(720px,calc(100vw-24px))]"
      fitViewport
      onClose={() => void requestClose()}
    >
      {loadError ? (
        <p role="alert" className="px-4 py-3 text-[12px] text-red-400">
          Could not open the composer: {loadError}
        </p>
      ) : !init || !groups ? (
        <div className="flex justify-center py-8 text-content/45">
          <Loader className="size-4 animate-spin" />
        </div>
      ) : (
        <div className="flex flex-col px-4 pb-3">
          <div className="grid grid-cols-[64px_1fr] items-center gap-2 border-b border-stroke py-1.5">
            <label className="text-[11px] text-content/50">From</label>
            <FromPicker
              groups={groups}
              value={from}
              onChange={(a) => {
                setFrom(a);
                setFromNote(null);
                forceClose.current = false;
                touch();
              }}
            />
          </div>
          {(["to", "cc", "bcc"] as const).map((f) => (
            <RecipientField
              key={f}
              label={f === "to" ? "To" : f === "cc" ? "Cc" : "Bcc"}
              value={recipients[f]}
              text={texts[f]}
              invalid={badField === f}
              onText={(t) => setTexts((x) => ({ ...x, [f]: t }))}
              onCommit={(t) => commit(f, t)}
              onRemove={(i) => {
                setRecipients((r) => ({
                  ...r,
                  [f]: r[f].filter((_, j) => j !== i),
                }));
                touch();
              }}
            />
          ))}
          <div className="grid grid-cols-[64px_1fr] items-center gap-2 border-b border-stroke py-1.5">
            <label
              htmlFor="mail-subject"
              className="text-[11px] text-content/50"
            >
              Subject
            </label>
            <input
              id="mail-subject"
              value={subject}
              onChange={(event) => {
                setSubject(event.target.value);
                touch();
              }}
              className="h-7 min-w-0 rounded-md border border-content/10 bg-transparent px-2 text-[12px] text-content outline-none focus:border-accent/50"
            />
          </div>

          <LazyMailEditor
            content={init.doc}
            onChange={() => {
              touch();
              syncAttached();
            }}
            onSend={() => void send()}
            onFiles={onFiles}
            onImage={() => void pick(true)}
            onEditor={setEditor}
          />

          {quote ? (
            <div className="mt-3 overflow-hidden rounded-lg border border-content/10 bg-content/[0.02]">
              <div className="flex items-center gap-2 px-3 py-1.5 text-[11px] text-content/50">
                <span className="min-w-0 flex-1 truncate">
                  {quote.attribution}
                </span>
                <span className="shrink-0 rounded-full border border-content/10 px-1.5 text-[10px]">
                  not editable {"·"} appended on send
                </span>
                <SecondaryButton
                  onClick={quoteInline}
                  disabled={quote.plain.trim() === ""}
                  title={
                    quote.plain.trim() === ""
                      ? "The original has no plain text to insert; it is still sent below your message."
                      : undefined
                  }
                >
                  Quote inline
                </SecondaryButton>
              </div>
              {/* A text node: the original is never parsed in the app's own DOM. */}
              <p className="truncate px-3 pb-2 text-[12px] text-content/45">
                {quote.preview}
              </p>
            </div>
          ) : null}

          <div className="mt-3 flex flex-wrap items-center gap-2">
            {attachments.map((p) => (
              <span
                key={p.id}
                className="flex items-center gap-1.5 rounded-md border border-content/10 px-2 py-1 text-[12px] text-content/70"
              >
                <Paperclip className="size-3" />
                <span className="max-w-[200px] truncate">{p.name}</span>
                <span className="text-content/45">{sizeLabel(p.size)}</span>
                <button
                  type="button"
                  aria-label={`Remove ${p.name}`}
                  onClick={() => removePending(p.id)}
                  className="text-content/45 hover:text-content"
                >
                  <X className="size-3" />
                </button>
              </span>
            ))}
            <button
              type="button"
              onClick={() => void pick(false)}
              className="flex items-center gap-1.5 rounded-md border border-dashed border-content/15 px-2 py-1 text-[12px] text-content/50 hover:text-content focus-visible:outline-2 focus-visible:outline-accent"
            >
              {staging > 0 ? (
                <Loader className="size-3 animate-spin" />
              ) : (
                <Plus className="size-3" />
              )}
              Attach file
            </button>
          </div>

          {entry ? (
            <p className="mt-3 text-[11px] text-content/45">
              Sends through {entry.sourceLabel} {"·"} {entry.region}. The From
              address picks the source, never the other way round.
            </p>
          ) : null}

          {missing.length ? (
            <p role="alert" className="mt-3 text-[12px] text-amber-400">
              {missing.map((p) => p.name).join(", ")}{" "}
              {missing.length === 1 ? "is" : "are"} no longer on this computer.
              Remove {missing.length === 1 ? "it" : "them"} and attach again, or
              this message cannot be sent.
            </p>
          ) : null}
          {fromNote ? (
            <p role="alert" className="mt-3 text-[12px] text-amber-400">
              {fromNote}
            </p>
          ) : null}
          {saveError ? (
            <p role="alert" className="mt-3 text-[12px] text-red-400">
              Your latest changes are not being saved: {saveError}
            </p>
          ) : null}
          {notice ? (
            <p role="alert" className="mt-3 text-[12px] text-red-400">
              {notice}
            </p>
          ) : null}
          {problems.map((p) => (
            <p
              key={p.kind}
              role="alert"
              className="mt-3 text-[12px] text-amber-400"
            >
              Not sent: {p.message}
            </p>
          ))}

          {mode.kind === "checkFirst" ? (
            <div
              role="alert"
              className="mt-3 rounded-lg border border-amber-400/40 bg-amber-400/10 px-3 py-2 text-[12px] text-content/80"
            >
              <p className="font-medium text-amber-400">
                Check the recipient first.
              </p>
              <p className="mt-1">
                This message may already have been delivered: the connection
                dropped before Amazon SES confirmed it. Look in the recipient
                {"’"}s inbox or your Sent mail before sending it again. Edits
                are still saved, but they do not allow sending again.
              </p>
              {rowError ? (
                <p className="mt-1 text-content/50">{rowError}</p>
              ) : null}
              {confirmRearm ? (
                <div className="mt-2 flex items-center gap-2">
                  <span>Sending again may deliver it twice.</span>
                  <SecondaryButton
                    danger
                    disabled={busy !== null}
                    onClick={() => void rearm()}
                  >
                    Allow sending again
                  </SecondaryButton>
                  <SecondaryButton onClick={() => setConfirmRearm(false)}>
                    Cancel
                  </SecondaryButton>
                </div>
              ) : (
                <div className="mt-2">
                  <SecondaryButton onClick={() => setConfirmRearm(true)}>
                    I checked the recipient
                  </SecondaryButton>
                </div>
              )}
            </div>
          ) : null}
          {mode.kind === "send" && rowState === "failed" && rowError ? (
            <p role="alert" className="mt-3 text-[12px] text-red-400">
              The last attempt failed and nothing was delivered: {rowError}
            </p>
          ) : null}
          {mode.kind === "busy" ? (
            <p className="mt-3 text-[12px] text-content/60">
              This message is being sent. It cannot be changed until that
              finishes.
            </p>
          ) : null}
          {report ? (
            <div role="status" className="mt-3 text-[12px] text-content/70">
              <p>Sent.</p>
              {report.bookkeepingError ? (
                <p className="text-amber-400">
                  It was delivered, but recording it here failed:{" "}
                  {report.bookkeepingError}
                </p>
              ) : null}
              {!report.archive.stored ? (
                <p className="text-amber-400">
                  The sent copy is on this computer only
                  {report.archive.reason ? `: ${report.archive.reason}` : "."}
                </p>
              ) : null}
            </div>
          ) : null}

          <div className="mt-4 flex items-center gap-2">
            {mode.kind === "send" ? (
              <button
                type="button"
                disabled={busy !== null || staging > 0}
                onClick={() => void send()}
                className="flex items-center gap-1.5 rounded-md bg-accent px-3 py-1 text-[12px] font-medium text-white hover:opacity-90 focus-visible:outline-2 focus-visible:outline-accent disabled:cursor-default disabled:opacity-40"
              >
                {busy === "send" ? (
                  <Loader className="size-3 animate-spin" />
                ) : (
                  <Send className="size-3" />
                )}
                {mode.label}
              </button>
            ) : null}
            {report ? null : (
              <SecondaryButton
                onClick={saveDraft}
                disabled={mode.kind === "busy" || mode.kind === "done"}
              >
                Save draft
              </SecondaryButton>
            )}
            <span className="ml-auto text-[11px] text-content/45">
              {savedAt
                ? `Draft saved locally · ${savedAt.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })}`
                : ""}
            </span>
            {report ? (
              <SecondaryButton onClick={onClose}>Close</SecondaryButton>
            ) : (
              <SecondaryButton
                danger
                aria-label="Discard draft"
                onClick={() => void discard()}
              >
                <Trash2 className="size-3.5" />
              </SecondaryButton>
            )}
          </div>
        </div>
      )}
    </Modal>
  );
}

// ---------- reopening a draft ----------

/**
 * Object URLs do not outlive the window, so a stored draft's images come back
 * with dead `src`s. Each Image node names its attachment, whose content id is
 * derived from the draft id, so the file is found again and shown afresh.
 */
async function rebind(
  row: OutboxRow,
  doc: TipTapNode | null,
): Promise<{ doc: TipTapNode | null; pending: Pending[] }> {
  const pending: Pending[] = [];
  const byAtt = new Map<string, string>();
  for (const a of row.attachments) {
    if (!a.cid) {
      pending.push({
        id: crypto.randomUUID(),
        name: a.filename,
        mime: a.mime,
        path: a.path,
        size: 0,
        inline: false,
        previewUrl: null,
      });
    }
  }
  if (!doc) return { doc, pending };
  const ids: string[] = [];
  mapImages(doc, (n) => {
    const id = n.attrs?.["data-mono-att"];
    if (typeof id === "string" || typeof id === "number") ids.push(String(id));
    return n;
  });
  for (const id of new Set(ids)) {
    const cid = contentIdFor(String(row.id), id);
    const a = row.attachments.find(
      (x) => x.cid?.toLowerCase() === cid.toLowerCase(),
    );
    if (!a) continue;
    let previewUrl: string | null = null;
    try {
      previewUrl = URL.createObjectURL(
        base64Blob(
          await invoke<string>("read_file_base64", { path: a.path }),
          a.mime,
        ),
      );
    } catch {
      // The file is gone: the image stays broken and sending names the problem.
    }
    pending.push({
      id,
      name: a.filename,
      mime: a.mime,
      path: a.path,
      size: 0,
      inline: true,
      previewUrl,
    });
    if (previewUrl) byAtt.set(id, previewUrl);
  }
  return {
    pending,
    doc: mapImages(doc, (n) => {
      const url = byAtt.get(String(n.attrs?.["data-mono-att"]));
      return url ? { ...n, attrs: { ...n.attrs, src: url } } : n;
    }),
  };
}

async function originalOf(row: OutboxRow) {
  if (row.threadId === null || row.replyToMessageId === null) return null;
  try {
    const detail = await mailThread(row.threadId);
    return detail.messages.find((m) => m.id === row.replyToMessageId) ?? null;
  } catch {
    return null;
  }
}

/**
 * The one place that decides which staged files a message carries: every plain
 * attachment, plus the inline images the body still references. `persist` sends
 * exactly this and Send's missing-file check blocks on exactly this, so the two
 * cannot disagree about whether a file is part of the message.
 */
function compose(
  pending: Pending[],
  quote: Quote | null,
  doc: TipTapNode,
  draftId: string,
) {
  const out = serializeCompose({
    bodyJson: doc,
    draftId,
    quotedHtml: quote?.html ?? null,
    quotedText: quote?.text ?? null,
  });
  const inline = out.inlineImages.flatMap((img) => {
    const p = pending.find((x) => x.id === img.attachmentId);
    return p ? [{ p, cid: img.contentId }] : [];
  });
  const plain = pending.filter((p) => !p.inline);
  return { out, inline, attached: [...plain, ...inline.map((i) => i.p)] };
}

/**
 * A draft stores paths, not bytes, so a file can be deleted or renamed between
 * composing and sending. Checked when a draft opens and again on every Send,
 * which is when it matters; the result is its own banner so a send attempt
 * cannot clear it. `inspect_paths` returns only paths that still exist;
 * anything other than a list (a host without the command) means "cannot tell",
 * so nothing is claimed and Rust's own file error remains the backstop.
 */
async function missingFiles(files: { path: string }[]): Promise<string[]> {
  if (files.length === 0) return [];
  const slash = (p: string) => p.replace(/\\/g, "/");
  try {
    const found = await invoke<{ path: string }[] | null>("inspect_paths", {
      paths: files.map((f) => f.path),
    });
    if (!Array.isArray(found)) return [];
    const here = new Set(found.map((f) => slash(f.path)));
    return files.filter((f) => !here.has(slash(f.path))).map((f) => f.path);
  } catch {
    return [];
  }
}
