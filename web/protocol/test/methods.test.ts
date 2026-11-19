import { describe, expect, it } from "vitest";
import {
  METHODS,
  METHOD_CLASSES,
  METHOD_LIST,
  SessionStartTargetSchema,
  RuntimeModeSchema,
  classify,
  isMethodName,
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
  "project.changes": {
    params: { projectId: "p1" },
    result: { branch: "main", files: [{ relative: "src/a.ts", status: "modified", additions: 1, deletions: 0, staged: false, unstaged: true, lines: [{ kind: "add", text: "x", oldNumber: null, newNumber: 1 }] }] },
  },
  "project.worktrees": { params: { projectId: "p1" }, result: { worktrees: [{ path: "/w/a", name: "feature" }] } },
  "project.files": { params: { projectId: "p1", path: "src" }, result: { entries: [{ name: "a.ts", isDir: false, ignored: false }] } },
  "project.file": { params: { projectId: "p1", path: "src/a.ts" }, result: { path: "src/a.ts", text: "x", truncated: false } },
  "chat.mentionedFile": {
    params: { conversationId: "s1", path: "docs/report.html", asset: "img/a.png", offset: 0 },
    result: { path: "/work/app/docs/report.html", name: "report.html", size: 12, kind: "html", encoding: "utf8", data: "<p></p>", offset: 0, total: 7, assets: ["img/a.png"], project: { projectId: "p1", path: "docs/report.html" } },
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
  "mono.reset": { params: { monoId: "m1" }, result: null },
  "mono.delete": { params: { monoId: "m1" }, result: null },
  "workspace.list": {
    params: {},
    result: {
      projects: [{ id: "p1", name: "app", path: "/work/app", branch: "main", additions: 0, deletions: 0 }],
      monos: [{ id: "m1", name: "Ada", mascot: "cat", color: "#aabbcc", projectIds: ["p1"], unread: 0, status: "idle", lastMessage: "", lastActiveAt: 1 }],
      sessions: [session],
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
    expect(METHOD_CLASSES).toEqual(["read", "control", "param", "denied", "unsupportedWeb"]);
  });

  it("marks device.enroll QUIC-only and the host-answered set as host", () => {
    expect(METHODS["device.enroll"].quicOnly).toBe(true);
    const host = METHOD_LIST.filter(({ def }) => def.transport === "host").map(({ name }) => name);
    expect(host.sort()).toEqual(
      ["conversation.watch", "device.enroll", "device.leave", "device.push", "device.selection", "sessions.list", "workspace.list"].sort(),
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
  it("accepts only the five kinds and the three encodings", () => {
    const result = METHODS["chat.mentionedFile"].result;
    const base = { path: "/a", name: "a", size: 0, kind: "binary", encoding: "none", data: "", offset: 0, total: 0 };
    expect(result.safeParse(base).success).toBe(true);
    expect(result.safeParse({ ...base, kind: "video" }).success).toBe(false);
    expect(result.safeParse({ ...base, encoding: "hex" }).success).toBe(false);
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
    expect(classify("mono.reset", {})).toBe("denied");
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
  it("device.selection: get is read, set is control", () => {
    expect(classify("device.selection", {})).toBe("read");
    expect(classify("device.selection", { projects: ["p1"] })).toBe("control");
    expect(classify("device.selection", { monos: [] })).toBe("control");
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
