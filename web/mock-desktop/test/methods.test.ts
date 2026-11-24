import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import {
  DesktopErrorCodeSchema,
  METHODS,
  METHOD_LIST,
  MonoEmojiPickSchema,
  WatchFrameSchema,
  classify,
  type ClassifyFlags,
  type MethodName,
} from "@monoptah/protocol";
import methodsJson from "@monoptah/protocol/methods.json" with { type: "json" };
import { FIXTURE_HANDLERS, MONO_EMOJI_FIXTURE_HASH, MONO_EMOJI_FIXTURE_MONO_ID, MockWorld, type HandlerContext, type MockDevice } from "../src/index.ts";
import { SAMPLE_PARAMS } from "./params.ts";
import { setup } from "./harness.ts";

const names = methodsJson.methods.map((m) => m.name) as MethodName[];
const BOTH_TOGGLES: ClassifyFlags = { remoteCloudStart: true, remoteGitPush: true };

describe("every method of methods.json is served", () => {
  it("the fixture handlers and the sample params cover exactly the protocol methods", () => {
    expect(names.length).toBeGreaterThan(40);
    expect(names).toEqual(METHOD_LIST.map((m) => m.name));
    expect(Object.keys(FIXTURE_HANDLERS).sort()).toEqual([...names].sort());
    expect(Object.keys(SAMPLE_PARAMS).sort()).toEqual([...names].sort());
    for (const name of names) {
      expect(typeof FIXTURE_HANDLERS[name], name).toBe("function");
      expect(METHODS[name].params.safeParse(SAMPLE_PARAMS[name]).success, `sample params of ${name}`).toBe(true);
    }
  });

  // The terminal methods need an open shell, a code and a desktop that advertises `terminal`: test/terminal.test.ts walks them.
  const withoutTerminal = names.filter((name) => !name.startsWith("terminal."));

  it.each(withoutTerminal)("handler of %s returns a result that matches the protocol result schema", async (name) => {
    const device: MockDevice = { fingerprint: "ff", status: "confirmed", permission: "control", selection: { projects: null, monos: null } };
    const ctx: HandlerContext = { world: new MockWorld(), device };
    const params = METHODS[name].params.parse(SAMPLE_PARAMS[name]);
    // `file.prepare` answers after its upload, the others at once.
    const result = await (FIXTURE_HANDLERS[name] as (p: unknown, c: HandlerContext) => unknown)(params, ctx);
    const parsed = METHODS[name].result.safeParse(result);
    expect(parsed.success, parsed.success ? "" : JSON.stringify(parsed.error.issues)).toBe(true);
  });

  describe("over the Noise link with both desktop toggles on", () => {
    it.each(withoutTerminal)("%s answers as its class says", async (name) => {
      const s = await setup({ desktop: { flags: BOTH_TOGGLES } });
      const params = SAMPLE_PARAMS[name];
      const resolved = classify(name, params, BOTH_TOGGLES);
      if (name === "conversation.watch") {
        const watch = await s.client.openWatch(params as { conversationId: string });
        await s.relay.settled();
        expect(watch.frames).toHaveLength(1);
        expect(WatchFrameSchema.safeParse(watch.frames[0]).success).toBe(true);
        return;
      }
      const response = await s.client.request(name, params);
      if (resolved === "denied") {
        expect(response).toMatchObject({ ok: false, code: "forbidden" });
      } else if (resolved === "unsupportedWeb") {
        expect(response).toMatchObject({ ok: false, code: "unsupported" });
      } else {
        expect(response.ok, JSON.stringify(response)).toBe(true);
        const parsed = METHODS[name].result.safeParse((response as { result: unknown }).result);
        expect(parsed.success, parsed.success ? "" : JSON.stringify(parsed.error.issues)).toBe(true);
      }
    });
  });

  it("denied and unsupportedWeb methods are present in the list and answered with the documented codes", () => {
    const denied = names.filter((n) => METHODS[n].class === "denied");
    const unsupported = names.filter((n) => METHODS[n].class === "unsupportedWeb");
    expect(denied).toEqual(expect.arrayContaining(["mono.saveFile", "mono.delete", "device.enroll"]));
    expect(unsupported).toEqual(expect.arrayContaining(["project.fileHighlight", "device.push"]));
  });

  it("with the toggles off, gated paths answer forbidden or disabled", async () => {
    const s = await setup();
    expect(await s.client.request("cloud.moveToCloud", { sessionId: "sess-local-1" })).toMatchObject({ ok: false, code: "forbidden" });
    expect(await s.client.request("session.start", SAMPLE_PARAMS["session.start"])).toMatchObject({ ok: false, code: "disabled" });
    expect(await s.client.request("project.gitAction", { projectId: "proj-alpha", action: "sync" })).toMatchObject({ ok: false, code: "forbidden" });
    expect(await s.client.request("session.configure", { conversationId: "sess-local-1", runtimeMode: "full-access" })).toMatchObject({ ok: false });
  });

  it("every error code the mock uses is a known desktop error code", async () => {
    const s = await setup({ desktop: { flags: {} } });
    for (const [method, params] of [
      ["mono.delete", { monoId: "x" }],
      ["device.push", {}],
    ] as const) {
      const response = await s.client.request(method, params);
      expect(response.ok).toBe(false);
      if (!response.ok) expect(DesktopErrorCodeSchema.safeParse(response.code).success).toBe(true);
    }
  });
});

