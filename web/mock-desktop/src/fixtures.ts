// Canned data for the mock desktop. Everything here is obviously fake: no real names,
// paths, tokens or hosts. `FIXTURE_HANDLERS` has one handler per method of
// `@monoptah/protocol` (the type is a `Record<MethodName, ...>`, so a method added to the
// protocol fails the typecheck until it is served here, and a test walks methods.json).
//
// The data lives in a `MockWorld` that handlers read and change. A change of a
// conversation notifies its watchers, which turn it into watch frames that carry the
// `cloud` and `approvals` fields on change only (architecture.md 6.6). Time-consuming
// things (a cloud start that takes minutes) are scripted steps that a test, or the
// `autoAdvanceMs` timer of the mock desktop, advances one at a time.
import {
  STARTUP_STEP_IDS,
  MOVE_STEP_IDS,
  type Approval,
  type CloudField,
  type DesktopErrorCode,
  type MethodName,
  type ResultOf,
  type SessionCard,
  type Turn,
  type UsageLimit,
  type WatchFrame,
} from "@monoptah/protocol";
import type { METHODS } from "@monoptah/protocol";
import { MENTION_MESSAGE, MentionError, mentionedFile } from "./mentioned.ts";
import type { z } from "zod";

/** Time used by default so fixtures are deterministic. */
export const FIXTURE_NOW = 1_700_000_000_000;

/** An error the desktop answers as `{ok: false, error, code}`. */
export class FixtureError extends Error {
  readonly code: DesktopErrorCode | undefined;
  constructor(message: string, code?: DesktopErrorCode) {
    super(message);
    this.name = "FixtureError";
    this.code = code;
  }
}

export interface MockDevice {
  /** Lowercase hex SHA-256 of the device static key. */
  fingerprint: string;
  /** `pending`: unknown or not yet confirmed. `revoked`: BYE on the next handshake. */
  status: "confirmed" | "pending" | "revoked";
  permission: "read" | "control";
  name?: string;
  /** Project and Mono ids the device may see; `null` means all (architecture.md 7.1). */
  selection: { projects: string[] | null; monos: string[] | null };
}

export interface WorldCapabilities {
  approvals: boolean;
  cloud: boolean;
}

type Target = { kind: "local" | "remote" | "codespace" | "sandbox"; instance?: string };

interface SessionRecord {
  id: string;
  projectId: string;
  title: string;
  harness: string;
  model: string;
  branch: string;
  updatedAt: number;
  busy: boolean;
  waiting?: "approval" | "question";
  target: Target;
  cloudState?: CloudField["state"];
  startup?: { step: (typeof STARTUP_STEP_IDS)[number]; error?: string };
  cloud?: CloudField;
  turns: Turn[];
  approvals: Approval[];
  /** The usage limit the conversation waits on (a test sets it); resumed through `session.resumeUsage`. */
  usageLimit?: UsageLimit | null;
  /** Text of a `conversation.send` that woke the instance and waits for it. */
  pendingSend: string | null;
  script: (() => void)[];
}

/** A session folder as the desktop keeps it: `sessionIds` in order, newest folder first in `MockWorld.folders`. */
export interface FolderRecord {
  id: string;
  projectId: string;
  name: string;
  sessionIds: string[];
  collapsed: boolean;
  colorIndex?: number;
}

interface CloudInstanceRecord {
  kind: "codespace" | "sandbox";
  instance: string;
  displayName: string;
  state: CloudField["state"];
  rawState: string;
  lastUsedAt: number | null;
  costEstimatePerHour: number;
  stopReport?: "stopping" | "takingLong" | "stillRunning";
}

/** What one watch has already been sent, so a frame carries only what changed. */
export interface WatchCursor {
  first: boolean;
  turns: Map<string, string>;
  busy: boolean | null;
  cloud: string | null;
  approvals: string | null;
}

export interface MockWorldOptions {
  now?: () => number;
  capabilities?: Partial<WorldCapabilities>;
  /** Desktop app version and the newest one in the update feed (default both "1.4.0", up to date). */
  appVersion?: string;
  latestVersion?: string;
}

const PROJECTS = [
  { id: "proj-alpha", name: "Example Alpha", path: "/example/alpha", machine: "example-machine", branch: "main", color: "#4f8cff", defaultHarness: "claude", defaultModel: "example-model-1" },
  { id: "proj-beta", name: "Example Beta", path: "/example/beta", machine: "example-machine", branch: "main", color: "#e0894a", defaultHarness: "claude", defaultModel: "example-model-1" },
] as const;

const MONOS = [{ id: "mono-example", name: "Example Mono", projectIds: ["proj-alpha"], unread: 0, status: "idle" }] as const;

export class MockWorld {
  readonly now: () => number;
  capabilities: WorldCapabilities;
  readonly sessions = new Map<string, SessionRecord>();
  readonly instances = new Map<string, CloudInstanceRecord>();
  /** Desktop app version and the newest version of the update feed; a test sets `latestVersion`. */
  appVersion: string;
  latestVersion: string;
  updateNotes = "Fixes and improvements (fake notes)";
  /** Set to make the feed unreadable: the status then reports `latest = current` and this error. */
  updateFeedError: string | undefined;
  /** Every accepted `desktop.update` that started an update (a test reads it; the version does not change until `completeUpdate`). */
  /** A test sets this to make the next accepted `desktop.update` reply `updating` and then fail (disk full, ...). */
  failNextUpdate: string | undefined;
  /** What `desktop.updateStatus` reports as `lastFailure`; cleared when the next install request starts. */
  lastUpdateFailure: string | undefined;
  readonly updatesStarted: { from: string; to: string }[] = [];
  /** Session folders of every project, in the desktop's order. */
  folders: FolderRecord[] = [];
  #nextFolder = 1;
  readonly #listeners = new Map<string, Set<() => void>>();
  #nextTurn = 100;
  #nextSession = 1;

