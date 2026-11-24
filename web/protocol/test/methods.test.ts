import { describe, expect, it } from "vitest";
import {
  METHODS,
  METHOD_CLASSES,
  METHOD_LIST,
  FolderCardSchema,
  SessionStartTargetSchema,
  RuntimeModeSchema,
  classify,
  isMethodName,
  isSafeTerminalSubdir,
  REQUEST_TIMEOUT_BY_METHOD,
  TERMINAL_EXIT_REASONS,
  TERMINAL_READ_MAX_WAIT_MS,
  TERMINAL_READ_TIMEOUT_SLACK_MS,
  type MethodName,
} from "../src/index.ts";
import { readText } from "./helpers.ts";

const turn = { id: "t1", items: [{ type: "text", id: "a", text: "Hello" }], completedAt: 1 };
const session = {
  id: "s1",
  projectId: "p1",
  title: "Fix the build",
  harness: "claude",
  model: "claude:opus",
  branch: "main",
  runtimeMode: "supervised",
  updatedAt: 1_700_000_000,
  target: { kind: "codespace", instance: "cs-1", state: "available" },
  cloudState: "available",
};
const Ok = { sessionId: "s1" };
const cloud = {
  kind: "codespace",
  instance: "cs-1",
  state: "starting",
  startup: { steps: [{ id: "create", status: "done", startedAt: 1, finishedAt: 2 }], current: "provision" },
};

