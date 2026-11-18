// Dev and test helper: one long conversation with every turn item type, held on the mock desktop so that
// only its last `tail` turns are sent on open and the rest is served page by page by `conversation.older`
// (the stock fixtures send a whole conversation and answer `older` with nothing).
import { FIXTURE_HANDLERS, type MockDesktop } from "@monoptah/mock-desktop";

export const LONG_SESSION_ID = "sess-long";

type World = MockDesktop["world"];
type MockTurn = ReturnType<World["read"]>["turns"][number];

const SNIPPETS = [
  "```ts\nexport function retry<T>(run: () => Promise<T>, times = 3): Promise<T> {\n  return run().catch((error) => (times > 1 ? retry(run, times - 1) : Promise.reject(error)));\n}\n```",
  "```sh\npnpm --filter @monoptah/app exec vitest run src/features/transcript\n```",
  "```json\n{ \"name\": \"example\", \"version\": \"1.0.0\" }\n```",
];

function reply(n: number): string {
  return [
    `## Step ${n}`,
    "",
    `I looked at \`src/features/sessions/screens.tsx\` and **changed** the list so it pages *older* items in. See [the docs](https://example.com/docs).`,
    "",
    "- first the loader",
    "- then the **virtualizer**",
    "  - with its anchor",
    "",
    SNIPPETS[n % SNIPPETS.length]!,
    "",
    "| File | Change |",
    "| --- | --- |",
    "| `a.ts` | added |",
    "| `b.ts` | removed |",
    "",
    "> Measured twice, cut once.",
  ].join("\n");
}

/** Turn `n` of the conversation, 0-based. Every id is unique and stable. */
export function longTurn(n: number, now: number): MockTurn[] {
  const at = now - (200 - n) * 90_000;
  const user: MockTurn = { id: `long-u${n}`, stampAt: n % 10 === 0 ? at : undefined, items: [{ type: "user", id: `long-u${n}-m`, text: n % 7 === 0 ? `Please continue with part ${n}.\n\nKeep the change small, add a test, and tell me what you ran.\nOne more line so the bubble clamps at four lines.\nAnd a fifth line.` : `Next: part ${n}`, at }] } as MockTurn;
  const work = n % 3 === 0 ? [{ type: "work", id: `long-w${n}`, live: false, phases: [{ id: `p${n}a`, kind: "research", title: "Read 4 files", steps: [{ id: `s${n}a`, label: "src/features/sessions/screens.tsx" }, { id: `s${n}b`, label: "src/data/conversation.ts" }] }, { id: `p${n}b`, kind: "run", title: "Ran the tests", steps: [{ id: `s${n}c`, label: "pnpm test", failed: n % 6 === 0 }] }] }] : [];
  const answer: MockTurn = {
    id: `long-a${n}`,
    footer: true,
    copyText: reply(n),
    completedAt: at + 40_000,
    durationMs: 40_000,
    metrics: { inputTokens: 12_000 + n, outputTokens: 800, cacheHitPercent: 71 },
    items: [
      { type: "header", id: `long-h${n}`, live: false, durationMs: 40_000, model: "Example Model One", harness: "claude", entries: [{ type: "prose", id: `long-e${n}`, text: "I will start with the loader." }] },
      ...work,
      { type: "text", id: `long-t${n}`, text: reply(n) },
      ...(n === 190 ? [{ type: "notice", id: `long-n${n}`, text: "The model is overloaded. Try again in a minute.", error: true }] : []),
    ],
  } as unknown as MockTurn;
  return [user, answer];
}

/**
 * Adds the conversation to `desktop`: `total` turns, the last `tail` held by the session (and sent on open),
 * the rest served by `conversation.older`. Installs the older handler on the shared fixture table.
 */
export function seedLongSession(desktop: MockDesktop, options: { total?: number; tail?: number } = {}): void {
  const { world } = desktop;
  const base = world.sessions.get("sess-local-1");
  if (!base) return;
  const total = options.total ?? 200;
  const tail = options.tail ?? 24;
  const now = Date.now();
  const all = Array.from({ length: total }, (_, n) => longTurn(n, now)).flat();
  const held = all.slice(-tail);
  const earlier = all.slice(0, all.length - tail);
  world.sessions.set(LONG_SESSION_ID, { ...base, id: LONG_SESSION_ID, title: "A long conversation about paging", turns: held, approvals: [], script: [], busy: false });

  const nextFrame = world.nextFrame.bind(world);
  world.nextFrame = (conversationId, cursor) => {
    const frame = nextFrame(conversationId, cursor);
    return frame && conversationId === LONG_SESSION_ID ? { ...frame, older: earlier.length > 0 } : frame;
  };
  const read = world.read.bind(world);
  world.read = (conversationId) => {
    const result = read(conversationId);
    return conversationId === LONG_SESSION_ID ? { ...result, older: earlier.length > 0 } : result;
  };
  const older = FIXTURE_HANDLERS["conversation.older"];
  FIXTURE_HANDLERS["conversation.older"] = (params, context) => {
    if (params.conversationId !== LONG_SESSION_ID) return older(params, context);
    const index = earlier.findIndex((turn) => turn.id === params.before);
    // `before` is the first turn the client holds: the page is the turns right before it.
    const end = index === -1 ? earlier.length : index;
    const start = Math.max(0, end - (params.limit ?? 20));
    return { turns: earlier.slice(start, end).map((turn) => structuredClone(turn)), older: start > 0 };
  };
}