  constructor(options: MockWorldOptions = {}) {
    this.now = options.now ?? (() => FIXTURE_NOW);
    this.capabilities = { approvals: true, cloud: true, ...options.capabilities };
    this.appVersion = options.appVersion ?? "1.4.0";
    this.latestVersion = options.latestVersion ?? this.appVersion;
    this.#seed();
  }

  // --- Seed ---

  #seed(): void {
    const at = this.now();
    this.#instance("codespace", "fake-codespace-one", "Fake codespace one", "available", "Available", 0.18);
    this.#instance("sandbox", "fake-sandbox-one", "Fake sandbox one", "stopped", "stopped", 0.05, at - 3_600_000);
    this.#instance("codespace", "fake-codespace-two", "Fake codespace two", "starting", "Provisioning", 0.18);
    this.#instance("sandbox", "fake-sandbox-two", "Fake sandbox two", "stopping", "stopping", 0.05, at - 7_200_000, "takingLong");

    this.#addSession({ id: "sess-local-1", projectId: "proj-alpha", title: "Fix the example parser", target: { kind: "local" } });
    const approval = this.#addSession({ id: "sess-local-approval", projectId: "proj-alpha", title: "Run the example checks", target: { kind: "local" }, busy: true });
    approval.waiting = "approval";
    approval.approvals = [{ requestId: "41", label: "Run example command", detail: "example-cli --check (fake detail)" }];
    approval.script = [() => this.#clearApprovals(approval)];
    const mentions = this.#addSession({ id: "sess-local-files", projectId: "proj-alpha", title: "Write the example report", target: { kind: "local" } });
    this.addAssistantText(mentions.id, MENTION_MESSAGE);
    this.#addSession({ id: "sess-codespace-1", projectId: "proj-alpha", title: "Refactor on the fake codespace", target: { kind: "codespace", instance: "fake-codespace-one" }, cloudState: "available" });
    this.#addSession({ id: "sess-sandbox-stopped", projectId: "proj-beta", title: "Stopped sandbox work", target: { kind: "sandbox", instance: "fake-sandbox-one" }, cloudState: "stopped" });
    const starting = this.#addSession({ id: "sess-codespace-starting", projectId: "proj-beta", title: "Starting codespace work", target: { kind: "codespace", instance: "fake-codespace-two" }, cloudState: "starting" });
    this.#beginStartup(starting, 1);
    this.folders.push({ id: "folder-parser", projectId: "proj-alpha", name: "Parser work", sessionIds: ["sess-local-1", "sess-local-approval"], collapsed: false });
    this.#addSession({ id: "sess-remote-1", projectId: "proj-beta", title: "Work on a remote machine", target: { kind: "remote", instance: "example-remote" }, cloudState: "available" });
  }

