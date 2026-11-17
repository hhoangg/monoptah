// The device identity the link connection uses (architecture.md 3.4): the key of keys.ts plus the
// server device id from `POST /api/devices`. Registration happens once, on first use; the grant
// request (`POST /api/desktops/:id/grants`) is not here, it belongs to the pairing UX (W-04).
import { RegisterDeviceBody, RegisterDeviceResponse } from "@monoptah/protocol";
import { base64UrlEncode } from "./base64url.ts";
import { fingerprint } from "./fingerprint.ts";
import { DeviceKeyError, bindDeviceId, loadOrCreateDeviceKeyHeld, withDeviceKeyLock } from "./keys.ts";

export interface DeviceIdentity {
  /** Server device id (UUID). */
  deviceId: string;
  /** Public half only; the private key is read by the connection at handshake time. */
  publicKey: Uint8Array;
  /** Lowercase hex SHA-256 of `publicKey`. */
  fingerprint: string;
}

export interface DeviceRegistration {
  name: string;
  publicKey: Uint8Array;
}

/** Registers the public key with the server and returns the server device id. */
export type DeviceRegistrar = (registration: DeviceRegistration) => Promise<string>;

/** HTTP statuses of a 4xx that is worth retrying: 401 is a login block, 408 and 429 are transient. */
const TRANSIENT_CLIENT_ERRORS = new Set([401, 408, 429]);

export class DeviceRegistrationError extends Error {
  readonly status: number | undefined;
  /** The server will not accept this registration however often it is retried (a 4xx such as `quota_exceeded`, a malformed answer). */
  readonly terminal: boolean;
  constructor(message: string, status?: number, terminal?: boolean) {
    super(message);
    this.name = "DeviceRegistrationError";
    this.status = status;
    this.terminal = terminal ?? (status !== undefined && status >= 400 && status < 500 && !TRANSIENT_CLIENT_ERRORS.has(status));
  }
}

const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

const MAX_DEVICE_NAME = 64;

/** A short device name for the UI and HS1, for example "Chrome on macOS". */
export function deviceNameFromUserAgent(userAgent: string = globalThis.navigator?.userAgent ?? ""): string {
  const ua = userAgent;
  let browser = "Browser";
  if (/Edg\//.test(ua)) browser = "Edge";
  else if (/OPR\/|Opera/.test(ua)) browser = "Opera";
  else if (/Firefox\/|FxiOS/.test(ua)) browser = "Firefox";
  else if (/Chrome\/|CriOS/.test(ua)) browser = "Chrome";
  else if (/Safari\//.test(ua)) browser = "Safari";
  let os = "";
  if (/iPhone|iPad|iPod/.test(ua)) os = "iOS";
  else if (/Android/.test(ua)) os = "Android";
  else if (/Windows/.test(ua)) os = "Windows";
  else if (/Mac OS X|Macintosh/.test(ua)) os = "macOS";
  else if (/CrOS/.test(ua)) os = "ChromeOS";
  else if (/Linux|X11/.test(ua)) os = "Linux";
  return (os ? `${browser} on ${os}` : browser).slice(0, MAX_DEVICE_NAME);
}

export const defaultDeviceRegistrar: DeviceRegistrar = async ({ name, publicKey }) => {
  const body = RegisterDeviceBody.parse({ name, publicKey: base64UrlEncode(publicKey) });
  const response = await fetch("/api/devices", {
    method: "POST",
    credentials: "same-origin",
    headers: { "Content-Type": "application/json", "X-Monoptah-CSRF": "1" },
    body: JSON.stringify(body),
  });
  if (!response.ok) throw new DeviceRegistrationError(`device registration failed (${response.status})`, response.status);
  const parsed = RegisterDeviceResponse.safeParse(await response.json().catch(() => null));
  if (!parsed.success) throw new DeviceRegistrationError("device registration returned an unexpected body", undefined, true);
  return parsed.data.deviceId;
};

let registrar: DeviceRegistrar = defaultDeviceRegistrar;

/** Replaces the registrar (W-04 owns the registration UX). Returns the previous one. */
export function setDeviceRegistrar(next: DeviceRegistrar): DeviceRegistrar {
  const previous = registrar;
  registrar = next;
  return previous;
}

let inFlight: Promise<DeviceIdentity> | null = null;

/** Creates the key and registers it on first use; later calls reuse the stored server device id. */
export function ensureDevice(): Promise<DeviceIdentity> {
  inFlight ??= register().finally(() => {
    inFlight = null;
  });
  return inFlight;
}

/** A registration whose POST succeeded but whose bind failed: the next run binds it again instead of registering a second device. */
let unbound: { deviceId: string; publicKey: Uint8Array } | null = null;

function sameBytes(a: Uint8Array, b: Uint8Array): boolean {
  return a.length === b.length && a.every((byte, i) => byte === b[i]);
}

/** Key creation and registration run under one cross-tab lock (keys.ts `withDeviceKeyLock`). */
function register(): Promise<DeviceIdentity> {
  return withDeviceKeyLock(async () => {
    const info = await loadOrCreateDeviceKeyHeld();
    let deviceId = info.deviceId;
    if (deviceId === undefined) {
      if (unbound && sameBytes(unbound.publicKey, info.publicKey)) {
        deviceId = unbound.deviceId;
      } else {
        unbound = null;
        deviceId = await registrar({ name: deviceNameFromUserAgent(), publicKey: info.publicKey });
        // A malformed id would be stored for good and break every later handshake.
        if (!UUID.test(deviceId)) throw new DeviceRegistrationError("device registration returned a malformed device id", undefined, true);
        unbound = { deviceId, publicKey: info.publicKey.slice() };
      }
      try {
        await bindDeviceId(deviceId, info.publicKey);
        unbound = null;
      } catch (error) {
        // The stored key is not the registered one any more: that registration is void.
        if (error instanceof DeviceKeyError) unbound = null;
        throw error;
      }
    }
    return { deviceId, publicKey: info.publicKey, fingerprint: fingerprint(info.publicKey) };
  });
}
