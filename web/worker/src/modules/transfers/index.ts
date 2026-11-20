// Encrypted file transfers through R2 (docs/features.md F-60a, security.md 7 "File transfers"). The desktop seals a
// file with a random key that never leaves the end-to-end channel, then stores the ciphertext here under
// `transfers/<desktopId>/<contentHash>.<keyId>`; a device paired with that desktop downloads it and decrypts it in the
// browser. The Worker only moves ciphertext: it cannot read a file and it never logs a body, a key or an object name.
//
//   HEAD /api/desktop/transfers/:object   desktop token (scope relay): is the object there, and how big
//   PUT  /api/desktop/transfers/:object   desktop token (scope relay): store it (an existing object is never replaced)
//   GET  /api/transfers/:desktopId/:object?device=ID   session cookie + a confirmed grant of the device: the bytes, with Range
//
// These routes are not in the JSON route registry (architecture.md 4.2): their bodies are binary and larger than the
// 32 KiB JSON limit, so they run their own checks. Nothing here lists a bucket or takes a path: the object name is a
// hash and a key id, and the prefix is the authenticated desktop's own id.
import { Hono, type Handler } from "hono";
import { z } from "zod";
import type { ApiRoute } from "@monoptah/protocol";
import { apiError, type AppContext, type AppEnv } from "../../http.ts";
import { authResolver, requireAuth } from "../../middleware/auth.ts";
import { ignoreCookies } from "../../middleware/browser.ts";
import { requireDesktopToken } from "../desktops/token.ts";
import { roomOf } from "../../relay/rooms.ts";

/** Largest plaintext a desktop seals (crates/monocode-core file_transfer::crypto::MAX_PLAINTEXT_BYTES). */
export const MAX_PLAINTEXT_BYTES = 64 * 1024 * 1024;
/** One AES-GCM chunk of plaintext (CHUNK_BYTES) and its tag. */
export const CHUNK_BYTES = 1024 * 1024;
export const TAG_BYTES = 16;
/** The sealed size of the largest plaintext: every chunk carries one tag. */
export const MAX_SEALED_BYTES = MAX_PLAINTEXT_BYTES + Math.ceil(MAX_PLAINTEXT_BYTES / CHUNK_BYTES) * TAG_BYTES;
/** The smallest sealed object: an empty file is one bare tag. */
export const MIN_SEALED_BYTES = TAG_BYTES;

/** `<64 hex SHA-256 of the content>.<16 hex key id>`: the only text that names an object. */
export const OBJECT_NAME = /^[0-9a-f]{64}\.[0-9a-f]{16}$/;
const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

/** The R2 key of an object: the prefix the lifecycle rule expires (docs/deploy-runbook.md), the desktop and the name. */
export function objectKey(desktopId: string, object: string): string {
  return `transfers/${desktopId}/${object}`;
}

const objectParam = z.string().regex(OBJECT_NAME);

/** The sealed size a plaintext of `plain` bytes has, as the desktop computes it. */
export function sealedSize(plain: number): number {
  return plain + Math.max(1, Math.ceil(plain / CHUNK_BYTES)) * TAG_BYTES;
}

function declaredLength(c: AppContext): number | null {
  const text = c.req.header("Content-Length");
  if (text === undefined || !/^\d{1,12}$/.test(text)) return null;
  return Number(text);
}

const head: Handler<AppEnv> = async (c) => {
  if (c.req.method !== "HEAD") return apiError(c, 405, "invalid", "Method not allowed");
  const auth = await requireDesktopToken(c, "relay");
  if (auth instanceof Response) return auth;
  const object = objectParam.safeParse(c.req.param("object"));
  if (!object.success) return apiError(c, 400, "invalid", "Invalid object name");
  const found = await c.env.TRANSFERS.head(objectKey(auth.desktopId, object.data));
  if (!found) return new Response(null, { status: 404, headers: { "Cache-Control": "no-store" } });
  return new Response(null, { status: 200, headers: { "Content-Length": String(found.size), "Cache-Control": "no-store" } });
};

const put: Handler<AppEnv> = async (c) => {
  const auth = await requireDesktopToken(c, "relay");
  if (auth instanceof Response) return auth;
  const object = objectParam.safeParse(c.req.param("object"));
  if (!object.success) return apiError(c, 400, "invalid", "Invalid object name");
  const length = declaredLength(c);
  if (length === null) return apiError(c, 411, "invalid", "Content-Length required");
  if (length > MAX_SEALED_BYTES) return apiError(c, 413, "invalid", "File too large");
  if (length < MIN_SEALED_BYTES || c.req.raw.body === null) return apiError(c, 400, "invalid", "Invalid object");
  const key = objectKey(auth.desktopId, object.data);
  try {
    // `etagDoesNotMatch: "*"` stores only when no object has this key: an object is never replaced, so a name always
    // means the same bytes (the desktop uploads again under a new key id when it lost the key).
    const stored = await c.env.TRANSFERS.put(key, c.req.raw.body, {
      onlyIf: { etagDoesNotMatch: "*" },
      httpMetadata: { contentType: "application/octet-stream" },
    });
    if (stored === null) return c.json({ ok: true, stored: false });
    if (stored.size !== length) {
      await c.env.TRANSFERS.delete(key);
      return apiError(c, 400, "invalid", "Body does not match Content-Length");
    }
    return c.json({ ok: true, stored: true }, 201);
  } catch {
    // The body ended early or the connection dropped: nothing was stored. No detail: it could echo input.
    return apiError(c, 400, "invalid", "Upload failed");
  }
};