  #instance(kind: "codespace" | "sandbox", instance: string, displayName: string, state: CloudField["state"], rawState: string, cost: number, lastUsedAt: number | null = null, stopReport?: CloudInstanceRecord["stopReport"]): void {
    this.instances.set(instance, { kind, instance, displayName, state, rawState, lastUsedAt, costEstimatePerHour: cost, ...(stopReport ? { stopReport } : {}) });
  }

  #addSession(init: { id: string; projectId: string; title: string; target: Target; busy?: boolean; cloudState?: CloudField["state"] }): SessionRecord {
    const at = this.now();
    const record: SessionRecord = {
      id: init.id,
      projectId: init.projectId,
      title: init.title,
      harness: "claude",
      model: "example-model-1",
      branch: "example-branch",
      updatedAt: at,
      busy: init.busy ?? false,
      target: init.target,
      turns: [
        { id: `${init.id}-t1`, stampAt: at - 60_000, items: [{ type: "user", id: `${init.id}-u1`, text: "Please look at the example task.", at: at - 60_000 }] },
        { id: `${init.id}-t2`, stampAt: at - 50_000, completedAt: at - 40_000, durationMs: 10_000, items: [{ type: "text", id: `${init.id}-a1`, text: "Done. This is canned example output." }] },
      ],
      approvals: [],
      pendingSend: null,
      script: [],
    };
    if (init.cloudState) {
      record.cloudState = init.cloudState;
      if (init.target.instance) record.cloud = { kind: init.target.kind, instance: init.target.instance, state: init.cloudState };
    }
    this.sessions.set(record.id, record);
    return record;
  }

  // --- Visibility and cards ---

  /** Cloud and remote sessions exist only for web devices that see the `cloud` capability. */
  visibleSessions(): SessionRecord[] {
    return [...this.sessions.values()].filter((s) => this.capabilities.cloud || s.target.kind === "local");
  }

  card(s: SessionRecord): SessionCard {
    return {
      id: s.id,
      projectId: s.projectId,
      title: s.title,
      harness: s.harness,
      model: s.model,
      modelName: "Example Model One",
      branch: s.branch,
      runtimeMode: "supervised",
      updatedAt: s.updatedAt,
      busy: s.busy,
      ...(s.waiting ? { waiting: s.waiting } : {}),
      ...(this.capabilities.cloud
        ? {
            target: { kind: s.target.kind, ...(s.target.instance ? { instance: s.target.instance } : {}), ...(s.cloudState ? { state: s.cloudState } : {}) },
            ...(s.cloudState ? { cloudState: s.cloudState } : {}),
            ...(s.startup ? { startup: s.startup } : {}),
          }
        : {}),
    };
  }

  // --- Folders (the desktop's Groups: a new folder goes first; a folder left empty goes) ---

  folderCard(folder: FolderRecord, visible: (session: SessionRecord) => boolean): { id: string; projectId: string; name: string; sessionIds: string[]; collapsed: boolean; colorIndex?: number } | null {
    const sessionIds = folder.sessionIds.filter((id) => {
      const s = this.sessions.get(id);
      return !!s && s.projectId === folder.projectId && this.visibleSessions().includes(s) && visible(s);
    });
    if (sessionIds.length === 0) return null;
    return { id: folder.id, projectId: folder.projectId, name: folder.name, sessionIds, collapsed: folder.collapsed, ...(folder.colorIndex ? { colorIndex: folder.colorIndex } : {}) };
  }

  #leaveFolders(sessionIds: string[]): void {
    for (const f of this.folders) f.sessionIds = f.sessionIds.filter((id) => !sessionIds.includes(id));
    this.folders = this.folders.filter((f) => f.sessionIds.length > 0);
  }

  #folder(projectId: string, folderId: string): FolderRecord {
    const folder = this.folders.find((f) => f.id === folderId && f.projectId === projectId);
    if (!folder) throw new FixtureError("Folder not found");
    return folder;
  }

  #folderSession(projectId: string, conversationId: string): SessionRecord {
    const s = this.session(conversationId);
    if (s.projectId !== projectId) throw new FixtureError("Session is not in this project");
    return s;
  }

  createFolder(projectId: string, conversationIds: string[], name: string | undefined): string {
    const ids = [...new Set(conversationIds)];
    for (const id of ids) this.#folderSession(projectId, id);
    this.#leaveFolders(ids);
    const id = `folder-new-${this.#nextFolder++}`;
    // Like the desktop's Groups: an unnamed folder is "New folder", then "New folder 2" and so on within the project.
    const taken = (candidate: string) => this.folders.some((f) => f.projectId === projectId && f.name === candidate);
    let fallback = "New folder";
    for (let n = 2; taken(fallback); n++) fallback = `New folder ${n}`;
    this.folders.unshift({ id, projectId, name: name ?? fallback, sessionIds: ids, collapsed: false });
    return id;
  }

  renameFolder(projectId: string, folderId: string, name: string): void {
    this.#folder(projectId, folderId).name = name;
  }

  moveToFolder(projectId: string, conversationId: string, folderId: string | null): void {
    this.#folderSession(projectId, conversationId);
    if (folderId === null) return this.#leaveFolders([conversationId]);
    const folder = this.#folder(projectId, folderId);
    for (const f of this.folders) f.sessionIds = f.sessionIds.filter((id) => id !== conversationId);
    folder.sessionIds.push(conversationId);
    folder.collapsed = false;
    this.folders = this.folders.filter((f) => f.sessionIds.length > 0);
  }

  deleteFolder(projectId: string, folderId: string): void {
    this.#folder(projectId, folderId);
    this.folders = this.folders.filter((f) => f.id !== folderId);
  }

  session(conversationId: string): SessionRecord {
    const s = this.sessions.get(conversationId);
    if (!s || !this.visibleSessions().includes(s)) throw new FixtureError(`Unknown conversation ${conversationId}`);
    return s;
  }

  /** Appends a finished assistant turn with `text` to a conversation (the mention fixtures use it). */
  addAssistantText(conversationId: string, text: string): void {
    const s = this.session(conversationId);
    const at = this.now();
    const id = `${s.id}-m${s.turns.length + 1}`;
    s.turns.push({ id, stampAt: at - 30_000, completedAt: at - 20_000, durationMs: 10_000, items: [{ type: "text", id: `${id}-a`, text }] });
  }

  // --- Reads ---

  read(conversationId: string): ResultOf<"conversation.read"> {
    const s = this.session(conversationId);
    return {
      id: s.id,
      turns: s.turns.map((t) => structuredClone(t)),
      busy: s.busy,
      usageLimit: s.usageLimit ?? null,
      older: false,
      question: null,
      ...(this.capabilities.cloud && s.cloud ? { cloud: structuredClone(s.cloud) } : {}),
      ...(this.capabilities.approvals ? { approvals: structuredClone(s.approvals) } : {}),
    };
  }

  // --- Watches ---

  newCursor(): WatchCursor {
    return { first: true, turns: new Map(), busy: null, cloud: null, approvals: null };
  }

  /** The next watch frame for `cursor`, or null when nothing changed since the last one. */
  nextFrame(conversationId: string, cursor: WatchCursor): WatchFrame | null {
    const s = this.session(conversationId);
    const changedTurns = s.turns.filter((t) => cursor.turns.get(t.id) !== JSON.stringify(t));
    const cloudJson = this.capabilities.cloud && s.cloud ? JSON.stringify(s.cloud) : null;
    const approvalsJson = this.capabilities.approvals ? JSON.stringify(s.approvals) : null;
    const cloudChanged = cloudJson !== null && cloudJson !== cursor.cloud;
    // The first frame always states the approvals (an empty array clears a stale card).
    const approvalsChanged = approvalsJson !== null && approvalsJson !== cursor.approvals;
    if (!cursor.first && changedTurns.length === 0 && !cloudChanged && !approvalsChanged && cursor.busy === s.busy) return null;

    const frame: WatchFrame = {
      usageLimit: s.usageLimit ?? null,
      busy: s.busy,
      older: false,
      order: s.turns.map((t) => t.id),
      turns: changedTurns.map((t) => structuredClone(t)),
      ...(cursor.first ? { question: null } : {}),
      ...(cloudChanged && s.cloud ? { cloud: structuredClone(s.cloud) } : {}),
      ...(approvalsChanged ? { approvals: structuredClone(s.approvals) } : {}),
    };
    for (const t of changedTurns) cursor.turns.set(t.id, JSON.stringify(t));
    cursor.busy = s.busy;
    cursor.cloud = cloudJson;
    cursor.approvals = approvalsJson;
    cursor.first = false;
    return frame;
  }

  subscribe(conversationId: string, listener: () => void): () => void {
    let set = this.#listeners.get(conversationId);
    if (!set) this.#listeners.set(conversationId, (set = new Set()));
    set.add(listener);
    return () => set.delete(listener);
  }

  #notify(s: SessionRecord): void {
    s.updatedAt = this.now();
    for (const listener of [...(this.#listeners.get(s.id) ?? [])]) listener();
  }

  // --- Scripted progress ---

  /** True while some session still has a scripted step. */
  get hasScript(): boolean {
    return [...this.sessions.values()].some((s) => s.script.length > 0);
  }

  /** Runs the next scripted step of a session (a startup step, an approval clearing, a move step). False when none is left. */
  advance(conversationId: string): boolean {
    const s = this.sessions.get(conversationId);
    const step = s?.script.shift();
    if (!s || !step) return false;
    step();
    this.#notify(s);
    return true;
  }

  /** Advances every session that has a script by one step. */
  advanceAll(): void {
    for (const id of [...this.sessions.keys()]) this.advance(id);
  }

  #beginStartup(s: SessionRecord, from: number): void {
    const at = this.now();
    const steps = STARTUP_STEP_IDS.map((id, i) => ({
      id,
      status: (i < from ? "done" : i === from ? "running" : "pending") as "done" | "running" | "pending",
      startedAt: i <= from ? at : null,
      finishedAt: i < from ? at : null,
    }));
    const current = STARTUP_STEP_IDS[from] as (typeof STARTUP_STEP_IDS)[number];
    s.cloudState = "starting";
    s.startup = { step: current };
    s.cloud = { kind: s.target.kind, instance: s.target.instance ?? "", state: "starting", startup: { steps, current } };
    this.#setInstanceState(s, "starting", "Provisioning");
    s.script = STARTUP_STEP_IDS.slice(from).map(() => () => this.#advanceStartup(s));
  }

  #advanceStartup(s: SessionRecord): void {
    const startup = s.cloud?.startup;
    if (!s.cloud || !startup) return;
    const index = startup.steps.findIndex((step) => step.id === startup.current);
    const at = this.now();
    const done = startup.steps[index];
    if (done) Object.assign(done, { status: "done", finishedAt: at });
    const next = startup.steps[index + 1];
    if (!next) {
      s.cloud = { kind: s.cloud.kind, instance: s.cloud.instance, state: "available" };
      s.cloudState = "available";
      delete s.startup;
      this.#setInstanceState(s, "available", "Available");
      this.#flushPendingSend(s);
      return;
    }
    Object.assign(next, { status: "running", startedAt: at });
    startup.current = next.id;
    s.startup = { step: next.id };
  }

  #flushPendingSend(s: SessionRecord): void {
    if (s.pendingSend === null) return;
    this.#appendExchange(s, s.pendingSend);
    s.pendingSend = null;
  }

  #appendExchange(s: SessionRecord, text: string): void {
    const at = this.now();
    const n = this.#nextTurn++;
    s.turns.push(
      { id: `${s.id}-t${n}`, stampAt: at, items: [{ type: "user", id: `${s.id}-u${n}`, text, at }] },
      { id: `${s.id}-t${n + 1}`, stampAt: at, completedAt: at, durationMs: 1, items: [{ type: "text", id: `${s.id}-a${n}`, text: "Canned reply from the mock desktop." }] },
    );
    this.#nextTurn += 1;
  }

  #setInstanceState(s: SessionRecord, state: CloudField["state"], rawState: string): void {
    const instance = s.target.instance ? this.instances.get(s.target.instance) : undefined;
    if (!instance) return;
    instance.state = state;
    instance.rawState = rawState;
    if (state !== "stopping") delete instance.stopReport;
  }

  #clearApprovals(s: SessionRecord): void {
    s.approvals = [];
    delete s.waiting;
  }

  // --- Mutations used by the handlers ---

  /**
   * The wake gate of the desktop (send.rs `before_send`): a web request to a cloud session whose instance is
   * `stopped`, `stopping` or `unknown` is refused with `wake_required` unless it carries `wake: true`. Returns
   * true when the request consents to a wake of a stopped instance. A remote machine never needs `wake`.
   */
  #wakeGate(s: SessionRecord, wake: boolean | undefined): boolean {
    if (s.target.kind === "local" || s.target.kind === "remote") return false;
    // A wake-send in flight leaves the cached state as it was on the desktop, so it is gated like a stopped instance.
    const waking = s.pendingSend !== null;
    if (s.cloudState === "starting" && !waking) throw new FixtureError("The instance is still starting; try again when it is ready", "starting");
    if (s.cloudState === "unavailable") throw new FixtureError("This instance cannot be used from the web", "unsupported_target");
    if (s.cloudState === "stopping" && s.cloud?.autoStop) throw new FixtureError("The instance is stopping", "auto_stopping");
    if (waking || s.cloudState === "stopped" || s.cloudState === "stopping" || s.cloudState === "unknown") {
      if (wake !== true) throw new FixtureError("Sending wakes the stopped cloud instance; confirm to continue", "wake_required");
      if (waking) throw new FixtureError("A message is already waiting for the instance to start", "busy");
      return true;
    }
    return false;
  }

  send(conversationId: string, text: string, wake?: boolean): ResultOf<"conversation.send"> {
    const s = this.session(conversationId);
    if (this.#wakeGate(s, wake)) {
      s.pendingSend = text;
      this.#beginStartup(s, 0);
      this.#notify(s);
      return { ok: true, wake: true, queued: true, sessionId: s.id };
    }
    this.#appendExchange(s, text);
    this.#notify(s);
    return { sessionId: s.id };
  }

  /** `session.resumeUsage` and `session.resumeUsageAtReset`: the same gate as a send; `wake: true` starts the instance. */
  resumeUsage(conversationId: string, wake?: boolean): void {
    const s = this.session(conversationId);
    if (this.#wakeGate(s, wake)) {
      this.#beginStartup(s, 0);
      this.#notify(s);
    }
  }

  resume(sessionId: string): { accepted: boolean } {
    const s = this.session(sessionId);
    if (s.cloudState !== "stopped") return { accepted: false };
    this.#beginStartup(s, 0);
    this.#notify(s);
    return { accepted: true };
  }

  retryStart(sessionId: string): { accepted: boolean } {
    return this.resume(sessionId);
  }

  stop(target: { sessionId: string } | { kind: "codespace" | "sandbox"; instance: string }): { stopped: boolean; warning?: string } {
    const instanceName = "sessionId" in target ? this.session(target.sessionId).target.instance : target.instance;
    const instance = instanceName ? this.instances.get(instanceName) : undefined;
    if (!instance) throw new FixtureError("Unknown cloud instance");
    const sessions = [...this.sessions.values()].filter((s) => s.target.instance === instance.instance);
    const busy = sessions.filter((s) => s.busy);
    instance.state = "stopping";
    instance.rawState = "stopping";
    instance.stopReport = "stopping";
    for (const s of sessions) {
      s.cloudState = "stopping";
      s.cloud = { kind: instance.kind, instance: instance.instance, state: "stopping" };
      s.script = [() => this.#finishStop(s, instance)];
      this.#notify(s);
    }
    return { stopped: true, ...(busy.length ? { warning: `Interrupted ${busy.length} busy session(s)` } : {}) };
  }

  #finishStop(s: SessionRecord, instance: CloudInstanceRecord): void {
    instance.state = "stopped";
    instance.rawState = "stopped";
    delete instance.stopReport;
    s.cloudState = "stopped";
    s.cloud = { kind: instance.kind, instance: instance.instance, state: "stopped" };
    s.busy = false;
  }

  keepRunning(sessionId: string): { kept: boolean } {
    const s = this.session(sessionId);
    if (s.cloud) delete s.cloud.autoStop;
    this.#notify(s);
    return { kept: true };
  }

  startMove(sessionId: string, toCloud: boolean): { accepted: boolean } {
    const s = this.session(sessionId);
    const steps = MOVE_STEP_IDS.map((_id, i) => () => {
      if (!s.cloud) return;
      if (i === MOVE_STEP_IDS.length - 1) {
        delete s.cloud.move;
        return;
      }
      s.cloud.move = { step: MOVE_STEP_IDS[i + 1] as (typeof MOVE_STEP_IDS)[number], status: "running" };
    });
    s.cloud = s.cloud ?? { kind: toCloud ? "sandbox" : "local", instance: toCloud ? "fake-sandbox-one" : "", state: "available" };
    s.cloud.move = { step: "snapshot", status: "running" };
    s.script = steps;
    this.#notify(s);
    return { accepted: true };
  }

  cancelMove(sessionId: string): { accepted: boolean } {
    const s = this.session(sessionId);
    if (s.cloud) delete s.cloud.move;
    s.script = [];
    this.#notify(s);
    return { accepted: true };
  }

  answerApproval(conversationId: string, requestId: string): void {
    const s = this.session(conversationId);
    if (!s.approvals.some((a) => a.requestId === requestId)) throw new FixtureError("No such approval");
    s.script = [];
    this.#clearApprovals(s);
    this.#notify(s);
  }

  startSession(params: { projectId: string; text: string; target?: { kind: "codespace" | "sandbox"; instance?: string } | undefined }): { sessionId: string } {
    if (!PROJECTS.some((p) => p.id === params.projectId)) throw new FixtureError(`Unknown project ${params.projectId}`, "not_selected");
    const id = `sess-new-${this.#nextSession++}`;
    const target: Target = params.target ? { kind: params.target.kind, instance: params.target.instance && params.target.instance !== "new" ? params.target.instance : `fake-${params.target.kind}-new` } : { kind: "local" };
    const s = this.#addSession({ id, projectId: params.projectId, title: params.text.slice(0, 40) || "New session", target, busy: true });
    s.turns = [];
    if (params.target) {
      if (!this.instances.has(target.instance as string)) this.#instance(params.target.kind, target.instance as string, `Fake ${params.target.kind} new`, "starting", "Provisioning", 0.1);
      s.cloudState = "starting";
      this.#beginStartup(s, 0);
      s.pendingSend = params.text;
    } else {
      this.#appendExchange(s, params.text);
    }
    return { sessionId: id };
  }

  // --- Catalog ---

  projects(): typeof PROJECTS {
    return PROJECTS;
  }
  monos(): typeof MONOS {
    return MONOS;
  }
  projectIds(): string[] {
    return PROJECTS.map((p) => p.id);
  }

  // --- Desktop app update ---

  /** Sessions an update and restart would interrupt. */
  runningSessions(): { id: string; title: string }[] {
    return [...this.sessions.values()].filter((s) => s.busy).map((s) => ({ id: s.id, title: s.title }));
  }

  updateStatus(): ResultOf<"desktop.updateStatus"> {
    const failed = this.updateFeedError !== undefined;
    const latest = failed ? this.appVersion : this.latestVersion;
    return {
      current: this.appVersion,
      latest,
      updateAvailable: latest !== this.appVersion,
      notes: failed ? "" : this.updateNotes,
      runningSessions: this.runningSessions(),
      ...(failed ? { error: this.updateFeedError } : {}),
      ...(this.lastUpdateFailure !== undefined ? { lastFailure: this.lastUpdateFailure } : {}),
    };
  }

  /** The desktop never downgrades and asks first when sessions run. */
  update(confirm: boolean): ResultOf<"desktop.update"> {
    const status = this.updateStatus();
    if (!status.updateAvailable) return { status: "up_to_date", current: this.appVersion };
    if (status.runningSessions.length > 0 && !confirm) {
      return { status: "confirmation_required", runningSessions: status.runningSessions, message: "Running sessions will be interrupted" };
    }
    this.lastUpdateFailure = undefined;
    this.updatesStarted.push({ from: this.appVersion, to: status.latest });
    if (this.failNextUpdate !== undefined) {
      // The reply below is still `updating`; the failure only shows in the next status read, and the link stays up.
      this.lastUpdateFailure = `Update to ${status.latest} failed: ${this.failNextUpdate}`;
      this.failNextUpdate = undefined;
    }
    return { status: "updating", from: this.appVersion, to: status.latest };
  }

  /** What the relaunch does: the app comes back on the new version. */
  completeUpdate(): void {
    this.appVersion = this.latestVersion;
  }
}

