// Dev-only entry of the mock API. `main.tsx` imports this module dynamically, and only when
// `import.meta.env.DEV && import.meta.env.VITE_MOCK_API === "1"`, so it never reaches the production
// bundle (`scripts/check-dist.mjs` fails the build when MOCK_MARKER is found in `dist/client`).
//
// Dev hooks (mock mode only):
//  - `window.__monoptahMock` with `reset(options)`, `set(patch)`, `snapshot()` and `recoveryCodes()`.
//  - A `?mock=` query on the first page load picks a starting state, comma separated:
//    `signedin`, `signedout`, `recoveryonly`, `noack` (recoveryAckAt null), `paused` (invites
//    paused), `onepasskey`, `threepasskeys`, `nodesktops`, `fivedesktops` (at the desktop limit).
//    Example: `/settings?mock=signedin,noack`.
//  - Pairing codes for /pair/:code (state.ts): `WXYZ-4829` (valid, renews itself), `NAME-2345` (hostile
//    machine name), `EXPR-2345` (expired); any other code is unknown. The matching fingerprint for the
//    link is `MOCK_PAIR_FINGERPRINT`: `/pair/WXYZ-4829#fp=0123456789abcdef` repeated 4 times; a different
//    `#fp` is the mismatch state, no `#fp` the missing one.
//  - sessionStorage key `monoptah.mock.signedIn` ("1") keeps the mock session across reloads.
import { setApiTransport } from "../api";
import { createMockServer } from "./transport";
import { createMockState, nowSeconds, SEED_HANDLE, SEED_INVITE_CODE, type SeedOptions } from "./state";

/** Unique string that must never appear in a production bundle. */
export const MOCK_MARKER = "__MONOPTAH_MOCK_API__";

const SIGNED_IN_KEY = "monoptah.mock.signedIn";

export interface MockPatch {
  signedIn?: boolean;
  recoveryOnly?: boolean;
  recoveryAckAt?: number | null;
  invitesPaused?: boolean;
  passkeyCount?: number;
  /** Seconds since the last re-auth; null clears the fresh window. */
  reauthAgeSeconds?: number | null;
}

export interface MockHooks {
  /** Replaces the whole state with a fresh seed. */
  reset(options?: SeedOptions): void;
  /** Changes single fields of the current state. */
  set(patch: MockPatch): void;
  /** A copy of the visible state (never the recovery codes). */
  snapshot(): Record<string, unknown>;
  /** Valid recovery codes of the account, for exercising /recover by hand. Dev only. */
  recoveryCodes(): string[];
  seedInvite: string;
  seedHandle: string;
}

declare global {
  interface Window {
    __monoptahMock?: MockHooks;
  }
}

/** Reads the `?mock=` preset list into seed options. */
export function seedFromQuery(search: string, storedSignedIn: boolean): SeedOptions {
  const flags = new Set(
    (new URLSearchParams(search).get("mock") ?? "")
      .toLowerCase()
      .split(",")
      .map((flag) => flag.trim())
      .filter(Boolean),
  );
  const options: SeedOptions = { signedIn: storedSignedIn };
  if (flags.has("signedin")) options.signedIn = true;
  if (flags.has("signedout")) options.signedIn = false;
  if (flags.has("recoveryonly")) {
    options.recoveryOnly = true;
    options.signedIn = true;
  }
  if (flags.has("noack")) options.recoveryAckAt = null;
  if (flags.has("paused")) options.invitesPaused = true;
  if (flags.has("onepasskey")) options.passkeyCount = 1;
  if (flags.has("threepasskeys")) options.passkeyCount = 3;
  if (flags.has("nodesktops")) options.desktopCount = 0;
  if (flags.has("fivedesktops")) options.desktopCount = 5;
  return options;
}

function readStoredSignedIn(): boolean {
  try {
    return sessionStorage.getItem(SIGNED_IN_KEY) === "1";
  } catch {
    return false;
  }
}

function persistSignedIn(signedIn: boolean): void {
  try {
    if (signedIn) sessionStorage.setItem(SIGNED_IN_KEY, "1");
    else sessionStorage.removeItem(SIGNED_IN_KEY);
  } catch {
    // Storage may be blocked; the mock session then ends on reload.
  }
}

export function installMockApi(): void {
  let server = createMockServer({ ...seedFromQuery(window.location.search, readStoredSignedIn()), latencyMs: 120 });

  // Follows the real flag after every call, so a reload keeps the mock session.
  const syncSession = () => persistSignedIn(server.state.signedIn);
  setApiTransport(async (url, init) => {
    try {
      return await server.transport(url, init);
    } finally {
      syncSession();
    }
  });
  syncSession();

  const hooks: MockHooks = {
    reset(options = {}) {
      server = createMockServer({ ...options, latencyMs: 120 });
      syncSession();
    },
    set(patch) {
      const { state } = server;
      if (patch.signedIn !== undefined) state.signedIn = patch.signedIn;
      if (patch.recoveryOnly !== undefined) {
        state.recoveryOnly = patch.recoveryOnly;
        if (patch.recoveryOnly) state.signedIn = true;
      }
      if (patch.recoveryAckAt !== undefined) state.recoveryAckAt = patch.recoveryAckAt;
      if (patch.invitesPaused !== undefined) state.invitesPaused = patch.invitesPaused;
      if (patch.passkeyCount !== undefined) state.passkeys = createMockState({ passkeyCount: patch.passkeyCount }).passkeys;
      if (patch.reauthAgeSeconds !== undefined) {
        state.reauthAt = patch.reauthAgeSeconds === null ? null : nowSeconds() - patch.reauthAgeSeconds;
      }
      syncSession();
    },
    snapshot() {
      const { user, signedIn, recoveryOnly, recoveryAckAt, invitesPaused, reauthAt, passkeys, sessions, invites, desktops, pairings } = server.state;
      const visible = { user, signedIn, recoveryOnly, recoveryAckAt, invitesPaused, reauthAt, passkeys, sessions, invites, desktops, pairings: Object.fromEntries(pairings) };
      return JSON.parse(JSON.stringify(visible)) as Record<string, unknown>;
    },
    recoveryCodes() {
      return [...server.state.recoveryCodes];
    },
    seedInvite: SEED_INVITE_CODE,
    seedHandle: SEED_HANDLE,
  };
  window.__monoptahMock = hooks;
  Object.defineProperty(window, MOCK_MARKER, { value: true, enumerable: false });
}