/** One realistic params and result per method. The type makes a missing entry a compile error. */
const fixtures: Record<MethodName, { params: unknown; result: unknown }> = {
  "conversation.read": {
    params: { conversationId: "s1" },
    result: { id: "s1", messages: [], turns: [turn], busy: false, usageLimit: null, older: true, question: null, cloud, approvals: [{ requestId: "3", label: "Run npm test", detail: "npm test" }] },
  },
  "conversation.older": { params: { conversationId: "s1", before: "t1", limit: 20 }, result: { turns: [turn], older: false } },
  "conversation.watch": {
    params: { conversationId: "s1", tail: 3, from: "t1", incremental: true },
    result: { usageLimit: null, busy: true, older: false, order: ["t1"], turns: [turn] },
  },
  "conversation.send": {
    params: { conversationId: "s1", text: "Please continue", attachments: [{ name: "a.png", mimeType: "image/png", data: "iVBORw0KGgo=" }] },
    result: { sessionId: "s1", wake: true, queued: true },
  },
  "commands.list": {
    params: { conversationId: "s1" },
    result: {
      commands: [
        { id: "plan", kind: "builtin", label: "Plan mode", detail: "Review a plan before building", icon: "plan", available: true, mode: true, active: false },
        { id: "compact", kind: "builtin", label: "Compact context", detail: "Free space", icon: "compact", available: false, reason: "Send a message first", mode: false, active: false },
        { id: "lint", kind: "skill", label: "lint", detail: "Lint the project", icon: "skill", available: true, mode: false, active: false },
      ],
    },
  },
  "commands.run": { params: { conversationId: "s1", command: "plan", enabled: true }, result: { command: "plan", active: true } },
  "models.list": {
    params: { conversationId: "s1" },
    result: {
      harnesses: [{ id: "claude", label: "Claude Code" }],
      models: [{ id: "claude:opus", harness: "claude", name: "Opus", settings: [{ id: "effort", label: "Effort", kind: "select", value: "high", options: [{ value: "high", label: "High" }] }] }],
      runtimeModes: [{ id: "supervised", label: "Supervised", hint: "Ask before edits" }],
      accounts: [{ id: "default", provider: "claude", label: "Personal", isDefault: true }],
      selectedAccounts: { claude: "default" },
      current: { harness: "claude", model: "claude:opus", modelSettings: { effort: "high" }, runtimeMode: "full-access" },
    },
  },
  "accounts.usage": {
    params: { provider: "claude" },
    result: { accounts: { default: { status: { tone: "ready", label: "Ready", detail: "ok" }, updatedAt: 1, stale: false, meters: [{ title: "5h", usedPercent: 10, percent: 10, label: "10% used", reset: "in 2h" }] } } },
  },
  "session.configure": { params: { conversationId: "s1", harness: "claude", model: "claude:opus", modelSettings: { effort: "high" }, runtimeMode: "auto-accept-edits" }, result: Ok },
  "session.start": { params: { projectId: "p1", text: "Add a test", harness: "claude", runtimeMode: "supervised", workspaceMode: "worktree" }, result: Ok },
  "session.resumeUsage": { params: { conversationId: "s1", token: "tok" }, result: Ok },
  "session.resumeUsageAtReset": { params: { conversationId: "s1", token: "tok", enabled: true }, result: Ok },
  "session.answerQuestion": {
    params: { conversationId: "s1", requestId: 4, reply: { kind: "answered", answers: { q1: ["o1"] }, custom: { q2: "text" } } },
    result: Ok,
  },
  "session.keepQuestionOpen": { params: { conversationId: "s1", requestId: 4 }, result: Ok },
  "session.answerApproval": { params: { conversationId: "s1", requestId: "3", decision: "allow" }, result: Ok },
  "session.cancel": { params: { conversationId: "s1" }, result: Ok },
  "session.rename": { params: { conversationId: "s1", title: "New title" }, result: Ok },
  "session.archive": { params: { conversationId: "s1", cloud: "stop" }, result: Ok },
  "session.delete": { params: { conversationId: "s1" }, result: Ok },
  "folder.create": { params: { projectId: "p1", conversationIds: ["s1", "s2"], name: "Docs" }, result: { folderId: "f1" } },
  "folder.rename": { params: { projectId: "p1", folderId: "f1", name: "Specs" }, result: { folderId: "f1" } },
  "folder.move": { params: { projectId: "p1", conversationId: "s1", folderId: null }, result: { folderId: null } },
  "folder.delete": { params: { projectId: "p1", folderId: "f1" }, result: { folderId: "f1" } },
  "project.changes": {
    params: { projectId: "p1" },
    result: { branch: "main", files: [{ relative: "src/a.ts", status: "modified", additions: 1, deletions: 0, staged: false, unstaged: true, lines: [{ kind: "add", text: "x", oldNumber: null, newNumber: 1 }] }] },
  },
  "project.worktrees": { params: { projectId: "p1" }, result: { worktrees: [{ path: "/w/a", name: "feature" }] } },
  "project.files": { params: { projectId: "p1", path: "src" }, result: { entries: [{ name: "a.ts", isDir: false, ignored: false }] } },
  "project.file": { params: { projectId: "p1", path: "src/a.ts" }, result: { path: "src/a.ts", text: "x", truncated: false } },
  "chat.mentionedFile": {
    params: { conversationId: "s1", path: "docs/report.html", asset: "img/a.png", offset: 0, source: false },
    result: { path: "/work/app/docs/report.html", name: "report.html", size: 12, kind: "html", encoding: "utf8", data: "<p></p>", offset: 0, total: 7, assets: ["img/a.png"], project: { projectId: "p1", path: "docs/report.html" } },
  },
  "file.prepare": {
    params: { conversationId: "s1", path: "docs/report.html", asset: "img/a.png" },
    result: {
      path: "/work/app/docs/report.html",
      name: "report.html",
      kind: "html",
      mime: "text/html",
      size: 12,
      hash: "a".repeat(64),
      object: `${"a".repeat(64)}.${"b".repeat(16)}`,
      key: "A".repeat(43) + "=",
      nonce: "A".repeat(11) + "=",
      chunkBytes: 1048576,
      reused: false,
      assets: ["img/a.png"],
      project: { projectId: "p1", path: "docs/report.html" },
    },
  },
  "project.search": { params: { projectId: "p1", query: "app" }, result: { results: [{ path: "src/app.ts", name: "app.ts" }] } },
  "project.fileHighlight": { params: { job: "j1", from: 0 }, result: { palette: ["#fff"], lines: [], done: true } },
  "project.git": {
    params: { projectId: "p1" },
    result: { branch: "main", files: 2, remote: true, remoteName: "origin", upstream: true, defaultBranch: "main", ahead: 1, behind: 0, aheadOfDefault: 1, pr: { number: 5, url: "https://example.com/pr/5" } },
  },
  "project.gitAction": { params: { projectId: "p1", action: "commit", message: "fix", paths: ["a.ts"] }, result: { message: "ok" } },
  "mono.document": { params: { documentId: "d1" }, result: { id: "d1", title: "Doc" } },
  "mono.profile": { params: { monoId: "m1" }, result: { id: "m1" } },
  "mono.file": { params: { monoId: "m1", file: "soul" }, result: { text: "soul", hash: "abc" } },
  "mono.create": { params: {}, result: { monoId: "m2" } },
  "mono.update": { params: { monoId: "m1", name: "Ada", color: "#aabbcc", subsessions: { harness: "claude", model: "claude:opus" } }, result: null },
  "mono.saveFile": { params: { monoId: "m1", file: "memory", text: "x", expectedHash: "abc" }, result: { hash: "def" } },
  "mono.emoji": { params: { monoId: "m1", hash: "a".repeat(64) }, result: { hash: "a".repeat(64), mime: "image/png", encoding: "base64", data: "AAAA" } },
  "mono.reset": { params: { monoId: "m1", ticket: "t1", deleteFiles: false }, result: null },
  "mono.delete": { params: { monoId: "m1" }, result: null },
  "workspace.list": {
    params: {},
    result: {
      projects: [{ id: "p1", name: "app", path: "/work/app", branch: "main", additions: 0, deletions: 0 }],
      monos: [{ id: "m1", name: "Ada", mascot: "cat", color: "#aabbcc", projectIds: ["p1"], unread: 0, status: "idle", lastMessage: "", lastActiveAt: 1 }],
      sessions: [session],
      folders: [{ id: "f1", projectId: "p1", name: "Docs", sessionIds: ["s1"], collapsed: true, colorIndex: 3, customColor: "#aabbcc" }],
      monoTools: true,
      monoFiles: true,
      push: true,
    },
  },
  "sessions.list": { params: { projectId: "p1", offset: 0, limit: 30, query: "fix" }, result: { sessions: [session], hasMore: false } },
  "device.leave": { params: {}, result: null },
  "device.push": { params: { token: "t" }, result: null },
  "device.enroll": { params: { invitation: "x" }, result: null },
  "device.selection": { params: {}, result: { projects: ["p1"], monos: null } },
  "cloud.instances": {
    params: {},
    result: {
      instances: [{ kind: "codespace", instance: "cs-1", displayName: "cs-1", state: "available", rawState: "Available", lastUsedAt: 1, sessions: [{ sessionId: "s1", title: "Fix", busy: false }], costEstimatePerHour: 0.36, stopReport: "stopping" }],
      backends: [{ kind: "codespace", ready: true }],
      otherCount: 2,
    },
  },
  "cloud.startOptions": { params: { projectId: "p1" }, result: { backends: [{ kind: "sandbox", ready: true }], branches: ["main"], machines: [{ id: "basic", name: "Basic" }], sizes: [{ id: "small" }] } },
  "cloud.stop": { params: { sessionId: "s1" }, result: { stopped: true, warning: "1 busy session" } },
  "cloud.resume": { params: { sessionId: "s1" }, result: { accepted: true } },
  "cloud.retryStart": { params: { sessionId: "s1" }, result: {} },
  "cloud.keepRunning": { params: { sessionId: "s1" }, result: {} },
  "cloud.moveToCloud": { params: { sessionId: "s1", target: { kind: "sandbox", instance: "new", size: "small" } }, result: {} },
  "cloud.moveToLocal": { params: { sessionId: "s1" }, result: {} },
  "cloud.cancelMove": { params: { sessionId: "s1" }, result: {} },
  "desktop.updateStatus": {
    params: {},
    result: { current: "1.0.0", latest: "1.1.0", updateAvailable: true, notes: "Fixes", runningSessions: [{ id: "s1", title: "Fix bug" }], lastFailure: "Update to 1.1.0 failed: disk full" },
  },
  "terminal.open": { params: { projectId: "p1", subdir: "src/app", cols: 80, rows: 24, ticket: "tk1" }, result: { terminalId: "t1", cols: 80, rows: 24, cwd: "app/src", idleTimeoutMs: 900000, maxDurationMs: 28800000 } },
  "terminal.read": { params: { terminalId: "t1", from: 0, waitMs: 15000, maxBytes: 32768 }, result: { from: 0, next: 5, data: "aGVsbG8=", exited: null } },
  "terminal.write": { params: { terminalId: "t1", data: "bHMK" }, result: {} },
  "terminal.resize": { params: { terminalId: "t1", cols: 100, rows: 30 }, result: {} },
  "terminal.close": { params: { terminalId: "t1" }, result: {} },
  "desktop.update": {
    params: { confirm: true },
    result: { status: "confirmation_required", runningSessions: [{ id: "s1", title: "Fix bug" }] },
  },
};

