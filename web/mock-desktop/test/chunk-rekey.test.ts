import { afterEach, describe, expect, it, vi } from "vitest";
import {
  CHUNK_HEADER_SIZE,
  FrameType,
  MAX_CHUNK_DATA,
  MAX_NOISE_PLAINTEXT,
  PlaintextKind,
  REKEY_AFTER_MS,
} from "@monoptah/protocol";
import { LARGE_FILE_PATH, fingerprint, largeText } from "../src/index.ts";
import { setup } from "./harness.ts";

afterEach(() => {
  vi.useRealTimers();
});

describe("chunking", () => {
  it("a response above 65,519 bytes is sent as chunks and reassembles", async () => {
    const { client, relay } = await setup();
    const before = relay.forwarded.length;
    const file = await client.call("project.file", { projectId: "proj-alpha", path: LARGE_FILE_PATH });
    expect(file.text).toBe(largeText());
    expect(file.text.length).toBeGreaterThan(MAX_NOISE_PLAINTEXT);
    const transport = relay.forwarded.slice(before).filter((f) => f.direction === "desktop->device" && f.frame.type === FrameType.TRANSPORT);
    // 200,000 bytes plus the envelope in chunks of at most 65,511 bytes of data: 4 messages.
    expect(transport.length).toBe(Math.ceil((200_000 + 60) / MAX_CHUNK_DATA));
    expect(transport.every((f) => f.frame.body.length <= 65_535)).toBe(true);
    expect(CHUNK_HEADER_SIZE + MAX_CHUNK_DATA).toBe(MAX_NOISE_PLAINTEXT);
  });

  it("a request above 65,519 bytes is chunked by the client and reassembled by the desktop", async () => {
    const { client, relay } = await setup();
    const text = "x".repeat(100_000);
    const before = relay.forwarded.length;
    await client.call("conversation.send", { conversationId: "sess-local-1", text });
    const sent = relay.forwarded.slice(before).filter((f) => f.direction === "device->desktop");
    expect(sent.length).toBe(2);
    const read = await client.call("conversation.read", { conversationId: "sess-local-1" });
    const users = read.turns.flatMap((t) => t.items).filter((i) => i.type === "user") as unknown as { text: string }[];
    expect(users.at(-1)?.text).toHaveLength(100_000);
  });

  it("several large responses in a row keep working (msgId advances)", async () => {
    const { client } = await setup();
    for (let i = 0; i < 3; i++) {
      expect((await client.call("project.file", { projectId: "proj-alpha", path: LARGE_FILE_PATH })).text.length).toBe(200_000);
    }
  });

  it("a response above the configured limit is replaced by a too_large error and the link survives", async () => {
    const { client } = await setup({ desktop: { maxResponseBytes: 100_000 } });
    await expect(client.call("project.file", { projectId: "proj-alpha", path: LARGE_FILE_PATH })).rejects.toMatchObject({ code: "too_large" });
    expect((await client.call("workspace.list")).projects).toHaveLength(2);
  });

  it("an out-of-order chunk ends the session with BYE", async () => {
    const { client, relay, desktop } = await setup();
    // A CHUNK with index 1 for a message that never started.
    const bogus = new Uint8Array(CHUNK_HEADER_SIZE + 4);
    bogus[0] = PlaintextKind.CHUNK;
    new DataView(bogus.buffer).setUint16(5, 1);
    client.sendRawPlaintext(bogus);
    await relay.settled();
    expect(client.byeReceived).toBe(true);
    expect(desktop.sessions).toHaveLength(0);
  });

  it("an unknown plaintext kind ends the session with BYE", async () => {
    const { client, relay, desktop } = await setup();
    client.sendRawPlaintext(Uint8Array.of(0x7f, 1, 2));
    await relay.settled();
    expect(client.byeReceived).toBe(true);
    expect(desktop.sessions).toHaveLength(0);
  });

  it("more than 4 messages in reassembly end the session with BYE", async () => {
    const { client, relay, desktop } = await setup();
    for (let msgId = 1; msgId <= 5; msgId++) {
      const first = new Uint8Array(CHUNK_HEADER_SIZE + 4);
      first[0] = PlaintextKind.CHUNK;
      new DataView(first.buffer).setUint32(1, msgId);
      client.sendRawPlaintext(first);
    }
    await relay.settled();
    expect(client.byeReceived).toBe(true);
    expect(desktop.sessions).toHaveLength(0);
  });
});

describe("rekey (architecture.md 5.4)", () => {
  it("both sides rekey after the message threshold and the link keeps working", async () => {
    const { client, desktop, relay } = await setup({ desktop: { rekeyAfterMessages: 3 }, client: { rekeyAfterMessages: 4 } });
    for (let i = 0; i < 12; i++) expect((await client.call("workspace.list")).projects).toHaveLength(2);
    expect(client.rekeysSent).toBeGreaterThan(0);
    expect(client.rekeysReceived).toBeGreaterThan(0);
    const info = desktop.sessions[0];
    expect(info?.rekeysSent).toBeGreaterThan(0);
    expect(info?.rekeysReceived).toBe(client.rekeysSent);
    expect(client.rekeysReceived).toBe(info?.rekeysSent);
    // REKEY messages are ordinary TRANSPORT frames on the wire.
    expect(relay.forwarded.filter((f) => f.frame.type === FrameType.TRANSPORT).length).toBeGreaterThan(24);
  });

  it("rekeys in the middle of a chunked response", async () => {
    const { client, desktop } = await setup({ desktop: { rekeyAfterMessages: 2 } });
    const file = await client.call("project.file", { projectId: "proj-alpha", path: LARGE_FILE_PATH });
    expect(file.text.length).toBe(200_000);
    expect(desktop.sessions[0]?.rekeysSent).toBeGreaterThanOrEqual(2);
  });

  it("rekeys after the time threshold", async () => {
    vi.useFakeTimers();
    const { client, desktop } = await setup({ desktop: { rekeyAfterMs: REKEY_AFTER_MS } });
    await client.call("models.list");
    expect(desktop.sessions[0]?.rekeysSent).toBe(0);
    vi.advanceTimersByTime(REKEY_AFTER_MS + 1);
    await client.call("models.list");
    expect(desktop.sessions[0]?.rekeysSent).toBe(1);
    expect(client.rekeysReceived).toBe(1);
  });

  it("a state message after a rekey still decrypts (permission change)", async () => {
    const { client, desktop, relay, deviceKey } = await setup({ desktop: { rekeyAfterMessages: 2 } });
    for (let i = 0; i < 4; i++) await client.call("models.list");
    desktop.setDevice(fingerprint(deviceKey.publicKey), { permission: "read" });
    await relay.settled();
    expect(client.state?.permission).toBe("read");
  });
});
