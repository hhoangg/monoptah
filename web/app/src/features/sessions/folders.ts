// Session folders in a project's list: the desktop's folders (`workspace.list` `folders`) grouped over the sessions
// the list has loaded, and the collapse state. Pure functions here; `useFolderCollapse` keeps the toggle on this device.
import { useCallback, useEffect, useSyncExternalStore } from "react";
import type { FolderCard, SessionCard } from "@monoptah/protocol";

export interface SessionGroup {
  folder: FolderCard;
  sessions: SessionCard[];
}

/** A project's folders, in the desktop's order. */
export function projectFolders(folders: readonly FolderCard[] | undefined, projectId: string): FolderCard[] {
  return (folders ?? []).filter((folder) => folder.projectId === projectId);
}

/** The ids of every session some folder holds. */
export function folderedIds(folders: readonly FolderCard[]): Set<string> {
  return new Set(folders.flatMap((folder) => folder.sessionIds));
}

/** The folder a session is in, if any. */
export function folderOf(folders: readonly FolderCard[], sessionId: string): FolderCard | undefined {
  return folders.find((folder) => folder.sessionIds.includes(sessionId));
}

/** Whether a project id is a remote-machine project's (an opaque `remote-` id or an encoded `remote://` path): the desktop keeps no folders for those and refuses edits. */
export function isRemoteProject(projectId: string): boolean {
  if (/^remote-[0-9a-f]{16}$/.test(projectId)) return true;
  try {
    return decodeURIComponent(projectId).toLowerCase().startsWith("remote://");
  } catch {
    return false;
  }
}

/** Whether a session of this project can be filed: the desktop sends folders at all (an older one does not) and the project is local. */
export function canEditFolders(catalogFolders: readonly FolderCard[] | undefined, projectId: string): boolean {
  return catalogFolders !== undefined && !isRemoteProject(projectId);
}

/** The text a search matches, as `sessions.list` does: title, model and branch. */
export function matchesQuery(session: Pick<SessionCard, "title" | "model" | "branch">, query: string): boolean {
  const needle = query.trim().toLowerCase();
  return !needle || [session.title, session.model, session.branch].some((field) => field.toLowerCase().includes(needle));
}

/**
 * The folders with their sessions, in the folders' order and each folder's own order. `known` holds every session the
 * client has (the catalog's and the loaded pages'); a member it does not know, or `keep` rejects, is left out, and so is
 * a folder with none left.
 */
export function groupSessions(folders: readonly FolderCard[], known: ReadonlyMap<string, SessionCard>, keep: (session: SessionCard) => boolean): SessionGroup[] {
  const groups: SessionGroup[] = [];
  for (const folder of folders) {
    const sessions = folder.sessionIds.flatMap((id) => {
      const session = known.get(id);
      return session && keep(session) ? [session] : [];
    });
    if (sessions.length > 0) groups.push({ folder, sessions });
  }
  return groups;
}

/** The desktop's folder palette (tabGroups.ts); index 0 is the default wash and has no color here. */
const PALETTE = [
  null,
  "hsl(211 92% 62%)",
  "hsl(12 80% 58%)",
  "hsl(45 90% 55%)",
  "hsl(142 55% 50%)",
  "hsl(330 70% 62%)",
  "hsl(280 55% 62%)",
  "hsl(175 55% 48%)",
  "hsl(25 85% 58%)",
] as const;

