// Every module of workstreams.md 3.1 plus `health` and `me` from the protocol registry. A new module is a
// one-line registration here (WS-A).
import type { Hono } from "hono";
import type { AppEnv } from "../http.ts";
import type { Cleanup } from "./cleanup.ts";
import * as adminCleanup from "./admin/cleanup.ts";
import * as auditCleanup from "./audit/cleanup.ts";
import * as authCleanup from "./auth/cleanup.ts";
import * as desktopsCleanup from "./desktops/cleanup.ts";
import * as devicesCleanup from "./devices/cleanup.ts";
import * as grantsCleanup from "./grants/cleanup.ts";
import * as healthCleanup from "./health/cleanup.ts";
import * as invitesCleanup from "./invites/cleanup.ts";
import * as meCleanup from "./me/cleanup.ts";
import * as pairingCleanup from "./pairing/cleanup.ts";
import * as passkeysCleanup from "./passkeys/cleanup.ts";
import * as pushCleanup from "./push/cleanup.ts";
import * as ratelimitCleanup from "./ratelimit/cleanup.ts";
import * as recoveryCleanup from "./recovery/cleanup.ts";
import * as relayCleanup from "./relay/cleanup.ts";
import * as sessionsCleanup from "./sessions/cleanup.ts";
import { routes as admin } from "./admin/index.ts";
import { routes as audit } from "./audit/index.ts";
import { routes as auth } from "./auth/index.ts";
import { routes as desktops } from "./desktops/index.ts";
import { routes as devices } from "./devices/index.ts";
import { routes as grants } from "./grants/index.ts";
import { routes as health } from "./health/index.ts";
import { routes as invites } from "./invites/index.ts";
import { routes as me } from "./me/index.ts";
import { routes as pairing } from "./pairing/index.ts";
import { routes as passkeys } from "./passkeys/index.ts";
import { routes as push } from "./push/index.ts";
import { routes as recovery } from "./recovery/index.ts";
import { routes as relay } from "./relay/index.ts";
import { routes as sessions } from "./sessions/index.ts";

export interface ModuleEntry {
  /** Absent for `ratelimit`, which is middleware and a table, not routes. */
  routes?: Hono<AppEnv>;
  cleanup: Cleanup;
}

export const MODULES = {
  health: { routes: health, cleanup: healthCleanup.cleanup },
  auth: { routes: auth, cleanup: authCleanup.cleanup },
  me: { routes: me, cleanup: meCleanup.cleanup },
  audit: { routes: audit, cleanup: auditCleanup.cleanup },
  sessions: { routes: sessions, cleanup: sessionsCleanup.cleanup },
  passkeys: { routes: passkeys, cleanup: passkeysCleanup.cleanup },
  recovery: { routes: recovery, cleanup: recoveryCleanup.cleanup },
  invites: { routes: invites, cleanup: invitesCleanup.cleanup },
  admin: { routes: admin, cleanup: adminCleanup.cleanup },
  ratelimit: { cleanup: ratelimitCleanup.cleanup },
  pairing: { routes: pairing, cleanup: pairingCleanup.cleanup },
  desktops: { routes: desktops, cleanup: desktopsCleanup.cleanup },
  devices: { routes: devices, cleanup: devicesCleanup.cleanup },
  grants: { routes: grants, cleanup: grantsCleanup.cleanup },
  push: { routes: push, cleanup: pushCleanup.cleanup },
  relay: { routes: relay, cleanup: relayCleanup.cleanup },
} as const satisfies Record<string, ModuleEntry>;

export type ModuleName = keyof typeof MODULES;
