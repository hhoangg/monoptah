// An in-memory stand-in for the `/api/desktops*`, `/api/devices*` and grant routes, for tests of the
// desktop screens. Owner: WS-M. The account mock (`features/accounts/mock`) serves everything else
// (`/api/me`, `/api/auth/reauth/*`); the fresh gate is imitated here: a fresh route answers
// `fresh_required` until the re-auth verify call went through.
import { createMockServer } from "@/features/accounts/mock/transport";
import type { ApiTransport } from "@/features/accounts/api";
import { base64UrlDecode } from "@/link/base64url";
import { fingerprint } from "@/link/fingerprint";
import type { DesktopSummary, DeviceSummary, Grant } from "./api";

export interface FakeServerOptions {
  desktops?: DesktopSummary[];
  devices?: DeviceSummary[];
  grants?: Record<string, Grant[]>;
  /** Directory public keys (base64url, 32 bytes) per desktop id, for the verify page. Others get a fixed short key. */
  publicKeys?: Record<string, string>;
  /** What the directory CLAIMS as `fingerprint` per desktop id (default: the real SHA-256 of the key). For tests of a lying directory. */
  fingerprintClaims?: Record<string, string>;
  /** Revoke and revoked-grant calls need a re-auth first. Default true (the real routes are fresh). */
  requireFresh?: boolean;
}

export interface FakeServer {
  transport: ApiTransport;
  desktops: DesktopSummary[];
  devices: DeviceSummary[];
  grants: Record<string, Grant[]>;
  calls: Array<{ method: string; path: string; body: unknown }>;
  /** Paths answered with 409 `conflict`. */
  conflicts: Set<string>;
  reauths: number;
  /** Tickets handed out by the selection-ticket route, in order. */
  tickets: string[];
}

export const FP_A = "a1b2c3d4e5f60718293a4b5c6d7e8f90a1b2c3d4e5f60718293a4b5c6d7e8f90";
export const FP_B = "0f1e2d3c4b5a69788796a5b4c3d2e1f00f1e2d3c4b5a69788796a5b4c3d2e1f0";

export function desktop(over: Partial<DesktopSummary> = {}): DesktopSummary {
  return { id: "d1", name: "Studio Mac", color: "#459cf7", online: true, pendingCount: 0, os: "macOS", appVersion: "1.4.0", lastSeenAt: null, ...over };
}

export function grant(over: Partial<Grant> = {}): Grant {
  return { deviceId: "dev1", deviceName: "Chrome on macOS", fingerprint: FP_A, state: "confirmed", permission: "read", requestedPermission: null, confirmedAt: 1_700_000_000, updatedAt: 1_700_000_000, ...over };
}

export function device(over: Partial<DeviceSummary> = {}): DeviceSummary {
  return { id: "dev1", name: "Chrome on macOS", fingerprint: FP_A, createdAt: 1_700_000_000, lastSeenAt: 1_700_000_100, ...over };
}

function json(status: number, body: unknown): Response {
  return new Response(JSON.stringify(body), { status, headers: { "Content-Type": "application/json" } });
}

function error(status: number, code: string, message = code): Response {
  return json(status, { error: message, code });
}