/** The color of a folder's icon, or undefined for the default one. A custom color must be `#rrggbb`. */
export function folderColor(folder: Pick<FolderCard, "colorIndex" | "customColor">): string | undefined {
  if (folder.customColor && /^#[0-9a-fA-F]{6}$/.test(folder.customColor)) return folder.customColor;
  return (folder.colorIndex ? PALETTE[folder.colorIndex] : null) ?? undefined;
}

export const FOLDER_COLLAPSE_KEY = "monoptah.folders.collapsed";

/** What this device chose for one folder, and what the desktop said when it chose. */
interface CollapseChoice {
  desktop: boolean;
  local: boolean;
}

type Choices = Record<string, CollapseChoice>;

function readChoices(desktopId: string): Choices {
  try {
    const raw = window.localStorage.getItem(`${FOLDER_COLLAPSE_KEY}.${desktopId}`);
    const parsed: unknown = raw ? JSON.parse(raw) : null;
    if (!parsed || typeof parsed !== "object") return {};
    const out: Choices = {};
    for (const [id, value] of Object.entries(parsed)) {
      const choice = value as Partial<CollapseChoice> | null;
      if (choice && typeof choice.desktop === "boolean" && typeof choice.local === "boolean") out[id] = { desktop: choice.desktop, local: choice.local };
    }
    return out;
  } catch {
    return {};
  }
}

/** Whether a folder shows closed: this device's own choice, until the desktop folds or unfolds it again. */
export function isCollapsed(folder: Pick<FolderCard, "id" | "collapsed">, choices: Choices): boolean {
  const desktop = folder.collapsed === true;
  const choice = choices[folder.id];
  return choice && choice.desktop === desktop ? choice.local : desktop;
}

/** The choices of one desktop, held outside React so the list and the pruning below share them. */
class ChoiceStore {
  #choices: Choices;
  readonly #listeners = new Set<() => void>();
  readonly #key: string;

  constructor(desktopId: string) {
    this.#key = `${FOLDER_COLLAPSE_KEY}.${desktopId}`;
    this.#choices = readChoices(desktopId);
  }

  readonly subscribe = (listener: () => void): (() => void) => {
    this.#listeners.add(listener);
    return () => this.#listeners.delete(listener);
  };

  readonly snapshot = (): Choices => this.#choices;

  set(next: Choices): void {
    this.#choices = next;
    try {
      window.localStorage.setItem(this.#key, JSON.stringify(next));
    } catch {
      // Storage blocked or full: the choice lasts until the page reloads.
    }
    for (const listener of this.#listeners) listener();
  }
}

const stores = new Map<string, ChoiceStore>();

function choiceStore(desktopId: string): ChoiceStore {
  let store = stores.get(desktopId);
  if (!store) {
    store = new ChoiceStore(desktopId);
    stores.set(desktopId, store);
  }
  return store;
}

/** Forgets every kept choice (tests start from a clean storage). */
export function resetFolderCollapse(): void {
  stores.clear();
}

/**
 * The collapse state of the folders of one desktop. Collapse is the desktop's UI state, so a device only reads it as
 * the starting point and keeps its own toggle (no edit right needed, the desktop's sidebar is left as it is). When the
 * desktop changes a folder's state, that wins and the old choice is forgotten. The choice is kept in localStorage under
 * a `monoptah.` key. `all` is every folder of the catalog; until it arrives (or on a desktop that sends none) nothing is pruned.
 */
export function useFolderCollapse(desktopId: string, all: readonly Pick<FolderCard, "id" | "collapsed">[] | undefined) {
  const store = choiceStore(desktopId);
  const choices = useSyncExternalStore(store.subscribe, store.snapshot);
  // Forgets a choice when the desktop folded or unfolded the folder since it was made (the desktop's state is the new
  // start), and when the folder is gone. `all` is every folder of the catalog, so another project's choices stay.
  useEffect(() => {
    if (!all) return;
    const current = store.snapshot();
    const live = new Map(all.map((folder) => [folder.id, folder] as const));
    const stale = Object.keys(current).filter((id) => {
      const folder = live.get(id);
      return !folder || current[id]?.desktop !== (folder.collapsed === true);
    });
    if (stale.length === 0) return;
    const next = { ...current };
    for (const id of stale) delete next[id];
    store.set(next);
  }, [all, store]);
  const collapsed = useCallback((folder: Pick<FolderCard, "id" | "collapsed">) => isCollapsed(folder, choices), [choices]);
  const toggle = useCallback(
    (folder: Pick<FolderCard, "id" | "collapsed">) => {
      const current = store.snapshot();
      store.set({ ...current, [folder.id]: { desktop: folder.collapsed === true, local: !isCollapsed(folder, current) } });
    },
    [store],
  );
  return { collapsed, toggle };
}
