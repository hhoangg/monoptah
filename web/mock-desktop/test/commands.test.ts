import { describe, expect, it } from "vitest";
import { KNOWN_CAPABILITIES } from "@monoptah/protocol";
import { fingerprint } from "../src/index.ts";
import { setup } from "./harness.ts";

type Row = { id: string; kind: string; available: boolean; reason?: string; mode: boolean; active: boolean };

async function list(client: Awaited<ReturnType<typeof setup>>["client"], params: Record<string, unknown>): Promise<Row[]> {
  return ((await client.call("commands.list", params)) as { commands: Row[] }).commands;
}

describe("the / menu", () => {
  it("lists the built-ins in the native order, then the skills", async () => {
    const { client } = await setup();
    const rows = await list(client, { conversationId: "sess-local-1" });
    expect(rows.map((row) => row.id)).toEqual(["draft", "orchestrator", "operator", "plan", "btw", "compact", "add-to-folder", "review", "lint"]);
    expect(rows.find((row) => row.id === "plan")).toMatchObject({ available: true, mode: true, active: false });
    expect(rows.find((row) => row.id === "review")).toMatchObject({ kind: "skill", available: true });
  });

  it("says why a command is not available before the session has a turn, and in a new session", async () => {
    const { client } = await setup();
    const row = (rows: Row[], id: string) => rows.find((r) => r.id === id);
    expect(row(await list(client, { conversationId: "sess-local-1" }), "btw")).toMatchObject({ available: true });
    const fresh = await list(client, { projectId: "proj-alpha" });
    expect(row(fresh, "operator")).toMatchObject({ available: false });
    expect(row(fresh, "compact")?.reason).toMatch(/finished turn/);
    expect(row(fresh, "plan")).toMatchObject({ available: true });
  });

  it("a plan, operator or orchestrator run switches exactly one mode and the list reports it", async () => {
    const { client, desktop } = await setup();
    expect(await client.call("commands.run", { conversationId: "sess-local-1", command: "plan" })).toMatchObject({ command: "plan", active: true });
    expect((await list(client, { conversationId: "sess-local-1" })).filter((row) => row.active).map((row) => row.id)).toEqual(["plan"]);
    expect(await client.call("commands.run", { conversationId: "sess-local-1", command: "operator", enabled: true })).toMatchObject({ active: true });
    expect((await list(client, { conversationId: "sess-local-1" })).filter((row) => row.active).map((row) => row.id)).toEqual(["operator"]);
    expect(await client.call("commands.run", { conversationId: "sess-local-1", command: "operator" })).toMatchObject({ active: false });
    expect(desktop.world.commandRuns.map((run) => run.command)).toEqual(["plan", "operator", "operator"]);
  });

  it("draft keeps the text without a turn and refuses a second one", async () => {
    const { client, desktop } = await setup();
    await client.call("commands.run", { conversationId: "sess-local-1", command: "draft", text: "Later" });
    expect(desktop.world.slashState("sess-local-1").draft).toBe("Later");
    expect(await client.request("commands.run", { conversationId: "sess-local-1", command: "draft", text: "Again" })).toMatchObject({ ok: false });
    expect((await list(client, { conversationId: "sess-local-1" })).find((row) => row.id === "draft")).toMatchObject({ available: false });
  });

  it("a read-only device gets every row disabled and cannot run anything", async () => {
    const { client } = await setup({ permission: "read" });
    const rows = await list(client, { conversationId: "sess-local-1" });
    expect(rows.length).toBeGreaterThan(0);
    expect(rows.every((row) => !row.available && row.reason)).toBe(true);
    expect(await client.request("commands.run", { conversationId: "sess-local-1", command: "operator" })).toMatchObject({ ok: false, code: "read_only" });
  });

  it("a device that shares only another project cannot ask for this project's new-session menu", async () => {
    const { client, desktop, deviceKey } = await setup();
    desktop.setDevice(fingerprint(deviceKey.publicKey), { selection: { projects: ["proj-beta"], monos: null } });
    expect(await client.request("commands.list", { projectId: "proj-alpha" })).toMatchObject({ ok: false, code: "not_selected" });
  });

  it("a desktop that does not advertise slashCommands answers like an older one", async () => {
    const { client } = await setup({ desktop: { capabilities: KNOWN_CAPABILITIES.filter((name) => name !== "slashCommands" && name !== "terminal") } });
    expect(await client.request("commands.list", { conversationId: "sess-local-1" })).toMatchObject({ ok: false });
  });
});
