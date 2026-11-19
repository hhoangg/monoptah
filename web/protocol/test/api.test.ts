import { describe, expect, it } from "vitest";
import { z } from "zod";
import {
  AckRecoveryBody,
  API_AUTH,
  API_MODULES,
  API_ROUTES,
  ApiErrorSchema,
  CreateInviteResponse,
  HealthResponse,
  MAX_INVITE_INPUT,
  MAX_RECOVERY_CODE_INPUT,
  MAX_TRANSPORT_INPUT,
  MAX_TRANSPORTS_INPUT,
  LogoutBody,
  MeResponse,
  PairPollResponse,
  PairStartBody,
  PushNotifyBody,
  PushPayloadSchema,
  PushPrefsStateResponse,
  PushTestBody,
  PushTestResponse,
  RecoverBody,
  RegisterOptionsBody,
  RegisterVerifyBody,
  RegisterVerifyResponse,
  findRoute,
  routesOfModule,
  type ApiAuth,
} from "../src/index.ts";
import { readText } from "./helpers.ts";

interface DocRow {
  routes: { method: string; path: string }[];
  auth: ApiAuth;
}

/** Parses the route table of architecture.md section 4.2. */
function parseRouteTable(): DocRow[] {
  const doc = readText("../../docs/architecture.md");
  const start = doc.indexOf("### 4.2 Hono route table");
  const end = doc.indexOf("### 4.3 D1 schema");
  expect(start).toBeGreaterThan(-1);
  expect(end).toBeGreaterThan(start);
  const rows: DocRow[] = [];
  for (const line of doc.slice(start, end).split("\n")) {
    if (!line.startsWith("| `")) continue;
    const cells = line.split("|").map((cell) => cell.trim());
    const routes = [...(cells[1] ?? "").matchAll(/`(GET|POST|PUT|PATCH|DELETE) ([^`]+)`/g)].map((m) => ({
      method: m[1] as string,
      path: (m[2] as string).split("?")[0] as string,
    }));
    expect(routes.length, `no route in: ${line}`).toBeGreaterThan(0);
    rows.push({ routes, auth: authOf(cells[2] ?? "") });
  }
  return rows;
}

function authOf(cell: string): ApiAuth {
  const token = /^token `(\w+)`/.exec(cell);
  if (token) return `token:${token[1]}` as ApiAuth;
  if (cell.startsWith("session + device grant")) return "session+grant";
  if (cell.startsWith("public")) return "public";
  if (cell.startsWith("fresh")) return "fresh";
  if (cell.startsWith("admin")) return "admin";
  if (cell.startsWith("session")) return "session";
  throw new Error(`unrecognised auth cell: ${cell}`);
}

const key = (route: { method: string; path: string }) => `${route.method} ${route.path}`;

describe("api.ts and the route table of architecture.md 4.2", () => {
  const docRoutes = parseRouteTable().flatMap((row) => row.routes);

  it("finds the documented rows (guards the parser)", () => {
    expect(docRoutes.length).toBeGreaterThanOrEqual(55);
    expect(docRoutes.map(key)).toContain("GET /api/sessions");
    expect(docRoutes.map(key)).toContain("DELETE /api/sessions/:id");
    expect(docRoutes.map(key)).toContain("DELETE /api/desktop/me");
    expect(docRoutes.map(key)).toContain("GET /relay/device/:desktopId");
  });

  it("has every documented route", () => {
    const registry = new Set(API_ROUTES.map(key));
    expect(docRoutes.map(key).filter((k) => !registry.has(k))).toEqual([]);
  });

  it("has no route that is not documented", () => {
    const documented = new Set(docRoutes.map(key));
    expect(API_ROUTES.map(key).filter((k) => !documented.has(k))).toEqual([]);
  });

  it("lists each route once", () => {
    const keys = API_ROUTES.map(key);
    expect(new Set(keys).size).toBe(keys.length);
    expect(keys.length).toBe(docRoutes.length);
  });

  it("agrees with the auth column", () => {
    const authByKey = new Map(parseRouteTable().flatMap((row) => row.routes.map((route) => [key(route), row.auth] as const)));
    for (const route of API_ROUTES) {
      expect(route.auth, key(route)).toBe(authByKey.get(key(route)));
    }
  });
});

describe("route registry shape", () => {
  it("uses known auth values and modules", () => {
    for (const route of API_ROUTES) {
      expect(API_AUTH).toContain(route.auth);
      expect(API_MODULES).toContain(route.module);
    }
  });

  it("declares a params schema exactly for the path parameters", () => {
    for (const route of API_ROUTES) {
      const named = [...route.path.matchAll(/:(\w+)/g)].map((m) => m[1] as string).sort();
      const shape = route.request.params instanceof z.ZodObject ? Object.keys(route.request.params.shape).sort() : [];
      expect(shape, key(route)).toEqual(named);
    }
  });

  it("gives every module at least one route and findRoute works", () => {
    for (const module of API_MODULES) expect(routesOfModule(module).length, module).toBeGreaterThan(0);
    expect(findRoute("GET", "/api/health")?.module).toBe("health");
    expect(findRoute("GET", "/nope")).toBeUndefined();
  });

  it("marks the two relay routes as upgrades", () => {
    expect(API_ROUTES.filter((route) => route.upgrade).map(key).sort()).toEqual(["GET /relay/desktop/:desktopId", "GET /relay/device/:desktopId"]);
  });

  it("only token routes use a token scope", () => {
    for (const route of API_ROUTES) {
      if (route.path.startsWith("/api/desktop/") || route.path === "/api/push/notify" || route.path === "/relay/desktop/:desktopId") {
        expect(route.auth.startsWith("token:"), key(route)).toBe(true);
      }
    }
  });
});

describe("schemas", () => {
  it("parses health and the generic error", () => {
    expect(HealthResponse.parse({ ok: true })).toEqual({ ok: true });
    expect(ApiErrorSchema.parse({ error: "Not available yet", code: "unsupported" }).code).toBe("unsupported");
  });

  it("pair poll covers every status, and approved carries the token", () => {
    for (const status of ["pending", "slow_down", "denied", "expired"]) expect(PairPollResponse.safeParse({ status }).success).toBe(true);
    const approved = { status: "approved", desktopId: "d1", desktopToken: "token-value-for-tests", account: { handle: "alice" }, relayUrl: "wss://example.test" };
    expect(PairPollResponse.parse(approved)).toEqual(approved);
    expect(PairPollResponse.safeParse({ status: "approved" }).success).toBe(false);
    expect(PairPollResponse.safeParse({ status: "done" }).success).toBe(false);
  });

  it("pair start validates its fields", () => {
    expect(PairStartBody.safeParse({ publicKey: "AAAA", machineName: "Studio Mac", os: "macos", appVersion: "1.0.0", challenge: "BBBB" }).success).toBe(true);
    expect(PairStartBody.safeParse({ publicKey: "AAAA" }).success).toBe(false);
  });

  it("registration returns exactly 10 recovery codes", () => {
    const user = { id: "u1", handle: "alice", displayName: "Alice", role: "user" };
    expect(RegisterVerifyResponse.safeParse({ user, recoveryCodes: Array(10).fill("abcd-abcd-abcd-abcd"), batch: 1 }).success).toBe(true);
    expect(RegisterVerifyResponse.safeParse({ user, recoveryCodes: ["abcd"], batch: 1 }).success).toBe(false);
    expect(CreateInviteResponse.shape.code).toBeDefined();
  });

  it("push notify carries no content, and the payload stays small", () => {
    const body = { category: "agentFinished", project: "app", status: "finished", sessionId: "s1", deviceIds: ["dev1"] };
    expect(PushNotifyBody.safeParse(body).success).toBe(true);
    expect(PushNotifyBody.safeParse({ ...body, category: "everything" }).success).toBe(false);
    const payload = { v: 1, t: "Studio Mac", b: "app: Finished", url: "/d/d1/s/s1", tag: "s1" };
    expect(PushPayloadSchema.parse(payload)).toEqual(payload);
  });

  it("push notify accepts a Mono target without a session id (the Worker enforces the id rules)", () => {
    const mono = { category: "monoMessages", project: "Ada", status: "message", monoId: "m1", deviceIds: ["dev1"] };
    expect(PushNotifyBody.safeParse(mono).success).toBe(true);
    expect(PushNotifyBody.safeParse({ ...mono, sessionId: "s1" }).success).toBe(true);
  });

  it("push prefs state, push test and their routes", () => {
    const all = { agentFinished: true, agentInput: true, monoMessages: true };
    expect(PushPrefsStateResponse.parse({ enabled: true, categories: all, stored: false })).toEqual({ enabled: true, categories: all, stored: false });
    expect(PushPrefsStateResponse.safeParse({ enabled: true, categories: all }).success).toBe(false);
    expect(PushTestBody.safeParse({ deviceId: "d1" }).success).toBe(true);
    expect(PushTestBody.safeParse({}).success).toBe(false);
    expect(PushTestResponse.safeParse({ sent: 1, results: [{ outcome: "ok", status: 201 }, { outcome: "failed", status: 0 }] }).success).toBe(true);
    expect(PushTestResponse.safeParse({ sent: 0, results: [{ outcome: "weird", status: 0 }] }).success).toBe(false);
    expect(findRoute("GET", "/api/push/prefs/:desktopId")).toMatchObject({ auth: "session", owner: true, module: "push" });
    expect(findRoute("POST", "/api/push/test")).toMatchObject({ auth: "session", rateLimit: "push-test", module: "push" });
  });
});

describe("rate limit groups (security.md 3.5)", () => {
  it("names the group of every limited route as the 3.5 table does", () => {
    const groups = Object.fromEntries(API_ROUTES.filter((route) => route.rateLimit).map((route) => [`${route.method} ${route.path}`, route.rateLimit]));
    expect(groups).toEqual({
      "POST /api/auth/register/options": "register",
      "POST /api/auth/register/verify": "register",
      "POST /api/auth/login/options": "login",
      "POST /api/auth/login/verify": "login",
      "POST /api/auth/recover": "recover",
      "POST /api/pair/start": "pair-start",
      "POST /api/pair/poll": "pair-poll",
      "POST /api/push/notify": "push-notify",
      "POST /api/push/test": "push-test",
    });
  });
});

describe("outer input bounds and the Me contract (fix-up point 8)", () => {
  it("bounds the recovery code, the invite and the reported transports", () => {
    expect(RecoverBody.safeParse({ handle: "bob-1", code: "A".repeat(MAX_RECOVERY_CODE_INPUT) }).success).toBe(true);
    expect(RecoverBody.safeParse({ handle: "bob-1", code: "A".repeat(MAX_RECOVERY_CODE_INPUT + 1) }).success).toBe(false);
    const options = { handle: "bob-1", displayName: "Bob" };
    expect(RegisterOptionsBody.safeParse({ ...options, invite: "i".repeat(MAX_INVITE_INPUT) }).success).toBe(true);
    expect(RegisterOptionsBody.safeParse({ ...options, invite: "i".repeat(MAX_INVITE_INPUT + 1) }).success).toBe(false);
    const withTransports = (transports: string[]) => RegisterVerifyBody.safeParse({ challengeId: "c", response: { id: "a", type: "public-key", response: { transports } } });
    expect(withTransports(Array.from({ length: MAX_TRANSPORTS_INPUT }, (_, i) => `t${i}`)).success).toBe(true);
    expect(withTransports(Array.from({ length: MAX_TRANSPORTS_INPUT + 1 }, (_, i) => `t${i}`)).success).toBe(false);
    expect(withTransports(["x".repeat(MAX_TRANSPORT_INPUT + 1)]).success).toBe(false);
  });

  it("MeResponse carries the unused recovery code count", () => {
    const me = { user: { id: "u", handle: "bob-1", displayName: "Bob", role: "user" }, invitesLeft: 3, recoveryOnly: false, recoveryAckAt: null };
    expect(MeResponse.safeParse(me).success).toBe(false);
    expect(MeResponse.safeParse({ ...me, recoveryCodesLeft: 10 }).success).toBe(true);
    expect(MeResponse.safeParse({ ...me, recoveryCodesLeft: -1 }).success).toBe(false);
  });

  it("the ack and logout bodies are bounded", () => {
    expect(AckRecoveryBody.safeParse({ batch: 2 }).success).toBe(true);
    expect(AckRecoveryBody.safeParse({}).success).toBe(false);
    expect(AckRecoveryBody.safeParse({ batch: 0 }).success).toBe(false);
    expect(LogoutBody.safeParse(undefined).success).toBe(true);
    expect(LogoutBody.safeParse({ deviceId: "d".repeat(64) }).success).toBe(true);
    expect(LogoutBody.safeParse({ deviceId: "d".repeat(65) }).success).toBe(false);
  });

  it("registers the ack route as a session route and the device forget as a fresh one", () => {
    expect(findRoute("POST", "/api/recovery-codes/ack")?.auth).toBe("session");
    expect(findRoute("DELETE", "/api/devices/:id")?.auth).toBe("fresh");
  });
});
