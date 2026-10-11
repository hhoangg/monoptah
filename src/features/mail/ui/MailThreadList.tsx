import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { LIST_PAGE_SIZE, listWindowSize } from "../../../shared/lib/listWindow";
import { Loader, Paperclip, Star } from "../../../shared/ui/icons";
import { MAIL_MESSAGES_CHANGED_EVENT, mailThreads } from "../model/mail";
import { useDensity } from "../model/mailPrefs";
import type {
  MailAddress,
  MailMailbox,
  ThreadCursor,
  ThreadRow,
  ThreadScope,
} from "../model/mailTypes";
import { domainOf } from "../model/mailDomain";

/** Rows fetched per backend page; the DOM window grows in LIST_PAGE_SIZE steps. */
const FETCH_LIMIT = 100;

/**
 * Consecutive fetched pages that add no row matching the active filter before
 * paging pauses for a "Load more" press. Without it a filter matching nothing
 * would pull the whole mailbox with no user gesture.
 */
const MAX_IDLE_PAGES = 3;

const DAY_MS = 86_400_000;
const FILTER_FIELDS = ["from", "to", "content"] as const;
type FilterField = (typeof FILTER_FIELDS)[number];
export type ThreadFilters = Record<FilterField, string>;
export const NO_FILTERS: ThreadFilters = { from: "", to: "", content: "" };

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

const anyActive = (filters: ThreadFilters) =>
  FILTER_FIELDS.some((f) => filters[f].trim() !== "");

export function filterThreads(rows: ThreadRow[], filters: ThreadFilters) {
  const f = {
    from: filters.from.trim().toLowerCase(),
    to: filters.to.trim().toLowerCase(),
    content: filters.content.trim().toLowerCase(),
  };
  if (!f.from && !f.to && !f.content) return rows;
  const people = (row: ThreadRow) =>
    row.participants
      .map((p) => `${p.name ?? ""} ${p.addr}`)
      .join(" ")
      .toLowerCase();
  return rows.filter((row) => {
    // A thread row carries one participant list, not a from/to split, so both
    // filters match against it.
    if (f.from && !people(row).includes(f.from)) return false;
    if (f.to && !people(row).includes(f.to)) return false;
    if (
      f.content &&
      !`${row.subject} ${row.snippet}`.toLowerCase().includes(f.content)
    )
      return false;
    return true;
  });
}

const CHIP =
  "rounded-full border px-2 py-0.5 text-[11px] focus-visible:outline-2 focus-visible:outline-accent";

/**
 * List state lives in MailView, above the keyed threads section: that section
 * remounts when the reading pane changes side, and the loaded pages, window
 * size, filters and scroll offset must survive it.
 */