describe("every method has a typed entry", () => {
  it("covers exactly the METHODS table", () => {
    expect(Object.keys(fixtures).sort()).toEqual(Object.keys(METHODS).sort());
  });

  for (const { name, def } of METHOD_LIST) {
    it(`${name}: accepts a realistic fixture (class ${def.class}, ${def.transport})`, () => {
      const fixture = fixtures[name];
      expect(def.params.safeParse(fixture.params).success, JSON.stringify(def.params.safeParse(fixture.params).error?.issues)).toBe(true);
      expect(def.result.safeParse(fixture.result).success, JSON.stringify(def.result.safeParse(fixture.result).error?.issues)).toBe(true);
    });
  }

  it("uses only the agreed class and transport strings", () => {
    for (const { def } of METHOD_LIST) {
      expect(METHOD_CLASSES).toContain(def.class);
      expect(["app", "host"]).toContain(def.transport);
    }
    expect(METHOD_CLASSES).toEqual(["read", "control", "param", "denied", "unsupportedWeb", "terminal"]);
  });

  it("marks device.enroll QUIC-only and the host-answered set as host", () => {
    expect(METHODS["device.enroll"].quicOnly).toBe(true);
    const host = METHOD_LIST.filter(({ def }) => def.transport === "host").map(({ name }) => name);
    expect(host.sort()).toEqual(
      [
        "conversation.watch",
        "device.enroll",
        "device.leave",
        "device.push",
        "device.selection",
        "sessions.list",
        "terminal.close",
        "terminal.open",
        "terminal.read",
        "terminal.resize",
        "terminal.write",
        "workspace.list",
      ].sort(),
    );
  });

  it("types every method named in the 7.1 table of architecture.md", () => {
    const doc = readText("../../docs/architecture.md");
    const section = doc.slice(doc.indexOf("### 7.1 Permission classification"), doc.indexOf("### 7.2 Grants sync"));
    const names = new Set([...section.matchAll(/`((?:[a-z]+)\.(?:[A-Za-z]+))`/g)].map((m) => m[1] as string));
    // Named in 7.1 only to say they are never exposed.
    const neverExposed = new Set(["cloud.delete", "cloud.create", "cloud.rebuild"]);
    const missing = [...names].filter((name) => !neverExposed.has(name) && !isMethodName(name));
    expect(missing).toEqual([]);
    expect(names.size).toBeGreaterThan(40);
    for (const name of neverExposed) expect(isMethodName(name)).toBe(false);
  });
});

