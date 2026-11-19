import { describe, expect, it } from "vitest";
import {
  API_ERROR_CODES,
  ApiErrorSchema,
  CLOSE_CODES,
  CloseCodes,
  DESKTOP_ERROR_CODES,
  EnvelopeRequestSchema,
  KNOWN_CAPABILITIES,
  RELAY_ERROR_CODES,
  StateMessageSchema,
  closeEntry,
  closeReaction,
  decodeEnvelopeJson,
  encodeEnvelope,
  hasCapability,
  knownErrorCode,
  parseDesktopMessage,
  parseDeviceMessage,
  responseKind,
  EnvelopeResponseSchema,
} from "../src/index.ts";
import { vectors } from "./helpers.ts";

describe("close codes (architecture.md 4.5)", () => {
  it("lists exactly the 11 documented codes", () => {
    expect(CLOSE_CODES.map((entry) => entry.code)).toEqual([1000, 1001, 1008, 1009, 4001, 4003, 4008, 4009, 4010, 4011, 4012]);
    expect(Object.values(CloseCodes).sort()).toEqual(CLOSE_CODES.map((entry) => entry.code).sort());
  });
  it("matches the committed vectors", () => {
    expect(vectors.close).toEqual(JSON.parse(JSON.stringify(CLOSE_CODES)));
  });
  it("device reactions", () => {
    expect(closeReaction(1000).action).toBe("reconnect_backoff");
    expect(closeReaction(1001).action).toBe("reconnect_once");
    expect(closeReaction(1008).action).toBe("stop");
    expect(closeReaction(1009).action).toBe("stop");
    expect(closeReaction(4001).action).toBe("login");
    expect(closeReaction(4003)).toEqual({ action: "stop", message: "Access revoked" });
    expect(closeReaction(4008).action).toBe("backoff_60s");
    expect(closeReaction(4009).action).toBe("retry_once");
    expect(closeReaction(4010).action).toBe("backoff_60s");
    expect(closeReaction(4011).action).toBe("stop");
    expect(closeReaction(4012).action).toBe("reconnect_new_handshake");
  });
  it("desktop stops and shows the pair-again message on 4001, 4003 and 4011", () => {
    for (const code of [4001, 4003, 4011]) {
      expect(closeReaction(code, "desktop")).toEqual({ action: "stop", message: "Web app: access revoked, pair again" });
    }
    expect(closeReaction(4012, "desktop").action).toBe("reconnect_new_handshake");
  });
  it("treats an unlisted code (1006) as a plain reconnect", () => {
    expect(closeEntry(1006)).toBeUndefined();
    expect(closeReaction(1006).action).toBe("reconnect_backoff");
  });
});

describe("error codes", () => {
  it("has every desktop code of architecture.md 5.5 and workstreams.md 2.1", () => {
    expect([...DESKTOP_ERROR_CODES].sort()).toEqual(
      [
        "busy",
        "forbidden",
        "read_only",
        "not_selected",
        "path_denied",
        "not_found",
        "revoked",
        "disabled",
        "too_large",
        "unsupported",
        "unsupported_target",
        "starting",
        "auto_stopping",
        "instance_busy",
        "not_pending",
        "needs_desktop",
        "wake_required",
      ].sort(),
    );
  });
  it("has the API codes the spec names", () => {
    for (const code of ["unsupported", "quota_exceeded", "invites_paused", "unauthorized", "forbidden", "csrf", "bad_origin", "rate_limited", "not_found", "invalid"]) {
      expect(API_ERROR_CODES).toContain(code);
    }
    expect(RELAY_ERROR_CODES).toContain("desktop_offline");
  });
  it("matches the committed vectors", () => {
    expect(vectors.errors.desktop).toEqual([...DESKTOP_ERROR_CODES].sort());
    expect(vectors.errors.api).toEqual([...API_ERROR_CODES].sort());
    expect(vectors.errors.relay).toEqual([...RELAY_ERROR_CODES].sort());
  });
  it("parses the API error shape", () => {
    expect(ApiErrorSchema.parse({ error: "Not available yet", code: "unsupported" })).toEqual({ error: "Not available yet", code: "unsupported" });
    expect(() => ApiErrorSchema.parse({ error: "x", code: "made_up" })).toThrow();
  });
});

