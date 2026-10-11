import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { LIST_PAGE_SIZE, listWindowSize } from "../../../shared/lib/listWindow";
import { Loader, Paperclip, Pencil, Star } from "../../../shared/ui/icons";
import { MAIL_MESSAGES_CHANGED_EVENT, mailThreads } from "../model/mail";
import { useDensity } from "../model/mailPrefs";
import {
  NO_CHIPS,
  QUERY_MATCH_MODE,
  SEARCH_DEBOUNCE_MS,
  buildRule,
  chipRules,
  parseQuery,
  type ChipFilters,
} from "../model/mailQuery";
import type {
  MailAddress,
  MailMailbox,
  MatchMode,
  Rule,
  ThreadCursor,
  ThreadRow,
  ThreadScope,
} from "../model/mailTypes";
import { domainOf } from "../model/mailDomain";

/** Rows fetched per backend page; the DOM window grows in LIST_PAGE_SIZE steps. */
const FETCH_LIMIT = 100;

const DAY_MS = 86_400_000;
const CHIP_FIELDS = ["from", "to", "content"] as const;
type ChipField = (typeof CHIP_FIELDS)[number];
const CHIP_LABEL: Record<ChipField, string> = {
  from: "From",
  to: "To",
  content: "Content",
};
const CHIP_HINT: Record<ChipField, string> = {
  from: "The sender's name or address contains this",
  to: "A To recipient's name or address contains this",
  content: "The message body contains this phrase",
};

/**
 * `seconds` is epoch seconds (ThreadRow.lastMessageAt). Passing it straight to
 * `new Date` would render January 1970, hence the explicit conversion here.
 */
export function formatMailDate(seconds: number, now: number = Date.now()) {
  const date = new Date(seconds * 1000);
  const today = new Date(now);
  const startOfToday = new Date(
    today.getFullYear(),
    today.getMonth(),
    today.getDate(),
  ).getTime();
  const startOfThatDay = new Date(
    date.getFullYear(),
    date.getMonth(),
    date.getDate(),
  ).getTime();
  const daysAgo = Math.round((startOfToday - startOfThatDay) / DAY_MS);
  if (daysAgo <= 0) {
    return date.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
  }
  if (daysAgo === 1) return "Yesterday";
  if (daysAgo < 7) return date.toLocaleDateString([], { weekday: "short" });
  const sameYear = date.getFullYear() === today.getFullYear();
  return date.toLocaleDateString(
    [],
    sameYear
      ? { month: "short", day: "numeric" }
      : { month: "short", day: "numeric", year: "numeric" },
  );
}

/** Whether the scope shows mail from more than one domain. */
export function scopeSpansDomains(
  scope: ThreadScope,
  mailboxes: Pick<MailMailbox, "address">[],
): boolean {
  if (scope.kind === "mailbox") return false;
  return new Set(mailboxes.map((m) => domainOf(m.address))).size > 1;
}

const nameOf = (p: MailAddress) =>
  p.name?.trim() || p.addr.slice(0, p.addr.lastIndexOf("@")) || p.addr;

/** The mailbox's own address is noise next to the people the thread is with. */
export function participantsLabel(row: ThreadRow): string {
  const others = row.participants.filter(
    (p) => p.addr.toLowerCase() !== row.mailboxAddress.toLowerCase(),
  );
  return (others.length > 0 ? others : row.participants).map(nameOf).join(", ");
}

const CHIP =
  "rounded-full border px-2 py-0.5 text-[11px] focus-visible:outline-2 focus-visible:outline-accent";

/**
 * Where a listing starts before any filter is applied. MailView resolves the
 * stored prefs into one of these; null means the choice is still loading, so
 * nothing is fetched under a scope that may not exist.
 */
export type ListBase =
  | { kind: "all" }
  | { kind: "mailbox"; mailboxId: number; address: string }
  | { kind: "folder"; folderId: number }
  | { kind: "archived" };

function useDebounced<T>(value: T, ms: number): T {
  const [settled, setSettled] = useState(value);
  useEffect(() => {
    const timer = setTimeout(() => setSettled(value), ms);
    return () => clearTimeout(timer);
  }, [value, ms]);
  return settled;
}

type Draft = { chips: ChipFilters; text: string };
const EMPTY_DRAFT: Draft = { chips: NO_CHIPS, text: "" };