/** The device of a download: its id from the query, and the desktop it is confirmed for. */
const DEVICE_GRANT_SQL =
  "SELECT g.state AS state FROM device_grants g JOIN devices v ON v.id = g.device_id JOIN desktops d ON d.id = g.desktop_id WHERE g.device_id = ? AND g.desktop_id = ? AND g.state = 'confirmed' AND v.user_id = ? AND v.revoked_at IS NULL AND d.user_id = ? AND d.revoked_at IS NULL";

/** `bytes=a-b` or `bytes=a-` only. Anything else is read as no range. */
export function parseRange(header: string | undefined, size: number): { offset: number; length: number } | "unsatisfiable" | null {
  const match = /^bytes=(\d{1,12})-(\d{0,12})$/.exec(header ?? "");
  if (!match) return null;
  const start = Number(match[1]);
  const end = match[2] === "" ? size - 1 : Math.min(Number(match[2]), size - 1);
  if (start >= size || end < start) return "unsatisfiable";
  return { offset: start, length: end - start + 1 };
}

const download: Handler<AppEnv> = async (c) => {
  const { user, session } = requireAuth(c);
  const desktopId = c.req.param("desktopId") ?? "";
  const object = objectParam.safeParse(c.req.param("object"));
  const deviceId = c.req.query("device") ?? "";
  if (!UUID.test(desktopId) || !UUID.test(deviceId) || !object.success) return apiError(c, 400, "invalid", "Invalid request");
  const grant = await c.env.DB.prepare(DEVICE_GRANT_SQL).bind(deviceId, desktopId, user.id, user.id).first();
  // One answer for every refusal: the caller learns nothing about whose desktop it is or which objects exist.
  if (!grant) return apiError(c, 404, "not_found", "Not found");
  // The device id in the URL must be the one whose link this session holds: when the device has a live relay socket,
  // it is bound to a session, and another session of the same user cannot borrow its id. No live link: the grant decides.
  const holders = await roomOf(c.env, desktopId).deviceSessions(deviceId).catch(() => [] as string[]);
  if (holders.length > 0 && !holders.includes(session.idHash)) return apiError(c, 404, "not_found", "Not found");
  const key = objectKey(desktopId, object.data);
  const meta = await c.env.TRANSFERS.head(key);
  if (!meta) return apiError(c, 404, "not_found", "Not found");
  const range = parseRange(c.req.header("Range"), meta.size);
  if (range === "unsatisfiable") return new Response(null, { status: 416, headers: { "Content-Range": `bytes */${meta.size}`, "Cache-Control": "no-store" } });
  const body = await c.env.TRANSFERS.get(key, range ? { range } : undefined);
  if (!body) return apiError(c, 404, "not_found", "Not found");
  const headers: Record<string, string> = {
    "Content-Type": "application/octet-stream",
    "Content-Length": String(range ? range.length : body.size),
    "Accept-Ranges": "bytes",
    // Ciphertext only, and tied to a session: never stored by a cache.
    "Cache-Control": "no-store",
    "X-Content-Type-Options": "nosniff",
    // Ciphertext is never a page: a direct navigation downloads it, and a document made of it could not run anything.
    "Content-Disposition": 'attachment; filename="transfer.bin"',
    "Content-Security-Policy": "sandbox",
  };
  if (range) headers["Content-Range"] = `bytes ${range.offset}-${range.offset + range.length - 1}/${body.size}`;
  return new Response(body.body, { status: range ? 206 : 200, headers });
};

const sessionRoute: ApiRoute = { method: "GET", path: "/api/transfers/:desktopId/:object", auth: "session", module: "relay", request: {}, response: z.null() };

export const routes = new Hono<AppEnv>();
// Token routes never read a cookie (architecture.md 4.1).
// Hono answers a HEAD request through the GET handler of the same path, so `head` is registered as a GET route and
// refuses a real GET.
routes.get("/api/desktop/transfers/:object", ignoreCookies, head);
routes.put("/api/desktop/transfers/:object", ignoreCookies, put);
routes.get("/api/transfers/:desktopId/:object", authResolver(sessionRoute), download);
