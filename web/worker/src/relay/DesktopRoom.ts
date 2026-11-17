import { DurableObject } from "cloudflare:workers";
import type { Env } from "../env.ts";

/** What `revoke` closes (architecture.md 4.4): one device, one cookie session, or every socket. */
export type RevokeTarget = { deviceId: string } | { sessionIdHash: string } | { all: true };

/**
 * One room per desktop (`env.ROOM.idFromName(desktopId)`), SQLite-backed, WebSocket Hibernation API.
 * STUB (A-01): only the RPC surface that WS-A handlers call exists. WS-R (R-02) replaces the bodies:
 * the join, frame routing, quotas and presence of architecture.md 4.4.
 *
 * Rules that hold for every later change (capacity.md 1): no timers, no alarms, no outbound sockets, and a
 * constructor that does nothing beyond the bindings, so an idle room hibernates and costs no duration.
 */
export class DesktopRoom extends DurableObject<Env> {
  /** Closes the matching sockets with `code` (4001, 4003 or 4011). No-op until R-02. */
  async revoke(_target: RevokeTarget, _code: number): Promise<void> {}

  /** Whether the desktop socket is live. Always offline until R-02. */
  async presence(): Promise<{ online: boolean }> {
    return { online: false };
  }
}