export function useMailThreadList(scope: ThreadScope, folderSelected: boolean) {
  const [rows, setRows] = useState<ThreadRow[]>([]);
  const [cursor, setCursor] = useState<ThreadCursor | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [limit, setLimit] = useState(LIST_PAGE_SIZE);
  const [filters, setFilters] = useState<ThreadFilters>(NO_FILTERS);
  const [reloadKey, setReloadKey] = useState(0);
  const [idlePages, setIdlePagesState] = useState(0);
  const idleRef = useRef(0);
  const setIdlePages = (n: number) => {
    idleRef.current = n;
    setIdlePagesState(n);
  };
  const filtersRef = useRef(filters);
  filtersRef.current = filters;
  // Survives the list remounting, so a layout change keeps the user's place.
  const scrollTop = useRef(0);
  // Bumped per first-page load so a page fetch begun under an older scope or
  // reload cannot append into the current list.
  const fetchingRef = useRef(false);
  const generation = useRef(0);
  const loadedScope = useRef<string | null>(null);
  const scopeKey = scope.kind === "all" ? "all" : `m${scope.mailboxId}`;

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
    // Before the folder early return: a page in flight from the previous scope
    // must not append onto the list cleared below.
    generation.current += 1;
    fetchingRef.current = false;
    setIdlePages(0);
    // Reset on the scope change itself, so a failed first fetch does not leave
    // the previous scope's window behind. A background reload keeps it.
    const scopeChanged = loadedScope.current !== scopeKey;
    loadedScope.current = scopeKey;
    if (scopeChanged) setLimit(LIST_PAGE_SIZE);
    if (folderSelected) {
      setRows([]);
      setCursor(null);
      setLoading(false);
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
    // `scope` is an object; scopeKey is its identity.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [scopeKey, folderSelected, reloadKey]);

  const filtered = filterThreads(rows, filters);
  const shown = listWindowSize(filtered.length, limit);
  const windowHasMore = shown < filtered.length;
  const pageHasMore = cursor !== null;
  const hasMore = windowHasMore || pageHasMore;

  const grow = (manual = false) => {
    if (windowHasMore) {
      setLimit((current) => current + LIST_PAGE_SIZE);
      return;
    }
    if (!cursor || fetchingRef.current) return;
    if (!manual && idleRef.current >= MAX_IDLE_PAGES) return;
    if (manual) setIdlePages(0);
    fetchingRef.current = true;
    const after = cursor;
    const mine = generation.current;
    mailThreads({ scope, cursor: after, limit: FETCH_LIMIT })
      .then((page) => {
        if (mine !== generation.current) return;
        setIdlePages(
          filterThreads(page.rows, filtersRef.current).length > 0 ||
            !anyActive(filtersRef.current)
            ? 0
            : idleRef.current + 1,
        );
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

  const paused = pageHasMore && idlePages >= MAX_IDLE_PAGES;
  return {
    moreOnServer: pageHasMore,
    paused,
    loadMore: () => grow(true),
    rows,
    filtered,
    shown,
    hasMore,
    loading,
    error,
    filters,
    setFilters: (next: ThreadFilters) => {
      setIdlePages(0);
      setFilters(next);
    },
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
  /** Smart folders are not queryable yet; their threads come with Task 13. */
  folderSelected?: boolean;
};

export function MailThreadList({
  list,
  scope,
  mailboxes,
  selectedId,
  onSelect,
  folderSelected = false,
}: Props) {
  const {
    rows,
    filtered,
    shown,
    hasMore,
    loading,
    error,
    filters,
    setFilters,
    moreOnServer,
    paused,
    loadMore,
  } = list;
  const density = useDensity();
  const [open, setOpen] = useState<FilterField | null>(null);
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
    // rows.length: with a filter active a fetched page may not change `shown`,
    // and a new observer is what re-reports a sentinel that is still visible.
  }, [hasMore, shown, rows.length]);

  const showDomain = scopeSpansDomains(scope, mailboxes);
  const compact = density === "compact";
  const now = Date.now();
  const anyFilter = FILTER_FIELDS.some((f) => filters[f].trim() !== "");

  return (
    <div className="flex min-h-0 flex-1 flex-col" data-mail-threads>
      <div className="flex shrink-0 flex-wrap items-center gap-1.5 border-b border-stroke px-3 py-2">
        {FILTER_FIELDS.map((field) => {
          const active = filters[field].trim() !== "";
          return (
            <span key={field} className="flex items-center gap-1">
              <button
                type="button"
                aria-pressed={active || open === field}
                title="Filters the threads loaded so far"
                onClick={() => setOpen(open === field ? null : field)}
                className={`${CHIP} ${
                  active
                    ? "border-accent/40 bg-accent/10 text-content"
                    : "border-content/10 bg-content/5 text-content/70 hover:bg-content/10"
                }`}
              >
                {field === "from" ? "From" : field === "to" ? "To" : "Content"}
                {active ? `: ${filters[field].trim()}` : ""}
              </button>
              {open === field ? (
                <input
                  autoFocus
                  aria-label={`Filter by ${field}`}
                  value={filters[field]}
                  onChange={(e) =>
                    setFilters({ ...filters, [field]: e.target.value })
                  }
                  onKeyDown={(e) => {
                    if (e.key === "Enter" || e.key === "Escape") setOpen(null);
                  }}
                  className="h-6 w-28 rounded-md border border-content/10 bg-transparent px-1.5 text-[11px] text-content outline-none focus:border-accent/50"
                />
              ) : null}
            </span>
          );
        })}
        {anyFilter ? (
          <button
            type="button"
            onClick={() => setFilters(NO_FILTERS)}
            className="text-[11px] text-content/50 hover:text-content"
          >
            Clear
          </button>
        ) : null}
        <button
          type="button"
          disabled
          title="Smart folders are not available yet"
          className="ml-auto rounded-md border border-content/10 px-2 py-0.5 text-[11px] text-content/70 opacity-50"
        >
          Save as folder
        </button>
      </div>

      <div ref={scrollRef} className="min-h-0 flex-1 overflow-y-auto">
        {/* role restored: list-style none strips list semantics in WebKit */}
        <ul role="list" aria-label="Threads" className="m-0 list-none p-0">
          {filtered.slice(0, shown).map((row) => {
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
        {anyFilter && moreOnServer ? (
          <p className="px-3 py-1 text-[11px] text-content/45">
            Filtering the {rows.length} threads loaded so far, not the whole
            mailbox.
          </p>
        ) : null}
        {paused ? (
          <div className="px-3 py-2">
            <button
              type="button"
              onClick={loadMore}
              className="rounded-md border border-content/10 px-2.5 py-1 text-[12px] text-content/70 hover:bg-content/10 hover:text-content focus-visible:outline-2 focus-visible:outline-accent"
            >
              Load more
            </button>
          </div>
        ) : null}
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
        {!loading && !error && folderSelected ? (
          <p className="px-3 py-2 text-[12px] text-content/50">
            Smart folder results are not available yet.
          </p>
        ) : null}
        {!loading && !error && !folderSelected && filtered.length === 0 ? (
          <p className="px-3 py-2 text-[12px] text-content/50">
            {rows.length === 0 ? "No mail here yet." : "No threads match."}
          </p>
        ) : null}
      </div>
    </div>
  );
}