/** A mailbox is scoped in a filtered listing by the same rule `in:` makes. */
function mailboxScopeRule(address: string): Rule {
  const built = buildRule("in", address, false);
  if (!("rule" in built)) throw new Error("unreachable: in: always builds");
  return built.rule;
}

/**
 * List state lives in MailView, above the keyed threads section: that section
 * remounts when the reading pane changes side, and the loaded pages, window
 * size, filters and scroll offset must survive it.
 *
 * Filters are rules the backend evaluates, so they cover the whole mailbox and
 * not just the pages loaded so far. A smart folder is already a filter and
 * takes none on top, since a folder's own match mode cannot be combined with
 * these.
 */
export function useMailThreadList(base: ListBase | null) {
  const [rows, setRows] = useState<ThreadRow[]>([]);
  const [cursor, setCursor] = useState<ThreadCursor | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [limit, setLimit] = useState(LIST_PAGE_SIZE);
  const [draft, setDraft] = useState<Draft>(EMPTY_DRAFT);
  const [reloadKey, setReloadKey] = useState(0);
  // Survives the list remounting, so a layout change keeps the user's place.
  const scrollTop = useRef(0);
  // Bumped per first-page load so a page fetch begun under an older scope or
  // reload cannot append into the current list.
  const fetchingRef = useRef(false);
  const generation = useRef(0);
  const loadedScope = useRef<string | null>(null);

  // The box and the chips act after a pause: each settled change is a real
  // query, and the last word may run a prefix scan of the index.
  const settled = useDebounced(draft, SEARCH_DEBOUNCE_MS);
  const applied = useMemo(() => {
    const chips = chipRules(settled.chips);
    const box = parseQuery(settled.text, { prefixLast: true });
    return {
      rules: [...chips.rules, ...box.rules],
      errors: [...chips.errors, ...box.errors],
    };
  }, [settled]);
  // A half-typed `is:unre` is an error, not "no filter": keep the last query
  // that was valid instead of flashing the whole mailbox.
  const lastValid = useRef<Rule[]>([]);
  if (applied.errors.length === 0) lastValid.current = applied.rules;
  const filterRules =
    applied.errors.length === 0 ? applied.rules : lastValid.current;

  const mailboxAddress = base?.kind === "mailbox" ? base.address : null;
  const mailboxRules = useMemo(
    () => (mailboxAddress ? [mailboxScopeRule(mailboxAddress)] : []),
    [mailboxAddress],
  );
  let serverScope: ThreadScope | null = null;
  if (base?.kind === "folder") {
    serverScope = { kind: "folder", folderId: base.folderId };
  } else if (base?.kind === "archived") {
    // No filters on top: a rules scope lists only inbox mail.
    serverScope = { kind: "archived" };
  } else if (base && filterRules.length > 0) {
    serverScope = {
      kind: "rules",
      rules: [...mailboxRules, ...filterRules],
      matchMode: QUERY_MATCH_MODE,
    };
  } else if (base?.kind === "mailbox") {
    serverScope = { kind: "mailbox", mailboxId: base.mailboxId };
  } else if (base) {
    serverScope = { kind: "all" };
  }
  const scopeKey = JSON.stringify(serverScope);
  const scopeRef = useRef(serverScope);
  scopeRef.current = serverScope;

  // What "Save as folder" copies: exact phrases, never the typing-time prefix.
  const saved = useMemo(() => {
    const chips = chipRules(draft.chips);
    const box = parseQuery(draft.text);
    return {
      rules: [...mailboxRules, ...chips.rules, ...box.rules],
      errors: [...chips.errors, ...box.errors],
    };
  }, [draft, mailboxRules]);

  // Reload from the first page; the sync event means rows may have moved.
  useEffect(() => {
    let disposed = false;
    let stop: (() => void) | undefined;
    void listen(MAIL_MESSAGES_CHANGED_EVENT, () => setReloadKey((k) => k + 1))
      .then((unlisten) => {
        if (disposed) unlisten();
        else stop = unlisten;
      })
      .catch(() => undefined);
    return () => {
      disposed = true;
      stop?.();
    };
  }, []);

  useEffect(() => {
    // Before the early return: a page in flight from the previous scope must
    // not append onto the list cleared below.
    generation.current += 1;
    fetchingRef.current = false;
    // Reset on the scope change itself, so a failed first fetch does not leave
    // the previous scope's window behind. A background reload keeps it.
    const scopeChanged = loadedScope.current !== scopeKey;
    loadedScope.current = scopeKey;
    if (scopeChanged) setLimit(LIST_PAGE_SIZE);
    const scope = scopeRef.current;
    if (!scope) {
      setRows([]);
      setCursor(null);
      setLoading(true);
      return;
    }
    let stale = false;
    setLoading(true);
    mailThreads({ scope, limit: FETCH_LIMIT })
      .then((page) => {
        if (stale) return;
        setRows(page.rows);
        setCursor(page.nextCursor);
        setError(null);
      })
      .catch((err) => {
        if (!stale) setError(err instanceof Error ? err.message : String(err));
      })
      .finally(() => {
        if (!stale) setLoading(false);
      });
    return () => {
      stale = true;
    };
  }, [scopeKey, reloadKey]);

  const shown = listWindowSize(rows.length, limit);
  const windowHasMore = shown < rows.length;
  const pageHasMore = cursor !== null;
  const hasMore = windowHasMore || pageHasMore;

  const grow = () => {
    if (windowHasMore) {
      setLimit((current) => current + LIST_PAGE_SIZE);
      return;
    }
    const scope = scopeRef.current;
    if (!cursor || !scope || fetchingRef.current) return;
    fetchingRef.current = true;
    const after = cursor;
    const mine = generation.current;
    mailThreads({ scope, cursor: after, limit: FETCH_LIMIT })
      .then((page) => {
        if (mine !== generation.current) return;
        setRows((current) => [...current, ...page.rows]);
        setCursor(page.nextCursor);
        setLimit((current) => current + LIST_PAGE_SIZE);
      })
      .catch((err) => {
        if (mine === generation.current)
          setError(err instanceof Error ? err.message : String(err));
      })
      .finally(() => {
        if (mine === generation.current) fetchingRef.current = false;
      });
  };

  return {
    rows,
    shown,
    hasMore,
    loading,
    error,
    chips: draft.chips,
    text: draft.text,
    setChips: (chips: ChipFilters) => setDraft({ ...draft, chips }),
    setText: (text: string) => setDraft({ ...draft, text }),
    clearFilters: () => setDraft(EMPTY_DRAFT),
    /** Why the box's current text is not (fully) applied, or null. */
    queryError: applied.errors[0]
      ? filterRules.length > 0
        ? applied.errors[0]
        : `${applied.errors[0]} No filter is applied.`
      : null,
    filtering: filterRules.length > 0,
    hasInput:
      draft.text.trim() !== "" ||
      CHIP_FIELDS.some((f) => draft.chips[f].trim() !== ""),
    /** The rules and mode "Save as folder" starts from, and what is wrong with them. */
    saveRules: saved.rules,
    saveErrors: saved.errors,
    saveMatchMode: QUERY_MATCH_MODE as MatchMode,
    /** Re-reads the first page, for a change the sync event does not announce. */
    reload: () => setReloadKey((k) => k + 1),
    grow,
    scrollTop,
  };
}

