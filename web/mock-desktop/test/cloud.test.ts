import { afterEach, describe, expect, it, vi } from "vitest";
import { CloudFieldSchema, HEARTBEAT_INTERVAL_MS, STARTUP_STEP_IDS, WatchFrameSchema, type WatchFrame } from "@monoptah/protocol";
import { setup } from "./harness.ts";

afterEach(() => {
  vi.useRealTimers();
});

function lastWithCloud(frames: WatchFrame[]): NonNullable<WatchFrame["cloud"]> {
  const cloud = frames.filter((f) => f.cloud !== undefined).at(-1)?.cloud;
  if (!cloud) throw new Error("no frame carried a cloud field");
  return cloud;
}

describe("catalog and reads carry the cloud fields", () => {
  it("workspace.list has local, codespace, sandbox and remote sessions with target, cloudState and startup", async () => {
    const { client } = await setup();
    const list = await client.call("workspace.list");
    expect(list.projects).toHaveLength(2);
    const byId = new Map(list.sessions.map((s) => [s.id, s]));
    expect(byId.get("sess-local-1")?.target).toEqual({ kind: "local" });
    expect(byId.get("sess-codespace-1")).toMatchObject({ target: { kind: "codespace", instance: "fake-codespace-one", state: "available" }, cloudState: "available" });
    expect(byId.get("sess-sandbox-stopped")).toMatchObject({ target: { kind: "sandbox" }, cloudState: "stopped" });
    expect(byId.get("sess-remote-1")).toMatchObject({ target: { kind: "remote" } });
    expect(byId.get("sess-codespace-starting")).toMatchObject({ cloudState: "starting", startup: { step: "provision" } });
    expect(byId.get("sess-local-approval")).toMatchObject({ waiting: "approval" });
  });

  it("sessions.list pages and filters", async () => {
    const { client } = await setup();
    const page = await client.call("sessions.list", { projectId: "proj-beta", limit: 2 });
    expect(page.sessions).toHaveLength(2);
    expect(page.hasMore).toBe(true);
    const rest = await client.call("sessions.list", { projectId: "proj-beta", offset: 2, limit: 2 });
    expect(rest.hasMore).toBe(false);
    expect((await client.call("sessions.list", { projectId: "proj-alpha", query: "parser" })).sessions.map((s) => s.id)).toEqual(["sess-local-1"]);
  });

  it("conversation.read has cloud for cloud sessions and approvals for the advertising desktop", async () => {
    const { client } = await setup();
    const cloudRead = await client.call("conversation.read", { conversationId: "sess-codespace-1" });
    expect(CloudFieldSchema.parse(cloudRead.cloud)).toMatchObject({ kind: "codespace", instance: "fake-codespace-one", state: "available" });
    expect(cloudRead.approvals).toEqual([]);
    const local = await client.call("conversation.read", { conversationId: "sess-local-1" });
    expect(local.cloud).toBeUndefined();
    const approval = await client.call("conversation.read", { conversationId: "sess-local-approval" });
    expect(approval.approvals).toEqual([{ requestId: 41, label: "Run example command", detail: "example-cli --check (fake detail)" }]);
  });

  it("without the approvals capability the approvals field is absent", async () => {
    const { client } = await setup({ desktop: { capabilities: ["cloud"] } });
    const read = await client.call("conversation.read", { conversationId: "sess-local-approval" });
    expect("approvals" in read).toBe(false);
    expect(await client.request("session.answerApproval", { conversationId: "sess-local-approval", requestId: 41, decision: "allow" })).toMatchObject({ error: "Unsupported mobile operation" });
  });

  it("cloud.instances has cost estimates, a stop report and the other-instances count", async () => {
    const { client } = await setup();
    const result = await client.call("cloud.instances");
    expect(result.otherCount).toBe(2);
    expect(result.instances.length).toBeGreaterThanOrEqual(3);
    expect(result.instances.every((i) => typeof i.costEstimatePerHour === "number")).toBe(true);
    expect(result.instances.find((i) => i.stopReport)).toMatchObject({ instance: "fake-sandbox-two", stopReport: "takingLong", state: "stopping" });
    const codespace = result.instances.find((i) => i.instance === "fake-codespace-one");
    expect(codespace?.sessions).toEqual([{ sessionId: "sess-codespace-1", title: "Refactor on the fake codespace", busy: false }]);
  });
});

