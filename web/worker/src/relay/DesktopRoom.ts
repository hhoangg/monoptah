import { DurableObject } from "cloudflare:workers";
import {
  CloseCodes,
  FrameType,
  MAX_FRAME_SIZE,
  ROOM_QUOTAS,
  decodeFrame,
  desktopMaySend,
  deviceMaySend,
  encodeCtrlBody,
  encodeErrorBody,
  encodeFrame,
  overwriteDeviceId,
  type CtrlMessage,
} from "@monoptah/protocol";
import type { Env } from "../env.ts";
import { SocketLimiter, type RateConfig } from "./bucket.ts";
import { isDesktopOnline } from "./presence.ts";

/**
 * What `revoke` closes (architecture.md 4.4): one device, one cookie session, several cookie sessions in one call
 * (so logout everywhere needs one call per desktop, not one per session and desktop), or every socket.
 */
export type RevokeTarget = { deviceId: string } | { sessionIdHash: string } | { sessionIdHashes: string[] } | { all: true };

/** Kept in `serializeAttachment` (well under 16 KiB); the only state that survives hibernation. */
interface Attachment {
  role: "desktop" | "device";
  desktopId: string;
  /** Lowercase UUID of a device socket, null for the desktop. */
  deviceId: string | null;
  userId: string;
  sessionIdHash: string | null;
  joinedAt: number;
  /** Channels this device socket used (capped), so revoke can send BYE for each. */
  channels: number[];
  /** Grant state of a device socket (`pending` sockets are capped per room); a missing header means `pending`. */
  grantState?: "pending" | "confirmed";
  /** Absolute expiry of the cookie session behind a device socket, in ms; the room closes the socket past it (4001). */
  expiresAt?: number;
  /** Random id keying the in-memory rate limiter. */
  sid: string;
  /** Set once the socket is closed or closing; every later effect is skipped. */
  gone?: boolean;
}

const MAX_CHANNELS = 32;
/** Storage key of the revocation list (R13B-1): refuses a join that races a revocation, whatever order the calls arrive in. */
const REVOKED_KEY = "revoked";
/** How long a revoked device id or session hash is remembered: the absolute session lifetime (30 days). */
export const REVOCATION_TTL_MS = 30 * 86_400_000;
/** Entries kept per list; when full, the entries closest to expiry go first. */
export const REVOCATION_LIST_LIMIT = 256;
/** The expiry sweep over all device sockets runs at most this often (ms). */
const SWEEP_INTERVAL_MS = 1000;

/** Revoked device ids and session hashes with their expiry (ms). Lives in the room's storage, so it survives eviction. */
interface RevokedList {
  devices: Record<string, number>;
  sessions: Record<string, number>;
}

function prune(list: Record<string, number>, now: number): void {
  for (const key of Object.keys(list)) if ((list[key] as number) <= now) delete list[key];
  const keys = Object.keys(list);
  if (keys.length <= REVOCATION_LIST_LIMIT) return;
  keys.sort((a, b) => (list[a] as number) - (list[b] as number));
  for (const key of keys.slice(0, keys.length - REVOCATION_LIST_LIMIT)) delete list[key];
}
/** Device sockets of devices whose grant is still `pending`, per room; a new one closes 4010. */
export const PENDING_DEVICE_SOCKETS_PER_ROOM = 4;
/** `last_seen_at` is only rewritten when the stored value is at least this many seconds old. */
export const LAST_SEEN_INTERVAL_SECONDS = 3600;
export const TOUCH_DESKTOP_SEEN_SQL = `UPDATE desktops SET last_seen_at = ?1 WHERE id = ?2 AND (last_seen_at IS NULL OR last_seen_at < ?1 - ${LAST_SEEN_INTERVAL_SECONDS})`;
export const TOUCH_DEVICE_SEEN_SQL = `UPDATE devices SET last_seen_at = ?1 WHERE id = ?2 AND (last_seen_at IS NULL OR last_seen_at < ?1 - ${LAST_SEEN_INTERVAL_SECONDS})`;
const ZERO_UUID = "00000000-0000-0000-0000-000000000000";
const OPEN = 1;

