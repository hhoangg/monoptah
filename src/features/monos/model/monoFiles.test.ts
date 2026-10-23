// @vitest-environment happy-dom
import { afterEach, beforeEach, expect, it, vi } from "vitest";

const invoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invoke(...args),
}));

import { createMono, findMono, HANDED_KEY, updateMono } from "./mono";
import {
  defaultSoul,
  loadMonoFiles,
  MEMORY_MAX_LINES,
  memoryWithinBudget,
  MONO_PROMPT_VERSION,
  planAgentContext,
  monoContext,
  monoTurn,
  recordAgentContext,
  resetMonoDefaults,
} from "./monoFiles";

beforeEach(() => {
  const stored = new Map<string, string>();
  vi.stubGlobal("localStorage", {
    getItem: (key: string) => stored.get(key) ?? null,
    setItem: (key: string, value: string) => stored.set(key, value),
  });
  invoke.mockReset();
});
afterEach(() => vi.unstubAllGlobals());

const files = (soulHash = "s1", memoryHash = "m1") => ({
  id: "agent",
  dir: "/data/agents/agent",
  soul: "# Soul\n\nBe brief.",
  soulHash,
  memory: "- 2026-10-04 · releases are tag-triggered",
  memoryHash,
  memoryPath: "/data/agents/agent/MEMORY.md",
  topics: ["releases"],
});

it("hands a new native session everything, then nothing it already has", () => {
  expect(planAgentContext("chat", undefined, files())).toEqual({
    soul: true,
    memory: true,
  });
  // The first turn is recorded before its reply names the provider session.
  recordAgentContext("chat", undefined, files());
  expect(planAgentContext("chat", "native-1", files())).toEqual({
    soul: false,
    memory: false,
  });
  recordAgentContext("chat", "native-1", files());
  expect(planAgentContext("chat", "native-1", files())).toEqual({
    soul: false,
    memory: false,
  });
});

it("resends memory when it changed, and everything for a new session", () => {
  recordAgentContext("chat", "native-1", files());
  expect(planAgentContext("chat", "native-1", files("s1", "m2"))).toEqual({
    soul: false,
    memory: true,
  });
  expect(planAgentContext("chat", "native-1", files("s2", "m1"))).toEqual({
    soul: true,
    memory: true,
  });
  // A model switch or a rotated session starts a different native session.
  expect(planAgentContext("chat", "native-2", files())).toEqual({
    soul: true,
    memory: true,
  });
});

it("refreshes standing rules in an existing conversation after a context upgrade", () => {
  localStorage.setItem(
    HANDED_KEY,
    JSON.stringify({
      chat: { provider: "native-1", soulHash: "s1:v6", memoryHash: "m1" },
    }),
  );
  expect(planAgentContext("chat", "native-1", files())).toEqual({
    soul: true,
    memory: true,
  });
  recordAgentContext("chat", "native-1", files());
  expect(planAgentContext("chat", "native-1", files())).toEqual({
    soul: false,
    memory: false,
  });
});

it("loads only whole lines within the memory budget", () => {
  const memory = Array.from(
    { length: MEMORY_MAX_LINES + 5 },
    (_, i) => `- fact ${i}`,
  ).join("\n");
  const budget = memoryWithinBudget(memory);
  expect(budget.lines).toBe(MEMORY_MAX_LINES + 5);
  expect(budget.droppedLines).toBe(5);
  expect(budget.text.split("\n")).toHaveLength(MEMORY_MAX_LINES);
  expect(memoryWithinBudget("").lines).toBe(0);
});

