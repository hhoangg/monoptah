// Fixtures and a WebSocket wrapper for the relay tests. Rows are written straight into D1.
import { env } from "cloudflare:workers";
import { SELF } from "cloudflare:test";
import { decodeFrame, encodeFrame, FrameType, type Frame } from "@monoptah/protocol";
import { sha256Hex } from "../../../src/lib/crypto.ts";
import { createTestUser, loginAs, type TestSession } from "../../support/accounts.ts";

export const ORIGIN = env.RP_ORIGIN;
export const ZERO_ID = "00000000-0000-0000-0000-000000000000";

export interface RoomFixture {
  userId: string;
  session: TestSession;
  desktopId: string;
  /** Plain relay token of the desktop. */
  token: string;
  deviceId: string;
}

export async function addDesktop(userId: string, options: { publicKey?: string; fingerprint?: string; revoked?: boolean } = {}): Promise<string> {
  const id = crypto.randomUUID();
  await env.DB.prepare(
    "INSERT INTO desktops (id, user_id, name, public_key, fingerprint, created_at, revoked_at) VALUES (?, ?, 'Desktop', ?, ?, 1, ?)",
  )
    .bind(id, userId, options.publicKey ?? "key", options.fingerprint ?? "fingerprint", options.revoked ? 2 : null)
    .run();
  return id;
}

export async function addToken(desktopId: string, options: { scope?: string; revoked?: boolean } = {}): Promise<string> {
  const token = `tok-${crypto.randomUUID()}`;
  await env.DB.prepare("INSERT INTO desktop_tokens (id, desktop_id, token_hash, scope, created_at, revoked_at) VALUES (?, ?, ?, ?, 1, ?)")
    .bind(crypto.randomUUID(), desktopId, await sha256Hex(token), options.scope ?? "relay push grants", options.revoked ? 2 : null)
    .run();
  return token;
}

export async function addDevice(userId: string, options: { revoked?: boolean } = {}): Promise<string> {
  const id = crypto.randomUUID();
  await env.DB.prepare("INSERT INTO devices (id, user_id, name, public_key, fingerprint, created_at, revoked_at) VALUES (?, ?, 'Device', 'key', 'fp', 1, ?)")
    .bind(id, userId, options.revoked ? 2 : null)
    .run();
  return id;
}

export async function addGrant(deviceId: string, desktopId: string, state: "pending" | "confirmed" | "revoked" = "confirmed"): Promise<void> {
  await env.DB.prepare("INSERT INTO device_grants (device_id, desktop_id, state, updated_at) VALUES (?, ?, ?, 1)").bind(deviceId, desktopId, state).run();
}

/** A user with a session, one desktop with a relay token, and one device with a confirmed grant. */
export async function seedRoom(options: { publicKey?: string; fingerprint?: string; grant?: "pending" | "confirmed" | "revoked" } = {}): Promise<RoomFixture> {
  const user = await createTestUser(env);
  const session = await loginAs(user.id);
  const desktopId = await addDesktop(user.id, options);
  const token = await addToken(desktopId);
  const deviceId = await addDevice(user.id);
  await addGrant(deviceId, desktopId, options.grant ?? "confirmed");
  return { userId: user.id, session, desktopId, token, deviceId };
}

/** Another device of the same user with a confirmed grant on `desktopId`. */
export async function seedExtraDevice(f: RoomFixture): Promise<string> {
  const id = await addDevice(f.userId);
  await addGrant(id, f.desktopId);
  return id;
}

export const desktopHeaders = (token: string): Record<string, string> => ({ Authorization: `Bearer ${token}` });
export const deviceHeaders = (session: Pick<TestSession, "cookie">): Record<string, string> => ({ Cookie: session.cookie, Origin: ORIGIN });

export function desktopPath(desktopId: string): string {
  return `/relay/desktop/${desktopId}`;
}
export function devicePath(desktopId: string, deviceId: string): string {
  return `/relay/device/${desktopId}?device=${deviceId}`;
}

/** Raw upgrade request through the Worker. */
export function upgrade(path: string, headers: Record<string, string> = {}): Promise<Response> {
  return SELF.fetch(`${ORIGIN}${path}`, { headers: { Upgrade: "websocket", ...headers } });
}

