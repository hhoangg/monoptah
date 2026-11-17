// In-memory state of the dev mock server. Nothing here is real: ids, codes and keys are random or
// obviously fake, and recovery codes live in memory only (never in storage).
import type { InviteSchema, PasskeySchema, SessionInfoSchema, DesktopSummarySchema } from "@monoptah/protocol";

type Parsed<T extends { parse(data: unknown): unknown }> = ReturnType<T["parse"]>;
export type MockPasskey = Parsed<typeof PasskeySchema>;
export type MockSession = Parsed<typeof SessionInfoSchema>;
export type MockInvite = Parsed<typeof InviteSchema>;
export type MockDesktop = Parsed<typeof DesktopSummarySchema>;

export interface MockPrefs {
  enabled: boolean;
  categories: { agentFinished: boolean; agentInput: boolean; monoMessages: boolean };
}

export interface MockChallenge {
  purpose: "register" | "login" | "reauth" | "add-passkey";
  expiresAt: number;
  invite?: string;
  handle?: string;
  displayName?: string;
}

export interface MockState {
  user: { id: string; handle: string; displayName: string; role: "user" | "admin" };
  signedIn: boolean;
  recoveryOnly: boolean;
  /** null forces the recovery-codes regenerate screen. */
  recoveryAckAt: number | null;
  invitesPaused: boolean;
  /** Unix seconds of the last passkey re-auth, or null. */
  reauthAt: number | null;
  passkeys: MockPasskey[];
  sessions: MockSession[];
  invites: MockInvite[];
  /** Invite id -> plain code, so a created invite can be used to register. */
  inviteCodes: Map<string, string>;
  /** Codes that can still register an account (the seed bootstrap code and created invites). */
  registrationCodes: Set<string>;
  invitesGranted: number;
  desktops: MockDesktop[];
  prefs: Map<string, MockPrefs>;
  /** Normalised valid recovery codes. */
  recoveryCodes: Set<string>;
  challenges: Map<string, MockChallenge>;
  counter: number;
  /** Failed recovery attempts, for the 429 simulation. */
  recoverFailures: number;
}

export const SEED_INVITE_CODE = "DEV-INVITE-0001";
export const SEED_HANDLE = "dev-user";
export const FRESH_WINDOW_SECONDS = 300;

const CROCKFORD = "0123456789ABCDEFGHJKMNPQRSTVWXYZ";

export function nowSeconds(): number {
  return Math.floor(Date.now() / 1000);
}

export function randomBytes(length: number): Uint8Array {
  return crypto.getRandomValues(new Uint8Array(length));
}

export function base64url(bytes: Uint8Array): string {
  let binary = "";
  for (const byte of bytes) binary += String.fromCharCode(byte);
  return btoa(binary).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

export function randomId(prefix: string): string {
  return `${prefix}-${base64url(randomBytes(9))}`;
}

/** 16 Crockford base32 characters, `XXXX-XXXX-XXXX-XXXX`, random on every call. */
export function generateRecoveryCode(): string {
  const bytes = randomBytes(16);
  const chars = Array.from(bytes, (byte) => CROCKFORD[byte % 32]);
  return [0, 4, 8, 12].map((i) => chars.slice(i, i + 4).join("")).join("-");
}

export function normalizeRecoveryCode(code: string): string {
  return code.replace(/[\s-]/g, "").toUpperCase().replace(/O/g, "0").replace(/[IL]/g, "1");
}

export function newRecoveryBatch(state: MockState): string[] {
  const codes = Array.from({ length: 10 }, generateRecoveryCode);
  state.recoveryCodes = new Set(codes.map(normalizeRecoveryCode));
  return codes;
}

function seedPasskeys(count: number): MockPasskey[] {
  const now = nowSeconds();
  const all: MockPasskey[] = [
    { id: "pk-seed-1", name: "Fake laptop passkey", createdAt: now - 40 * 86400, lastUsedAt: now - 3600, deviceType: "multiDevice", backedUp: true, transports: ["internal", "hybrid"] },
    { id: "pk-seed-2", name: "Fake security key", createdAt: now - 12 * 86400, lastUsedAt: null, deviceType: "singleDevice", backedUp: false, transports: ["usb", "nfc"] },
    { id: "pk-seed-3", name: "Fake phone passkey", createdAt: now - 5 * 86400, lastUsedAt: now - 86400, deviceType: "multiDevice", backedUp: true, transports: ["internal"] },
  ];
  return all.slice(0, count);
}

export interface SeedOptions {
  signedIn?: boolean;
  recoveryOnly?: boolean;
  recoveryAckAt?: "now" | null;
  invitesPaused?: boolean;
  passkeyCount?: number;
}

/** A fresh state: one existing account, three sessions, two desktops, the seed invite code. */
export function createMockState(options: SeedOptions = {}): MockState {
  const now = nowSeconds();
  const state: MockState = {
    user: { id: "user-dev-1", handle: SEED_HANDLE, displayName: "Dev User", role: "user" },
    signedIn: options.signedIn ?? false,
    recoveryOnly: options.recoveryOnly ?? false,
    recoveryAckAt: options.recoveryAckAt === null ? null : now - 86400,
    invitesPaused: options.invitesPaused ?? false,
    reauthAt: null,
    passkeys: seedPasskeys(options.passkeyCount ?? 2),
    sessions: [
      { id: "sess-current", createdAt: now - 7200, lastSeenAt: now, userAgent: "Fake Browser/1.0 (current)", amr: options.recoveryOnly ? "recovery" : "passkey", current: true },
      { id: "sess-other-1", createdAt: now - 3 * 86400, lastSeenAt: now - 7200, userAgent: "Fake Phone Browser/2.0", amr: "passkey", current: false },
      { id: "sess-other-2", createdAt: now - 9 * 86400, lastSeenAt: now - 5 * 86400, userAgent: null, amr: "passkey", current: false },
    ],
    invites: [],
    inviteCodes: new Map(),
    registrationCodes: new Set([SEED_INVITE_CODE]),
    invitesGranted: 3,
    desktops: [
      { id: "desktop-dev-1", name: "Fake Studio Mac", color: "#6d8cff", online: true, pendingCount: 0, os: "macos", appVersion: "0.0.0-mock", lastSeenAt: now },
      { id: "desktop-dev-2", name: "Fake Linux box", color: null, online: false, pendingCount: 1, os: "linux", appVersion: "0.0.0-mock", lastSeenAt: now - 2 * 86400 },
    ],
    prefs: new Map(),
    recoveryCodes: new Set(),
    challenges: new Map(),
    counter: 0,
    recoverFailures: 0,
  };
  newRecoveryBatch(state);
  return state;
}

export function defaultPrefs(): MockPrefs {
  return { enabled: true, categories: { agentFinished: true, agentInput: true, monoMessages: true } };
}