const DEVICE_RATE: RateConfig = ROOM_QUOTAS.deviceRate;
const DESKTOP_RATE: RateConfig = { ...ROOM_QUOTAS.desktopRate, burst: ROOM_QUOTAS.desktopRate.messagesPerSecond };

function logClose(role: string, code: number): void {
  console.log(`relay close role=${role} code=${code}`);
}

/**
 * One room per desktop (`env.ROOM.idFromName(desktopId)`), SQLite-backed, WebSocket Hibernation API
 * (architecture.md 4.4). The Worker authenticated every upgrade and forwards it with `x-role`, `x-desktop-id`,
 * `x-user-id` and for devices `x-device-id` and `x-session-id-hash`; this class trusts those headers because the
 * room has no public route.
 *
 * Rules that hold for every later change (capacity.md 1): no timers, no alarms, no outbound sockets, and a
 * constructor that does nothing beyond the bindings and the ping auto-response, so an idle room hibernates and
 * costs no duration. Frame bytes are never logged.
 */
export class DesktopRoom extends DurableObject<Env> {
  /** Rate limiters by socket id; lost on hibernation (best effort). */
  readonly #limiters = new Map<string, SocketLimiter>();
  #lastSweep = 0;

  constructor(ctx: DurableObjectState, env: Env) {
    super(ctx, env);
    // Answers keepalives without waking the object; the runtime keeps it across hibernation.
    ctx.setWebSocketAutoResponse(new WebSocketRequestResponsePair(ROOM_QUOTAS.textFramePing, ROOM_QUOTAS.textFramePong));
  }

  // --- Join ---