describe("state message", () => {
  const confirmed = {
    type: "state",
    state: "confirmed",
    permission: "control",
    proto: 1,
    capabilities: ["approvals", "cloud", "cloudStart", "cloudMove", "gitPush", "monoTools", "monoFiles", "mentionedFiles"],
    desktop: { name: "Studio Mac", version: "1.4.0" },
  };
  it("parses a confirmed message", () => {
    const parsed = StateMessageSchema.parse(confirmed);
    expect(parsed.permission).toBe("control");
    for (const name of KNOWN_CAPABILITIES) expect(hasCapability(parsed, name)).toBe(true);
  });
  it("accepts unknown capabilities and unknown keys", () => {
    const parsed = StateMessageSchema.parse({ ...confirmed, capabilities: ["cloud", "teleport"], extra: 1 });
    expect(hasCapability(parsed, "teleport")).toBe(true);
    expect(hasCapability(parsed, "approvals")).toBe(false);
    expect(parsed.extra).toBe(1);
  });
  it("accepts the bare pending and revoked messages", () => {
    expect(StateMessageSchema.parse({ type: "state", state: "pending" }).state).toBe("pending");
    expect(StateMessageSchema.parse({ type: "state", state: "revoked" }).state).toBe("revoked");
  });
  it("rejects a bad state or permission", () => {
    expect(() => StateMessageSchema.parse({ ...confirmed, state: "maybe" })).toThrow();
    expect(() => StateMessageSchema.parse({ ...confirmed, permission: "root" })).toThrow();
    expect(() => StateMessageSchema.parse({ ...confirmed, type: "status" })).toThrow();
  });
});

describe("envelope (architecture.md 5.5)", () => {
  it("accepts the documented request, watch open and cancel", () => {
    const request = { version: 1, rid: 7, id: "cmd-1", method: "conversation.send", params: { conversationId: "c", text: "hi" } } as const;
    expect(parseDeviceMessage(encodeEnvelope(request))).toEqual(request);
    const watch = { version: 1, rid: 8, id: "w", method: "conversation.watch", params: { conversationId: "c", tail: 3, incremental: true }, watch: true } as const;
    expect(parseDeviceMessage(encodeEnvelope(watch))).toEqual(watch);
    expect(parseDeviceMessage(encodeEnvelope({ rid: 8, cancel: true }))).toEqual({ rid: 8, cancel: true });
  });
  it("enforces id length 128, version 1 and u32 rid", () => {
    const base = { version: 1, rid: 1, id: "a", method: "m", params: {} };
    expect(EnvelopeRequestSchema.safeParse({ ...base, id: "a".repeat(128) }).success).toBe(true);
    expect(EnvelopeRequestSchema.safeParse({ ...base, id: "a".repeat(129) }).success).toBe(false);
    expect(EnvelopeRequestSchema.safeParse({ ...base, id: "" }).success).toBe(false);
    expect(EnvelopeRequestSchema.safeParse({ ...base, version: 2 }).success).toBe(false);
    expect(EnvelopeRequestSchema.safeParse({ ...base, rid: 2 ** 32 }).success).toBe(false);
    expect(EnvelopeRequestSchema.safeParse({ ...base, rid: 2 ** 32 - 1 }).success).toBe(true);
    expect(EnvelopeRequestSchema.safeParse({ ...base, rid: -1 }).success).toBe(false);
  });
  it("classifies the five desktop messages", () => {
    const kinds = (value: unknown) => {
      const message = parseDesktopMessage(encodeEnvelope(value as never));
      return "type" in message ? "state" : responseKind(message);
    };
    expect(kinds({ rid: 7, ok: true, result: { a: 1 } })).toBe("result");
    expect(kinds({ rid: 7, ok: false, error: "Desktop is busy; try again", code: "busy" })).toBe("error");
    expect(kinds({ rid: 8, ok: true, watching: true })).toBe("watching");
    expect(kinds({ rid: 8, ok: true, heartbeat: true })).toBe("heartbeat");
    expect(kinds({ type: "state", state: "pending" })).toBe("state");
  });
  it("narrows an error code only when known", () => {
    const known = EnvelopeResponseSchema.parse({ rid: 1, ok: false, error: "x", code: "read_only" });
    const unknown = EnvelopeResponseSchema.parse({ rid: 1, ok: false, error: "x", code: "from_the_future" });
    expect(known.ok === false && knownErrorCode(known)).toBe("read_only");
    expect(unknown.ok === false && knownErrorCode(unknown)).toBeUndefined();
  });
  it("rejects garbage", () => {
    expect(() => parseDesktopMessage(new TextEncoder().encode('{"nope":1}'))).toThrow();
    expect(() => decodeEnvelopeJson(Uint8Array.of(0xff, 0xfe))).toThrow();
  });
});