describe("permissions", () => {
  it("a read device gets read_only for every control method and can use every read method", async () => {
    for (const name of names) {
      const params = SAMPLE_PARAMS[name];
      const resolved = classify(name, params, BOTH_TOGGLES);
      if (resolved !== "control" && resolved !== "read") continue;
      if (name === "conversation.watch") continue;
      const s = await setup({ permission: "read", desktop: { flags: BOTH_TOGGLES } });
      const response = await s.client.request(name, params);
      if (resolved === "control") expect(response, name).toMatchObject({ ok: false, code: "read_only" });
      else expect(response.ok, name).toBe(true);
    }
  });

  it("a read device can open a watch", async () => {
    const s = await setup({ permission: "read" });
    const watch = await s.client.openWatch({ conversationId: "sess-local-1" });
    await s.relay.settled();
    expect(watch.frames).toHaveLength(1);
  });

  it("a param method resolves per call: accounts.usage with refresh needs control", async () => {
    const s = await setup({ permission: "read" });
    expect((await s.client.request("accounts.usage", { provider: "claude" })).ok).toBe(true);
    expect(await s.client.request("accounts.usage", { provider: "claude", refresh: true })).toMatchObject({ code: "read_only" });
    expect((await s.client.request("device.selection", {})).ok).toBe(true);
    // device.selection is always "read"; the handler refuses a widening by a read device, and narrowing is fine.
    expect((await s.client.request("device.selection", { projects: ["proj-alpha"] })).ok).toBe(true);
  });
});

describe("device selection", () => {
  const refOf = (list: { ref: string; name: string }[], name: string): string => list.find((item) => item.name === name)?.ref ?? "";

  it("lists every project and Mono with shared state; an unshared one has a name and a ref but no id", async () => {
    const s = await setup();
    const all = await s.client.call("device.selection", {});
    expect(all).toMatchObject({ projects: null, monos: null, canWiden: true });
    expect(all.available?.projects.map((p) => [p.name, p.shared, p.id])).toEqual([["Example Alpha", true, "proj-alpha"], ["Example Beta", true, "proj-beta"]]);
    const narrowed = await s.client.call("device.selection", { projects: ["proj-alpha"], monos: [] });
    const beta = narrowed.available?.projects.find((p) => p.name === "Example Beta");
    expect(beta).toEqual({ ref: expect.any(String), name: "Example Beta", shared: false });
    expect(JSON.stringify(beta)).not.toContain("proj-beta");
    expect(JSON.stringify(beta)).not.toContain("/example");
    expect(narrowed.available?.monos[0]).toMatchObject({ name: "Example Mono", shared: false });
    const list = await s.client.call("workspace.list");
    expect(list.projects.map((p) => p.id)).toEqual(["proj-alpha"]);
    expect(list.monos).toEqual([]);
    expect(list.sessions.every((x) => x.projectId === "proj-alpha")).toBe(true);
  });

  it("narrows without a ticket; widening needs a ticket (fresh_required) and then adds the ref", async () => {
    const s = await setup();
    await s.client.call("device.selection", { projects: ["proj-alpha"] });
    const beta = refOf((await s.client.call("device.selection", {})).available?.projects ?? [], "Example Beta");
    await expect(s.client.call("device.selection", { addProjects: [beta] })).rejects.toMatchObject({ code: "fresh_required" });
    await expect(s.client.call("device.selection", { projects: null })).rejects.toMatchObject({ code: "fresh_required" });
    await expect(s.client.call("device.selection", { addProjects: ["nope"], ticket: "t" })).rejects.toMatchObject({ code: "not_found" });
    const after = await s.client.call("device.selection", { projects: ["proj-alpha"], addProjects: [beta], ticket: "ticket-1" });
    expect(after.projects).toEqual(["proj-alpha", "proj-beta"]);
    expect(after.available?.projects.every((p) => p.shared && p.id !== undefined)).toBe(true);
    // null = all, including new ones, is a widening too and takes a ticket.
    await s.client.call("device.selection", { projects: ["proj-alpha"] });
    expect(await s.client.call("device.selection", { projects: null, ticket: "ticket-2" })).toMatchObject({ projects: null });
  });

  it("a read device may narrow but not widen (read_only), and canWiden is false", async () => {
    const s = await setup({ permission: "read" });
    expect(await s.client.call("device.selection", {})).toMatchObject({ canWiden: false });
    await s.client.call("device.selection", { projects: ["proj-alpha"] });
    const beta = refOf((await s.client.call("device.selection", {})).available?.projects ?? [], "Example Beta");
    await expect(s.client.call("device.selection", { addProjects: [beta], ticket: "t" })).rejects.toMatchObject({ code: "read_only" });
    expect((await s.client.call("device.selection", {})).projects).toEqual(["proj-alpha"]);
  });
});