describe("watch frames carry cloud and approvals on change only", () => {
  it("approvals: one pending approval in the first frame, then cleared with an empty array", async () => {
    const { client, relay } = await setup();
    const watch = await client.openWatch({ conversationId: "sess-local-approval" });
    await relay.settled();
    expect(watch.frames).toHaveLength(1);
    expect(WatchFrameSchema.safeParse(watch.frames[0]).success).toBe(true);
    expect(watch.frames[0]?.approvals).toHaveLength(1);
    expect(watch.frames[0]?.cloud).toBeUndefined();

    await client.call("session.answerApproval", { conversationId: "sess-local-approval", requestId: 41, decision: "allow" });
    await relay.settled();
    expect(watch.frames).toHaveLength(2);
    expect(watch.frames[1]?.approvals).toEqual([]);
    // Unchanged fields are not repeated.
    expect(watch.frames[1]?.turns).toEqual([]);
    expect(watch.frames[1]?.cloud).toBeUndefined();
  });

  it("approvals: the desktop user can clear it locally (scripted step)", async () => {
    const { client, relay, desktop } = await setup();
    const watch = await client.openWatch({ conversationId: "sess-local-approval" });
    expect(desktop.advance("sess-local-approval")).toBe(true);
    await relay.settled();
    expect(watch.frames.at(-1)?.approvals).toEqual([]);
    expect(desktop.advance("sess-local-approval")).toBe(false);
  });

  it("startup: the steps progress frame by frame until the instance is available", async () => {
    const { client, relay, desktop } = await setup();
    const watch = await client.openWatch({ conversationId: "sess-codespace-starting" });
    await relay.settled();
    const first = CloudFieldSchema.parse(watch.frames[0]?.cloud);
    expect(first).toMatchObject({ state: "starting", startup: { current: "provision" } });
    expect(first.startup?.steps.map((s) => s.id)).toEqual([...STARTUP_STEP_IDS]);
    expect(first.startup?.steps.map((s) => s.status)).toEqual(["done", "running", ...Array(7).fill("pending")]);

    const seen: string[] = [];
    while (desktop.advance("sess-codespace-starting")) {
      await relay.settled();
      seen.push(lastWithCloud(watch.frames).startup?.current ?? "done");
    }
    expect(seen).toEqual(["buildContainer", "blockingCommands", "configureCodespace", "connect", "openProject", "claudeSetup", "lifecycleCommands", "done"]);
    const final = lastWithCloud(watch.frames);
    expect(final).toEqual({ kind: "codespace", instance: "fake-codespace-two", state: "available" });
    // The catalog agrees and the startup marker is gone.
    const card = (await client.call("workspace.list")).sessions.find((s) => s.id === "sess-codespace-starting");
    expect(card?.cloudState).toBe("available");
    expect(card?.startup).toBeUndefined();
  });

  it("cloud.resume answers accepted and pushes cloud progress into the watch", async () => {
    const { client, relay, desktop } = await setup();
    const watch = await client.openWatch({ conversationId: "sess-sandbox-stopped" });
    await relay.settled();
    expect(watch.frames[0]?.cloud).toMatchObject({ state: "stopped" });

    expect(await client.call("cloud.resume", { sessionId: "sess-sandbox-stopped" })).toEqual({ accepted: true });
    await relay.settled();
    expect(lastWithCloud(watch.frames)).toMatchObject({ state: "starting", startup: { current: "create" } });
    // A send while it starts (not queued by a send) is refused with `starting`.
    await expect(client.call("conversation.send", { conversationId: "sess-sandbox-stopped", text: "too early" })).rejects.toMatchObject({ code: "starting" });
    // Resume again is a no-op.
    expect(await client.call("cloud.resume", { sessionId: "sess-sandbox-stopped" })).toEqual({ accepted: false });

    while (desktop.advance("sess-sandbox-stopped")) await relay.settled();
    expect(lastWithCloud(watch.frames)).toMatchObject({ state: "available" });
    expect((await client.call("conversation.send", { conversationId: "sess-sandbox-stopped", text: "now fine" })).sessionId).toBe("sess-sandbox-stopped");
  });

  it("send to a stopped cloud session wakes it ({ok, wake, queued}); a second send is busy; the queued text is delivered at the end", async () => {
    const { client, relay, desktop } = await setup();
    const watch = await client.openWatch({ conversationId: "sess-sandbox-stopped" });
    const first = await client.call("conversation.send", { conversationId: "sess-sandbox-stopped", text: "wake me with this" });
    expect(first).toMatchObject({ ok: true, wake: true, queued: true });
    await expect(client.call("conversation.send", { conversationId: "sess-sandbox-stopped", text: "second" })).rejects.toMatchObject({ code: "busy" });
    await relay.settled();
    expect(lastWithCloud(watch.frames)).toMatchObject({ state: "starting" });

    while (desktop.advance("sess-sandbox-stopped")) await relay.settled();
    expect(lastWithCloud(watch.frames).state).toBe("available");
    const delivered = watch.frames.flatMap((f) => f.turns).flatMap((t) => t.items);
    expect(delivered.some((item) => item.type === "user" && (item as { text?: string }).text === "wake me with this")).toBe(true);
    // The pending send is cleared: the next send is an ordinary one.
    expect((await client.call("conversation.send", { conversationId: "sess-sandbox-stopped", text: "after" })).wake).toBeUndefined();
  });

  it("cloud.stop reports stopping, then the stopped state reaches the watch", async () => {
    const { client, relay, desktop } = await setup();
    const watch = await client.openWatch({ conversationId: "sess-codespace-1" });
    const result = await client.call("cloud.stop", { sessionId: "sess-codespace-1" });
    expect(result.stopped).toBe(true);
    await relay.settled();
    expect(lastWithCloud(watch.frames).state).toBe("stopping");
    expect((await client.call("cloud.instances")).instances.find((i) => i.instance === "fake-codespace-one")).toMatchObject({ state: "stopping", stopReport: "stopping" });
    desktop.advance("sess-codespace-1");
    await relay.settled();
    expect(lastWithCloud(watch.frames).state).toBe("stopped");
  });

  it("session.archive of a cloud session needs keep or stop, and delete answers unsupported_target", async () => {
    const { client } = await setup();
    expect(await client.request("session.archive", { conversationId: "sess-codespace-1" })).toMatchObject({ ok: false });
    expect((await client.request("session.archive", { conversationId: "sess-codespace-1", cloud: "stop" })).ok).toBe(true);
    expect(await client.request("session.delete", { conversationId: "sess-codespace-1" })).toMatchObject({ ok: false, code: "unsupported_target" });
  });

  it("a move streams the 7 move steps through the cloud field and can be cancelled", async () => {
    const { client, relay, desktop } = await setup({ desktop: { flags: { remoteCloudStart: true, remoteGitPush: true } } });
    const watch = await client.openWatch({ conversationId: "sess-codespace-1" });
    await client.call("cloud.moveToLocal", { sessionId: "sess-codespace-1" });
    await relay.settled();
    expect(lastWithCloud(watch.frames).move).toEqual({ step: "snapshot", status: "running" });
    desktop.advance("sess-codespace-1");
    await relay.settled();
    expect(lastWithCloud(watch.frames).move?.step).toBe("push");
    await client.call("cloud.cancelMove", { sessionId: "sess-codespace-1" });
    await relay.settled();
    expect(lastWithCloud(watch.frames).move).toBeUndefined();
  });

  it("watches: unknown conversation, watch without the flag, and the per-device limit", async () => {
    const { client } = await setup();
    await expect(client.openWatch({ conversationId: "nope" })).rejects.toBeTruthy();
    expect(await client.request("conversation.watch", { conversationId: "sess-local-1" })).toMatchObject({ ok: false });
    for (let i = 0; i < 4; i++) await client.openWatch({ conversationId: "sess-local-1" });
    await expect(client.openWatch({ conversationId: "sess-local-1" })).rejects.toMatchObject({ code: "busy" });
  });
});

