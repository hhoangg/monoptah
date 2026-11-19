// The lifecycle of one desktop link in this tab (architecture.md 3.2, 3.3, 3.6, 4.5): directory and
// key pin, relay socket, Noise session, `state`, close-code driven reconnects with backoff, visibility
// and online handling, the registry of watches that are reopened after a reconnect, and the
// `resynced` event for the data layer. One `LinkConnection` per desktop id and tab (`getLinkConnection`).
//
// Rules kept here: a request never queues and is never re-sent after a reconnect (it rejects with
// `not_ready` or `session_ended`); every reconnect is a fresh Noise session; the device private key is
// copied for the handshake and zeroed right after, never stored; the store gets public information only.
// A BYE or relay ERROR before any `state` is unauthenticated: it blocks with `desktop_refused`, it never
// sets `revoked` (which also wipes the persisted cache). Only an encrypted `state: "revoked"` and the
// close code 4003 do.
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
import { DeviceKeyError, getDeviceKeyPair } from "./keys.ts";
import type { KeyPair } from "./noise.ts";
import { commitPin, inspectPin, type PinCheck, type PinInspection } from "./pins.ts";
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
/** `store.lastFrameAt` is refreshed at most this often (`connection.lastFrameAt` is exact). */
export const FRAME_STORE_THROTTLE_MS = 5_000;

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

/**
 * The pre-connect grant hook: runs before every (re)connect of the relay, once the device is registered and before the
 * relay is joined. `"denied"` means the server refuses this device for this desktop (a revoked grant, a forgotten
 * device): the link blocks as `revoked` and does not retry. `"ok"` (or nothing) goes on. A throw is a failed check
 * (network): the link retries with backoff. The relay stays the authority: a hook that cannot decide returns `"ok"`.
 */
export type GrantCheck = "ok" | "denied";
export type PreConnectHook = (context: { desktopId: string; deviceId: string }) => Promise<GrantCheck | void>;

let preConnectHook: PreConnectHook | null = null;

/** Installs the pre-connect hook of every connection in this tab; returns a function that restores the previous one. */
export function setPreConnectHook(hook: PreConnectHook | null): () => void {
  const previous = preConnectHook;
  preConnectHook = hook;
  return () => {
    preConnectHook = previous;
  };
}

/** The error text of a watch the desktop ended after it was acknowledged (`web_link/pacer.rs` `WATCH_ENDED`). */
export const WATCH_ENDED_MESSAGE = "Watch ended";
/** A watch that ends again and again without delivering a frame in between is reported, not reopened forever. */
export const MAX_WATCH_ENDED_REOPENS = 3;

