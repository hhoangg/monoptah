import { createHash } from "node:crypto";
import { mkdtempSync, readFileSync, realpathSync, rmSync } from "node:fs";
import type { AddressInfo } from "node:net";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, expect, it, vi } from "vitest";
import { HostEngine } from "./engine";
import { createHostServer } from "./server";
import { HostStore } from "./store";
import { claudeProjectDirName } from "./transcript-sync";

const FIXTURE = join(__dirname, "..", "crates/monocode-core/tests/fixtures/transcript_sync/base.jsonl");
const cleanups: Array<() => Promise<void> | void> = [];
afterEach(async () => {
  vi.unstubAllEnvs();
  for (const cleanup of cleanups.splice(0).reverse()) await cleanup();
});

it("advertises both methods and imports a transcript over the API", async () => {
  const directory = realpathSync(mkdtempSync(join(tmpdir(), "move-server-test-")));
  vi.stubEnv("CLAUDE_CONFIG_DIR", join(directory, "claude"));
  const store = new HostStore(join(directory, "host.db"));
  const bind = vi.fn();
  const provider = {
    send: async () => {}, cancel: async () => {}, stop: async () => {},
    bind, approve: () => {}, answer: () => {},
  };
  const engine = new HostEngine(store, { claude: provider });
  const project = await engine.openProject(directory);
  const server = createHostServer(engine, ["claude"]);
  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
  cleanups.push(async () => {
    await engine.close();
    server.closeAllConnections();
    await new Promise<void>((resolve) => server.close(() => resolve()));
    store.close();
    rmSync(directory, { recursive: true, force: true });
  });
  const url = `http://127.0.0.1:${(server.address() as AddressInfo).port}/rpc`;
  const { token } = store.issueDevice("Laptop");
  const call = async (method: string, params: unknown = {}, bearer = token) => {
    const response = await fetch(url, {
      method: "POST",
      headers: { Authorization: `Bearer ${bearer}` },
      body: JSON.stringify({ version: 1, environmentId: store.environmentId, method, params }),
    });
    return { status: response.status, value: (await response.json()) as { result?: any; error?: string } };
  };

  const described = await call("environment.describe");
  expect(described.value.result.capabilities).toEqual(
    expect.arrayContaining(["workspace.applySnapshot", "sessions.import", "uploads"]),
  );

  const data = readFileSync(FIXTURE);
  const begun = await call("uploads.begin", {
    name: "t.jsonl", size: data.length, sha256: createHash("sha256").update(data).digest("hex"),
  });
  const { uploadId } = begun.value.result;
  await call("uploads.chunk", { uploadId, offset: 0, data: data.toString("base64") });
  await call("uploads.commit", { uploadId });

  const args = {
    projectId: project.id, providerSessionId: "11111111-1111-4111-8111-111111111111",
    uploadId, cwd: directory, title: "Moved", model: "claude:test",
  };
  expect((await call("sessions.import", args, "invalid")).status).toBe(401);
  const done = await call("sessions.import", args);
  expect(done.status).toBe(200);
  expect(done.value.result).toMatchObject({ decision: "installed", reused: false });
  expect(
    readFileSync(join(directory, "claude", "projects", claudeProjectDirName(directory), `${args.providerSessionId}.jsonl`), "utf8"),
  ).toContain(directory);
  expect(bind).toHaveBeenCalledWith(done.value.result.sessionId, args.providerSessionId, directory);
  expect((await call("workspace.applySnapshot", { projectId: project.id, branch: "x" })).status).toBe(400);
});
