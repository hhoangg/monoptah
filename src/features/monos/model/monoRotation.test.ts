// @vitest-environment happy-dom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Block } from "../../sessions/model/session";
import {
  loadMonoRotation,
  planRotation,
  rotationReason,
  saveMonoRotation,
} from "./monoRotation";

beforeEach(() => {
  const stored = new Map<string, string>();
  vi.stubGlobal("localStorage", {
    getItem: (key: string) => stored.get(key) ?? null,
    setItem: (key: string, value: string) => stored.set(key, value),
  });
});
afterEach(() => vi.unstubAllGlobals());

const HOUR = 60 * 60 * 1000;
const T0 = new Date(2026, 9, 4, 9).getTime();

let next = 0;
function turn(user: string, reply: string, at: number, extra: Block[] = []) {
  const id = `u${next++}`;
  return [
    { id, role: "user", text: user, startedAt: at, durationMs: 60_000 },
    ...extra,
    { id: `${id}-tool`, role: "assistant", text: "", tool: { title: "Read" } },
    { id: `${id}-a`, role: "assistant", text: reply },
  ] as Block[];
}

it.each([128_000, 200_000, 1_000_000])(
  "rotates at 80% of a %i-token context window",
  (window) => {
    const session = (used: number) => ({
      providerSessionId: "native",
      context: { used, window },
      blocks: [],
    });
    expect(rotationReason(session(window * 0.8 - 1))).toBeUndefined();
    expect(rotationReason(session(window * 0.8))).toBe("context");
    expect(rotationReason(session(window))).toBe("context");
    expect(
      rotationReason({
        ...session(window),
        providerSessionId: undefined,
      }),
    ).toBeUndefined();
  },
);

it.each([45_000, 600_000])(
  "keeps a %i-token conversation in a large window across a long break",
  (used) => {
    vi.useFakeTimers();
    try {
      vi.setSystemTime(T0 + 48 * HOUR);
      const session = {
        providerSessionId: "native",
        context: { used, window: 1_000_000 },
        blocks: turn("hi", "hello", T0),
      };
      expect(rotationReason(session)).toBeUndefined();
    } finally {
      vi.useRealTimers();
    }
  },
);

it.each([
  undefined,
  { used: 900_000 },
  { used: 900_000, window: 0 },
  { used: 900_000, window: -1 },
  { used: 900_000, window: Number.NaN },
  { used: Number.NaN, window: 1_000_000 },
  { used: Number.POSITIVE_INFINITY, window: 1_000_000 },
  { used: -1, window: 1_000_000 },
])("does not rotate without a usable context percentage (%#)", (context) => {
  expect(
    rotationReason({ providerSessionId: "native", context, blocks: [] }),
  ).toBeUndefined();
});

it("briefs the fresh session with recent turns word for word and earlier ones as lines", () => {
  const blocks = [
    ...turn(
      "set up the release checklist",
      "Done, it is in docs/release.md",
      T0,
    ),
    ...turn("what is CI doing", "Two jobs are red on main", T0 + HOUR),
    ...turn("fix the lint job", "Fixed and pushed in #712", T0 + 2 * HOUR),
  ];
  const { rotation, brief } = planRotation(
    blocks,
    undefined,
    "context",
    T0 + 3 * HOUR,
  );
  const text = brief("mono-1");
  expect(text).toContain("<previous_conversation>");
  expect(text).toContain('sessions.read {"sessionId":"mono-1"}');
  expect(text).toMatch(
    /Earlier, oldest first:\n- .*User: set up the release checklist → You: Done, it is in docs\/release\.md/,
  );
  expect(text).toContain("Most recent, word for word:");
  expect(text).toContain("what is CI doing\n\nYou:\nTwo jobs are red on main");
  expect(text).toContain("fix the lint job\n\nYou:\nFixed and pushed in #712");
  expect(text).not.toContain("Read");
  expect(rotation.afterBlockId).toBe(blocks[blocks.length - 1].id);
  expect(rotation.earlier).toHaveLength(3);
});

it("remembers a completion report without overwriting the answer to the user's chat", () => {
  const blocks: Block[] = [
    ...turn("Explain the design", "The design uses a queue.", T0),
    {
      id: "notification",
      role: "user",
      text: "Hidden notification instructions",
      internal: true,
      monoSessionCompletion: {
        sessionId: "worker",
        title: "API fix",
        status: "completed",
      },
    },
    {
      id: "report",
      role: "assistant",
      text: "The API fix is done; all tests passed.",
    },
  ];
  const text = planRotation(blocks, undefined, "context", T0 + HOUR).brief(
    "mono",
  );
  expect(text).toContain(
    "Explain the design\n\nYou:\nThe design uses a queue.",
  );
  expect(text).toContain('reviewing session "API fix"');
  expect(text).toContain("The API fix is done; all tests passed.");
  expect(text).not.toContain("Hidden notification instructions");
});

it("carries earlier lines across rotations, briefing only what came since", () => {
  const first = [...turn("one", "1", T0), ...turn("two", "2", T0 + HOUR)];
  const planned = planRotation(first, undefined, "context", T0 + 2 * HOUR);
  // A saved rotation from before idle rotation was removed still carries over.
  saveMonoRotation("mono-1", { ...planned.rotation, reason: "idle" });
  const blocks = [
    ...first,
    ...turn("three", "3", T0 + 3 * HOUR),
    ...turn("four", "4", T0 + 4 * HOUR),
  ];
  const text = planRotation(
    blocks,
    loadMonoRotation("mono-1"),
    "context",
    T0 + 5 * HOUR,
  ).brief("mono-1");
  expect(text).toMatch(
    /Earlier, oldest first:\n- .*User: one → You: 1\n- .*User: two → You: 2/,
  );
  expect(text).toContain("three\n\nYou:\n3");
  expect(text).toContain("four\n\nYou:\n4");
});