export interface LinkConnectionDeps {
  /** Replaces the module's pre-connect hook (tests). */
  preConnect?: PreConnectHook;
  fetch?: typeof fetch;
  openRelay?: (desktopId: string, deviceId: string) => Promise<RelayConnection>;
  ensureDevice?: () => Promise<DeviceIdentity>;
  getDeviceKeyPair?: () => Promise<KeyPair>;
  /** Replaces the whole pin step (tests). It must pin by itself: the connection does not commit after it. */
  checkPin?: (desktopId: string, directoryPublicKey: Uint8Array) => Promise<PinCheck>;
  /** Reads the pin without writing (default `inspectPin`). */
  inspectPin?: typeof inspectPin;
  /** Pins a first-use key after the handshake with it succeeded (default `commitPin`). */
  commitPin?: typeof commitPin;
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
  /** Bumped on every open and every drop: an ack of an older open is stopped, not kept. */
  token: number;
  /** Reopens after "Watch ended" since the last frame. */
  endedReopens: number;
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
  /** The name the server's directory serves: it follows a rename, the name in a state frame does not. */
  #directoryName: string | null = null;
  #timer: ReturnType<typeof setTimeout> | null = null;
  #readyTimer: ReturnType<typeof setTimeout> | null = null;
  #attempting = false;
  #attemptToken = 0;
  #longWait = false;
  /** An attempt is due but the tab is hidden or offline. */
  #waiting = false;
  #failures = 0;
  #quickUsed = false;
  /** One automatic retry of `desktop_refused` on the next visible transition. */
  #refusedRetried = false;
  #lastStoredFrameAt = 0;
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
    this.#watches.set(id, { registration, handle: null, opening: false, token: 0, endedReopens: 0 });
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
    this.#refusedRetried = false;
    this.store.setState({ blocked: null, revoked: false, lastError: null });
    this.#reconnectNow();
  }

  start(): void {
    if (this.#running) return;
    this.#running = true;
    this.#stopped = false;
    this.#failures = 0;
    this.#quickUsed = false;
    this.#refusedRetried = false;
    this.#lastStoredFrameAt = 0;
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
    // An attempt that is still awaiting must not block the next `start()`: its callbacks are stale (the
    // generation moved) and its `finally` must not clear the flag of the new attempt (the token moved).
    this.#attempting = false;
    this.#attemptToken += 1;
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
      if (error instanceof DeviceRegistrationError) {
        if (error.status === 401) return this.#block("login_required");
        // A 4xx such as quota_exceeded, or an answer that cannot be used: retrying cannot help.
        if (error.terminal) return this.#terminal({ code: "device_registration_rejected" });
      }
      if (error instanceof DeviceKeyError && error.code === "unreadable") return this.#terminal({ code: "device_key_unreadable" });
      return this.#fail({ code: "device_registration" }, { kind: "retry", delay: "backoff" });
    }
    if (this.#isStale(gen)) return;
    this.store.setState({ deviceFingerprint: device.fingerprint });

    // Ensure the grant before the relay is joined: a revoked grant blocks here, a failed check retries.
    const hook = this.#deps.preConnect ?? preConnectHook;
    if (hook) {
      let check: GrantCheck | void;
      try {
        check = await hook({ desktopId: this.desktopId, deviceId: device.deviceId });
      } catch {
        if (this.#isStale(gen)) return;
        return this.#fail({ code: "grant_check" }, { kind: "retry", delay: "backoff" });
      }
      if (this.#isStale(gen)) return;
      if (check === "denied") return this.#block("revoked", { revoked: true, error: { code: "grant_denied" } });
    }

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

    let pin: PinInspection;
    try {
      pin = await this.#inspectPin(directory);
    } catch {
      if (this.#isStale(gen)) return;
      return this.#fail({ code: "pin_storage" }, { kind: "retry", delay: "backoff" });
    }
    if (this.#isStale(gen)) return;
    // The fingerprint shown as "this desktop" is the PINNED key's. A first-use key is shown once it is pinned.
    this.#directoryName = directory.name;
    this.store.setState({
      desktopName: directory.name,
      presence: directory.online ? "online" : "offline",
      ...(pin.pinnedFingerprint === null ? {} : { desktopFingerprint: pin.pinnedFingerprint }),
    });
    // A changed key never reaches a socket.
    if (pin.status === "changed") return this.#block("key_changed");

    let relay: RelayConnection;
    try {
      const open = this.#deps.openRelay ?? ((desktopId, deviceId) => openRelay({ desktopId, deviceId, openTimeoutMs: this.#deps.sessionOptions?.readyTimeoutMs }));
      relay = await open(this.desktopId, device.deviceId);
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
    let deviceKey: KeyPair | null = null;
    try {
      // A copy for the handshake only: the session zeroes it, and so does the `finally` below.
      const stored = await (this.#deps.getDeviceKeyPair ?? getDeviceKeyPair)();
      deviceKey = { privateKey: stored.privateKey.slice(), publicKey: stored.publicKey.slice() };
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
        onActivity: () => this.#noteFrame(gen),
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
      if (error instanceof DeviceKeyError && error.code === "unreadable") return this.#terminal({ code: "device_key_unreadable" });
      if (error instanceof LinkSessionError) return this.#failFromSession(error.reason, error.detail, false);
      return this.#fail({ code: "handshake_failed" }, { kind: "retry", delay: "backoff" });
    } finally {
      deviceKey?.privateKey.fill(0);
    }
    if (this.#isStale(gen)) {
      session.close();
      return;
    }
    // The handshake with this key succeeded, so a first-use key is pinned now (not before).
    if (pin.status === "new" && !this.#deps.checkPin) {
      let committed: Awaited<ReturnType<typeof commitPin>>;
      try {
        committed = await (this.#deps.commitPin ?? commitPin)(this.desktopId, directory.publicKey, device.deviceId);
      } catch {
        return this.#dropNewSession(gen, session, () => this.#fail({ code: "pin_storage" }, { kind: "retry", delay: "backoff" }));
      }
      if (committed === "changed") return this.#dropNewSession(gen, session, () => this.#block("key_changed"));
      if (this.#isStale(gen)) {
        session.close();
        return;
      }
    }
    this.store.setState({ desktopFingerprint: directory.fingerprint });
    this.#session = session;
    this.#sessionCount += 1;
    if (this.#sessionCount > 1) this.#needsResync = true;
    this.#reactToState();
  }

  /** The pin step: read-only unless a test replaces it with `checkPin`. */
  async #inspectPin(directory: Directory): Promise<PinInspection> {
    if (this.#deps.checkPin) {
      const check = await this.#deps.checkPin(this.desktopId, directory.publicKey);
      return { status: check === "changed" ? "changed" : "match", pinnedFingerprint: check === "changed" ? null : directory.fingerprint };
    }
    return (this.#deps.inspectPin ?? inspectPin)(this.desktopId, directory.publicKey);
  }

  /** Ends a session that was never adopted (its callbacks must not run), then applies the outcome. */
  #dropNewSession(gen: number, session: LinkSession, outcome: () => void): void {
    if (this.#isStale(gen)) {
      session.close();
      return;
    }
    this.#gen += 1;
    session.close();
    outcome();
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

  #noteFrame(gen: number): void {
    if (gen !== this.#gen) return;
    const now = this.#deps.now();
    if (now - this.#lastStoredFrameAt < FRAME_STORE_THROTTLE_MS) return;
    this.#lastStoredFrameAt = now;
    this.store.setState({ lastFrameAt: now });
  }

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
      desktopName: this.#directoryName ?? state.desktop?.name ?? this.store.getState().desktopName,
      desktopVersion: state.desktop?.version ?? this.store.getState().desktopVersion,
      proto: state.proto ?? this.store.getState().proto,
      lastFrameAt: session.lastFrameAt,
    });
    this.#lastStoredFrameAt = session.lastFrameAt;
    if (state.proto !== undefined && state.proto > PROTOCOL_VERSION) return this.#block("web_outdated");
    if (state.proto !== undefined && state.proto < PROTOCOL_VERSION) return this.#block("desktop_outdated");
    if (state.state === "revoked") return this.#block("revoked", { revoked: true });
    if (state.state === "pending") {
      // A pending state is shown as it comes; it is never cached as confirmed.
      this.#clearReadyTimer();
      // The desktop keeps a watch until it is told to stop (cap per device): cancel them before dropping.
      this.#dropWatchHandles(true);
      this.store.setState({ status: "pending", lastError: null });
      return;
    }
    if (this.status === "ready") return;
    this.store.setState({ status: "ready", lastError: null });
    this.#retrying = false;
    this.#refusedRetried = false;
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
        // After ready it is a desktop restart: reconnect. Before any `state` the BYE is unauthenticated
        // (a wrong key, a stale channel, a revoked device all look the same): it blocks as `desktop_refused`,
        // never as revoked, and wipes nothing. A user reconnect or the next visible transition tries once more.
        if (afterReady) return this.#fail({ code: "bye" }, { kind: "retry", delay: "backoff" });
        return this.#block("desktop_refused", { error: { code: "bye" } });
      case "unsupported_proto":
        return this.#block("web_outdated");
      case "desktop_proto_older":
        return this.#block("desktop_outdated");
      case "relay_error":
        // errors.ts: the desktop refuses an OLDER web prologue, so the web app is the outdated side.
        if (detail.relayCode === "unsupported_version") return this.#block("web_outdated");
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
    if (!this.#running || globalThis.document?.hidden === true) return;
    if (this.#stopped) {
      if (this.store.getState().blocked === "desktop_refused" && !this.#refusedRetried) {
        this.#refusedRetried = true;
        this.#stopped = false;
        this.store.setState({ blocked: null, lastError: null });
        this.#reconnectNow();
      }
      return;
    }
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
    // A pending session has no traffic at all (no requests, no watches): silence proves nothing.
    if (this.status === "pending") return false;
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
    const token = ++slot.token;
    const params = registration.reopenParams();
    session
      .watch(
        registration.method,
        params,
        (frame) => {
          slot.endedReopens = 0;
          registration.onFrame(frame);
        },
        {
          onDead: () => this.#onWatchDead(gen, id),
          onError: (error) => {
            if (gen !== this.#gen) return;
            const current = this.#watches.get(id);
            if (current) current.handle = null;
            if (current && this.#reopenEndedWatch(id, current, error)) return;
            registration.onError?.(error);
          },
        },
      )
      .then((handle) => {
        if (slot.token === token) slot.opening = false;
        if (gen !== this.#gen || this.#watches.get(id) !== slot || slot.token !== token) {
          handle.stop();
          return;
        }
        slot.handle = handle;
      })
      .catch((error: unknown) => {
        if (slot.token === token) slot.opening = false;
        if (gen !== this.#gen || slot.token !== token) return;
        registration.onError?.(error instanceof LinkRequestError ? error : new LinkRequestError("The watch could not be opened", "session_ended"));
      });
  }

  /**
   * The desktop ended an open watch without a code ("Watch ended", `web_link/pacer.rs`): open it again with the
   * params of `reopenParams()`, which carry `from` (the turns already held), so nothing is missed or duplicated.
   * Returns false when the error is something else or the watch ended too often without a frame in between.
   */
  #reopenEndedWatch(id: number, slot: WatchSlot, error: LinkRequestError): boolean {
    if (error.message !== WATCH_ENDED_MESSAGE || error.code !== undefined) return false;
    if (slot.endedReopens >= MAX_WATCH_ENDED_REOPENS) return false;
    slot.endedReopens += 1;
    queueMicrotask(() => {
      if (this.#watches.get(id) === slot && this.status === "ready") this.#openWatch(id);
    });
    return true;
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

  /** Forgets the open watches (`stop` first cancels them on the desktop) and invalidates opens in flight. */
  #dropWatchHandles(stop = false): void {
    for (const slot of this.#watches.values()) {
      if (stop) slot.handle?.stop();
      slot.handle = null;
      slot.opening = false;
      slot.token += 1;
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