describe("heartbeat (injectable timers)", () => {
  it("sends a heartbeat every 25 s on an open watch and stops after cancel", async () => {
    vi.useFakeTimers();
    const { client, relay } = await setup();
    const watch = await client.openWatch({ conversationId: "sess-local-1" });
    await relay.settled();
    vi.advanceTimersByTime(HEARTBEAT_INTERVAL_MS - 1);
    await relay.settled();
    expect(watch.heartbeats).toBe(0);
    vi.advanceTimersByTime(1);
    await relay.settled();
    expect(watch.heartbeats).toBe(1);
    vi.advanceTimersByTime(HEARTBEAT_INTERVAL_MS * 2);
    await relay.settled();
    expect(watch.heartbeats).toBe(3);
    watch.cancel();
    await relay.settled();
    vi.advanceTimersByTime(HEARTBEAT_INTERVAL_MS * 4);
    await relay.settled();
    expect(watch.heartbeats).toBe(3);
  });

  it("uses an injected timer implementation and heartbeat period", async () => {
    const intervals: { fn: () => void; ms: number; cleared: boolean }[] = [];
    const timers = {
      setInterval: (fn: () => void, ms: number) => {
        const entry = { fn, ms, cleared: false };
        intervals.push(entry);
        return entry;
      },
      clearInterval: (handle: unknown) => {
        (handle as { cleared: boolean }).cleared = true;
      },
      setTimeout: () => ({}),
      clearTimeout: () => undefined,
    };
    const { client, relay } = await setup({ desktop: { timers, heartbeatMs: 1234 } });
    const watch = await client.openWatch({ conversationId: "sess-local-1" });
    expect(intervals).toHaveLength(1);
    expect(intervals[0]?.ms).toBe(1234);
    intervals[0]?.fn();
    await relay.settled();
    expect(watch.heartbeats).toBe(1);
    watch.cancel();
    await relay.settled();
    expect(intervals[0]?.cleared).toBe(true);
  });

  it("a closed session clears its watch timers", async () => {
    vi.useFakeTimers();
    const s = await setup();
    await s.client.openWatch({ conversationId: "sess-local-1" });
    expect(vi.getTimerCount()).toBeGreaterThan(0);
    s.client.bye();
    await s.relay.settled();
    expect(vi.getTimerCount()).toBe(0);
  });
});