// --- Handlers ---

export interface HandlerContext {
  world: MockWorld;
  device: MockDevice;
}

type ParamsFor<M extends MethodName> = z.output<(typeof METHODS)[M]["params"]>;
export type FixtureHandler<M extends MethodName> = (params: ParamsFor<M>, ctx: HandlerContext) => ResultOf<M>;

function project(world: MockWorld, projectId: string): void {
  if (!world.projectIds().includes(projectId)) throw new FixtureError(`Unknown project ${projectId}`, "not_selected");
}

/** A folder call names a project the device shares, as the desktop checks it. */
function folderProject(ctx: HandlerContext, projectId: string): void {
  project(ctx.world, projectId);
  if (!selected(ctx, projectId)) throw new FixtureError("This project is not shared with this device", "not_selected");
}

/** An opaque, id-free handle for one catalog item (FNV-1a, 2 rounds), like the real desktop's keyed hash. Mock only. */
export function selectionRef(kind: "project" | "mono", id: string): string {
  let out = "";
  for (const seed of [0x811c9dc5, 0x01000193]) {
    let hash = seed;
    for (const char of `${kind}\0${id}`) hash = Math.imul(hash ^ char.charCodeAt(0), 0x01000193) >>> 0;
    out += hash.toString(16).padStart(8, "0");
  }
  return `r${out}`;
}

