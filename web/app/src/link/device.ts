// The device identity the link connection uses (architecture.md 3.4): the key of keys.ts plus the
// server device id from `POST /api/devices`. Registration happens once, on first use; the grant
// request (`POST /api/desktops/:id/grants`) is not here, it belongs to the pairing UX (W-04).
import { RegisterDeviceBody, RegisterDeviceResponse } from "@monoptah/protocol";
import { base64UrlEncode } from "./base64url.ts";
import { fingerprint } from "./fingerprint.ts";
import { bindDeviceId, loadOrCreateDeviceKey } from "./keys.ts";

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

export class DeviceRegistrationError extends Error {
  readonly status: number | undefined;
  constructor(message: string, status?: number) {
    super(message);
    this.name = "DeviceRegistrationError";
    this.status = status;
  }
}

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
  if (!parsed.success) throw new DeviceRegistrationError("device registration returned an unexpected body");
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

async function register(): Promise<DeviceIdentity> {
  const info = await loadOrCreateDeviceKey();
  let deviceId = info.deviceId;
  if (deviceId === undefined) {
    deviceId = await registrar({ name: deviceNameFromUserAgent(), publicKey: info.publicKey });
    await bindDeviceId(deviceId);
  }
  return { deviceId, publicKey: info.publicKey, fingerprint: fingerprint(info.publicKey) };
}