describe("full-access is never sendable", () => {
  it("is a value the desktop may report", () => {
    expect(RuntimeModeSchema.safeParse("full-access").success).toBe(true);
    const models = METHODS["models.list"].result.safeParse(fixtures["models.list"].result);
    expect(models.success).toBe(true);
  });
  it("is rejected by session.configure and session.start params", () => {
    expect(METHODS["session.configure"].params.safeParse({ conversationId: "s1", runtimeMode: "full-access" }).success).toBe(false);
    expect(METHODS["session.start"].params.safeParse({ projectId: "p1", text: "x", runtimeMode: "full-access" }).success).toBe(false);
    for (const mode of ["supervised", "auto-accept-edits", "auto"]) {
      expect(METHODS["session.configure"].params.safeParse({ conversationId: "s1", runtimeMode: mode }).success).toBe(true);
    }
  });
});

describe("file.prepare", () => {
  const { params, result } = METHODS["file.prepare"];
  it("is a read method answered by the app", () => {
    expect(METHODS["file.prepare"].class).toBe("read");
    expect(METHODS["file.prepare"].transport).toBe("app");
  });
  it("needs a path, takes an optional conversation and asset, and limits them to 1024 chars", () => {
    expect(params.safeParse({ path: "/tmp/a.png" }).success).toBe(true);
    expect(params.safeParse({ conversationId: "s1", path: "a".repeat(1024), asset: "b".repeat(1024) }).success).toBe(true);
    expect(params.safeParse({ conversationId: "s1" }).success).toBe(false);
    expect(params.safeParse({ path: "" }).success).toBe(false);
    expect(params.safeParse({ path: "a".repeat(1025) }).success).toBe(false);
    expect(params.safeParse({ path: "a", asset: "b".repeat(1025) }).success).toBe(false);
  });
  it("answers an object named by a hash and a key id, never by a path", () => {
    const ok = { path: "/a", name: "a", kind: "text", mime: "text/plain", size: 1, hash: "a".repeat(64), object: `${"a".repeat(64)}.${"b".repeat(16)}`, key: "A".repeat(43) + "=", nonce: "A".repeat(11) + "=", chunkBytes: 1, reused: true };
    expect(result.safeParse(ok).success).toBe(true);
    expect(result.safeParse({ ...ok, object: "../etc/passwd" }).success).toBe(false);
    expect(result.safeParse({ ...ok, hash: "A".repeat(64) }).success).toBe(false);
    expect(result.safeParse({ ...ok, key: "short" }).success).toBe(false);
    expect(result.safeParse({ ...ok, kind: "video" }).success).toBe(false);
  });
});

