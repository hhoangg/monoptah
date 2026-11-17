// The lifecycle of one desktop link in this tab (architecture.md 3.2, 3.3, 3.6, 4.5): directory and
// key pin, relay socket, Noise session, `state`, close-code driven reconnects with backoff, visibility
// and online handling, the registry of watches that are reopened after a reconnect, and the
// `resynced` event for the data layer. One `LinkConnection` per desktop id and tab (`getLinkConnection`).
//
// Rules kept here: a request never queues and is never re-sent after a reconnect (it rejects with
// `not_ready` or `session_ended`); every reconnect is a fresh Noise session; the device private key is
// read at handshake time only and never stored; the store gets public information only.
import {
  CloseCodes,
  DEAD_AFTER_MS,
  DesktopDirectoryResponse,
  PROTOCOL_VERSION,
  closeReaction,
  type CloseAction,
  type MethodName,
  type ParamsOf,
  type ResultOf,
  type StateMessage,
  type WatchFrame,
} from "@monoptah/protocol";
import { randomBytes } from "@noble/hashes/utils.js";
import { base64UrlDecode } from "./base64url.ts";
import { DeviceRegistrationError, deviceNameFromUserAgent, ensureDevice, type DeviceIdentity } from "./device.ts";
import { fingerprint } from "./fingerprint.ts";
import { getDeviceKeyPair } from "./keys.ts";
import type { KeyPair } from "./noise.ts";
import { checkPin } from "./pins.ts";
import { RelayConnectError, openRelay, type RelayConnection } from "./relay.ts";
import {
  LinkRequestError,
  LinkSession,
  LinkSessionError,
  type LinkEndDetail,
  type LinkEndReason,
  type LinkSessionOptions,
  type RequestOptions,
  type WatchHandle,
} from "./session.ts";
import { getLinkStore, initialLinkState, type LinkBlockedReason, type LinkErrorInfo, type LinkState, type LinkStatus } from "./store.ts";
import type { StoreApi } from "zustand/vanilla";

export const BACKOFF_BASE_MS = 500;
export const BACKOFF_MAX_MS = 30_000;
/** `ready` this long resets the backoff exponent (3.3). */
export const BACKOFF_RESET_AFTER_READY_MS = 10_000;
/** Wait for the close codes whose reaction is `backoff_60s`. */
export const BACKOFF_LONG_MS = 60_000;

/** `min(30 s, 0.5 s * 2^n) * random(0.5..1)`. */
export function backoffDelay(attempt: number, random: () => number = Math.random): number {
  return Math.min(BACKOFF_MAX_MS, BACKOFF_BASE_MS * 2 ** attempt) * (0.5 + random() * 0.5);
}

/** What the device does after the relay socket closed with `code`, derived from `CLOSE_CODES`. */
export type ClosePlan =
  | { kind: "retry"; delay: "backoff" | "once" | "long" }
  | { kind: "block"; reason: LinkBlockedReason; revoked: boolean };

export function planForClose(code: number): ClosePlan {
  const action: CloseAction = closeReaction(code, "device").action;
  switch (action) {
    case "reconnect_backoff":
    case "reconnect_new_handshake":
      return { kind: "retry", delay: "backoff" };
    case "reconnect_once":
    case "retry_once":
      return { kind: "retry", delay: "once" };
    case "backoff_60s":
      return { kind: "retry", delay: "long" };
    case "login":
      return { kind: "block", reason: "login_required", revoked: false };
    case "stop":
      if (code === CloseCodes.REVOKED) return { kind: "block", reason: "revoked", revoked: true };
      if (code === CloseCodes.BLOCKED) return { kind: "block", reason: "account_blocked", revoked: false };
      return { kind: "block", reason: "policy", revoked: false };
  }
}

/** The Noise channel of this tab: a random u16 chosen once. */
export const TAB_CHANNEL = (() => {
  const bytes = randomBytes(2);
  return ((bytes[0] ?? 0) << 8) | (bytes[1] ?? 0);
})();

