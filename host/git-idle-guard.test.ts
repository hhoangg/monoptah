import { execFileSync } from "node:child_process";
import { mkdirSync, mkdtempSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, expect, it, vi } from "vitest";
import { HostEngine, type HostProvider } from "./engine";
import { HostStore } from "./store";
import { WorkspaceCommands } from "./workspace-commands";
import { GIT_NETWORK_TIMEOUT_MS } from "./workspace";

const calls = vi.hoisted(() => [] as Array<{ args: string[]; timeout: unknown }>);
vi.mock("node:child_process", async (original) => {
  const actual = await original<typeof import("node:child_process")>();
  const { promisify } = await import("node:util");
  const execFile = Object.assign(
    (...args: unknown[]) => (actual.execFile as (...a: unknown[]) => unknown)(...args),
    {
      [promisify.custom]: (file: string, args: string[], options: { timeout?: unknown }) => {
        if (file === "git") calls.push({ args, timeout: options?.timeout });
        return promisify(actual.execFile)(file, args, options as never);
      },
    },
  );
  return { ...actual, execFile };
});

const cleanups: Array<() => void | Promise<void>> = [];
afterEach(async () => {
  calls.length = 0;
  for (const cleanup of cleanups.splice(0).reverse()) await cleanup();
});

function setup() {
  const base = realpathSync.native(mkdtempSync(join(tmpdir(), "monoptah-idle-")));
  const home = join(base, "repo");
  mkdirSync(home);
  const store = new HostStore(join(base, "host.db"));
  const run = (...args: string[]) =>
    execFileSync("git", args, { cwd: home, encoding: "utf8" }).trim();
  run("init", "-q", "-b", "main");
  run("config", "user.name", "Test");
  run("config", "user.email", "test@example.com");
  run("config", "commit.gpgsign", "false");
  writeFileSync(join(home, "f.txt"), "one\n");
  run("add", ".");
  run("commit", "-q", "-m", "Initial");
  const project = store.addProject(home, "repo");
  const turns: Array<() => void> = [];
  const provider: HostProvider = {
    send: vi.fn(() => new Promise<void>((resolve) => turns.push(resolve))),
    cancel: vi.fn(async () => turns.at(-1)?.()),
    stop: vi.fn(async () => turns.at(-1)?.()),
    bind: vi.fn(),
    approve: vi.fn(),
    answer: vi.fn(),
  };
  const engine = new HostEngine(store, { codex: provider, claude: provider });
  const commands = new WorkspaceCommands(store, (id, action, what) =>
    engine.withIdleProject(id, action, what),
  );
  cleanups.push(async () => {
    turns.forEach((finish) => finish());
    await engine.close();
    store.close();
    rmSync(base, { recursive: true, force: true });
  });
  const startSession = async () => {
    const { sessionId } = engine.command({
      type: "create",
      commandId: "c",
      projectId: project.id,
      harness: "codex",
      model: "codex:test",
      runtimeMode: "supervised",
    });
    engine.command({ type: "send", commandId: "s", sessionId, text: "Work" });
    await vi.waitFor(() => expect(turns).toHaveLength(1));
    return () => turns.forEach((finish) => finish());
  };
  return { home, run, commands, startSession };
}

const dirty = (home: string) => writeFileSync(join(home, "f.txt"), "two\n");

it.each([
  ["git_discard_file", (home: string) => ({ cwd: home, relative: "f.txt" })],
  ["git_discard_all", (home: string) => ({ cwd: home })],
  ["git_stash", (home: string) => ({ cwd: home })],
])("%s is refused while a session runs and allowed when idle", async (command, args) => {
  const { home, run, commands, startSession } = setup();
  dirty(home);
  const finish = await startSession();
  await expect(commands.run(command, args(home))).rejects.toThrow(
    /Wait for running host sessions before (discarding|stashing) changes/,
  );
  expect(run("status", "--porcelain")).toContain("f.txt");
  finish();
  await vi.waitFor(async () => {
    await commands.run(command, args(home));
  });
  expect(run("status", "--porcelain")).toBe("");
});

it("push, pull and sync pass a 90 s timeout", async () => {
  expect(GIT_NETWORK_TIMEOUT_MS).toBe(90_000);
  const { home, commands } = setup();
  const timeouts = async (command: string, wanted: string) => {
    calls.length = 0;
    await commands.run(command, { cwd: home }).catch(() => undefined);
    return calls.filter((call) => call.args.includes(wanted)).map((call) => call.timeout);
  };
  expect(await timeouts("git_push", "push")).toEqual([90_000]);
  expect(await timeouts("git_pull", "pull")).toEqual([90_000]);
  // Sync stops at the failing pull (no remote); both steps are the same code paths.
  expect(await timeouts("git_sync", "pull")).toEqual([90_000]);
});

it("leaves other git commands on their old timeout", async () => {
  const { home, commands } = setup();
  await commands.run("git_head_message", { cwd: home });
  expect(calls.find((call) => call.args.includes("log"))?.timeout).toBe(30_000);
});