describe("chat.mentionedFile", () => {
  const params = METHODS["chat.mentionedFile"].params;
  it("is a read method answered by the app", () => {
    expect(METHODS["chat.mentionedFile"].class).toBe("read");
    expect(METHODS["chat.mentionedFile"].transport).toBe("app");
  });
  it("limits the path and asset to 1024 chars and the offset to a non-negative integer", () => {
    expect(params.safeParse({ conversationId: "mono:m1", path: "a".repeat(1024) }).success).toBe(true);
    expect(params.safeParse({ conversationId: "s1", path: "a".repeat(1025) }).success).toBe(false);
    expect(params.safeParse({ conversationId: "s1", path: "a", asset: "b".repeat(1025) }).success).toBe(false);
    expect(params.safeParse({ conversationId: "s1", path: "a", offset: -1 }).success).toBe(false);
    expect(params.safeParse({ conversationId: "s1", path: "a", offset: 1.5 }).success).toBe(false);
    expect(params.safeParse({ path: "a" }).success).toBe(false);
  });
  it("takes an optional boolean source flag", () => {
    expect(params.safeParse({ conversationId: "s1", path: "a.html", source: true }).success).toBe(true);
    expect(params.safeParse({ conversationId: "s1", path: "a.html", source: "yes" }).success).toBe(false);
  });
  it("accepts only the five kinds and the three encodings", () => {
    const result = METHODS["chat.mentionedFile"].result;
    const base = { path: "/a", name: "a", size: 0, kind: "binary", encoding: "none", data: "", offset: 0, total: 0 };
    expect(result.safeParse(base).success).toBe(true);
    expect(result.safeParse({ ...base, kind: "video" }).success).toBe(false);
    expect(result.safeParse({ ...base, encoding: "hex" }).success).toBe(false);
  });
});

describe("terminal methods", () => {
  const names = ["terminal.open", "terminal.read", "terminal.write", "terminal.resize", "terminal.close"] as const;
  it("are class terminal on the host transport, and classify() keeps the class (control does not imply it)", () => {
    for (const name of names) {
      expect(METHODS[name].class).toBe("terminal");
      expect(METHODS[name].transport).toBe("host");
      expect(classify(name, {})).toBe("terminal");
      expect(classify(name, {}, { remoteGitPush: true, remoteCloudStart: true })).toBe("terminal");
    }
  });
  it("terminal.open validates the size, the subdir and the passkey ticket", () => {
    const open = METHODS["terminal.open"].params;
    const base = { projectId: "p", cols: 80, rows: 24, ticket: "tk" };
    expect(open.safeParse(base).success).toBe(true);
    expect(open.safeParse({ ...base, subdir: "src/app" }).success).toBe(true);
    // A ticket is required (no ticket, no shell) and has a bounded size.
    expect(open.safeParse({ projectId: "p", cols: 80, rows: 24 }).success).toBe(false);
    expect(open.safeParse({ ...base, ticket: "" }).success).toBe(false);
    expect(open.safeParse({ ...base, ticket: "x".repeat(257) }).success).toBe(false);
    for (const cols of [9, 501, 80.5]) expect(open.safeParse({ ...base, cols }).success, `cols ${cols}`).toBe(false);
    for (const rows of [1, 201, 0.5]) expect(open.safeParse({ ...base, rows }).success, `rows ${rows}`).toBe(false);
    expect(open.safeParse({ ...base, cols: 10, rows: 2 }).success).toBe(true);
    expect(open.safeParse({ ...base, cols: 500, rows: 200 }).success).toBe(true);
    expect(open.safeParse({ cols: 80, rows: 24, ticket: "tk" }).success).toBe(false);
    expect(open.safeParse({ ...base, projectId: "" }).success).toBe(false);
    expect(open.safeParse({ ...base, projectId: "p".repeat(257) }).success).toBe(false);
    expect(open.safeParse({ ...base, projectId: "p".repeat(256) }).success).toBe(true);
    for (const subdir of ["../etc", "a/../b", "a\\..\\b", "/abs", "\\abs", "C:\\x", "", "x".repeat(513)]) {
      expect(open.safeParse({ ...base, subdir }).success, subdir).toBe(false);
    }
    expect(open.safeParse({ ...base, subdir: "x".repeat(512) }).success).toBe(true);
    expect(open.safeParse({ ...base, subdir: "a..b/c" }).success).toBe(true);
  });
  it("isSafeTerminalSubdir rejects a NUL byte and a parent segment, keeps dotted names", () => {
    expect(isSafeTerminalSubdir("a\0b")).toBe(false);
    expect(isSafeTerminalSubdir("..")).toBe(false);
    expect(isSafeTerminalSubdir("./a")).toBe(true);
    expect(isSafeTerminalSubdir(".hidden/..cache")).toBe(true);
  });
  it("terminal.open answers with the opened terminal only (no challenge step)", () => {
    const result = METHODS["terminal.open"].result;
    const opened = result.parse({ terminalId: "t", cols: 80, rows: 24, cwd: "app/src", idleTimeoutMs: 900000, maxDurationMs: 28800000, future: 1 });
    expect(opened).toMatchObject({ terminalId: "t", future: 1 });
    expect(result.safeParse({ challengeId: "c", expiresInMs: 60000, codeLength: 6, attempts: 3 }).success).toBe(false);
    expect(result.safeParse({ cwd: "x" }).success).toBe(false);
  });
  it("terminal.read limits waitMs, maxBytes and from, and reports an exit", () => {
    const read = METHODS["terminal.read"].params;
    expect(read.safeParse({ terminalId: "t", from: 0 }).success).toBe(true);
    expect(read.safeParse({ terminalId: "t", from: 0, waitMs: 20000, maxBytes: 32768 }).success).toBe(true);
    expect(read.safeParse({ terminalId: "t", from: 0, waitMs: 0, maxBytes: 1 }).success).toBe(true);
    expect(read.safeParse({ terminalId: "t", from: -1 }).success).toBe(false);
    expect(read.safeParse({ terminalId: "t", from: 1.5 }).success).toBe(false);
    expect(read.safeParse({ terminalId: "t", from: 0, waitMs: 20001 }).success).toBe(false);
    expect(read.safeParse({ terminalId: "t", from: 0, maxBytes: 0 }).success).toBe(false);
    expect(read.safeParse({ terminalId: "t", from: 0, maxBytes: 32769 }).success).toBe(false);
    const result = METHODS["terminal.read"].result;
    expect(result.safeParse({ from: 0, next: 0, data: "", exited: { reason: "idle" } }).success).toBe(true);
    expect(result.safeParse({ from: 0, next: 0, data: "", exited: { reason: "exit", code: 2 } }).success).toBe(true);
    expect(result.safeParse({ from: 0, next: 0, data: "" }).success).toBe(false);
    for (const reason of TERMINAL_EXIT_REASONS) expect(result.safeParse({ from: 0, next: 0, data: "", exited: { reason } }).success).toBe(true);
  });
  it("terminal.write takes at most 8192 decoded bytes of base64 and resize validates the size", () => {
    const write = METHODS["terminal.write"].params;
    expect(write.safeParse({ terminalId: "t", data: btoa("a".repeat(8192)) }).success).toBe(true);
    expect(write.safeParse({ terminalId: "t", data: btoa("a".repeat(8193)) }).success).toBe(false);
    expect(write.safeParse({ terminalId: "t", data: "not base64!" }).success).toBe(false);
    expect(write.safeParse({ terminalId: "", data: "AA==" }).success).toBe(false);
    const resize = METHODS["terminal.resize"].params;
    expect(resize.safeParse({ terminalId: "t", cols: 80, rows: 24 }).success).toBe(true);
    expect(resize.safeParse({ terminalId: "t", cols: 9, rows: 24 }).success).toBe(false);
    expect(METHODS["terminal.close"].params.safeParse({ terminalId: "t" }).success).toBe(true);
    expect(METHODS["terminal.close"].params.safeParse({}).success).toBe(false);
  });
  it("terminal.read has a client timeout entry of at least waitMs max plus 10 s", () => {
    expect(REQUEST_TIMEOUT_BY_METHOD["terminal.read"]).toBe(TERMINAL_READ_MAX_WAIT_MS + TERMINAL_READ_TIMEOUT_SLACK_MS);
  });
});