export interface LinkConnectionDeps {
  fetch?: typeof fetch;
  openRelay?: (desktopId: string, deviceId: string) => Promise<RelayConnection>;
  ensureDevice?: () => Promise<DeviceIdentity>;
  getDeviceKeyPair?: () => Promise<KeyPair>;
  checkPin?: typeof checkPin;
  connectSession?: (options: LinkSessionOptions) => Promise<LinkSession>;
  now?: () => number;
  random?: () => number;
  channel?: number;
  /** Silence after which a visible tab reconnects (default `DEAD_AFTER_MS`, 75 s). */
  staleAfterMs?: number;
  /** Passed to every session (tests shorten the handshake timeout). */
  sessionOptions?: Partial<LinkSessionOptions>;
  store?: StoreApi<LinkState>;
}

export interface WatchRegistration {
  method: string;
  /** Params for every open of the watch, the first included (W-05 returns `from` here). */
  reopenParams: () => Record<string, unknown>;
  onFrame: (frame: WatchFrame) => void;
  /** The desktop ended the watch with an error, or opening it failed. Not reopened until the next reconnect. */
  onError?: (error: LinkRequestError) => void;
  onDead?: () => void;
}

export interface WatchRegistrationHandle {
  readonly id: number;
  /** Stops the watch and removes the registration. */
  unregister(): void;
}

interface Directory {
  publicKey: Uint8Array;
  fingerprint: string;
  name: string;
  online: boolean;
}

class DirectoryError extends Error {
  readonly status: number | undefined;
  constructor(message: string, status?: number) {
    super(message);
    this.name = "DirectoryError";
    this.status = status;
  }
}

interface WatchSlot {
  registration: WatchRegistration;
  handle: WatchHandle | null;
  opening: boolean;
}

export class LinkConnection {
  readonly desktopId: string;
  readonly store: StoreApi<LinkState>;

  readonly #deps: Required<Pick<LinkConnectionDeps, "now" | "random" | "channel" | "staleAfterMs">> & LinkConnectionDeps;
  #refs = 0;
  #running = false;
  /** A block or a terminal directory error: no more attempts until `reconnect()`. */
  #stopped = false;
  /** Invalidates the callbacks of an older attempt or session. */
  #gen = 0;
  #session: LinkSession | null = null;
  #latestState: StateMessage | null = null;
  #timer: ReturnType<typeof setTimeout> | null = null;
  #readyTimer: ReturnType<typeof setTimeout> | null = null;
  #attempting = false;
  #attemptToken = 0;
  #longWait = false;
  /** An attempt is due but the tab is hidden or offline. */
  #waiting = false;
  #failures = 0;
  #quickUsed = false;
  #retrying = false;
  #sessionCount = 0;
  #needsResync = false;
  #nextWatchId = 1;
  readonly #watches = new Map<number, WatchSlot>();
  readonly #resyncListeners = new Set<() => void>();

