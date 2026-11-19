import { describe, expect, it } from "vitest";
import { FakeClient, requestError } from "./testing.ts";
import { MAX_PENDING_COMMAND_IDS, Mutator, commandKey, commandSignature, getMutator, stableStringify } from "./mutations.ts";

function ids(): () => string {
  let n = 0;
  return () => `id${++n}`;
}

describe("command ids per signature", () => {
  it("stableStringify ignores key order", () => {
    expect(stableStringify({ b: 1, a: { d: 1, c: 2 } })).toBe(stableStringify({ a: { c: 2, d: 1 }, b: 1 }));
    expect(commandSignature("m", { a: 1, b: 2 })).toBe(commandSignature("m", { b: 2, a: 1 }));
    expect(commandSignature("m", { a: 1 })).not.toBe(commandSignature("n", { a: 1 }));
  });

  it("keeps the id of a signature after a failure and uses it again on the retry", async () => {
    const client = new FakeClient();
    const mutator = new Mutator(client.asClient(), ids());
    client.request.mockRejectedValueOnce(requestError("session_ended"));
    await expect(mutator.mutate("conversation.send", { conversationId: "c", text: "hi" })).rejects.toMatchObject({ code: "session_ended" });
    expect(mutator.pendingId("conversation.send", { conversationId: "c", text: "hi" })).toBe("id1");
    await mutator.mutate("conversation.send", { text: "hi", conversationId: "c" });
    expect(client.request.mock.calls.map((call) => (call[2] as { id: string }).id)).toEqual(["id1", "id1"]);
  });

  it("drops the id on success so the same call afterwards gets a new one", async () => {
    const client = new FakeClient();
    const mutator = new Mutator(client.asClient(), ids());
    await mutator.mutate("conversation.send", { conversationId: "c", text: "hi" });
    expect(mutator.pendingId("conversation.send", { conversationId: "c", text: "hi" })).toBeUndefined();
    await mutator.mutate("conversation.send", { conversationId: "c", text: "hi" });
    expect(client.request.mock.calls.map((call) => (call[2] as { id: string }).id)).toEqual(["id1", "id2"]);
  });

  it("uses different ids for different params and bounds the table", async () => {
    const client = new FakeClient();
    client.request.mockRejectedValue(requestError("busy"));
    const mutator = new Mutator(client.asClient(), ids());
    for (let i = 0; i < MAX_PENDING_COMMAND_IDS + 5; i += 1) {
      await mutator.mutate("conversation.send", { conversationId: "c", text: `m${i}` }).catch(() => undefined);
    }
    expect(mutator.pendingId("conversation.send", { conversationId: "c", text: "m0" })).toBeUndefined();
    expect(mutator.pendingId("conversation.send", { conversationId: "c", text: `m${MAX_PENDING_COMMAND_IDS + 4}` })).toBeDefined();
  });

  it("never sends again by itself: a failed call is one request", async () => {
    const client = new FakeClient();
    client.request.mockRejectedValue(requestError("not_ready"));
    const mutator = getMutator(client.asClient());
    expect(getMutator(client.asClient())).toBe(mutator);
    await expect(mutator.mutate("conversation.send", { conversationId: "c", text: "x" })).rejects.toBeDefined();
    client.reconnect();
    client.setStatus("ready");
    expect(client.request).toHaveBeenCalledTimes(1);
  });
});

describe("command id table keys (M12)", () => {
  it("holds a SHA-256 of the signature, not the params with their attachments", async () => {
    const client = new FakeClient();
    const mutator = new Mutator(client.asClient(), ids());
    const big = "A".repeat(1_000_000);
    client.request.mockRejectedValueOnce(requestError("session_ended"));
    const params = { conversationId: "c", text: "hi", attachments: [{ name: "a.png", mediaType: "image/png", data: big }] };
    await expect(mutator.mutate("conversation.send", params as never)).rejects.toMatchObject({ code: "session_ended" });
    const keys = mutator.pendingKeys();
    expect(keys).toEqual([commandKey("conversation.send", params)]);
    expect(keys[0]).toMatch(/^[0-9a-f]{64}$/);
    expect(mutator.pendingId("conversation.send", params)).toBe("id1");
    expect(commandKey("conversation.send", params)).not.toBe(commandKey("conversation.send", { ...params, text: "other" }));
  });
});