describe("param validation", () => {
  it("limits attachments to 10 and the text to 100,000 chars", () => {
    const attachment = { name: "a.txt", mimeType: "text/plain", data: "AA==" };
    const send = METHODS["conversation.send"].params;
    expect(send.safeParse({ conversationId: "s", text: "x", attachments: Array(10).fill(attachment) }).success).toBe(true);
    expect(send.safeParse({ conversationId: "s", text: "x", attachments: Array(11).fill(attachment) }).success).toBe(false);
    expect(send.safeParse({ conversationId: "s", text: "x".repeat(100_001) }).success).toBe(false);
    expect(send.safeParse({ conversationId: "s", text: "x", attachments: [{ name: "", mimeType: "a/b", data: "" }] }).success).toBe(false);
  });
  it("accepts only the documented session.start target keys", () => {
    expect(SessionStartTargetSchema.safeParse({ kind: "codespace", instance: "new", branch: "main", machine: "basic" }).success).toBe(true);
    expect(SessionStartTargetSchema.safeParse({ kind: "sandbox", size: "small" }).success).toBe(true);
    for (const forbidden of ["kit", "template", "image", "cpus", "memory", "ttl"]) {
      expect(SessionStartTargetSchema.safeParse({ kind: "sandbox", [forbidden]: "x" }).success, forbidden).toBe(false);
    }
    expect(SessionStartTargetSchema.safeParse({ kind: "local" }).success).toBe(false);
  });
  it("session.archive takes cloud keep or stop only", () => {
    const archive = METHODS["session.archive"].params;
    expect(archive.safeParse({ conversationId: "s", cloud: "keep" }).success).toBe(true);
    expect(archive.safeParse({ conversationId: "s", cloud: "delete" }).success).toBe(false);
  });
  it("the folder methods are control class and validate ids and names", () => {
    for (const name of ["folder.create", "folder.rename", "folder.move", "folder.delete"] as const) {
      expect(METHODS[name].class).toBe("control");
      expect(classify(name, {})).toBe("control");
    }
    const create = METHODS["folder.create"].params;
    expect(create.safeParse({ projectId: "p", conversationIds: ["s"] }).success).toBe(true);
    expect(create.safeParse({ projectId: "p", conversationIds: [] }).success).toBe(false);
    expect(create.safeParse({ projectId: "p", conversationIds: Array.from({ length: 51 }, (_, i) => `s${i}`) }).success).toBe(false);
    expect(create.safeParse({ projectId: "p", conversationIds: ["s"], name: "  " }).success).toBe(false);
    expect(create.safeParse({ projectId: "p", conversationIds: ["s"], name: "x".repeat(101) }).success).toBe(false);
    expect(create.safeParse({ conversationIds: ["s"] }).success).toBe(false);
    const rename = METHODS["folder.rename"].params;
    expect(rename.safeParse({ projectId: "p", folderId: "f", name: "A" }).success).toBe(true);
    expect(rename.safeParse({ projectId: "p", folderId: "f", name: "" }).success).toBe(false);
    const move = METHODS["folder.move"].params;
    expect(move.safeParse({ projectId: "p", conversationId: "s", folderId: null }).success).toBe(true);
    expect(move.safeParse({ projectId: "p", conversationId: "s", folderId: "f" }).success).toBe(true);
    expect(move.safeParse({ projectId: "p", conversationId: "s" }).success).toBe(false);
    expect(METHODS["folder.delete"].params.safeParse({ projectId: "p", folderId: "" }).success).toBe(false);
  });
  it("a folder card needs a hex custom color and keeps unknown fields", () => {
    const card = { id: "f", projectId: "p", name: "N", sessionIds: ["s"] };
    expect(FolderCardSchema.safeParse(card).success).toBe(true);
    expect(FolderCardSchema.safeParse({ ...card, customColor: "red" }).success).toBe(false);
    expect(FolderCardSchema.parse({ ...card, future: 1 })).toMatchObject({ future: 1 });
    // A catalog from an older desktop has no folders.
    const { folders: _folders, ...older } = fixtures["workspace.list"].result as Record<string, unknown>;
    expect(METHODS["workspace.list"].result.safeParse(older).success).toBe(true);
  });
  it("cloud.stop takes a session or an instance", () => {
    const stop = METHODS["cloud.stop"].params;
    expect(stop.safeParse({ sessionId: "s" }).success).toBe(true);
    expect(stop.safeParse({ kind: "codespace", instance: "cs" }).success).toBe(true);
    expect(stop.safeParse({ kind: "codespace" }).success).toBe(false);
  });
  it("session.answerApproval names the request by the desktop's string id only", () => {
    const answer = METHODS["session.answerApproval"].params;
    expect(answer.safeParse({ conversationId: "s", requestId: "ap-1", decision: "allow" }).success).toBe(true);
    expect(answer.safeParse({ conversationId: "s", requestId: 3, decision: "allow" }).success).toBe(false);
    expect(answer.safeParse({ conversationId: "s", requestId: "ap-1", decision: "always" }).success).toBe(false);
  });
  it("session.answerQuestion needs a well-formed reply", () => {
    const answer = METHODS["session.answerQuestion"].params;
    expect(answer.safeParse({ conversationId: "s", requestId: 1, reply: { kind: "skipped" } }).success).toBe(true);
    expect(answer.safeParse({ conversationId: "s", requestId: 1, reply: { kind: "answered" } }).success).toBe(false);
  });
});