  override async fetch(request: Request): Promise<Response> {
    if (request.headers.get("Upgrade")?.toLowerCase() !== "websocket") return new Response("Expected WebSocket", { status: 426 });
    const role = request.headers.get("x-role");
    const desktopId = request.headers.get("x-desktop-id");
    const userId = request.headers.get("x-user-id");
    if ((role !== "desktop" && role !== "device") || !desktopId || !userId) return new Response("Bad request", { status: 400 });
    const deviceId = role === "device" ? (request.headers.get("x-device-id") ?? "").toLowerCase() : null;
    if (role === "device" && !deviceId) return new Response("Bad request", { status: 400 });

    const pair = new WebSocketPair();
    const [client, server] = [pair[0], pair[1]];
    const attachment: Attachment = {
      role,
      desktopId,
      deviceId,
      userId,
      sessionIdHash: role === "device" ? request.headers.get("x-session-id-hash") : null,
      ...(role === "device" ? { grantState: request.headers.get("x-grant-state") === "confirmed" ? ("confirmed" as const) : ("pending" as const) } : {}),
      joinedAt: Date.now(),
      channels: [],
      sid: crypto.randomUUID(),
    };
    if (role === "device") {
      const expires = Number(request.headers.get("x-session-expires-at"));
      if (Number.isFinite(expires) && expires > 0) attachment.expiresAt = expires * 1000;
    }

    if (role === "desktop") {
      for (const old of this.#live("desktop")) {
        this.#markGone(old);
        this.#close(old, CloseCodes.REPLACED);
        logClose("desktop", CloseCodes.REPLACED);
      }
      this.ctx.acceptWebSocket(server, ["desktop"]);
      server.serializeAttachment(attachment);
      for (const device of this.#devices()) this.#sendCtrl(device, { t: "presence", desktop: "online" }, this.#att(device)?.deviceId ?? ZERO_UUID);
      this.#sendCtrl(server, { t: "grants" }, ZERO_UUID);
    } else {
      const id = deviceId as string;
      const refusal = await this.#joinRefusal(id, attachment);
      if (refusal !== null) {
        server.accept();
        server.close(refusal, "refused");
        logClose("device", refusal);
        return new Response(null, { status: 101, webSocket: client });
      }
      this.#sweepExpired(true);
      const all = this.#devices();
      const same = all.filter((ws) => this.#att(ws)?.deviceId === id).length;
      const pendingSockets = all.filter((ws) => this.#att(ws)?.grantState === "pending").length;
      const pendingFull = attachment.grantState === "pending" && pendingSockets >= PENDING_DEVICE_SOCKETS_PER_ROOM;
      if (all.length >= ROOM_QUOTAS.deviceSocketsPerRoom || same >= ROOM_QUOTAS.deviceSocketsPerDevice || pendingFull) {
        server.accept();
        server.close(CloseCodes.ROOM_QUOTA, "room quota");
        logClose("device", CloseCodes.ROOM_QUOTA);
        return new Response(null, { status: 101, webSocket: client });
      }
      this.ctx.acceptWebSocket(server, [`device:${id}`]);
      server.serializeAttachment(attachment);
      this.#sendCtrl(server, { t: "presence", desktop: this.#online() ? "online" : "offline" }, id);
      const desktop = this.#desktop();
      if (desktop) this.#sendCtrl(desktop, { t: "device", id, event: "joined" }, ZERO_UUID);
    }
    return new Response(null, { status: 101, webSocket: client });
  }

  // --- Messages ---

  override async webSocketMessage(ws: WebSocket, message: string | ArrayBuffer): Promise<void> {
    const att = this.#att(ws);
    if (!att || att.gone) return;
    if (att.role === "device" && att.expiresAt !== undefined && Date.now() >= att.expiresAt) {
      this.#expire(ws, att);
      return;
    }
    // Synchronous: no await between the sweep and forwarding, so frames keep their order (N4).
    this.#sweepExpired(false);
    if (typeof message === "string") {
      if (message === ROOM_QUOTAS.textFramePing) {
        ws.send(ROOM_QUOTAS.textFramePong);
        return;
      }
      await this.#drop(ws, CloseCodes.PROTOCOL_ERROR);
      return;
    }
    if (message.byteLength > MAX_FRAME_SIZE) {
      await this.#drop(ws, CloseCodes.MESSAGE_TOO_BIG);
      return;
    }
    const now = Date.now();
    if (!this.#limiter(att, now).allow(message.byteLength, now)) {
      const limit = ROOM_QUOTAS.rateBreachesBeforeClose;
      if (this.#limiter(att, now).breach(now, limit, ROOM_QUOTAS.rateBreachWindowMs)) await this.#drop(ws, CloseCodes.RATE_LIMITED);
      return;
    }
    const bytes = new Uint8Array(message);
    let frame;
    try {
      frame = decodeFrame(bytes);
    } catch {
      await this.#drop(ws, CloseCodes.PROTOCOL_ERROR);
      return;
    }
    if (att.role === "device") this.#fromDevice(ws, att, frame, bytes);
    else this.#fromDesktop(frame, bytes);
  }

  #fromDevice(ws: WebSocket, att: Attachment, frame: ReturnType<typeof decodeFrame>, bytes: Uint8Array): void {
    if (!deviceMaySend(frame.type)) return;
    const deviceId = att.deviceId as string;
    if (!att.channels.includes(frame.channel) && att.channels.length < MAX_CHANNELS) {
      att.channels.push(frame.channel);
      ws.serializeAttachment(att);
    }
    const desktop = this.#desktop();
    if (!desktop) {
      this.#send(ws, encodeFrame({ type: FrameType.ERROR, channel: frame.channel, deviceId, body: encodeErrorBody({ code: "desktop_offline" }) }));
      return;
    }
    this.#send(desktop, overwriteDeviceId(bytes, deviceId));
  }

  #fromDesktop(frame: ReturnType<typeof decodeFrame>, bytes: Uint8Array): void {
    if (!desktopMaySend(frame.type)) return;
    for (const ws of this.#live(`device:${frame.deviceId}`)) this.#send(ws, bytes);
  }

  // --- Close ---

  override async webSocketClose(ws: WebSocket, code: number): Promise<void> {
    await this.#depart(ws, code);
    try {
      ws.close(code >= 1000 && code <= 4999 && code !== 1005 && code !== 1006 && code !== 1015 ? code : CloseCodes.NORMAL);
    } catch {
      // Already closed (the runtime replies to the peer's close itself).
    }
  }

  override async webSocketError(ws: WebSocket, error: unknown): Promise<void> {
    console.error(`relay socket error: error=${error instanceof Error ? error.name : "unknown"}`);
    await this.#depart(ws, CloseCodes.NORMAL);
    try {
      ws.close(CloseCodes.NORMAL);
    } catch {
      // Already closed.
    }
  }

  // --- RPC (called by the Worker through `rooms.ts`) ---

  /** Closes the matching sockets with `code` (4001, 4003 or 4011). */
  async revoke(target: RevokeTarget, code: number): Promise<void> {
    await this.#remember(target);
    const all = "all" in target;
    const sessions = "sessionIdHash" in target ? new Set([target.sessionIdHash]) : "sessionIdHashes" in target ? new Set(target.sessionIdHashes) : null;
    const wantedDevice = "deviceId" in target ? target.deviceId.toLowerCase() : null;
    const desktop = this.#desktop();
    const byes = new Set<string>();
    const affected = new Map<string, Attachment>();
    for (const ws of this.#devices()) {
      const att = this.#att(ws);
      if (!att?.deviceId) continue;
      const match = all || att.deviceId === wantedDevice || (sessions !== null && att.sessionIdHash !== null && sessions.has(att.sessionIdHash));
      if (!match) continue;
      if (desktop && !all) this.#sendByes(desktop, att, byes);
      this.#markGone(ws);
      this.#close(ws, code);
      logClose("device", code);
      affected.set(att.deviceId, att);
    }
    for (const att of affected.values()) if (!all) await this.#deviceLeft(att);
    if (all && desktop) {
      await this.#depart(desktop, code, true);
      this.#close(desktop, code);
      logClose("desktop", code);
    }
  }

  /** Rewrites the grant state kept on every socket of the device (no D1, no frames), so the pending cap stays accurate. */
  async setGrantState(deviceId: string, state: "pending" | "confirmed"): Promise<void> {
    for (const ws of this.ctx.getWebSockets(`device:${deviceId.toLowerCase()}`)) {
      const att = this.#att(ws);
      if (!att || att.gone || att.role !== "device") continue;
      att.grantState = state;
      ws.serializeAttachment(att);
    }
  }

  /** The session hashes of the live sockets of `deviceId` (file downloads bind to the session that holds the link). */
  async deviceSessions(deviceId: string): Promise<string[]> {
    const hashes = new Set<string>();
    for (const ws of this.ctx.getWebSockets(`device:${deviceId.toLowerCase()}`)) {
      const att = this.#att(ws);
      if (att && !att.gone && att.role === "device" && att.sessionIdHash) hashes.add(att.sessionIdHash);
    }
    return [...hashes];
  }

  /** Whether the desktop socket is live (architecture.md 4.4 "Presence"). */
  async presence(): Promise<{ online: boolean }> {
    return { online: this.#online() };
  }

  /** Sends the desktop socket, if any, a `CTRL {"t":"grants"}` frame so it re-reads `GET /api/desktop/grants`. */
  async notifyGrants(): Promise<void> {
    const desktop = this.#desktop();
    if (desktop) this.#sendCtrl(desktop, { t: "grants" }, ZERO_UUID);
  }

  // --- Revocation memory and expiry (R13B-1) ---

  async #loadRevoked(now: number): Promise<RevokedList> {
    const stored = await this.ctx.storage.get<RevokedList>(REVOKED_KEY);
    const list: RevokedList = { devices: { ...(stored?.devices ?? {}) }, sessions: { ...(stored?.sessions ?? {}) } };
    prune(list.devices, now);
    prune(list.sessions, now);
    return list;
  }

  /** Writes the target of a revoke into storage before any socket is touched, so a join that raced the revoke is refused. Idempotent. */
  async #remember(target: RevokeTarget): Promise<void> {
    if ("all" in target) return;
    const now = Date.now();
    const list = await this.#loadRevoked(now);
    const expiry = now + REVOCATION_TTL_MS;
    if ("deviceId" in target) list.devices[target.deviceId.toLowerCase()] = expiry;
    else for (const hash of "sessionIdHash" in target ? [target.sessionIdHash] : target.sessionIdHashes) list.sessions[hash] = expiry;
    prune(list.devices, now);
    prune(list.sessions, now);
    await this.ctx.storage.put(REVOKED_KEY, list);
  }

  /** The close code for a join that must be refused (revoked device 4003, revoked or expired session 4001), else null. */
  async #joinRefusal(deviceId: string, att: Attachment): Promise<number | null> {
    const now = Date.now();
    if (att.expiresAt !== undefined && now >= att.expiresAt) return CloseCodes.UNAUTHORIZED;
    const list = await this.#loadRevoked(now);
    if (list.devices[deviceId] !== undefined) return CloseCodes.REVOKED;
    if (att.sessionIdHash !== null && list.sessions[att.sessionIdHash] !== undefined) return CloseCodes.UNAUTHORIZED;
    return null;
  }

  /** BYE to the desktop for each channel the device used, once per device and channel. */
  #sendByes(desktop: WebSocket, att: Attachment, seen: Set<string>): void {
    for (const channel of att.channels) {
      const key = `${att.deviceId}:${channel}`;
      if (seen.has(key)) continue;
      seen.add(key);
      this.#send(desktop, encodeFrame({ type: FrameType.BYE, channel, deviceId: att.deviceId as string }));
    }
  }