export type MailThreadListState = ReturnType<typeof useMailThreadList>;

type Props = {
  list: MailThreadListState;
  scope: ThreadScope;
  /** Every mailbox in view of the scope; decides whether the domain chip shows. */
  mailboxes: Pick<MailMailbox, "address">[];
  selectedId: number | null;
  onSelect: (thread: ThreadRow) => void;
  /** Opens the folder editor with the current filters as its rules. */
  onSaveAsFolder?: (rules: Rule[], matchMode: MatchMode) => void;
  /** Opens the editor for the folder being viewed. */
  onEditFolder?: () => void;
};

export function MailThreadList({
  list,
  scope,
  mailboxes,
  selectedId,
  onSelect,
  onSaveAsFolder,
  onEditFolder,
}: Props) {
  const {
    rows,
    shown,
    hasMore,
    loading,
    error,
    chips,
    text,
    setChips,
    setText,
    clearFilters,
    queryError,
    filtering,
    hasInput,
    saveRules,
    saveErrors,
    saveMatchMode,
  } = list;
  const inFolder = scope.kind === "folder";
  const inArchive = scope.kind === "archived";
  const density = useDensity();
  const [open, setOpen] = useState<ChipField | null>(null);
  const scrollRef = useRef<HTMLDivElement>(null);
  const sentinelRef = useRef<HTMLDivElement>(null);
  const growRef = useRef(list.grow);
  growRef.current = list.grow;

  useLayoutEffect(() => {
    const root = scrollRef.current;
    if (root) root.scrollTop = list.scrollTop.current;
    return () => {
      if (root) list.scrollTop.current = root.scrollTop;
    };
    // Mount/unmount only.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    if (!hasMore) return;
    const sentinel = sentinelRef.current;
    const root = scrollRef.current;
    if (!sentinel || !root) return;
    const observer = new IntersectionObserver(
      ([entry]) => {
        if (entry?.isIntersecting) growRef.current();
      },
      { root, rootMargin: "240px" },
    );
    observer.observe(sentinel);
    return () => observer.disconnect();
    // rows.length: a new observer is what re-reports a sentinel that is still
    // visible after a page lands.
  }, [hasMore, shown, rows.length]);

  const showDomain = scopeSpansDomains(scope, mailboxes);
  const compact = density === "compact";
  const now = Date.now();
  const canSave = hasInput && saveErrors.length === 0 && saveRules.length > 0;

  return (
    <div className="flex min-h-0 flex-1 flex-col" data-mail-threads>
      {inFolder ? (
        <div className="flex shrink-0 items-center gap-2 border-b border-stroke px-3 py-2">
          <p className="min-w-0 flex-1 text-[11px] text-content/50">
            A smart folder is its own filter. Edit its conditions to change what
            it shows.
          </p>
          {onEditFolder ? (
            <button
              type="button"
              onClick={onEditFolder}
              className="flex shrink-0 items-center gap-1 rounded-md border border-content/10 px-2 py-0.5 text-[11px] text-content/70 hover:bg-content/10 hover:text-content focus-visible:outline-2 focus-visible:outline-accent"
            >
              <Pencil className="size-3" strokeWidth={1.75} />
              Edit folder
            </button>
          ) : null}
        </div>
      ) : inArchive ? (
        <div className="shrink-0 border-b border-stroke px-3 py-2">
          <p className="text-[11px] text-content/50">
            Mail you archived. Open a thread to read it or move it back to the
            inbox.
          </p>
        </div>
      ) : (
        <div className="shrink-0 border-b border-stroke px-3 py-2">
          <input
            type="search"
            aria-label="Search mail"
            placeholder="Search: from:  subject:  has:attachment  is:unread"
            value={text}
            onChange={(e) => setText(e.target.value)}
            className="h-7 w-full rounded-md border border-content/10 bg-transparent px-2 text-[12px] text-content outline-none placeholder:text-content/35 focus:border-accent/50"
          />
          {queryError ? (
            <p role="status" className="mt-1 text-[11px] text-red-400/90">
              {queryError}
            </p>
          ) : null}
          <div className="mt-1.5 flex flex-wrap items-center gap-1.5">
            {CHIP_FIELDS.map((field) => {
              const active = chips[field].trim() !== "";
              return (
                <span key={field} className="flex items-center gap-1">
                  <button
                    type="button"
                    aria-pressed={active || open === field}
                    title={CHIP_HINT[field]}
                    onClick={() => setOpen(open === field ? null : field)}
                    className={`${CHIP} ${
                      active
                        ? "border-accent/40 bg-accent/10 text-content"
                        : "border-content/10 bg-content/5 text-content/70 hover:bg-content/10"
                    }`}
                  >
                    {CHIP_LABEL[field]}
                    {active ? `: ${chips[field].trim()}` : ""}
                  </button>
                  {open === field ? (
                    <input
                      autoFocus
                      aria-label={`Filter by ${field}`}
                      value={chips[field]}
                      onChange={(e) =>
                        setChips({ ...chips, [field]: e.target.value })
                      }
                      onKeyDown={(e) => {
                        if (e.key === "Enter" || e.key === "Escape")
                          setOpen(null);
                      }}
                      className="h-6 w-28 rounded-md border border-content/10 bg-transparent px-1.5 text-[11px] text-content outline-none focus:border-accent/50"
                    />
                  ) : null}
                </span>
              );
            })}
            {hasInput ? (
              <button
                type="button"
                onClick={clearFilters}
                className="text-[11px] text-content/50 hover:text-content"
              >
                Clear
              </button>
            ) : null}
            {onSaveAsFolder ? (
              <button
                type="button"
                disabled={!canSave}
                title={
                  canSave
                    ? "Keep these filters as a smart folder"
                    : "Type a search or a filter to save it as a smart folder"
                }
                onClick={() => onSaveAsFolder(saveRules, saveMatchMode)}
                className="ml-auto rounded-md border border-content/10 px-2 py-0.5 text-[11px] text-content/70 hover:bg-content/10 hover:text-content focus-visible:outline-2 focus-visible:outline-accent disabled:opacity-50 disabled:hover:bg-transparent"
              >
                Save as folder
              </button>
            ) : null}
          </div>
        </div>
      )}

      <div ref={scrollRef} className="min-h-0 flex-1 overflow-y-auto">
        {/* role restored: list-style none strips list semantics in WebKit */}
        <ul role="list" aria-label="Threads" className="m-0 list-none p-0">
          {rows.slice(0, shown).map((row) => {
            const unread = row.unreadCount > 0;
            const selected = row.id === selectedId;
            return (
              <li key={row.id}>
                <button
                  type="button"
                  data-thread-id={row.id}
                  aria-current={selected ? "true" : undefined}
                  onClick={() => onSelect(row)}
                  className={`flex w-full min-w-0 items-start gap-2 border-b border-content/5 px-3 text-left hover:bg-content/5 focus-visible:outline-2 focus-visible:outline-accent ${
                    compact ? "py-1" : "py-2"
                  } ${selected ? "bg-content/10" : ""}`}
                >
                  <span
                    aria-label={unread ? "Unread" : undefined}
                    className={`mt-1.5 size-1.5 shrink-0 rounded-full ${unread ? "bg-accent" : ""}`}
                  />
                  <span className="flex min-w-0 flex-1 flex-col">
                    <span className="flex min-w-0 items-center gap-1.5 text-[12px]">
                      <span
                        className={`min-w-0 truncate ${unread ? "font-semibold text-content" : "text-content/80"}`}
                      >
                        {participantsLabel(row)}
                      </span>
                      {row.messageCount > 1 ? (
                        <span
                          title={`${row.messageCount} messages`}
                          className="shrink-0 text-[10px] text-content/50"
                        >
                          {row.messageCount}
                        </span>
                      ) : null}
                    </span>
                    <span
                      className={`truncate text-[12px] ${unread ? "text-content" : "text-content/70"}`}
                    >
                      {row.subject || "(no subject)"}
                    </span>
                    {compact ? null : (
                      <span className="truncate text-[11px] text-content/45">
                        {row.snippet}
                      </span>
                    )}
                  </span>
                  <span className="flex shrink-0 flex-col items-end gap-0.5">
                    <span className="text-[11px] text-content/50">
                      {formatMailDate(row.lastMessageAt, now)}
                    </span>
                    <span className="flex items-center gap-1">
                      {showDomain ? (
                        <span
                          data-chip="domain"
                          className="rounded-full border border-content/10 bg-content/5 px-1.5 text-[10px] text-content/60"
                        >
                          {row.domain}
                        </span>
                      ) : null}
                      <span
                        data-chip="mailbox"
                        className="max-w-28 truncate rounded-full border border-content/10 bg-content/5 px-1.5 text-[10px] text-content/60"
                      >
                        {row.mailboxAddress.slice(
                          0,
                          row.mailboxAddress.lastIndexOf("@") + 1,
                        )}
                      </span>
                      {row.hasAttachments ? (
                        <Paperclip
                          aria-label="Has attachments"
                          className="size-3 text-content/50"
                          strokeWidth={1.75}
                        />
                      ) : null}
                      {row.isStarred ? (
                        <Star
                          aria-label="Starred"
                          className="size-3 text-content/70"
                          strokeWidth={1.75}
                        />
                      ) : null}
                    </span>
                  </span>
                </button>
              </li>
            );
          })}
        </ul>
        {hasMore ? (
          <div ref={sentinelRef} aria-hidden className="h-px" />
        ) : null}
        {loading ? (
          <p className="px-3 py-2 text-[12px] text-content/50">
            <Loader className="mr-1 inline size-3 animate-spin" />
            Loading
          </p>
        ) : null}
        {error ? (
          <p role="alert" className="px-3 py-2 text-[12px] text-red-400/90">
            Could not load threads: {error}
          </p>
        ) : null}
        {!loading && !error && rows.length === 0 ? (
          <p className="px-3 py-2 text-[12px] text-content/50">
            {inFolder
              ? "Nothing in this folder yet. Matching mail that arrives will show up here."
              : inArchive
                ? "Nothing archived."
                : filtering
                  ? "No threads match."
                  : "No mail here yet."}
          </p>
        ) : null}
      </div>
    </div>
  );
}