describe("classify (architecture.md 7.1)", () => {
  it("returns fixed classes unchanged", () => {
    expect(classify("conversation.read", { conversationId: "s" })).toBe("read");
    expect(classify("cloud.instances")).toBe("read");
    expect(classify("conversation.send", {})).toBe("control");
    expect(classify("session.answerApproval", {})).toBe("control");
    expect(classify("cloud.stop", { sessionId: "s" })).toBe("control");
    expect(classify("mono.saveFile", {})).toBe("denied");
    expect(classify("mono.reset", {})).toBe("control");
    expect(classify("mono.delete", {})).toBe("denied");
    expect(classify("device.push", {})).toBe("unsupportedWeb");
    expect(classify("project.fileHighlight", {})).toBe("unsupportedWeb");
  });
  it("answers unknown for an unclassified method", () => {
    expect(classify("cloud.delete", {})).toBe("unknown");
    expect(classify("nope.nope")).toBe("unknown");
    expect(classify("toString")).toBe("unknown");
  });
  it("accounts.usage: refresh is control", () => {
    expect(classify("accounts.usage", { provider: "claude" })).toBe("read");
    expect(classify("accounts.usage", { provider: "claude", refresh: false })).toBe("read");
    expect(classify("accounts.usage", { provider: "claude", refresh: true })).toBe("control");
  });
  it("device.selection: every param combination is read (the desktop enforces narrow vs widen and the device kind)", () => {
    expect(classify("device.selection", {})).toBe("read");
    expect(classify("device.selection", { projects: ["p1"] })).toBe("read");
    expect(classify("device.selection", { monos: [] })).toBe("read");
    expect(classify("device.selection", { projects: null, addProjects: ["r1"], addMonos: ["r2"], ticket: "t" })).toBe("read");
  });
  it("project.gitAction: stage, unstage, generate and a local commit are control", () => {
    for (const action of ["stage", "unstage", "stageAll", "unstageAll", "generate"]) {
      expect(classify("project.gitAction", { action })).toBe("control");
    }
    expect(classify("project.gitAction", { action: "commit", message: "m" })).toBe("control");
    expect(classify("project.gitAction", { action: "commit", push: false })).toBe("control");
  });
  it("project.gitAction: sync, pr and a pushing commit are denied unless remoteGitPush", () => {
    for (const params of [{ action: "sync" }, { action: "pr" }, { action: "commit", push: true }, { action: "commit", createPr: true }]) {
      expect(classify("project.gitAction", params)).toBe("denied");
      expect(classify("project.gitAction", params, { remoteGitPush: true })).toBe("control");
      expect(classify("project.gitAction", params, { remoteCloudStart: true })).toBe("denied");
    }
    expect(classify("project.gitAction", { action: "rebase" }, { remoteGitPush: true })).toBe("denied");
    expect(classify("project.gitAction", {})).toBe("denied");
  });
  it("session.start: a target needs remoteCloudStart", () => {
    const target = { kind: "codespace", instance: "new" };
    expect(classify("session.start", { projectId: "p", text: "x" })).toBe("control");
    expect(classify("session.start", { projectId: "p", text: "x", target })).toBe("denied");
    expect(classify("session.start", { projectId: "p", text: "x", target }, { remoteCloudStart: true })).toBe("control");
    expect(classify("session.start", { projectId: "p", text: "x", target }, { remoteGitPush: true })).toBe("denied");
  });
  it("full-access is denied in session.configure and session.start whatever the flags", () => {
    const flags = { remoteGitPush: true, remoteCloudStart: true };
    expect(classify("session.configure", { conversationId: "s", runtimeMode: "full-access" }, flags)).toBe("denied");
    expect(classify("session.start", { projectId: "p", text: "x", runtimeMode: "full-access" }, flags)).toBe("denied");
    expect(classify("session.configure", { conversationId: "s", runtimeMode: "auto" })).toBe("control");
  });
  it("cloud moves need both toggles", () => {
    for (const name of ["cloud.moveToCloud", "cloud.moveToLocal", "cloud.cancelMove"]) {
      expect(classify(name, { sessionId: "s" })).toBe("denied");
      expect(classify(name, { sessionId: "s" }, { remoteCloudStart: true })).toBe("denied");
      expect(classify(name, { sessionId: "s" }, { remoteGitPush: true })).toBe("denied");
      expect(classify(name, { sessionId: "s" }, { remoteCloudStart: true, remoteGitPush: true })).toBe("control");
    }
  });
  it("resolves every param-class method to something other than param", () => {
    for (const { name, def } of METHOD_LIST) {
      if (def.class === "param") expect(["read", "control", "denied"]).toContain(classify(name, {}));
    }
  });
});

describe("project.git remoteName", () => {
  const status = { branch: "main", files: 0, remote: true, upstream: true, defaultBranch: "main", ahead: 0, behind: 0 };
  it("is an optional string, empty when there is no remote", () => {
    const result = METHODS["project.git"].result;
    expect(result.safeParse(status).success).toBe(true);
    expect(result.safeParse({ ...status, remoteName: "origin" }).success).toBe(true);
    expect(result.safeParse({ ...status, remote: false, remoteName: "" }).success).toBe(true);
    expect(result.safeParse({ ...status, remoteName: 3 }).success).toBe(false);
  });
});

describe("mono.profile resetFiles", () => {
  const result = METHODS["mono.profile"].result;
  it("is read when well formed and reads as absent when malformed, without failing the profile", () => {
    expect(result.parse({ id: "m1", resetFiles: { label: "Claude Code", supported: true, note: "n" } }).resetFiles).toEqual({ label: "Claude Code", supported: true, note: "n" });
    const bad = result.safeParse({ id: "m1", resetFiles: { label: 3, supported: "yes" } });
    expect(bad.success).toBe(true);
    expect(bad.success && bad.data.resetFiles).toBeUndefined();
    expect(bad.success && bad.data.id).toBe("m1");
  });
});