function selected(ctx: HandlerContext, projectId: string): boolean {
  return ctx.device.selection.projects === null || ctx.device.selection.projects.includes(projectId);
}

/** About 200 KB of fake text, larger than one Noise message, to exercise chunking. */
export const LARGE_FILE_PATH = "docs/large-fixture.txt";
export function largeText(bytes = 200_000): string {
  const line = "Example filler line for the chunking test, not real content.\n";
  return line.repeat(Math.ceil(bytes / line.length)).slice(0, bytes);
}

const ACK = (sessionId?: string) => (sessionId === undefined ? {} : { sessionId });

export const FIXTURE_HANDLERS: { [M in MethodName]: FixtureHandler<M> } = {
  "conversation.read": (p, { world }) => world.read(p.conversationId),
  "conversation.older": (p, { world }) => {
    world.session(p.conversationId);
    return { turns: [], older: false };
  },
  "conversation.watch": (p, { world }) => {
    const frame = world.nextFrame(p.conversationId, world.newCursor());
    if (!frame) throw new FixtureError("No frame");
    return frame;
  },
  "conversation.send": (p, { world }) => world.send(p.conversationId, p.text, p.wake),

  "models.list": () => ({
    harnesses: [
      { id: "claude", label: "Claude" },
      { id: "codex", label: "Codex" },
    ],
    models: [
      { id: "example-model-1", harness: "claude", name: "Example Model One", provider: "example", settings: [] },
      { id: "example-model-2", harness: "codex", name: "Example Model Two", provider: "example", settings: [] },
    ],
    runtimeModes: [
      { id: "supervised", label: "Supervised", hint: "Ask before each action" },
      { id: "auto-accept-edits", label: "Auto-accept edits", hint: "Accept file edits" },
      { id: "auto", label: "Auto", hint: "Run without asking" },
    ],
    accounts: [{ id: "example-account", provider: "claude", label: "Example account", isDefault: true }],
    current: { harness: "claude", model: "example-model-1", modelSettings: {}, runtimeMode: "supervised" },
  }),
  "accounts.usage": () => ({
    accounts: {
      "example-account": {
        status: { tone: "ok", label: "Active" },
        updatedAt: FIXTURE_NOW,
        stale: false,
        meters: [{ title: "Example window", usedPercent: 12, percent: 12, label: "12% used", reset: "in 3 hours" }],
      },
    },
  }),

  "session.configure": (p) => ACK(p.conversationId),
  "session.start": (p, { world }) => world.startSession(p),
  "session.resumeUsage": (p, { world }) => {
    world.resumeUsage(p.conversationId, p.wake);
    return ACK(p.conversationId);
  },
  "session.resumeUsageAtReset": (p, { world }) => {
    world.resumeUsage(p.conversationId, p.wake);
    return ACK(p.conversationId);
  },
  "session.answerQuestion": (p) => ACK(p.conversationId),
  "session.keepQuestionOpen": (p) => ACK(p.conversationId),
  "session.answerApproval": (p, { world }) => {
    world.answerApproval(p.conversationId, p.requestId);
    return ACK(p.conversationId);
  },
  "session.cancel": (p, { world }) => {
    world.session(p.conversationId).busy = false;
    return ACK(p.conversationId);
  },
  "session.rename": (p, { world }) => {
    world.session(p.conversationId).title = p.title;
    return ACK(p.conversationId);
  },
  "session.archive": (p, { world }) => {
    const s = world.session(p.conversationId);
    if (s.target.kind !== "local" && p.cloud === undefined) throw new FixtureError("Choose whether to keep the instance running or stop it");
    return ACK(p.conversationId);
  },
  "session.delete": (p, { world }) => {
    if (world.session(p.conversationId).target.kind !== "local") throw new FixtureError("Delete cloud sessions on the desktop", "unsupported_target");
    return ACK(p.conversationId);
  },

  "folder.create": (p, ctx) => {
    folderProject(ctx, p.projectId);
    return { folderId: ctx.world.createFolder(p.projectId, p.conversationIds, p.name) };
  },
  "folder.rename": (p, ctx) => {
    folderProject(ctx, p.projectId);
    ctx.world.renameFolder(p.projectId, p.folderId, p.name);
    return { folderId: p.folderId };
  },
  "folder.move": (p, ctx) => {
    folderProject(ctx, p.projectId);
    ctx.world.moveToFolder(p.projectId, p.conversationId, p.folderId);
    return { folderId: p.folderId };
  },
  "folder.delete": (p, ctx) => {
    folderProject(ctx, p.projectId);
    ctx.world.deleteFolder(p.projectId, p.folderId);
    return { folderId: p.folderId };
  },

  "project.changes": (p, { world }) => {
    project(world, p.projectId);
    return {
      branch: "example-branch",
      files: [
        {
          relative: "src/example.txt",
          status: "modified",
          additions: 1,
          deletions: 1,
          lines: [
            { kind: "hunk", text: "@@ -1 +1 @@", oldNumber: null, newNumber: null },
            { kind: "del", text: "old example line", oldNumber: 1, newNumber: null },
            { kind: "add", text: "new example line", oldNumber: null, newNumber: 1 },
          ],
        },
      ],
    };
  },
  "project.worktrees": (p, { world }) => {
    project(world, p.projectId);
    return { worktrees: [{ path: "/example/alpha", name: "main" }] };
  },
  "project.files": (p, { world }) => {
    project(world, p.projectId);
    return {
      entries: [
        { name: "src", isDir: true, ignored: false },
        { name: "docs", isDir: true, ignored: false },
        { name: "README.example.md", isDir: false, ignored: false },
      ],
    };
  },
  "project.file": (p, { world }) => {
    project(world, p.projectId);
    return { path: p.path, text: p.path === LARGE_FILE_PATH ? largeText() : "Example file text.\n", truncated: false };
  },
  "chat.mentionedFile": (p, { world }) => {
    // Mono chats (`mono:<id>`) exist as sessions of that id in a test world; the mention check is the fixture table.
    world.session(p.conversationId);
    try {
      return mentionedFile(p);
    } catch (error) {
      if (error instanceof MentionError) throw new FixtureError(error.message, error.code);
      throw error;
    }
  },
  "project.search": (p, { world }) => {
    project(world, p.projectId);
    return { results: [{ path: "src/example.txt", name: "example.txt" }] };
  },
  "project.fileHighlight": () => ({ palette: [], lines: [], done: true }),
  "project.git": (p, { world }) => {
    project(world, p.projectId);
    return { branch: "example-branch", files: 1, remote: true, upstream: true, defaultBranch: "main", ahead: 0, behind: 0, pr: null };
  },
  "project.gitAction": (p, { world }) => {
    project(world, p.projectId);
    return { message: `Example ${p.action} done` };
  },

  "mono.document": (p) => ({ id: p.documentId, title: "Example document", kind: "note" }),
  "mono.profile": (p) => ({ id: p.monoId, name: "Example Mono" }),
  "mono.file": (p) => ({ text: `Example ${p.file} text.`, hash: "example-hash-0001" }),
  "mono.create": () => ({ monoId: "mono-created" }),
  "mono.update": () => ({}),
  "mono.saveFile": () => ({ hash: "example-hash-0002" }),
  "mono.reset": () => ({}),
  "mono.delete": () => ({}),

  "workspace.list": (_p, ctx) => {
    const { world } = ctx;
    const visible = (id: string) => selected(ctx, id);
    return {
      projects: world.projects().filter((p) => visible(p.id)).map((p) => ({ ...p })),
      monos: world.monos().filter((m) => ctx.device.selection.monos === null || ctx.device.selection.monos.includes(m.id)).map((m) => ({ ...m, projectIds: [...m.projectIds] })),
      sessions: world.visibleSessions().filter((s) => visible(s.projectId)).map((s) => world.card(s)),
      folders: world.folders.flatMap((f) => (visible(f.projectId) ? [world.folderCard(f, () => true)] : [])).filter((f) => f !== null),
      monoTools: true,
      monoFiles: true,
      push: true,
    };
  },
  "sessions.list": (p, ctx) => {
    const { world } = ctx;
    const all = world
      .visibleSessions()
      .filter((s) => s.projectId === p.projectId && selected(ctx, s.projectId))
      .filter((s) => !p.query || s.title.toLowerCase().includes(p.query.toLowerCase()))
      .filter((s) => p.since === undefined || s.updatedAt >= p.since);
    const offset = p.offset ?? 0;
    const limit = p.limit ?? 30;
    return { sessions: all.slice(offset, offset + limit).map((s) => world.card(s)), hasMore: offset + limit < all.length };
  },
  "device.leave": () => ({}),
  "device.push": () => ({}),
  "device.enroll": () => ({}),
  "device.selection": (p, { world, device }) => {
    const current = device.selection;
    const projectCatalog = world.projects();
    const monoCatalog = world.monos();
    // Resolve refs over the whole catalog; an unknown ref fails the call, like the real desktop.
    const resolve = (kind: "project" | "mono", refs: string[] | undefined, catalog: readonly { id: string }[]): string[] => {
      const ids = new Map(catalog.map((item) => [selectionRef(kind, item.id), item.id]));
      return (refs ?? []).map((ref) => {
        const id = ids.get(ref);
        if (id === undefined) throw new FixtureError("That item is not on this desktop any more", "not_found");
        return id;
      });
    };
    const addProjects = resolve("project", p.addProjects, projectCatalog);
    const addMonos = resolve("mono", p.addMonos, monoCatalog);
    const merge = (base: string[] | null | undefined, now: string[] | null, add: string[]): string[] | null => {
      // Like the real desktop: an explicit id list only narrows (it is intersected with the current selection).
      const start = base === undefined ? now : base === null || now === null ? base : base.filter((id) => now.includes(id));
      return start === null ? null : [...new Set([...start, ...add])];
    };
    const nextProjects = merge(p.projects, current.projects, addProjects);
    const nextMonos = merge(p.monos, current.monos, addMonos);
    const widens = (next: string[] | null, now: string[] | null): boolean => now !== null && (next === null || next.some((id) => !now.includes(id)));
    if (widens(nextProjects, current.projects) || widens(nextMonos, current.monos)) {
      // A real desktop checks the ticket with the Worker; the mock cannot, so any non-empty ticket passes.
      if (device.permission === "read") throw new FixtureError("This device can only read", "read_only");
      if (p.ticket === undefined || p.ticket === "") throw new FixtureError("Confirm with your passkey to add projects", "fresh_required");
    }
    current.projects = nextProjects;
    current.monos = nextMonos;
    const entry = (kind: "project" | "mono", item: { id: string; name: string }, list: string[] | null) => {
      const shared = list === null || list.includes(item.id);
      return { ref: selectionRef(kind, item.id), name: item.name, shared, ...(shared ? { id: item.id } : {}) };
    };
    return {
      projects: current.projects,
      monos: current.monos,
      available: {
        projects: projectCatalog.map((item) => entry("project", item, current.projects)),
        monos: monoCatalog.map((item) => entry("mono", item, current.monos)),
      },
      canWiden: device.permission === "control",
    };
  },

  "cloud.instances": (_p, { world }) => ({
    instances: [...world.instances.values()].map((i) => ({
      kind: i.kind,
      instance: i.instance,
      displayName: i.displayName,
      state: i.state,
      rawState: i.rawState,
      lastUsedAt: i.lastUsedAt,
      sessions: [...world.sessions.values()]
        .filter((s) => s.target.instance === i.instance)
        .map((s) => ({ sessionId: s.id, title: s.title, busy: s.busy })),
      costEstimatePerHour: i.costEstimatePerHour,
      ...(i.stopReport ? { stopReport: i.stopReport } : {}),
    })),
    backends: [
      { kind: "codespace", ready: true },
      { kind: "sandbox", ready: true },
    ],
    otherCount: 2,
  }),
  "cloud.startOptions": (p, { world }) => {
    project(world, p.projectId);
    return {
      backends: [
        { kind: "codespace", ready: true },
        { kind: "sandbox", ready: true },
      ],
      branches: ["main", "example-branch"],
      machines: [{ id: "example-machine-small", name: "Example 2-core" }],
      sizes: [
        { id: "small", name: "Small" },
        { id: "large", name: "Large" },
      ],
    };
  },
  "cloud.stop": (p, { world }) => world.stop(p),
  "cloud.resume": (p, { world }) => world.resume(p.sessionId),
  "cloud.retryStart": (p, { world }) => world.retryStart(p.sessionId),
  "cloud.keepRunning": (p, { world }) => world.keepRunning(p.sessionId),
  "cloud.moveToCloud": (p, { world }) => world.startMove(p.sessionId, true),
  "cloud.moveToLocal": (p, { world }) => world.startMove(p.sessionId, false),
  "cloud.cancelMove": (p, { world }) => world.cancelMove(p.sessionId),
  "desktop.updateStatus": (_p, { world }) => world.updateStatus(),
  "desktop.update": (p, { world }) => world.update(p.confirm === true),
};
