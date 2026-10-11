import { useCallback, useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import {
  AlertCircle,
  ChevronDown,
  ChevronRight,
  ListFilter,
  Inbox,
  Loader,
  RefreshCw,
} from "../../../shared/ui/icons";
import {
  MAIL_MESSAGES_CHANGED_EVENT,
  MAIL_SYNC_STATE_EVENT,
  mailStatus,
  mailSyncNow,
} from "../model/mail";
import type {
  MailStatus,
  MailboxStatus,
  SourceStatus,
} from "../model/mailTypes";
import { collapsedDomains, useCollapsedDomains } from "../model/mailPrefs";

/** A saved filter. Task 12 supplies these; until then the list is empty. */
export type SmartFolderEntry = { id: string; name: string; unread: number };

export type MailDomainGroup = {
  domain: string;
  mailboxes: { mailbox: MailboxStatus; source: SourceStatus }[];
  unread: number;
  /** Errors of the sources serving this domain; never shown app-wide. */
  errors: { source: string; message: string }[];
  /** More than one source serves the domain, so rows name their source. */
  multiSource: boolean;
};

/** Domain is a display concern, read off the address. There is no domain table. */
export function domainOf(address: string): string {
  return address.slice(address.lastIndexOf("@") + 1).toLowerCase();
}

/**
 * One group per domain even when two sources serve it (different regions):
 * the source is a detail on the mailbox row, not another tree level.
 * Disabled sources are skipped because they do not sync.
 */
export function groupMailboxesByDomain(
  sources: SourceStatus[],
): MailDomainGroup[] {
  const groups = new Map<string, MailDomainGroup>();
  const ordered = sources
    .filter((source) => source.enabled)
    .sort((a, b) => a.sortOrder - b.sortOrder || a.id - b.id);
  for (const source of ordered) {
    const rows = [...source.mailboxes].sort(
      (a, b) => a.sortOrder - b.sortOrder || a.id - b.id,
    );
    for (const mailbox of rows) {
      const domain = domainOf(mailbox.address);
      let group = groups.get(domain);
      if (!group) {
        group = {
          domain,
          mailboxes: [],
          unread: 0,
          errors: [],
          multiSource: false,
        };
        groups.set(domain, group);
      }
      group.mailboxes.push({ mailbox, source });
      group.unread += mailbox.unread;
      if (
        source.lastError &&
        !group.errors.some((e) => e.source === source.label)
      ) {
        group.errors.push({ source: source.label, message: source.lastError });
      }
    }
  }
  for (const group of groups.values()) {
    group.multiSource =
      new Set(group.mailboxes.map((m) => m.source.id)).size > 1;
  }
  return [...groups.values()];
}

/** Local part plus `@`, as the mockup labels mailboxes under a domain. */
export function mailboxLabel(mailbox: MailboxStatus): string {
  if (mailbox.displayName) return mailbox.displayName;
  const at = mailbox.address.lastIndexOf("@");
  return at > 0 ? `${mailbox.address.slice(0, at)}@` : mailbox.address;
}

/** Status for the whole Mail view, refreshed when sync state or messages change. */
export function useMailStatus() {
  const [status, setStatus] = useState<MailStatus | null>(null);
  const [error, setError] = useState<string | null>(null);
  const refresh = useCallback(async () => {
    try {
      setStatus(await mailStatus());
      setError(null);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  }, []);

  useEffect(() => {
    let disposed = false;
    const stops: (() => void)[] = [];
    void refresh();
    for (const event of [MAIL_SYNC_STATE_EVENT, MAIL_MESSAGES_CHANGED_EVENT]) {
      void listen(event, () => void refresh())
        .then((stop) => {
          if (disposed) stop();
          else stops.push(stop);
        })
        .catch(() => undefined);
    }
    return () => {
      disposed = true;
      for (const stop of stops) stop();
    };
  }, [refresh]);

  return { status, error, refresh };
}

const BADGE =
  "shrink-0 rounded-full border border-content/10 bg-content/5 px-1.5 text-[10px] text-content/60";
const NODE =
  "flex w-full min-w-0 items-center gap-2 rounded-md px-2 py-1 text-left text-[12px] hover:bg-content/10 focus-visible:outline-2 focus-visible:outline-accent";

type Props = {
  status: MailStatus | null;
  error?: string | null;
  /** Selected mailbox id; null with no folder means "All mail". */
  mailboxId: number | null;
  folderId: string | null;
  smartFolders?: SmartFolderEntry[];
  onSelectAll: () => void;
  onSelectMailbox: (id: number) => void;
  onSelectFolder: (id: string) => void;
  onSynced?: () => void;
};

export function MailFolderList({
  status,
  error = null,
  mailboxId,
  folderId,
  smartFolders = [],
  onSelectAll,
  onSelectMailbox,
  onSelectFolder,
  onSynced,
}: Props) {
  const collapsed = useCollapsedDomains();
  const [syncing, setSyncing] = useState(false);
  const groups = status ? groupMailboxesByDomain(status.sources) : [];
  const allSelected = mailboxId === null && folderId === null;

  const toggle = (domain: string) =>
    collapsedDomains.save(
      collapsed.includes(domain)
        ? collapsed.filter((d) => d !== domain)
        : [...collapsed, domain],
    );

  const syncAll = async () => {
    setSyncing(true);
    try {
      await mailSyncNow({ kind: "all" });
    } catch {
      // Per-source failures arrive as lastError on the next status read.
    } finally {
      setSyncing(false);
      onSynced?.();
    }
  };

  return (
    <nav aria-label="Mail folders" className="flex min-h-0 flex-1 flex-col">
      <div className="min-h-0 flex-1 overflow-y-auto p-2">
        <button
          type="button"
          aria-current={allSelected ? "true" : undefined}
          onClick={onSelectAll}
          className={`${NODE} ${allSelected ? "bg-content/10 text-content" : "text-content/80"}`}
        >
          <Inbox className="size-3.5 shrink-0" strokeWidth={1.75} />
          <span className="min-w-0 flex-1 truncate">All mail</span>
          {status && status.unreadTotal > 0 ? (
            <span className={BADGE}>{status.unreadTotal}</span>
          ) : null}
        </button>

        {error && !status ? (
          <p role="alert" className="px-2 py-2 text-[12px] text-red-400/90">
            Could not load mail: {error}
          </p>
        ) : null}
        {!status && !error ? (
          <p className="px-2 py-2 text-[12px] text-content/50">
            <Loader className="mr-1 inline size-3 animate-spin" />
            Loading
          </p>
        ) : null}
        {status && groups.length === 0 ? (
          <p className="px-2 py-2 text-[12px] text-content/50">
            No mailboxes yet. Add a source in Settings.
          </p>
        ) : null}

        {groups.map((group) => {
          const isCollapsed = collapsed.includes(group.domain);
          const Chevron = isCollapsed ? ChevronRight : ChevronDown;
          return (
            <div key={group.domain} data-mail-domain={group.domain}>
              <button
                type="button"
                aria-expanded={!isCollapsed}
                onClick={() => toggle(group.domain)}
                className={`${NODE} mt-1 text-content/70`}
              >
                <Chevron className="size-3 shrink-0" strokeWidth={1.75} />
                <span className="min-w-0 flex-1 truncate">{group.domain}</span>
                {group.errors.length > 0 ? (
                  <span
                    role="img"
                    aria-label="Sync error"
                    title={group.errors
                      .map((e) => `${e.source}: ${e.message}`)
                      .join("\n")}
                    className="shrink-0 text-red-400"
                  >
                    <AlertCircle className="size-3" strokeWidth={1.75} />
                  </span>
                ) : null}
                {group.unread > 0 ? (
                  <span className={BADGE}>{group.unread}</span>
                ) : null}
              </button>
              {isCollapsed
                ? null
                : group.mailboxes.map(({ mailbox, source }) => {
                    const selected = mailboxId === mailbox.id && !folderId;
                    return (
                      <button
                        key={mailbox.id}
                        type="button"
                        aria-current={selected ? "true" : undefined}
                        onClick={() => onSelectMailbox(mailbox.id)}
                        className={`${NODE} pl-6 ${selected ? "bg-content/10 text-content" : "text-content/80"}`}
                      >
                        <span
                          aria-hidden
                          className="size-2 shrink-0 rounded-full bg-content/30"
                          style={
                            mailbox.color
                              ? { backgroundColor: mailbox.color }
                              : undefined
                          }
                        />
                        <span className="min-w-0 flex-1 truncate">
                          {mailboxLabel(mailbox)}
                        </span>
                        {group.multiSource ? (
                          <span className={`${BADGE} max-w-20 truncate`}>
                            {source.label}
                          </span>
                        ) : null}
                        {mailbox.unread > 0 ? (
                          <span className={BADGE}>{mailbox.unread}</span>
                        ) : null}
                      </button>
                    );
                  })}
            </div>
          );
        })}

        <div className="mt-3 px-2 pb-1 text-[10px] font-medium uppercase tracking-wide text-content/40">
          Smart folders
        </div>
        {smartFolders.length === 0 ? (
          <p className="px-2 py-1 text-[12px] text-content/50">
            No smart folders yet.
          </p>
        ) : (
          smartFolders.map((folder) => {
            const selected = folderId === folder.id;
            return (
              <button
                key={folder.id}
                type="button"
                aria-current={selected ? "true" : undefined}
                onClick={() => onSelectFolder(folder.id)}
                className={`${NODE} ${selected ? "bg-content/10 text-content" : "text-content/80"}`}
              >
                <ListFilter className="size-3.5 shrink-0" strokeWidth={1.75} />
                <span className="min-w-0 flex-1 truncate">{folder.name}</span>
                {folder.unread > 0 ? (
                  <span className={BADGE}>{folder.unread}</span>
                ) : null}
              </button>
            );
          })
        )}
      </div>
      <div className="flex shrink-0 items-center gap-2 border-t border-stroke px-3 py-2 text-[11px] text-content/50">
        <span className="min-w-0 flex-1 truncate">
          {status && status.sources.length > 0
            ? "Mail syncs in the background"
            : "No sources"}
        </span>
        <button
          type="button"
          title="Syncs every source"
          aria-label="Sync all sources"
          disabled={syncing}
          onClick={() => void syncAll()}
          className="rounded-md p-1 hover:bg-content/10 hover:text-content focus-visible:outline-2 focus-visible:outline-accent disabled:opacity-50"
        >
          <RefreshCw
            className={`size-3.5 ${syncing ? "animate-spin" : ""}`}
            strokeWidth={1.75}
          />
        </button>
      </div>
    </nav>
  );
}