  constructor(desktopId: string, deps: LinkConnectionDeps = {}) {
    this.desktopId = desktopId;
    this.store = deps.store ?? getLinkStore(desktopId);
    this.#deps = {
      ...deps,
      now: deps.now ?? Date.now,
      random: deps.random ?? Math.random,
      channel: deps.channel ?? TAB_CHANNEL,
      staleAfterMs: deps.staleAfterMs ?? DEAD_AFTER_MS,
    };
  }

  // --- Public API ---

  getState(): LinkState {
    return this.store.getState();
  }

  get status(): LinkStatus {
    return this.store.getState().status;
  }

  /** `lastFrameAt` of the live session (epoch ms) or null. */
  get lastFrameAt(): number | null {
    return this.#session?.lastFrameAt ?? null;
  }

  /** Counts a user of the connection; the first one starts it. */
  retain(): void {
    this.#refs += 1;
    if (this.#refs === 1) this.start();
  }

  /** The last release stops the connection (the registry keeps the object). */
  release(): void {
    if (this.#refs === 0) return;
    this.#refs -= 1;
    if (this.#refs === 0) this.stop();
  }

  /**
   * Sends a request. Rejects with `LinkRequestError` code `not_ready` unless the status is `ready`
   * (no queueing: a stale prompt must not run later), and with `session_ended` when the session ends
   * first. A request is never re-sent after a reconnect.
   */
  request<M extends MethodName>(method: M, params?: ParamsOf<M>, options?: RequestOptions): Promise<ResultOf<M>>;
  request(method: string, params?: unknown, options?: RequestOptions): Promise<unknown>;
  request(method: string, params: unknown = {}, options: RequestOptions = {}): Promise<unknown> {
    const session = this.#session;
    if (!session || this.status !== "ready" || !session.ready) {
      return Promise.reject(new LinkRequestError("The desktop link is not ready", "not_ready"));
    }
    return session.request(method, params as Record<string, unknown>, options);
  }

  /** Registers a watch: opened now when `ready`, and reopened after every reconnect. */
  registerWatch(registration: WatchRegistration): WatchRegistrationHandle {
    const id = this.#nextWatchId++;
    this.#watches.set(id, { registration, handle: null, opening: false });
    this.#syncWatchInfo();
    if (this.status === "ready") this.#openWatch(id);
    return {
      id,
      unregister: () => {
        const slot = this.#watches.get(id);
        if (!slot) return;
        this.#watches.delete(id);
        slot.handle?.stop();
        this.#syncWatchInfo();
      },
    };
  }

  /** Called after a reconnect once the new session is ready and the watches are reopened. */
  onResynced(listener: () => void): () => void {
    this.#resyncListeners.add(listener);
    return () => {
      this.#resyncListeners.delete(listener);
    };
  }

  /** Manual reconnect ("Reconnect" in settings, or after login or a re-pin). Clears a block. */
  reconnect(): void {
    if (!this.#running) return;
    this.#stopped = false;
    this.#failures = 0;
    this.#quickUsed = false;
    this.store.setState({ blocked: null, revoked: false, lastError: null });
    this.#reconnectNow();
  }

  start(): void {
    if (this.#running) return;
    this.#running = true;
    this.#stopped = false;
    this.#failures = 0;
    this.#quickUsed = false;
    this.#retrying = false;
    this.#sessionCount = 0;
    this.#needsResync = false;
    this.store.setState({ ...initialLinkState(), status: "connecting", watches: this.#watchInfo() }, true);
    globalThis.document?.addEventListener("visibilitychange", this.#onVisibility);
    globalThis.addEventListener?.("online", this.#onOnline);
    void this.#attempt();
  }

  stop(): void {
    if (!this.#running) return;
    this.#running = false;
    globalThis.document?.removeEventListener("visibilitychange", this.#onVisibility);
    globalThis.removeEventListener?.("online", this.#onOnline);
    this.#clearTimers();
    this.#teardown();
    this.#waiting = false;
    this.store.setState({ status: "idle", nextAttemptAt: null });
  }

  // --- Attempts ---

  #isStale(gen: number): boolean {
    return gen !== this.#gen || !this.#running;
  }

  #gateOpen(): boolean {
    if (globalThis.document?.hidden === true) return false;
    if (globalThis.navigator?.onLine === false) return false;
    return true;
  }

  async #attempt(): Promise<void> {
    if (!this.#running || this.#stopped || this.#attempting) return;
    this.#clearTimer();
    this.#longWait = false;
    if (!this.#gateOpen()) {
      this.#waiting = true;
      return;
    }
    this.#waiting = false;
    this.#attempting = true;
    const gen = ++this.#gen;
    const token = ++this.#attemptToken;
    this.store.setState({ status: this.#retrying ? "reconnecting" : "connecting", nextAttemptAt: null });
    try {
      await this.#run(gen);
    } finally {
      if (token === this.#attemptToken) this.#attempting = false;
    }
  }

  async #run(gen: number): Promise<void> {
    let device: DeviceIdentity;
    try {
      device = await (this.#deps.ensureDevice ?? ensureDevice)();
    } catch (error) {
      if (this.#isStale(gen)) return;
      if (error instanceof DeviceRegistrationError && error.status === 401) return this.#block("login_required");
      return this.#fail({ code: "device_registration" }, { kind: "retry", delay: "backoff" });
    }
    if (this.#isStale(gen)) return;
    this.store.setState({ deviceFingerprint: device.fingerprint });

    let directory: Directory;
    try {
      directory = await this.#fetchDirectory();
    } catch (error) {
      if (this.#isStale(gen)) return;
      const status = error instanceof DirectoryError ? error.status : undefined;
      if (status === 401) return this.#block("login_required");
      if (status === 403 || status === 404) return this.#terminal({ code: `directory_${status}` });
      return this.#fail({ code: "directory_error" }, { kind: "retry", delay: "backoff" });
    }
    if (this.#isStale(gen)) return;

    let pin: Awaited<ReturnType<typeof checkPin>>;
    try {
      pin = await (this.#deps.checkPin ?? checkPin)(this.desktopId, directory.publicKey);
    } catch {
      return this.#fail({ code: "pin_storage" }, { kind: "retry", delay: "backoff" });
    }
    if (this.#isStale(gen)) return;
    this.store.setState({ desktopFingerprint: directory.fingerprint, desktopName: directory.name, presence: directory.online ? "online" : "offline" });
    // A changed key never reaches a socket.
    if (pin === "changed") return this.#block("key_changed");

    let relay: RelayConnection;
    try {
      relay = await (this.#deps.openRelay ?? ((desktopId, deviceId) => openRelay({ desktopId, deviceId })))(this.desktopId, device.deviceId);
    } catch (error) {
      if (this.#isStale(gen)) return;
      const closeCode = error instanceof RelayConnectError ? error.closeCode : undefined;
      return this.#failFromClose(closeCode);
    }
    if (this.#isStale(gen)) {
      relay.close();
      return;
    }

    this.store.setState({ status: this.#retrying ? "reconnecting" : "handshaking" });
    let session: LinkSession;
    try {
      // The key is used for the handshake only and is not kept anywhere.
      const deviceKey = await (this.#deps.getDeviceKeyPair ?? getDeviceKeyPair)();
      session = await (this.#deps.connectSession ?? LinkSession.connect)({
        relay,
        desktopId: this.desktopId,
        deviceId: device.deviceId,
        channel: this.#deps.channel,
        desktopPublicKey: directory.publicKey,
        deviceKey,
        deviceName: deviceNameFromUserAgent(),
        now: this.#deps.now,
        ...this.#deps.sessionOptions,
        onState: (state) => this.#onState(gen, state),
        onEnd: (reason, detail) => this.#onEnd(gen, reason, detail),
        onPresence: (presence) => {
          if (gen === this.#gen) this.store.setState({ presence });
        },
        onRelayError: (code) => {
          if (gen === this.#gen && code === "desktop_offline") this.store.setState({ presence: "offline" });
        },
      });
    } catch (error) {
      relay.close();
      if (this.#isStale(gen)) return;
      if (error instanceof LinkSessionError) return this.#failFromSession(error.reason, error.detail, false);
      return this.#fail({ code: "handshake_failed" }, { kind: "retry", delay: "backoff" });
    }
    if (this.#isStale(gen)) {
      session.close();
      return;
    }
    this.#session = session;
    this.#sessionCount += 1;
    if (this.#sessionCount > 1) this.#needsResync = true;
    this.#reactToState();
  }

  async #fetchDirectory(): Promise<Directory> {
    const doFetch = this.#deps.fetch ?? globalThis.fetch.bind(globalThis);
    let response: Response;
    try {
      response = await doFetch(`/api/desktops/${encodeURIComponent(this.desktopId)}`, {
        credentials: "same-origin",
        headers: { Accept: "application/json" },
      });
    } catch {
      throw new DirectoryError("directory request failed");
    }
    if (!response.ok) throw new DirectoryError(`directory request failed (${response.status})`, response.status);
    const parsed = DesktopDirectoryResponse.safeParse(await response.json().catch(() => null));
    if (!parsed.success) throw new DirectoryError("directory response is invalid");
    let publicKey: Uint8Array;
    try {
      publicKey = base64UrlDecode(parsed.data.publicKey);
    } catch {
      throw new DirectoryError("directory key is invalid");
    }
    if (publicKey.length !== 32) throw new DirectoryError("directory key has the wrong length");
    return { publicKey, fingerprint: fingerprint(publicKey), name: parsed.data.name, online: parsed.data.online };
  }

  // --- Session callbacks ---

  #onState(gen: number, state: StateMessage): void {
    if (gen !== this.#gen) return;
    this.#latestState = state;
    // Before `connect` resolved the session object is not set yet: `#run` reacts after it is.
    if (this.#session) this.#reactToState();
  }

  #reactToState(): void {
    const state = this.#latestState;
    const session = this.#session;
    if (!state || !session) return;
    this.store.setState({
      permission: state.permission ?? this.store.getState().permission,
      capabilities: state.capabilities ?? this.store.getState().capabilities,
      desktopName: state.desktop?.name ?? this.store.getState().desktopName,
      desktopVersion: state.desktop?.version ?? this.store.getState().desktopVersion,
      proto: state.proto ?? this.store.getState().proto,
      lastFrameAt: session.lastFrameAt,
    });
    if (state.proto !== undefined && state.proto > PROTOCOL_VERSION) return this.#block("proto_desktop_newer");
    if (state.state === "revoked") return this.#block("revoked", { revoked: true });
    if (state.state === "pending") {
      // A pending state is shown as it comes; it is never cached as confirmed.
      this.#clearReadyTimer();
      this.#dropWatchHandles();
      this.store.setState({ status: "pending", lastError: null });
      return;
    }
    if (this.status === "ready") return;
    this.store.setState({ status: "ready", lastError: null });
    this.#retrying = false;
    this.#clearReadyTimer();
    this.#readyTimer = setTimeout(() => {
      this.#readyTimer = null;
      this.#failures = 0;
      this.#quickUsed = false;
    }, BACKOFF_RESET_AFTER_READY_MS);
    for (const id of this.#watches.keys()) this.#openWatch(id);
    if (this.#needsResync) {
      this.#needsResync = false;
      for (const listener of [...this.#resyncListeners]) listener();
    }
  }

  #onEnd(gen: number, reason: LinkEndReason, detail: LinkEndDetail): void {
    if (gen !== this.#gen) return;
    this.#clearReadyTimer();
    this.#dropWatchHandles();
    this.#session = null;
    this.#latestState = null;
    this.#retrying = true;
    this.store.setState({ status: "reconnecting" });
    this.#failFromSession(reason, detail, true);
  }

  // --- Failure policy ---

  #failFromSession(reason: LinkEndReason, detail: LinkEndDetail, afterReady = false): void {
    switch (reason) {
      case "bye":
        // BYE before any state: the desktop refuses this device (3.3, 6.3 "Revoked: BYE immediately").
        // After ready it is a desktop restart: reconnect, and the next BYE-before-state blocks.
        if (afterReady) return this.#fail({ code: "bye" }, { kind: "retry", delay: "backoff" });
        return this.#block("revoked", { revoked: true });
      case "unsupported_proto":
        return this.#block("proto_desktop_newer");
      case "relay_error":
        if (detail.relayCode === "unsupported_version") return this.#block("proto_desktop_older");
        if (detail.relayCode === "desktop_offline") this.store.setState({ presence: "offline" });
        return this.#fail({ code: detail.relayCode ?? "relay_error" }, { kind: "retry", delay: "backoff" });
      case "relay_closed":
        return this.#failFromClose(detail.closeCode);
      case "closed":
        return;
      default:
        return this.#fail({ code: reason }, { kind: "retry", delay: "backoff" });
    }
  }

  #failFromClose(closeCode: number | undefined): void {
    const error: LinkErrorInfo = closeCode === undefined ? { code: "relay_closed" } : { code: `close_${closeCode}`, closeCode };
    if (closeCode === undefined) return this.#fail(error, { kind: "retry", delay: "backoff" });
    const plan = planForClose(closeCode);
    if (plan.kind === "block") return this.#block(plan.reason, { revoked: plan.revoked, error });
    this.#fail(error, plan);
  }

  #fail(error: LinkErrorInfo, plan: Extract<ClosePlan, { kind: "retry" }>): void {
    if (!this.#running || this.#stopped) return;
    this.#teardown(false);
    this.#retrying = true;
    let delay: number;
    if (plan.delay === "long") delay = BACKOFF_LONG_MS;
    else if (plan.delay === "once" && !this.#quickUsed) {
      this.#quickUsed = true;
      delay = 0;
    } else delay = backoffDelay(this.#failures++, this.#deps.random);
    this.store.setState({ status: "reconnecting", lastError: error, nextAttemptAt: this.#deps.now() + delay });
    this.#clearTimer();
    this.#longWait = plan.delay === "long";
    this.#timer = setTimeout(() => {
      this.#timer = null;
      void this.#attempt();
    }, delay);
  }

  #block(reason: LinkBlockedReason, extra: { revoked?: boolean; error?: LinkErrorInfo } = {}): void {
    this.#stopped = true;
    this.#clearTimers();
    this.#teardown();
    this.store.setState({
      status: "closed",
      blocked: reason,
      nextAttemptAt: null,
      ...(extra.revoked ? { revoked: true } : {}),
      ...(extra.error ? { lastError: extra.error } : {}),
    });
  }

  /** No retry, no block screen: the directory says the desktop is not reachable for this account. */
  #terminal(error: LinkErrorInfo): void {
    this.#stopped = true;
    this.#clearTimers();
    this.#teardown();
    this.store.setState({ status: "closed", lastError: error, nextAttemptAt: null });
  }

  // --- Reconnect triggers ---

  readonly #onVisibility = (): void => {
    if (!this.#running || this.#stopped || globalThis.document?.hidden === true) return;
    if (this.#waiting || (this.#timer !== null && !this.#longWait)) {
      this.#clearTimer();
      void this.#attempt();
      return;
    }
    if (this.#attempting) return;
    if (this.#isDead()) this.#reconnectNow();
  };

  readonly #onOnline = (): void => {
    if (!this.#running || this.#stopped) return;
    if (this.#waiting) void this.#attempt();
  };

  #isDead(): boolean {
    const session = this.#session;
    if (!session || session.ended) return true;
    return this.#deps.now() - session.lastFrameAt > this.#deps.staleAfterMs;
  }

  /** Drops the current session and connects again at once (the gates still apply). */
  #reconnectNow(): void {
    this.#clearTimer();
    this.#teardown();
    this.#retrying = true;
    this.#attempting = false;
    void this.#attempt();
  }

  // --- Watches ---

  #watchInfo(): { id: number; method: string }[] {
    return [...this.#watches].map(([id, slot]) => ({ id, method: slot.registration.method }));
  }

  #syncWatchInfo(): void {
    this.store.setState({ watches: this.#watchInfo() });
  }

  #openWatch(id: number): void {
    const slot = this.#watches.get(id);
    const session = this.#session;
    if (!slot || !session || slot.handle || slot.opening) return;
    const registration = slot.registration;
    slot.opening = true;
    const gen = this.#gen;
    const params = registration.reopenParams();
    session
      .watch(registration.method, params, registration.onFrame, {
        onDead: () => this.#onWatchDead(gen, id),
        onError: (error) => {
          if (gen !== this.#gen) return;
          const current = this.#watches.get(id);
          if (current) current.handle = null;
          registration.onError?.(error);
        },
      })
      .then((handle) => {
        slot.opening = false;
        if (gen !== this.#gen || this.#watches.get(id) !== slot) {
          handle.stop();
          return;
        }
        slot.handle = handle;
      })
      .catch((error: unknown) => {
        slot.opening = false;
        if (gen !== this.#gen) return;
        registration.onError?.(error instanceof LinkRequestError ? error : new LinkRequestError("The watch could not be opened", "session_ended"));
      });
  }

  #onWatchDead(gen: number, id: number): void {
    if (gen !== this.#gen) return;
    const slot = this.#watches.get(id);
    if (slot) slot.handle = null;
    slot?.registration.onDead?.();
    if (this.#isDead()) {
      // Same check as a tab becoming visible.
      this.#reconnectNow();
      return;
    }
    this.#openWatch(id);
  }

  #dropWatchHandles(): void {
    for (const slot of this.#watches.values()) {
      slot.handle = null;
      slot.opening = false;
    }
  }

  // --- Helpers ---

  /** Ends the live session without reporting it. `bump` invalidates in-flight callbacks. */
  #teardown(bump = true): void {
    if (bump) this.#gen += 1;
    this.#clearReadyTimer();
    this.#dropWatchHandles();
    const session = this.#session;
    this.#session = null;
    this.#latestState = null;
    session?.close();
  }

  #clearTimer(): void {
    if (this.#timer !== null) clearTimeout(this.#timer);
    this.#timer = null;
  }

  #clearReadyTimer(): void {
    if (this.#readyTimer !== null) clearTimeout(this.#readyTimer);
    this.#readyTimer = null;
  }

  #clearTimers(): void {
    this.#clearTimer();
    this.#clearReadyTimer();
  }
}

// --- Registry: one connection per desktop id in this tab ---

const connections = new Map<string, LinkConnection>();

/** The connection of a desktop (created, not started, on first use). `deps` only apply at creation. */
export function getLinkConnection(desktopId: string, deps?: LinkConnectionDeps): LinkConnection {
  let connection = connections.get(desktopId);
  if (!connection) {
    connection = new LinkConnection(desktopId, deps);
    connections.set(desktopId, connection);
  }
  return connection;
}

/** Stops and forgets every connection. For tests and sign-out. */
export function resetLinkConnections(): void {
  for (const connection of connections.values()) connection.stop();
  connections.clear();
}