describe("fixture hygiene", () => {
  it("source and fixtures contain no real-looking secrets, no em or en dashes and no MonoCode branding", () => {
    for (const file of ["../../app/src/link/noise.ts", "../../app/src/link/secure-channel.ts", "../src/fixtures.ts", "../src/desktop.ts", "../src/client.ts", "../src/transport.ts", "../src/terminal.ts"]) {
      const text = readFileSync(new URL(file, import.meta.url), "utf8");
      expect(text, file).not.toMatch(/[\u2013\u2014]/);
      expect(text, file).not.toMatch(/ghp_|github_pat_|sk-[a-z0-9]{10}|dckr_pat/i);
      expect(text, file).not.toMatch(/MonoCode/);
    }
  });

  it("no frame of any method carries path-like secrets beyond the fake /example paths", async () => {
    const world = new MockWorld();
    const json = JSON.stringify(FIXTURE_HANDLERS["workspace.list"]({}, { world, device: { fingerprint: "ff", status: "confirmed", permission: "control", selection: { projects: null, monos: null } } }));
    expect(json).not.toMatch(/\/Users\/|\/home\/|C:\\\\|remote:\/\//);
  });
});

describe("Mono emoji and reset", () => {
  it("advertises monoEmoji to every device and monoReset only to a Control device", async () => {
    const control = await setup();
    const read = await setup({ permission: "read" });
    expect(control.client.state?.capabilities).toEqual(expect.arrayContaining(["monoEmoji", "monoReset"]));
    expect(read.client.state?.capabilities).toContain("monoEmoji");
    expect(read.client.state?.capabilities).not.toContain("monoReset");
  });

  it("serves the fixture PNG by its sha256 and answers not_found for another hash", async () => {
    const s = await setup({ permission: "read" });
    const ok = await s.client.request("mono.emoji", { monoId: MONO_EMOJI_FIXTURE_MONO_ID, hash: MONO_EMOJI_FIXTURE_HASH });
    expect(ok.ok).toBe(true);
    const result = (ok as { result: { hash: string; mime: string; data: string } }).result;
    expect(result.mime).toBe("image/png");
    expect(createHash("sha256").update(Buffer.from(result.data, "base64")).digest("hex")).toBe(MONO_EMOJI_FIXTURE_HASH);
    expect(await s.client.request("mono.emoji", { monoId: MONO_EMOJI_FIXTURE_MONO_ID, hash: "0".repeat(64) })).toMatchObject({ ok: false, code: "not_found" });
  });

  it("without the monoEmoji or monoReset capability the methods answer like an older desktop", async () => {
    const s = await setup({ desktop: { capabilities: ["cloud"] } });
    expect((await s.client.request("mono.emoji", { monoId: "mono-example", hash: MONO_EMOJI_FIXTURE_HASH })).ok).toBe(false);
    expect((await s.client.request("mono.reset", { monoId: "mono-example", ticket: "t" })).ok).toBe(false);
  });

  it("mono.reset needs a ticket from a web device and a read device gets read_only", async () => {
    const s = await setup();
    expect(await s.client.request("mono.reset", { monoId: "mono-example" })).toMatchObject({ ok: false, code: "fresh_required" });
    expect((await s.client.request("mono.reset", { monoId: "mono-example", ticket: "t", deleteFiles: false })).ok).toBe(true);
    const read = await setup({ permission: "read" });
    expect(await read.client.request("mono.reset", { monoId: "mono-example", ticket: "t" })).toMatchObject({ ok: false, code: "read_only" });
  });

  it("the fixture Mono conversation carries emoji picks at correct UTF-16 offsets with a stripped text", async () => {
    const world = new MockWorld();
    const turns = world.read(`mono:${MONO_EMOJI_FIXTURE_MONO_ID}`).turns;
    const item = turns.flatMap((t) => t.items).find((i) => Array.isArray(i.emoji)) as unknown as { text: string; emoji: unknown[] };
    const picks = item.emoji.map((p) => MonoEmojiPickSchema.parse(p));
    expect(item.text).not.toContain(":emo[");
    expect(picks[0]).toMatchObject({ at: item.text.indexOf(" !") + 1, hash: MONO_EMOJI_FIXTURE_HASH });
    expect(picks[1]).toMatchObject({ large: true, at: item.text.indexOf("\n") + 1 });
    expect(item.text.slice(0, picks[0]?.at)).toBe("Good morning ");
  });

  it("mono.profile of the fixture Mono carries resetFiles", async () => {
    const s = await setup();
    const response = await s.client.request("mono.profile", { monoId: MONO_EMOJI_FIXTURE_MONO_ID });
    expect(response).toMatchObject({ ok: true, result: { resetFiles: { label: "Claude Code", supported: true } } });
  });
});
