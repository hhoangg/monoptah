// The registry of the terminal sessions of the open desktop connection, and the state of the phone drawer. Keyed by the
// link connection, so a new connection (a sign-out, a test) never meets a session of the old one. React reads it through
// `useSyncExternalStore` (hooks below); the sessions themselves are plain classes (session.ts).
import { useCallback, useEffect, useMemo, useSyncExternalStore } from "react";
import { requestTerminalTicket } from "@/features/desktops/api";
import { useThisDevice } from "@/features/desktops/thisDevice";
import { useLinkConnection, useLinkDesktopId } from "@/link";
import type { LinkConnection } from "@/link";
import { TerminalSession } from "./session";

let sessions = new Map<object, Map<string, TerminalSession>>();
const listeners = new Set<() => void>();
/** Bumped when a session is added or removed, the drawer changes, or any session changes. */
let version = 0;
const watching = new WeakMap<TerminalSession, () => void>();

function notify(): void {
  version += 1;
  for (const listener of [...listeners]) listener();
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

const getVersion = (): number => version;

function mapOf(connection: object): Map<string, TerminalSession> {
  let map = sessions.get(connection);
  if (!map) {
    map = new Map();
    sessions.set(connection, map);
  }
  return map;
}

/** The session of a terminal id, made on first use. A saved resume entry reattaches it to its shell. */
export function ensureSession(connection: LinkConnection, desktopId: string, id: string): TerminalSession {
  const map = mapOf(connection);
  let session = map.get(id);
  if (!session) {
    session = new TerminalSession(desktopId, id, connection);
    map.set(id, session);
    watching.set(session, session.subscribe(notify));
    notify();
  }
  return session;
}

export function findSession(connection: LinkConnection, id: string): TerminalSession | undefined {
  return mapOf(connection).get(id);
}

export function listSessions(connection: LinkConnection, desktopId: string): TerminalSession[] {
  return [...mapOf(connection).values()].filter((session) => session.desktopId === desktopId);
}

/** Kills the shell of a terminal (when it has one) and forgets the session. */
export async function closeSession(connection: LinkConnection, id: string): Promise<void> {
  const session = mapOf(connection).get(id);
  if (!session) return;
  mapOf(connection).delete(id);
  drawerRemove(session.desktopId, id);
  watching.get(session)?.();
  await session.close();
  notify();
}

// --- Closing with a confirmation ---

export interface CloseRequest {
  /** How many running shells the close would end. */
  count: number;
  confirm: () => void;
}

let closeRequest: CloseRequest | null = null;

export function setCloseRequest(request: CloseRequest | null): void {
  closeRequest = request;
  notify();
}

/** The close that waits for the user's answer (rendered by `TerminalCloseConfirm`). */
export function useCloseRequest(): CloseRequest | null {
  useTerminalsVersion();
  return closeRequest;
}

/**
 * Closes the terminals `ids` (a tab, or a pane with tabs): `proceed` removes them from the layout and the shells are
 * killed. A running shell asks first (the request is shown by `TerminalCloseConfirm`); an unopened or ended one does not.
 */
export function guardTerminalClose(connection: LinkConnection, ids: string[], proceed: () => void): void {
  const run = (): void => {
    setCloseRequest(null);
    proceed();
    for (const id of ids) void closeSession(connection, id);
  };
  const live = ids.filter((id) => findSession(connection, id)?.live).length;
  if (live === 0) run();
  else setCloseRequest({ count: live, confirm: run });
}

/** Stops every session and forgets it without killing the shells or their resume entries (tests, and a page that goes away). */
export function resetTerminalSessions(): void {
  for (const map of sessions.values()) for (const session of map.values()) session.dispose({ keepResume: true });
  sessions = new Map();
  drawers.clear();
  notify();
}

export function newTerminalId(): string {
  return crypto.randomUUID().replace(/-/g, "").slice(0, 12);
}

/** The title of a terminal tab: "Terminal - <project>" once a project is chosen, else "Terminal"; `connected` is the green dot. */
export function terminalTitle(connection: LinkConnection, id: string): { title: string; connected: boolean } {
  const session = findSession(connection, id);
  const project = session?.titleProject ?? null;
  return { title: project ? `Terminal - ${project}` : "Terminal", connected: session?.connected ?? false };
}

// --- The phone drawer ---

export interface DrawerState {
  open: boolean;
  fullscreen: boolean;
  /** The terminals of the drawer, in the order of the switcher. */
  ids: string[];
  active: string | null;
}

const CLOSED: DrawerState = { open: false, fullscreen: false, ids: [], active: null };
const drawers = new Map<string, DrawerState>();
const idsKey = (desktopId: string): string => `monoptah.terminal.drawer.${desktopId}`;

function readIds(desktopId: string): string[] {
  try {
    const parsed: unknown = JSON.parse(sessionStorage.getItem(idsKey(desktopId)) ?? "[]");
    return Array.isArray(parsed) ? parsed.filter((id): id is string => typeof id === "string" && /^[A-Za-z0-9_-]{1,64}$/.test(id)) : [];
  } catch {
    return [];
  }
}

function writeIds(desktopId: string, ids: string[]): void {
  try {
    if (ids.length === 0) sessionStorage.removeItem(idsKey(desktopId));
    else sessionStorage.setItem(idsKey(desktopId), JSON.stringify(ids));
  } catch {
    // Without storage the drawer simply forgets its terminals on a reload.
  }
}

export function getDrawer(desktopId: string): DrawerState {
  let state = drawers.get(desktopId);
  if (!state) {
    // Terminals the drawer had before a reload: their resume entries reattach them.
    const ids = readIds(desktopId);
    state = { ...CLOSED, ids, active: ids[0] ?? null };
    drawers.set(desktopId, state);
  }
  return state;
}

function setDrawer(desktopId: string, change: Partial<DrawerState>): void {
  const next = { ...getDrawer(desktopId), ...change };
  drawers.set(desktopId, next);
  if (change.ids) writeIds(desktopId, next.ids);
  notify();
}

function drawerRemove(desktopId: string, id: string): void {
  const state = getDrawer(desktopId);
  if (!state.ids.includes(id)) return;
  const ids = state.ids.filter((other) => other !== id);
  setDrawer(desktopId, { ids, active: state.active === id ? (ids[0] ?? null) : state.active, ...(ids.length === 0 ? { open: false, fullscreen: false } : {}) });
}

// --- Hooks ---

/** Re-renders when any terminal session or the drawer changes. */
export function useTerminalsVersion(): number {
  return useSyncExternalStore(subscribe, getVersion, getVersion);
}

/** The session of a terminal tab (made on first use). Also tells it how to ask this device for a passkey ticket. */
export function useTerminalSession(id: string): TerminalSession {
  const connection = useLinkConnection();
  const desktopId = useLinkDesktopId();
  const device = useThisDevice();
  const session = useMemo(() => ensureSession(connection, desktopId, id), [connection, desktopId, id]);
  const deviceId = device?.deviceId ?? null;
  useEffect(() => {
    session.setTicketSource((projectId) => {
      if (!deviceId) return Promise.reject(new Error("This browser is not registered with your account yet."));
      return requestTerminalTicket(desktopId, deviceId, projectId);
    });
  }, [session, desktopId, deviceId]);
  return session;
}

/** The sessions of the open desktop that have a shell (running or reattaching): what the footer chip counts. */
export function useLiveTerminalCount(): number {
  const connection = useLinkConnection();
  const desktopId = useLinkDesktopId();
  useTerminalsVersion();
  return listSessions(connection, desktopId).filter((session) => session.live).length;
}

export function useDrawer(): DrawerState {
  const desktopId = useLinkDesktopId();
  useTerminalsVersion();
  return getDrawer(desktopId);
}

export interface TerminalCommands {
  /** Opens a new terminal drawer entry (a phone) and shows the drawer. */
  openDrawer: () => void;
  closeDrawer: () => void;
  toggleDrawer: () => void;
  setFullscreen: (on: boolean) => void;
  addToDrawer: () => void;
  activate: (id: string) => void;
}

export function useDrawerCommands(): TerminalCommands {
  const desktopId = useLinkDesktopId();
  return useMemo(() => {
    const addToDrawer = (): void => {
      const id = newTerminalId();
      const state = getDrawer(desktopId);
      setDrawer(desktopId, { ids: [...state.ids, id], active: id, open: true });
    };
    return {
      addToDrawer,
      openDrawer: () => {
        const state = getDrawer(desktopId);
        if (state.ids.length === 0) addToDrawer();
        else setDrawer(desktopId, { open: true, active: state.active && state.ids.includes(state.active) ? state.active : (state.ids[0] ?? null) });
      },
      closeDrawer: () => setDrawer(desktopId, { open: false, fullscreen: false }),
      toggleDrawer: () => {
        const state = getDrawer(desktopId);
        if (state.open) setDrawer(desktopId, { open: false, fullscreen: false });
        else if (state.ids.length === 0) addToDrawer();
        else setDrawer(desktopId, { open: true });
      },
      setFullscreen: (on) => setDrawer(desktopId, { fullscreen: on }),
      activate: (id) => setDrawer(desktopId, { active: id }),
    };
  }, [desktopId]);
}

/** Hook form of `closeSession` for the current connection. */
export function useCloseSession(): (id: string) => Promise<void> {
  const connection = useLinkConnection();
  return useCallback((id) => closeSession(connection, id), [connection]);
}
