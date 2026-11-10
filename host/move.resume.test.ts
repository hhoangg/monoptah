import { mkdtempSync, readFileSync, realpathSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, expect, it, vi } from "vitest";
import { createHash } from "node:crypto";

const spawned: string[][] = [];
const sent: string[] = [];
let onLine: ((line: string) => void) | undefined;
vi.mock("../src/integrations/harness/core/child", async (original) => ({
  ...(await original<typeof import("../src/integrations/harness/core/child")>()),
  resolveClaudeBinary: async () => ({ path: "/fake/claude" }),
  spawnChild: async (_id: string, _path: string, args: string[]) => {
    spawned.push(args);
  },
  killChild: async () => undefined,
  unwatchChild: () => undefined,
  watchChild: (_id: string, line: (l: string) => void) => {
    onLine = line;
  },
  writeChild: async (_id: string, line: string) => {
    sent.push(line);
  },
}));

const { HostEngine } = await import("./engine");
const { HostStore } = await import("./store");
const { HostUploads } = await import("./uploads");
const { HostMoves } = await import("./move");
const { hostProviders } = await import("./providers");

const FIXTURE = join(__dirname, "..", "crates/monocode-core/tests/fixtures/transcript_sync/base.jsonl");
const cleanups: Array<() => Promise<void> | void> = [];
afterEach(async () => {
  for (const cleanup of cleanups.splice(0).reverse()) await cleanup();
});

it("the first turn after an import resumes the SAME Claude session id and never forks", async () => {
  const dir = realpathSync(mkdtempSync(join(tmpdir(), "move-resume-test-")));
  const store = new HostStore(join(dir, "host.db"));
  const engine = new HostEngine(store, { claude: hostProviders.claude });
  cleanups.push(async () => {
    await engine.close();
    store.close();
    rmSync(dir, { recursive: true, force: true });
  });
  const uploads = new HostUploads(join(dir, "uploads"));
  const moves = new HostMoves(engine, uploads, { claudeConfigDir: () => join(dir, "claude") });
  const project = store.addProject(dir, "p");
  const data = readFileSync(FIXTURE);
  const { uploadId } = uploads.begin({
    name: "t.jsonl", size: data.length, sha256: createHash("sha256").update(data).digest("hex"),
  });
  uploads.chunk({ uploadId, offset: 0, data: data.toString("base64") });
  uploads.commit({ uploadId });
  const id = "11111111-1111-4111-8111-111111111111";
  const imported = await moves.importSession({
    projectId: project.id, providerSessionId: id, uploadId, cwd: dir, model: "claude:claude-sonnet-5",
  });
  expect(store.session(imported.sessionId).session.blocks).toHaveLength(0);

  engine.command({ type: "send", commandId: "first", sessionId: imported.sessionId, text: "continue" });
  await vi.waitFor(() => expect(spawned).toHaveLength(1));
  expect(spawned[0]).toEqual(expect.arrayContaining(["--resume", id]));
  expect(spawned[0][spawned[0].indexOf("--resume") + 1]).toBe(id);
  expect(spawned[0]).not.toContain("--fork-session");
  expect(spawned[0]).not.toContain("--session-id");
  // Play the Claude process: initialise, accept the prompt, finish the turn.
  const emit = (record: Record<string, unknown>) => onLine!(JSON.stringify(record));
  await vi.waitFor(() => expect(sent.some((line) => line.includes('"initialize"'))).toBe(true));
  emit({ type: "system", subtype: "init", session_id: id });
  emit({ type: "control_response", response: { subtype: "success", request_id: "monocode_1" } });
  await vi.waitFor(() => expect(sent.some((line) => line.includes('"type":"user"'))).toBe(true));
  emit({ type: "result", subtype: "success", session_id: id });
  await vi.waitFor(() => expect(store.session(imported.sessionId).status).toBe("idle"));
});