it("says how to keep memory and update the soul only at the user's request", () => {
  const look = {
    name: "Skull",
    mascot: "skull",
    color: "#fff",
    projects: [
      { path: "/code/monocode", name: "monocode" },
      { path: "/code/site", name: "site" },
    ],
  };
  const full = monoContext(look, files(), { soul: true, memory: true });
  expect(full).toContain("You are Skull, a Mono in Monoptah");
  expect(full).toContain("these 2 projects (monocode and site)");
  expect(full).toContain("- site: /code/site");
  expect(full).toContain("Be brief.");
  expect(full).toContain(
    "Only when the user asks you to change your soul or standing instructions",
  );
  expect(full).toContain("app soul.read {}");
  expect(full).toContain(
    'app soul.update {"text":"<complete updated Markdown>","expectedHash":"<hash from soul.read>"}',
  );
  expect(full).toContain("preserving the other instructions");
  expect(full).toContain(
    "Never change your soul on your own or edit its files directly",
  );
  expect(full).not.toContain("never edit that file yourself");
  expect(full).toContain("app memory.add");
  expect(full).toContain('app artifacts.write {"kind":"document"');
  expect(full).toContain("Artifacts are separate from the user's Notes");
  expect(full).toContain("without repeating its contents");
  expect(full).not.toContain("app documents.write");
  expect(full).toContain('"notifyOnComplete":true');
  expect(full).toContain(
    "Submitted sessions notify you on completion by default",
  );
  expect(full).toContain('set "notifyOnComplete":false on sessions.start');
  expect(full).toContain("waits for every session in that group to stop");
  expect(full).toContain("give one consolidated report");
  expect(full).toContain("acknowledge the action in your current reply");
  expect(full).toContain("dismisses your pending report for that session");
  expect(full).toContain("do not generate a later completion notification");
  expect(full).toContain(
    "Sessions launched during later turns form separate groups",
  );
  expect(full).toContain(
    "Topic notes, read on demand with memory.read: releases",
  );
  expect(full).toContain("releases are tag-triggered");
  const memoryOnly = monoContext(look, files(), {
    soul: false,
    memory: true,
  });
  expect(memoryOnly).not.toContain("<soul>");
  expect(memoryOnly).toContain("<memory>");
  const alone = monoContext({ ...look, projects: [] }, files(), {
    soul: true,
    memory: false,
  });
  expect(alone).toContain("has not given you any projects yet");
});

it("describes per-session delivery, nesting and folders for launched sessions", () => {
  const look = {
    name: "Skull",
    mascot: "skull",
    color: "#fff",
    projects: [{ path: "/code/monocode", name: "monocode" }],
  };
  const full = monoContext(look, files(), { soul: true, memory: true });
  expect(MONO_PROMPT_VERSION).toBe(21);
  expect(full).toContain('"kind":"interval"');
  expect(full).toContain('"completionDelivery":"each"');
  expect(full).toContain("as soon as that session stops");
  expect(full).toContain("without waiting for the turn or sibling sessions");
  expect(full).toContain("habit run always reports this way");
  expect(full).toContain("parentSessionId");
  expect(full).toContain("nested under that parent in the sidebar");
  expect(full).toContain("only one level deep");
  expect(full).toContain("lost when Monoptah restarts");
  expect(full).toContain("folderName");
  expect(full).toContain("folderId");
  expect(full).toContain("never renamed");
  expect(full).toContain("always creates a new folder");
  expect(full).toContain("Folder was not found in this project");
  expect(full).toContain('"Show sessions in sidebar"');
  expect(full).toContain('"Group in a folder"');
  expect(full).toContain("inset sidebar layout");
  expect(full).toContain("request ids without hyphens");
});

it("seeds SOUL.md once, moving the old instructions into it", async () => {
  const { id } = createMono(["/code/app"]);
  updateMono(id, (mono) => ({ ...mono, instructions: "Ship on Fridays" }));
  invoke.mockImplementation(async (command: string) =>
    command === "mono_load" ? { ...files("empty"), soul: null } : "seeded",
  );
  const [first, second] = await Promise.all([
    loadMonoFiles(id),
    loadMonoFiles(id),
  ]);
  expect(first).toBe(second);
  expect(first.soul).toBe(defaultSoul("Ship on Fridays"));
  expect(first.soulHash).toBe("seeded");
  const saves = invoke.mock.calls.filter(
    ([command]) => command === "mono_save",
  );
  expect(saves).toHaveLength(1);
  expect(saves[0][1]).toMatchObject({
    mono: id,
    path: "SOUL.md",
    expectedHash: "empty",
  });
  expect(findMono(id)?.instructions).toBeUndefined();
});