/** A client side WebSocket that records frames, text and the close code. */
export class Sock {
  readonly ws: WebSocket;
  readonly frames: Frame[] = [];
  readonly raw: Uint8Array[] = [];
  readonly texts: string[] = [];
  closeCode: number | null = null;
  #waiters: (() => void)[] = [];

  constructor(ws: WebSocket) {
    this.ws = ws;
    ws.binaryType = "arraybuffer";
    ws.accept();
    ws.addEventListener("message", (event) => {
      if (typeof event.data === "string") this.texts.push(event.data);
      else {
        const bytes = new Uint8Array(event.data as ArrayBuffer);
        this.raw.push(bytes);
        this.frames.push(decodeFrame(bytes));
      }
      this.#wake();
    });
    ws.addEventListener("close", (event) => {
      this.closeCode = event.code;
      this.#wake();
    });
  }

  #wake(): void {
    const list = this.#waiters;
    this.#waiters = [];
    for (const fn of list) fn();
  }

  send(data: Uint8Array | string): void {
    this.ws.send(data);
  }

  sendFrame(type: number, deviceId: string, channel = 1, body: Uint8Array = new Uint8Array(0)): void {
    this.ws.send(encodeFrame({ type: type as FrameType, channel, deviceId, body }));
  }

  /** Resolves when `predicate` holds (checked after every event), else rejects after `ms`. */
  async until(predicate: () => boolean, ms = 3000): Promise<void> {
    const deadline = Date.now() + ms;
    while (!predicate()) {
      const left = deadline - Date.now();
      if (left <= 0) throw new Error("timed out waiting for a socket event");
      await new Promise<void>((resolve) => {
        this.#waiters.push(resolve);
        setTimeout(resolve, Math.min(left, 100));
      });
    }
  }

  async closed(ms = 3000): Promise<number> {
    await this.until(() => this.closeCode !== null, ms);
    return this.closeCode as number;
  }

  /** CTRL bodies received, parsed. */
  ctrl(): unknown[] {
    return this.frames.filter((f) => f.type === FrameType.CTRL).map((f) => JSON.parse(new TextDecoder().decode(f.body)));
  }

  /** Frames of `type`. */
  ofType(type: number): Frame[] {
    return this.frames.filter((f) => f.type === type);
  }
}

/** Upgrades and returns the socket; throws when the Worker did not answer 101. */
export async function connect(path: string, headers: Record<string, string>): Promise<Sock> {
  const res = await upgrade(path, headers);
  if (res.status !== 101 || !res.webSocket) throw new Error(`upgrade answered ${res.status}`);
  return new Sock(res.webSocket);
}

export const connectDesktop = (f: Pick<RoomFixture, "desktopId" | "token">): Promise<Sock> => connect(desktopPath(f.desktopId), desktopHeaders(f.token));
export const connectDevice = (f: Pick<RoomFixture, "desktopId" | "session">, deviceId: string): Promise<Sock> =>
  connect(devicePath(f.desktopId, deviceId), deviceHeaders(f.session));

/** Closes every socket and waits a moment, so rooms do not leak between tests. */
export async function closeAll(...socks: Sock[]): Promise<void> {
  for (const s of socks) if (s.closeCode === null) s.ws.close(1000);
}

export function binary(length: number, fill = 0): Uint8Array {
  return new Uint8Array(length).fill(fill);
}

/** A valid HS1 frame of `bodySize` body bytes from claimed device `deviceId`. */
export function hs1(deviceId: string, channel = 1, bodySize = 8): Uint8Array {
  return encodeFrame({ type: FrameType.HS1, channel, deviceId, body: binary(bodySize, 7) });
}

export async function lastSeen(table: "desktops" | "devices", id: string): Promise<number | null> {
  const row = await env.DB.prepare(`SELECT last_seen_at AS v FROM ${table} WHERE id = ?`).bind(id).first<{ v: number | null }>();
  return row?.v ?? null;
}

export function roomStub(desktopId: string) {
  return env.ROOM.get(env.ROOM.idFromName(desktopId));
}