export function createFakeServer(options: FakeServerOptions = {}): FakeServer {
  const mock = createMockServer({ signedIn: true });
  const requireFresh = options.requireFresh ?? true;
  let fresh = false;
  let avatarUploads = 0;
  const server: FakeServer = {
    desktops: options.desktops ?? [],
    devices: options.devices ?? [],
    grants: options.grants ?? {},
    calls: [],
    conflicts: new Set(),
    reauths: 0,
    tickets: [],
    transport: async (url, init) => {
      const method = init.method ?? "GET";
      const path = url.split("?")[0] ?? url;
      const body: unknown = typeof init.body === "string" ? JSON.parse(init.body) : undefined;
      const isMine = /^\/api\/(desktops|devices)(\/|$)/.test(path);
      if (path === "/api/auth/reauth/verify") {
        const response = await mock.transport(url, init);
        if (response.ok) {
          fresh = true;
          server.reauths += 1;
        }
        return response;
      }
      if (!isMine) return mock.transport(url, init);
      server.calls.push({ method, path, body });
      if (server.conflicts.has(`${method} ${path}`)) return error(409, "conflict");
      const parts = path.split("/").filter(Boolean);
      const gate = (): Response | null => (requireFresh && !fresh ? error(403, "fresh_required") : null);

      if (parts[1] === "devices") {
        if (method === "GET" && parts.length === 2) return json(200, { devices: server.devices });
        if (method === "DELETE" && parts.length === 3) {
          const id = parts[2];
          if (!server.devices.some((item) => item.id === id)) return error(404, "not_found");
          server.devices = server.devices.filter((item) => item.id !== id);
          for (const key of Object.keys(server.grants)) server.grants[key] = (server.grants[key] ?? []).map((item) => (item.deviceId === id ? { ...item, state: "revoked" as const } : item));
          return json(200, { ok: true });
        }
        return error(404, "not_found");
      }

      const id = parts[2] ?? "";
      if (parts.length === 2 && method === "GET") return json(200, { desktops: server.desktops });
      const target = server.desktops.find((item) => item.id === id);
      if (!target) return error(404, "not_found");
      if (parts.length === 3) {
        if (method === "GET") {
          const key = options.publicKeys?.[id];
          return json(200, { ...target, publicKey: key ?? "cHVibGljLWtleQ", fingerprint: options.fingerprintClaims?.[id] ?? (key ? fingerprint(base64UrlDecode(key)) : FP_B) });
        }
        if (method === "PATCH") {
          const patch = body as { name?: string; color?: string | null; avatar?: { kind: "emoji"; emoji: string } | { kind: "image"; data: string } | null };
          // Like the real server: an emoji is kept, an image becomes a new random-looking token per upload (here a counter) the page builds its URL from, null clears.
          const avatar = patch.avatar === undefined ? {} : { avatar: patch.avatar === null ? null : patch.avatar.kind === "emoji" ? patch.avatar : { kind: "image" as const, hash: (avatarUploads += 1).toString(16).padStart(16, "0") } };
          Object.assign(target, patch.name !== undefined ? { name: patch.name.trim() } : {}, patch.color !== undefined ? { color: patch.color } : {}, avatar);
          return json(200, { desktop: target });
        }
        if (method === "DELETE") {
          const blocked = gate();
          if (blocked) return blocked;
          server.desktops = server.desktops.filter((item) => item.id !== id);
          return json(200, { ok: true });
        }
      }
      if (parts[3] === "grants") {
        const list = (server.grants[id] ??= []);
        if (parts.length === 4 && method === "GET") return json(200, { grants: list });
        if (parts.length === 4 && method === "POST") {
          const deviceId = (body as { deviceId: string }).deviceId;
          const existing = list.find((item) => item.deviceId === deviceId);
          // Like the real server: a revoked grant answers 409 (grants/handlers.ts).
          if (existing?.state === "revoked") return error(409, "conflict");
          if (existing) return json(200, { grant: existing });
          const created = grant({ deviceId, state: "pending", permission: null, confirmedAt: null });
          list.push(created);
          return json(200, { grant: created });
        }
        if (parts.length === 6 && parts[5] === "selection-ticket" && method === "POST") {
          // Like the real route: fresh, and only for a confirmed Control grant.
          const blocked = gate();
          if (blocked) return blocked;
          const current = list.find((item) => item.deviceId === parts[4]);
          if (!current || current.state !== "confirmed" || current.permission !== "control") return error(403, "forbidden");
          const ticket = `ticket-${server.tickets.length + 1}`;
          server.tickets.push(ticket);
          return json(200, { ticket, expiresAt: 1_900_000_000 });
        }
        if (parts.length === 5 && method === "PUT") {
          const current = list.find((item) => item.deviceId === parts[4]);
          if (!current) return error(404, "not_found");
          const put = body as { requestedPermission?: "read" | "control"; state?: "revoked" };
          // Like the real server: changing the requested permission of a revoked grant answers 409.
          if (put.requestedPermission && current.state === "revoked") return error(409, "conflict");
          if (put.state === "revoked") {
            const blocked = gate();
            if (blocked) return blocked;
            current.state = "revoked";
          }
          if (put.requestedPermission) current.requestedPermission = put.requestedPermission;
          return json(200, { grant: current });
        }
      }
      return error(404, "not_found");
    },
  };
  return server;
}
