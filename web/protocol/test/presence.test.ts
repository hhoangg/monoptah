import { describe, expect, it } from "vitest";
import { DeviceMessageSchema, PresenceMessageSchema, decodeEnvelopeJson, encodeEnvelope } from "../src/index.ts";

describe("presence message", () => {
  it("accepts a target and the cleared state, and round-trips through the envelope encoding", () => {
    for (const target of [{ kind: "session", id: "s1" }, { kind: "mono", id: "m1" }, null] as const) {
      const message = { t: "presence", version: 1, target } as const;
      expect(PresenceMessageSchema.parse(message)).toEqual(message);
      expect(DeviceMessageSchema.parse(decodeEnvelopeJson(encodeEnvelope(message)))).toEqual(message);
    }
  });

  it("rejects an unknown kind, an empty id and a missing target", () => {
    expect(PresenceMessageSchema.safeParse({ t: "presence", version: 1, target: { kind: "project", id: "p" } }).success).toBe(false);
    expect(PresenceMessageSchema.safeParse({ t: "presence", version: 1, target: { kind: "mono", id: "" } }).success).toBe(false);
    expect(PresenceMessageSchema.safeParse({ t: "presence", version: 1 }).success).toBe(false);
  });

  it("has no rid, so it is never taken for a request or a cancel", () => {
    const parsed = DeviceMessageSchema.parse({ t: "presence", version: 1, target: null });
    expect("rid" in parsed).toBe(false);
  });
});