  /**
   * Closes one device socket whose session expired (4001), with BYE per channel. No timer: runs on a message or a join.
   * Everything here is synchronous; the departure effects (CTRL left, a D1 write) run afterwards without being awaited,
   * so a sweep never opens the input gate between a join check and `acceptWebSocket`, and never delays a frame (N4).
   */
  #expire(ws: WebSocket, att: Attachment): void {
    const desktop = this.#desktop();
    if (desktop) this.#sendByes(desktop, att, new Set());
    this.#markGone(ws);
    this.#close(ws, CloseCodes.UNAUTHORIZED);
    logClose("device", CloseCodes.UNAUTHORIZED);
    void this.#deviceLeft(att).catch(() => undefined);
  }

  /** Closes every device socket past its session expiry; throttled unless `force`. Synchronous. */
  #sweepExpired(force: boolean): void {
    const now = Date.now();
    if (!force && now - this.#lastSweep < SWEEP_INTERVAL_MS) return;
    this.#lastSweep = now;
    for (const ws of this.#devices()) {
      const att = this.#att(ws);
      if (att?.expiresAt !== undefined && now >= att.expiresAt) this.#expire(ws, att);
    }
  }

  // --- Internals ---

  #att(ws: WebSocket): Attachment | null {
    return (ws.deserializeAttachment() as Attachment | null) ?? null;
  }

  #live(tag: string): WebSocket[] {
    return this.ctx.getWebSockets(tag).filter((ws) => ws.readyState === OPEN && !this.#att(ws)?.gone);
  }

  #devices(): WebSocket[] {
    return this.ctx.getWebSockets().filter((ws) => {
      const att = this.#att(ws);
      return ws.readyState === OPEN && att?.role === "device" && !att.gone;
    });
  }

  #desktop(): WebSocket | null {
    return this.#live("desktop")[0] ?? null;
  }

  #online(): boolean {
    const desktop = this.#desktop();
    if (!desktop) return false;
    const lastPing = this.ctx.getWebSocketAutoResponseTimestamp(desktop)?.getTime() ?? null;
    return isDesktopOnline(Date.now(), this.#att(desktop)?.joinedAt ?? 0, lastPing);
  }

  #limiter(att: Attachment, now: number): SocketLimiter {
    let limiter = this.#limiters.get(att.sid);
    if (!limiter) {
      limiter = new SocketLimiter(att.role === "device" ? DEVICE_RATE : DESKTOP_RATE, now);
      this.#limiters.set(att.sid, limiter);
    }
    return limiter;
  }

  #markGone(ws: WebSocket): void {
    const att = this.#att(ws);
    if (!att || att.gone) return;
    att.gone = true;
    ws.serializeAttachment(att);
    this.#limiters.delete(att.sid);
  }

  #close(ws: WebSocket, code: number): void {
    try {
      ws.close(code, "");
    } catch {
      // Already closed.
    }
  }

  #send(ws: WebSocket, data: Uint8Array | string): void {
    try {
      ws.send(data);
    } catch {
      // The socket is closing; its close handler cleans up.
    }
  }

  #sendCtrl(ws: WebSocket, message: CtrlMessage, deviceId: string): void {
    this.#send(ws, encodeFrame({ type: FrameType.CTRL, channel: 0, deviceId, body: encodeCtrlBody(message) }));
  }

  /** Closes `ws` for a protocol or quota reason and runs the departure effects. */
  async #drop(ws: WebSocket, code: number): Promise<void> {
    const att = this.#att(ws);
    this.#close(ws, code);
    logClose(att?.role ?? "unknown", code);
    await this.#depart(ws, code);
  }

  /** Departure effects of a socket, once: presence offline or device left, plus one conditional `last_seen_at` write (at most hourly per row). */
  async #depart(ws: WebSocket, _code: number, skipDeviceNotices = false): Promise<void> {
    const att = this.#att(ws);
    if (!att || att.gone) return;
    this.#markGone(ws);
    if (att.role === "desktop") {
      if (!skipDeviceNotices) {
        for (const device of this.#devices()) this.#sendCtrl(device, { t: "presence", desktop: "offline" }, this.#att(device)?.deviceId ?? ZERO_UUID);
      }
      await this.#writeLastSeen(TOUCH_DESKTOP_SEEN_SQL, att.desktopId);
    } else {
      this.#byeAbandonedChannels(att);
      await this.#deviceLeft(att);
    }
  }

  /**
   * A device socket that closes without sending BYE (a page reload or navigation drops the socket silently) would
   * leave its Noise sessions open on the desktop, which caps channels per device. Send the desktop a BYE for each
   * channel this socket used that no other live socket of the same device uses, so a tab that already took the
   * channel over keeps its session. Synchronous.
   */
  #byeAbandonedChannels(att: Attachment): void {
    const id = att.deviceId;
    if (!id || att.channels.length === 0) return;
    const desktop = this.#desktop();
    if (!desktop) return;
    const stillUsed = new Set<number>();
    for (const other of this.#live(`device:${id}`)) for (const channel of this.#att(other)?.channels ?? []) stillUsed.add(channel);
    for (const channel of att.channels) {
      if (stillUsed.has(channel)) continue;
      this.#send(desktop, encodeFrame({ type: FrameType.BYE, channel, deviceId: id }));
    }
  }

  /** When no live socket of the device remains: CTRL left to the desktop and one conditional `last_seen_at` write. */
  async #deviceLeft(att: Attachment): Promise<void> {
    const id = att.deviceId;
    if (!id || this.#live(`device:${id}`).length > 0) return;
    const desktop = this.#desktop();
    if (desktop) this.#sendCtrl(desktop, { t: "device", id, event: "left" }, ZERO_UUID);
    await this.#writeLastSeen(TOUCH_DEVICE_SEEN_SQL, id);
  }

  async #writeLastSeen(sql: string, id: string): Promise<void> {
    try {
      await this.env.DB.prepare(sql).bind(Math.floor(Date.now() / 1000), id).run();
    } catch (error) {
      console.error(`relay last_seen write failed: error=${error instanceof Error ? error.name : "unknown"}`);
    }
  }
}