it("resets a Mono to the default soul and name, over the soul it loaded", async () => {
  const { id } = createMono();
  updateMono(id, (mono) => ({ ...mono, name: "Broski" }));
  invoke.mockImplementation(async (command: string) =>
    command === "mono_load"
      ? { ...files("mine"), soul: "# Soul\n\nBe a pirate." }
      : "reset",
  );
  await resetMonoDefaults(id);
  const saves = invoke.mock.calls.filter(
    ([command]) => command === "mono_save",
  );
  expect(saves).toHaveLength(1);
  expect(saves[0][1]).toMatchObject({
    path: "SOUL.md",
    text: defaultSoul(),
    expectedHash: "mine",
  });
  expect(findMono(id)?.name).toBeUndefined();
});

it("reads a migrated Mono's old project folder once, then its own", async () => {
  const { id } = createMono(["/code/app"]);
  updateMono(id, (mono) => ({ ...mono, legacyProject: "/code/app" }));
  invoke.mockImplementation(async () => ({ ...files(), soul: "# Soul" }));
  await loadMonoFiles(id);
  expect(invoke.mock.calls[0]).toEqual([
    "mono_load",
    { mono: id, legacyProject: "/code/app" },
  ]);
  expect(findMono(id)?.legacyProject).toBeUndefined();
  await loadMonoFiles(id);
  expect(invoke.mock.calls[1]).toEqual([
    "mono_load",
    { mono: id, legacyProject: null },
  ]);
});

it("puts what the app adds ahead of the user's message, marked as its own", () => {
  expect(monoTurn("what is this", [])).toBe("what is this");
  const turn = monoTurn("what is this", ["<mono>\nYou are Cat.\n</mono>"]);
  expect(turn.startsWith("<monocode_context>")).toBe(true);
  expect(turn).toContain("they did not write it");
  expect(turn.indexOf("You are Cat.")).toBeLessThan(
    turn.indexOf("what is this"),
  );
  expect(turn.endsWith("</monocode_context>\n\nwhat is this")).toBe(true);
});

it("monoContext tells the Mono to read the guide", () => {
  const look = {
    name: "Skull",
    mascot: "skull",
    color: "#fff",
    projects: [{ path: "/code/monocode", name: "monocode" }],
  };
  const full = monoContext(look, files(), { soul: true, memory: true });
  const rules = full.slice(
    full.indexOf("<workflows_rules>"),
    full.indexOf("</workflows_rules>"),
  );
  expect(rules).toContain("app workflows.guide");
  expect(rules).toContain("workflows.validate");
  expect(rules).toContain("A workflow you add is always saved paused");
  expect(rules).toContain("only then save it with `workflows.add`");
});

it("monoContext includes workflows_rules", () => {
  const look = {
    name: "Skull",
    mascot: "skull",
    color: "#fff",
    projects: [{ path: "/code/monocode", name: "monocode" }],
  };
  const full = monoContext(look, files(), { soul: true, memory: true });
  expect(full).toContain("<workflows_rules>");
  expect(full).toContain("</workflows_rules>");
  expect(full).toContain("{{nodeId.field}}");
  expect(full).toContain("{{run.round}}");
  expect(full).toContain("Projects are always inputs, never paths.");
  expect(full).toContain("`workflows.resumeRun`");
  expect(full).toContain("show them the steps first");
  expect(full).toContain(
    "A session node's harness and model take exact ids from `app models.list`, and effort one of that model's listed values",
  );
  expect(full).toContain("workflows.run needs the workflow to be resumed (not paused)");
  expect(full).toContain(
    "A workflow you add is always saved paused, however clean it is: the user enables it in the Workflows panel",
  );
  expect(full.indexOf("</habits_rules>")).toBeLessThan(
    full.indexOf("<workflows_rules>"),
  );
  // Only the soul turn carries the rules, like the habits rules.
  expect(
    monoContext(look, files(), { soul: false, memory: true }),
  ).not.toContain("<workflows_rules>");
});