it("keeps long messages bounded", () => {
  const blocks = turn("x".repeat(10_000), "y".repeat(10_000), T0);
  const text = planRotation(blocks, undefined, "context", T0 + HOUR).brief("m");
  expect(text.length).toBeLessThan(6_000);
  expect(text).toContain("[…]");
});

it("remembers a workflow report as a post of its own, not the answer to the user's last message", () => {
  const blocks: Block[] = [
    ...turn("How is CI?", "Green on main.", T0),
    {
      id: "wf-report",
      role: "assistant",
      text: "## Ship it · completed\n\nPR #12 opened.",
      monoWorkflow: { id: "wf-abcd1234", name: "Ship it", runId: "r1", at: T0 + HOUR },
    },
  ];
  const text = planRotation(blocks, undefined, "context", T0 + 2 * HOUR).brief(
    "mono",
  );
  expect(text).toContain("How is CI?\n\nYou:\nGreen on main.");
  expect(text).toContain('from your workflow "Ship it"');
  expect(text).toContain("PR #12 opened.");
});

const UUID_A = "11111111-1111-4111-8111-111111111111";
const UUID_B = "22222222-2222-4222-8222-222222222222";

describe("retired provider sessions", () => {
  const blocks = turn("hello", "hi", T0);
  const plan = (
    previous: Parameters<typeof planRotation>[1],
    retiring?: Parameters<typeof planRotation>[4],
  ) => planRotation(blocks, previous, "context", T0 + HOUR, retiring).rotation;

  it("records the session being retired", () => {
    expect(plan(undefined, { harness: "claude", id: UUID_A }).retired).toEqual([
      { harness: "claude", id: UUID_A },
    ]);
  });

  it("carries earlier retired sessions forward and dedupes", () => {
    const first = plan(undefined, { harness: "codex", id: UUID_A });
    const second = plan(first, { harness: "codex", id: UUID_B });
    expect(second.retired).toEqual([
      { harness: "codex", id: UUID_A },
      { harness: "codex", id: UUID_B },
    ]);
    expect(plan(second, { harness: "codex", id: UUID_A }).retired).toEqual([
      { harness: "codex", id: UUID_B },
      { harness: "codex", id: UUID_A },
    ]);
  });

  it("carries retired sessions forward when nothing new is retired", () => {
    const first = plan(undefined, { harness: "claude", id: UUID_A });
    expect(plan(first).retired).toEqual([{ harness: "claude", id: UUID_A }]);
    expect(plan(first, { harness: "claude" }).retired).toEqual([
      { harness: "claude", id: UUID_A },
    ]);
  });

  it("keeps only the newest 200", () => {
    const many = Array.from({ length: 200 }, (_, i) => ({
      harness: "claude" as const,
      id: `00000000-0000-4000-8000-${String(i).padStart(12, "0")}`,
    }));
    const rotation = plan(
      { at: 1, reason: "context", earlier: [], retired: many },
      { harness: "claude", id: UUID_A },
    );
    expect(rotation.retired).toHaveLength(200);
    expect(rotation.retired![199]).toEqual({ harness: "claude", id: UUID_A });
    expect(rotation.retired![0]).toEqual(many[1]);
  });

  it("ignores an unsupported harness or a malformed id", () => {
    expect(
      plan(undefined, { harness: "cursor", id: UUID_A }).retired,
    ).toBeUndefined();
    expect(
      plan(undefined, { harness: "claude", id: "../x" }).retired,
    ).toBeUndefined();
    expect(
      plan(undefined, { harness: "claude", id: "" }).retired,
    ).toBeUndefined();
  });

  it.each([
    ["a null entry", null],
    ["an entry without an id", { harness: "claude" }],
    ["a non-string id", { harness: "claude", id: 42 }],
    ["a bad harness", { harness: "cursor", id: UUID_B }],
    ["a bad id", { harness: "codex", id: "../x" }],
    ["a string", "nope"],
  ])("drops %s from the stored list instead of throwing", (_, bad) => {
    const stored = {
      at: 1,
      reason: "context",
      earlier: [],
      retired: [bad, { harness: "claude", id: UUID_B }],
    } as unknown as Parameters<typeof planRotation>[1];
    let rotation!: ReturnType<typeof plan>;
    expect(() => {
      rotation = plan(stored, { harness: "claude", id: UUID_A });
    }).not.toThrow();
    expect(rotation.retired).toEqual([
      { harness: "claude", id: UUID_B },
      { harness: "claude", id: UUID_A },
    ]);
  });

  it("drops a stored list that is not an array", () => {
    const stored = {
      at: 1,
      reason: "context",
      earlier: [],
      retired: "x",
    } as unknown as Parameters<typeof planRotation>[1];
    expect(() => plan(stored)).not.toThrow();
    expect(plan(stored).retired).toBeUndefined();
    expect(plan(stored, { harness: "claude", id: UUID_A }).retired).toEqual([
      { harness: "claude", id: UUID_A },
    ]);
  });

  it("leaves the rotation exactly as before without a fifth argument", () => {
    const rotation = plan(undefined);
    expect("retired" in rotation).toBe(false);
  });
});
