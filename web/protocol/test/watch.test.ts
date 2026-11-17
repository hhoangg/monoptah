import { describe, expect, it } from "vitest";
import {
  CLOUD_STATES,
  CloudFieldSchema,
  MOVE_STEP_IDS,
  SessionTargetSchema,
  STARTUP_STEP_IDS,
  STEP_STATUSES,
  TARGET_KINDS,
  WatchFrameSchema,
  EnvelopeResponseSchema,
  WatchFrameResponseSchema,
} from "../src/index.ts";

const turn = { id: "t1", items: [{ type: "text", id: "a", text: "Hi", streaming: true }] };

describe("watch frame (crates/monocode-core/src/mobile/watch.rs)", () => {
  it("parses a first frame", () => {
    const frame = WatchFrameSchema.parse({
      usageLimit: null,
      busy: true,
      older: false,
      order: ["t1"],
      question: null,
      turns: [turn],
    });
    expect(frame.order).toEqual(["t1"]);
  });

  it("parses patches with append, value and remove", () => {
    const frame = WatchFrameSchema.parse({
      usageLimit: { token: "tok", resetsAt: null, resumeAtReset: false, canResume: true, canAutoResume: false, resetLabel: "soon", limitation: "5h" },
      busy: true,
      older: true,
      order: ["t1", "t2"],
      turns: [],
      patches: [
        {
          id: "t1",
          edits: [
            { path: ["items", 0, "text"], from: 12, append: " more" },
            { path: ["items", 1], value: { type: "notice", id: "n", text: "x" } },
            { path: ["items", 2], remove: true },
          ],
        },
      ],
    });
    const edits = frame.patches?.[0]?.edits ?? [];
    expect(edits).toHaveLength(3);
    expect("append" in (edits[0] ?? {})).toBe(true);
    expect("remove" in (edits[2] ?? {})).toBe(true);
    expect((edits[1] as { value?: unknown }).value).toEqual({ type: "notice", id: "n", text: "x" });
  });

  it("parses a question and null once answered", () => {
    const question = { requestId: 4, questions: [{ id: "q", prompt: "Which?", multiSelect: false, allowCustom: true, options: [{ id: "a", label: "A" }] }] };
    expect(WatchFrameSchema.parse({ usageLimit: null, busy: false, older: false, order: [], turns: [], question }).question?.requestId).toBe(4);
    expect(WatchFrameSchema.parse({ usageLimit: null, busy: false, older: false, order: [], turns: [], question: null }).question).toBeNull();
  });

  it("rejects a frame without order or with a bad turn", () => {
    expect(WatchFrameSchema.safeParse({ usageLimit: null, busy: true, older: false, turns: [] }).success).toBe(false);
    expect(WatchFrameSchema.safeParse({ usageLimit: null, busy: true, older: false, order: [], turns: [{ items: [] }] }).success).toBe(false);
  });

  it("carries cloud and approvals fields", () => {
    const frame = WatchFrameSchema.parse({
      usageLimit: null,
      busy: false,
      older: false,
      order: [],
      turns: [],
      cloud: { kind: "sandbox", instance: "sbx-1", state: "stopped", kept: true, autoStop: { at: 1_700_000_000_000, reason: "idle" }, move: { step: "upload", status: "running" } },
      approvals: [{ requestId: 9, label: "Run rm -rf build", detail: "rm -rf build" }],
    });
    expect(frame.cloud?.kept).toBe(true);
    expect(frame.approvals).toHaveLength(1);
    expect(WatchFrameSchema.parse({ usageLimit: null, busy: false, older: false, order: [], turns: [], approvals: [] }).approvals).toEqual([]);
  });

  it("is the result of an ok response", () => {
    const response = { rid: 8, ok: true, result: { usageLimit: null, busy: false, older: false, order: [], turns: [] } };
    expect(WatchFrameResponseSchema.safeParse(response).success).toBe(true);
    expect(EnvelopeResponseSchema.safeParse(response).success).toBe(true);
  });
});

describe("cloud enums of architecture.md 6.6", () => {
  it("has the exact lists", () => {
    expect([...STARTUP_STEP_IDS]).toEqual([
      "create",
      "provision",
      "buildContainer",
      "blockingCommands",
      "configureCodespace",
      "connect",
      "openProject",
      "claudeSetup",
      "lifecycleCommands",
    ]);
    expect([...STEP_STATUSES]).toEqual(["pending", "running", "done", "failed", "skipped"]);
    expect([...MOVE_STEP_IDS]).toEqual(["snapshot", "push", "workspace", "upload", "restoreCode", "restoreConversation", "flip"]);
    expect([...CLOUD_STATES]).toEqual(["available", "starting", "stopping", "stopped", "unavailable", "unknown"]);
    expect([...TARGET_KINDS]).toEqual(["local", "remote", "codespace", "sandbox"]);
  });

  it("accepts every value and rejects others", () => {
    for (const kind of TARGET_KINDS) expect(SessionTargetSchema.safeParse({ kind }).success).toBe(true);
    expect(SessionTargetSchema.safeParse({ kind: "vm" }).success).toBe(false);
    for (const step of STARTUP_STEP_IDS) {
      for (const status of STEP_STATUSES) {
        expect(CloudFieldSchema.safeParse({ kind: "codespace", instance: "i", state: "starting", startup: { steps: [{ id: step, status }], current: step } }).success).toBe(true);
      }
    }
    for (const step of MOVE_STEP_IDS) {
      expect(CloudFieldSchema.safeParse({ kind: "codespace", instance: "i", state: "available", move: { step, status: "running" } }).success).toBe(true);
    }
    expect(CloudFieldSchema.safeParse({ kind: "codespace", instance: "i", state: "available", move: { step: "teleport", status: "running" } }).success).toBe(false);
    expect(CloudFieldSchema.safeParse({ kind: "codespace", instance: "i", state: "weird" }).success).toBe(false);
    expect(CloudFieldSchema.safeParse({ kind: "codespace", instance: "i", state: "available", kept: false }).success).toBe(false);
  });
});
