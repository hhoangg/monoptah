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

/** A desktop pairing request, as the Worker's `pairing_requests` row (R-03). */
export interface MockPairing {
  machineName: string;
  os: string;
  /** Full lowercase hex SHA-256 of the desktop public key. */
  fingerprint: string;
  confirmationCode: string;
  /** Unix seconds; null renews the code to 10 minutes ahead on every read (dev convenience). */
  expiresAt: number | null;
  status: "pending" | "approved" | "denied";
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
  /** Pairing requests by normalised user code (no dash, upper case). */
  pairings: Map<string, MockPairing>;
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
export const PAIRING_TTL_SECONDS = 600;
/** Max desktops per account (Worker `MAX_DESKTOPS_PER_USER`, default 5). */
export const MAX_DESKTOPS = 5;

/** Dev pairing codes: open `/pair/<code>#fp=<MOCK_PAIR_FINGERPRINT>` to see each state. */
export const MOCK_PAIR_CODE = "WXYZ-4829";
export const MOCK_PAIR_LONG_NAME_CODE = "NAME-2345";
export const MOCK_PAIR_EXPIRED_CODE = "EXPR-2345";
/** Obviously fake: a repeating pattern, not the hash of any key. */
export const MOCK_PAIR_FINGERPRINT = "0123456789abcdef".repeat(4);
export const MOCK_PAIR_CONFIRMATION = "482916";

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

/** Same rule as the Worker: case-insensitive, dashes optional. */
export function normalizeUserCode(code: string): string {
  return code.toUpperCase().replaceAll("-", "");
}

export function normalizeRecoveryCode(code: string): string {
  return code.replace(/[\s-]/g, "").toUpperCase().replace(/O/g, "0").replace(/[IL]/g, "1");
}

export function newRecoveryBatch(state: MockState): string[] {
  const codes = Array.from({ length: 10 }, generateRecoveryCode);
  state.recoveryCodes = new Set(codes.map(normalizeRecoveryCode));
  return codes;
}

function seedPairings(): Map<string, MockPairing> {
  const now = nowSeconds();
  const base = { os: "macOS 15", fingerprint: MOCK_PAIR_FINGERPRINT, confirmationCode: MOCK_PAIR_CONFIRMATION, status: "pending" } as const;
  return new Map<string, MockPairing>([
    [normalizeUserCode(MOCK_PAIR_CODE), { ...base, machineName: "Ada's MacBook Pro", expiresAt: null }],
    [
      normalizeUserCode(MOCK_PAIR_LONG_NAME_CODE),
      { ...base, machineName: `<b>Fake</b>\u202E exe.txt ${"very-long-machine-name-".repeat(3)}`.slice(0, 64), expiresAt: null },
    ],
    [normalizeUserCode(MOCK_PAIR_EXPIRED_CODE), { ...base, machineName: "Old desktop", expiresAt: now - 60 }],
  ]);
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

function seedDesktops(count: number, now: number): MockDesktop[] {
  const all: MockDesktop[] = [
    { id: "desktop-dev-1", name: "Fake Studio Mac", color: "#6d8cff", online: true, pendingCount: 0, os: "macos", appVersion: "0.0.0-mock", lastSeenAt: now },
    { id: "desktop-dev-2", name: "Fake Linux box", color: null, online: false, pendingCount: 1, os: "linux", appVersion: "0.0.0-mock", lastSeenAt: now - 2 * 86400 },
    { id: "desktop-dev-3", name: "Fake Windows PC", color: null, online: false, pendingCount: 0, os: "windows", appVersion: "0.0.0-mock", lastSeenAt: now - 5 * 86400 },
    { id: "desktop-dev-4", name: "Fake Mac mini", color: null, online: false, pendingCount: 0, os: "macos", appVersion: "0.0.0-mock", lastSeenAt: now - 6 * 86400 },
    { id: "desktop-dev-5", name: "Fake Raspberry Pi", color: null, online: false, pendingCount: 0, os: "linux", appVersion: "0.0.0-mock", lastSeenAt: now - 9 * 86400 },
  ];
  return all.slice(0, count);
}

export interface SeedOptions {
  signedIn?: boolean;
  recoveryOnly?: boolean;
  recoveryAckAt?: "now" | null;
  invitesPaused?: boolean;
  passkeyCount?: number;
  /** 0 gives an account with no desktops, 5 one at the desktop limit. Default 2. */
  desktopCount?: number;
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
    desktops: seedDesktops(options.desktopCount ?? 2, now),
    pairings: seedPairings(),
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
