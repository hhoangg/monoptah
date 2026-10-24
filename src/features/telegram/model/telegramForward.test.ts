import { afterEach, describe, expect, it, vi } from "vitest";
import {
  newSession,
  type Block,
  type Session,
} from "../../sessions/model/session";
import type { UserQuestionPrompt } from "../../sessions/model/userQuestion";
import {
  FORWARD_DELAY_MS,
  dueForwards,
  forwardDelayMs,
  forwardPayload,
  isForwardable,
  isTopicLinkable,
  questionContext,
  sessionTicketId,
  topicTitle,
} from "./telegramForward";
import { clearSessionTicket, setSessionTicket } from "./sessionTicket";
import {
  clearWorkflowSessionContext,
  setWorkflowSessionContext,
} from "./workflowSessionContext";

const monoChats = vi.hoisted(() => new Set<string>());
vi.mock("../../monos/model/mono", () => ({
  isMonoSession: (id: string) => monoChats.has(id),
}));

const prompt: UserQuestionPrompt = {
  requestId: 7,
  questions: [
    {
      id: "q1",
      header: "Scope",
      prompt: "Which files?",
      multiSelect: true,
      allowCustom: true,
      options: [
        { id: "a", label: "All", description: "Everything" },
        { id: "b", label: "None" },
      ],
    },
  ],
};

function asking(overrides: Partial<Session> = {}): Session {
  const session = newSession("claude", "/work/acme");
  session.title = "Fix the login";
  session.pendingQuestion = prompt;
  return Object.assign(session, overrides);
}

function block(
  role: Block["role"],
  text: string,
  extra: Partial<Block> = {},
): Block {
  return {
    id: `${role}-${text.length}-${Math.random()}`,
    role,
    text,
    ...extra,
  };
}

const cleanup: string[] = [];
const ticketCleanup: string[] = [];
afterEach(() => {
  for (const id of ticketCleanup.splice(0)) clearSessionTicket(id);
  for (const id of cleanup.splice(0)) clearWorkflowSessionContext(id);
});

describe("isForwardable", () => {
  it("excludes orchestration workers, inbox ask, ephemeral, tui, mono chats, remote, codex auto-resolving", () => {
    expect(isForwardable(asking())).toBe(true);
    expect(isForwardable(asking({ orchestrationLeadId: "lead" }))).toBe(false);
    expect(isForwardable(asking({ inboxAsk: {} as Session["inboxAsk"] }))).toBe(
      false,
    );
    expect(isForwardable(asking({ ephemeral: true }))).toBe(false);
    expect(isForwardable(asking({ surface: "tui" }))).toBe(false);
    const mono = asking();
    monoChats.add(mono.id);
    expect(isForwardable(mono)).toBe(false);
    monoChats.delete(mono.id);
    expect(isForwardable(asking({ cwd: "remote://host/work/acme" }))).toBe(
      false,
    );
    expect(
      isForwardable(
        asking({
          pendingQuestion: { ...prompt, autoResolveAt: Date.now() + 1 },
        }),
      ),
    ).toBe(false);
  });
});

describe("isTopicLinkable", () => {
  it("ignores the pending question but excludes sessions that never forward", () => {
    expect(isTopicLinkable(asking())).toBe(true);
    expect(isTopicLinkable(asking({ pendingQuestion: undefined }))).toBe(true);
    expect(
      isTopicLinkable(
        asking({
          pendingQuestion: { ...prompt, autoResolveAt: Date.now() + 1 },
        }),
      ),
    ).toBe(true);
    expect(isTopicLinkable(asking({ orchestrationLeadId: "lead" }))).toBe(
      false,
    );
    expect(isTopicLinkable(asking({ ephemeral: true }))).toBe(false);
    expect(isTopicLinkable(asking({ surface: "tui" }))).toBe(false);
    expect(isTopicLinkable(asking({ cwd: "remote://host/work/acme" }))).toBe(
      false,
    );
    const mono = asking();
    monoChats.add(mono.id);
    expect(isTopicLinkable(mono)).toBe(false);
    monoChats.delete(mono.id);
  });
});

it("forwards sidebar-hidden sessions started by Monos", () => {
  expect(isForwardable(asking({ sidebarHidden: true }))).toBe(true);
});

describe("sessionTicketId", () => {
  it("ticket from the registry when the inbox card is gone", () => {
    const session = asking();
    expect(session.inboxCard).toBeUndefined();
    setSessionTicket(session.id, "ENG-42");
    ticketCleanup.push(session.id);
    expect(sessionTicketId(session)).toBe("ENG-42");
  });

  it("ticket from inbox card identifier", () => {
    const session = asking({
      inboxCard: { identifier: "ENG-42" } as Session["inboxCard"],
    });
    expect(sessionTicketId(session)).toBe("ENG-42");
  });

  it("ticket from workflow item", () => {
    const session = asking();
    setWorkflowSessionContext(session.id, {
      workflowName: "Triage",
      nodeTitle: "Fix",
      itemId: "CU-9",
    });
    cleanup.push(session.id);
    expect(sessionTicketId(session)).toBe("CU-9");
  });

  it("ticket from linked GitHub issue is #12", () => {
    const session = asking({
      linkedWorkItem: { kind: "issue", repo: "a/b", number: 12, url: "u" },
    });
    expect(sessionTicketId(session)).toBe("#12");
    expect(sessionTicketId(asking())).toBeUndefined();
  });
});

describe("topicTitle", () => {
  it("does not repeat a GitHub ticket already in the title", () => {
    const session = asking({
      title: "#12 Fix crash",
      linkedWorkItem: { kind: "issue", repo: "a/b", number: 12, url: "u" },
    });
    expect(topicTitle(session)).toBe("[#12] Fix crash");
    expect(forwardPayload(session, "/work/acme")?.header).toBe(
      "acme · Fix crash · #12",
    );
  });

  it("does not repeat a tracker key already in the title", () => {
    const session = asking({ title: "ENG-42 Fix crash" });
    setSessionTicket(session.id, "ENG-42");
    ticketCleanup.push(session.id);
    expect(topicTitle(session)).toBe("[ENG-42] Fix crash");
    expect(forwardPayload(session, "/work/acme")?.header).toBe(
      "acme · Fix crash · ENG-42",
    );
  });

  it("title without ticket is plain", () => {
    expect(topicTitle(asking())).toBe("Fix the login");
  });

  it("prefixes the ticket", () => {
    const session = asking({
      linkedWorkItem: { kind: "pr", repo: "a/b", number: 3, url: "u" },
    });
    expect(topicTitle(session)).toBe("[#3] Fix the login");
  });

  it("title trimmed to 128", () => {
    const session = asking({ title: "x".repeat(300) });
    expect(topicTitle(session)).toHaveLength(128);
    const ticketed = asking({
      title: "y".repeat(300),
      inboxCard: { identifier: "ENG-42" } as Session["inboxCard"],
    });
    const title = topicTitle(ticketed);
    expect(title).toHaveLength(128);
    expect(title.startsWith("[ENG-42] yyy")).toBe(true);
  });
});

describe("questionContext", () => {
  function said(text: string, overrides: Partial<Session> = {}): string {
    return questionContext(
      asking({ blocks: [block("assistant", text)], ...overrides }),
    );
  }

  /** Every whole word of the output must be a whole word of the source. */
  function expectWholeWords(output: string, source: string) {
    const words = new Set(source.split(/\s+/));
    for (const word of output.replace(/^\u2026 /, "").split(/\s+/)) {
      expect(words.has(word), `cut word: ${word}`).toBe(true);
    }
  }

  it("uses the last assistant text that is not internal", () => {
    const session = asking({
      blocks: [
        block("assistant", "old reply"),
        block("assistant", "The plan is ready."),
        block("tool", "tool output"),
        block("assistant", "internal note", { internal: true }),
        block("user", "user text"),
      ],
    });
    expect(questionContext(session)).toBe("The plan is ready.");
    expect(questionContext(asking())).toBe("");
  });

  it("passes a short message through untouched", () => {
    expect(said("  I updated the parser. It now handles quotes.  ")).toBe(
      "I updated the parser. It now handles quotes.",
    );
  });

  it("keeps whole sentences from the end and marks the cut with an ellipsis", () => {
    const sentences = Array.from(
      { length: 30 },
      (_, i) => `Sentence number ${i} explains one more detail.`,
    );
    const context = said(sentences.join(" "));
    expect(Array.from(context).length).toBeLessThanOrEqual(300);
    expect(Array.from(context).length).toBeGreaterThan(200);
    expect(context.startsWith("\u2026 Sentence number ")).toBe(true);
    expect(
      context.endsWith("Sentence number 29 explains one more detail."),
    ).toBe(true);
    // Only whole sentences: what follows the marker is a full sentence from the source.
    const kept = context.slice(2).split(/(?<=\.) /);
    for (const sentence of kept) expect(sentences).toContain(sentence);
  });

  it("does not add the marker when the selection starts at the beginning", () => {
    const context = said("First point. Second point. Third point.");
    expect(context.startsWith("\u2026")).toBe(false);
    expect(context).toBe("First point. Second point. Third point.");
  });

  it("strips headings, links, images, emphasis, inline code and html", () => {
    const text = [
      "## Findings",
      "",
      "I checked [the docs](https://example.com/docs) and ![diagram](x.png) with <b>care</b>.",
      "The **bold** and __also bold__ and *italic* and _more italic_ words use `npm run check` and ~~old~~ names.",
    ].join("\n");
    expect(said(text)).toBe(
      "Findings I checked the docs and with care. The bold and also bold and italic and more italic words use npm run check and old names.",
    );
  });

  it("keeps snake_case identifiers and multiplication stars intact", () => {
    expect(said("Rename some_var_name and compute 2 * 3 * 4.")).toBe(
      "Rename some_var_name and compute 2 * 3 * 4.",
    );
  });

  it("drops fenced code blocks, including an unterminated one", () => {
    const fenced = [
      "I changed the loader.",
      "```ts",
      "const secret = 1;",
      "```",
      "Tests pass now.",
    ].join("\n");
    expect(said(fenced)).toBe("I changed the loader. Tests pass now.");
    const open = ["Here is the patch.", "~~~", "diff --git a b", "+x"].join(
      "\n",
    );
    expect(said(open)).toBe("Here is the patch.");
  });

  it("gives an empty context when the message is only code or markup", () => {
    expect(said("```\nconst a = 1;\n```")).toBe("");
    expect(said("---\n\n| --- | --- |")).toBe("");
    expect(said("   \n  ")).toBe("");
  });

  it("does not reach back to an older message when the last one is only code", () => {
    const session = asking({
      blocks: [
        block("assistant", "An older explanation."),
        block("assistant", "```\nconst a = 1;\n```"),
      ],
    });
    expect(questionContext(session)).toBe("");
  });

  it("keeps list items as plain dash lines", () => {
    const text = [
      "Two options are open:",
      "",
      "* Keep the cache",
      "1. Drop the cache",
      "",
      "I lean towards the first.",
    ].join("\n");
    expect(said(text)).toBe(
      "Two options are open:\n- Keep the cache\n- Drop the cache\nI lean towards the first.",
    );
  });

  it("turns table rows into plain text and drops the rules", () => {
    const text = [
      "| File | Change |",
      "| --- | --- |",
      "| a.ts | rename |",
    ].join("\n");
    expect(said(text)).toBe("File, Change a.ts, rename");
  });

  it("skips a last paragraph that only repeats the question when an earlier one adds context", () => {
    const text = [
      "The migration touches three tables and takes a few minutes on a large database.",
      "",
      "Which files?",
    ].join("\n");
    expect(said(text)).toBe(
      "The migration touches three tables and takes a few minutes on a large database.",
    );
  });

  it("skips a trailing sentence that repeats the question inside a paragraph", () => {
    const text = "I found two loaders. Which files? ";
    expect(said(text)).toBe("I found two loaders.");
  });

  it("keeps the question text when nothing else was said", () => {
    expect(said("Which files?")).toBe("Which files?");
    expect(said("Which files?\n\nWhich files?")).toBe(
      "Which files? Which files?",
    );
  });

  it("keeps a last paragraph that goes beyond the question", () => {
    const text = "Scanning is done. Which files should I touch first?";
    expect(said(text)).toBe(
      "Scanning is done. Which files should I touch first?",
    );
  });

  it("cuts a single huge sentence at a word boundary and keeps its end", () => {
    const words = Array.from({ length: 400 }, (_, i) => `word${i}`);
    const text = words.join(" ");
    const context = said(text);
    expect(Array.from(context).length).toBeLessThanOrEqual(300);
    expect(context.startsWith("\u2026 word")).toBe(true);
    expect(context.endsWith("word399")).toBe(true);
    expectWholeWords(context, text);
  });

  it("never splits a multi-byte character and counts characters, not UTF-16 units", () => {
    const emoji = said(
      Array.from({ length: 200 }, () => "ok \u{1F600}").join(" "),
    );
    expect(Array.from(emoji).length).toBeLessThanOrEqual(300);
    expect(emoji).not.toMatch(/[\ud800-\udbff](?![\udc00-\udfff])/);
    expect(emoji).not.toMatch(/(?<![\ud800-\udbff])[\udc00-\udfff]/);

    const cjk = said(
      "\u4eca\u65e5\u306f\u5929\u6c17\u304c\u3044\u3044\u3002".repeat(60),
    );
    expect(Array.from(cjk).length).toBeLessThanOrEqual(300);
    expect(cjk.startsWith("\u2026 ")).toBe(true);
    expect(cjk.endsWith("\u3002")).toBe(true);

    const unbroken = said("\u{1F600}".repeat(700));
    expect(Array.from(unbroken).length).toBeLessThanOrEqual(300);
    expect(unbroken.startsWith("\u2026 ")).toBe(true);
  });

  it("keeps the contents of inline code exactly as written", () => {
    expect(said("Open `__init__.py` now.")).toBe("Open __init__.py now.");
    expect(said("Edit `src/__tests__/foo.test.ts` first.")).toBe(
      "Edit src/__tests__/foo.test.ts first.",
    );
    expect(said("Match `src/**/*.ts` files.")).toBe("Match src/**/*.ts files.");
    expect(said("It returns `Result<T, E>` or `Vec<String>`.")).toBe(
      "It returns Result<T, E> or Vec<String>.",
    );
    expect(said("Call `f(*args, **kwargs)` here.")).toBe(
      "Call f(*args, **kwargs) here.",
    );
    expect(said("Use `a**b**c` there.")).toBe("Use a**b**c there.");
    expect(said("Mail `<ops@example.com>` today.")).toBe(
      "Mail <ops@example.com> today.",
    );
    expect(said("Link `[a](b)` and `![i](j)` stay.")).toBe(
      "Link [a](b) and ![i](j) stay.",
    );
  });

  it("unwraps code spans next to emphasis and handles longer fences", () => {
    expect(said("Call **`init()`** then **bold**`tail`.")).toBe(
      "Call init() then boldtail.",
    );
    expect(said("Use ``a`b`` and ```x``` here.")).toBe("Use a`b and x here.");
    expect(said("Pad ` spaced ` ok.")).toBe("Pad spaced ok.");
  });

  it("leaves an unclosed backtick as plain text", () => {
    expect(said("Use the `broken quote here.")).toBe(
      "Use the `broken quote here.",
    );
    expect(said("Keep `ok` and a stray ` tick.")).toBe(
      "Keep ok and a stray ` tick.",
    );
  });

  it("does not split a sentence inside an inline code span", () => {
    expect(said("Bump to `v1.2.3` in `src/app.ts`. Then rebuild.")).toBe(
      "Bump to v1.2.3 in src/app.ts. Then rebuild.",
    );
    // The span's own full stop is not a sentence end, so the cut stays at a real one.
    const long = `Start ${"w ".repeat(150)}then \`x. y\` end.`;
    const context = said(long);
    expect(context.startsWith("… ")).toBe(true);
    expect(context.endsWith("then x. y end.")).toBe(true);
    expect(Array.from(context).length).toBeGreaterThan(100);
  });

  it("never lets a placeholder leak, even for text that looks like one", () => {
    expect(said("\uE000" + "0" + "\uE001 then `x`")).toBe("0 then x");
    const spans = Array.from({ length: 80 }, (_, i) => `see \`file${i}.ts\``);
    const context = said(`${spans.join(" ")} done`);
    expect(context).not.toMatch(/[\uE000\uE001]/);
    expect(context.endsWith("see file79.ts done")).toBe(true);
  });

  it("stays fast on a huge pathological message and keeps the end", () => {
    const inputs = [
      `${"*a ".repeat(7000)}the end.`,
      `${"**a_ ".repeat(4000)}the end.`,
      `${"[".repeat(20000)} the end.`,
      `${"<a ".repeat(7000)}the end.`,
      `x ${"`".repeat(20000)} the end.`,
      `${"x ".repeat(10000)}the end.`,
      `${"[a ".repeat(7000)}the end.`,
      `${"`a ".repeat(7000)}the end.`,
      `${"x `a` ".repeat(30000)}the end.`,
      `${Array.from({ length: 20000 }, (_, i) => `${"`".repeat(1 + (i % 7))}a `).join("")}the end.`,
      `${"Sentence number one. ".repeat(1000)}Last sentence here.`,
    ];
    for (const input of inputs) {
      const started = performance.now();
      const context = said(input);
      expect(performance.now() - started).toBeLessThan(100);
      expect(Array.from(context).length).toBeLessThanOrEqual(300);
      expect(context.startsWith("… ")).toBe(true);
      expect(context).toMatch(/(the end\.|Last sentence here\.)$/);
    }
  });

  describe("windowing a message longer than the scan window", () => {
    const lineOf = (i: number) =>
      `Step ${i}: the loader in \`src/app.ts\` stops \`if (!ready)\` early, then \`init()\` runs twice.`;
    const QUESTION = "Shall I commit this change?";

    function wordsOf(source: string): Set<string> {
      return new Set(source.replace(/`/g, "").split(/\s+/));
    }

    /** No unpaired code and no glued words: every output word is a source word. */
    function expectFaithful(output: string, source: string) {
      const known = wordsOf(source);
      expect(output.endsWith(QUESTION), output).toBe(true);
      expect(output).not.toMatch(/`/);
      for (const word of output.replace(/^… /, "").split(/\s+/)) {
        expect(
          known.has(word),
          `glued or cut word "${word}" in: ${output}`,
        ).toBe(true);
      }
      expect(output).toContain("src/app.ts");
    }

    it("never shifts code spans when the window cuts a multi-line paragraph", () => {
      for (let pad = 0; pad < 60; pad++) {
        const lines = Array.from({ length: 60 }, (_, i) => lineOf(i));
        const text = `${lines.join("\n")}\n${"p ".repeat(pad)}${QUESTION}`;
        expectFaithful(
          questionContext(asking({ blocks: [block("assistant", text)] })),
          text,
        );
      }
    });

    it("never shifts code spans in one huge line", () => {
      for (let pad = 0; pad < 60; pad++) {
        const text = `${Array.from({ length: 60 }, (_, i) => lineOf(i)).join(" ")} ${"p ".repeat(pad)}${QUESTION}`;
        expectFaithful(
          questionContext(asking({ blocks: [block("assistant", text)] })),
          text,
        );
      }
    });

    it("starts at a paragraph boundary when the window has one", () => {
      const first = Array.from({ length: 30 }, (_, i) => lineOf(i)).join("\n");
      const second = Array.from({ length: 20 }, (_, i) => lineOf(100 + i)).join(
        "\n",
      );
      const text = `${first}\n\n${second}\n${QUESTION}`;
      const context = questionContext(
        asking({ blocks: [block("assistant", text)] }),
      );
      expectFaithful(context, text);
      expect(context.startsWith("… ")).toBe(true);
    });

    it("does not drop a short final message after a huge one", () => {
      const text = `${"word ".repeat(3000)}\n\nDone. ${QUESTION}`;
      // Text before the paragraph was left out, so the marker stays.
      expect(said(text)).toBe(`\u2026 Done. ${QUESTION}`);
    });
  });

  describe("code spans stay inside their own block", () => {
    it("does not pair a stray backtick with one in a later list item", () => {
      const text = [
        "I made these changes:",
        "- Escaped the backtick (`) in the template",
        "- Renamed `foo` to `bar`",
        "- Updated `README.md` with the new flag",
        "",
        "Shall I commit this change?",
      ].join("\n");
      expect(said(text)).toBe(
        [
          "I made these changes:",
          "- Escaped the backtick (`) in the template",
          "- Renamed foo to bar",
          "- Updated README.md with the new flag",
          "Shall I commit this change?",
        ].join("\n"),
      );
    });

    it("does not pair a stray backtick in an ordered list item either", () => {
      const text = "1. Typed a ` by mistake\n2. Ran `npm test` after";
      expect(said(text)).toBe("- Typed a ` by mistake\n- Ran npm test after");
    });

    it("keeps a heading's stray backtick away from the prose below it", () => {
      expect(said("## The ` char\nUse `real` span here.")).toBe(
        "The ` char Use real span here.",
      );
    });

    it("keeps a table row's stray backtick away from the next row", () => {
      expect(said("| a | ` | c |\n| x | `npm test` | y |")).toBe(
        "a, `, c x, npm test, y",
      );
    });

    it("still joins a span that runs over several lines of one paragraph", () => {
      expect(said("Run `npm\nrun build` now.")).toBe("Run npm run build now.");
      expect(said("- Run `npm\n  run build` now")).toBe(
        "- Run npm run build now",
      );
    });

    it("handles Windows line endings", () => {
      expect(said("Run `npm\r\nrun build` now.")).toBe(
        "Run npm run build now.",
      );
      expect(said("Options:\r\n- Keep `a`\r\n- Drop `b`\r\n")).toBe(
        "Options:\n- Keep a\n- Drop b",
      );
      expect(said("# Title\r\n\r\nBody `x`.\r\n")).toBe("Title Body x.");
    });

    it("bounds the work for a message of a couple of megabytes", () => {
      const lines = Array.from(
        { length: 30000 },
        (_, i) =>
          `Line ${i} has \`code\` and plenty of words for padding this test.`,
      );
      const many = `${lines.join("\n")}\nFinal answer is ready now. Shall I commit this change?`;
      const oneLine = `${"word `x` ".repeat(250000)}Done. Final answer is ready now. Shall I commit this change?`;
      for (const text of [many, oneLine]) {
        expect(text.length).toBeGreaterThan(1_900_000);
        const started = performance.now();
        const context = said(text);
        expect(performance.now() - started).toBeLessThan(500);
        expect(context.startsWith("… ")).toBe(true);
        expect(
          context.endsWith(
            "Final answer is ready now. Shall I commit this change?",
          ),
        ).toBe(true);
      }
    });
  });

  it("is deterministic", () => {
    const text = "Alpha one. Beta two. ".repeat(40) + "```x```";
    expect(said(text)).toBe(said(text));
  });

  it("caps a long realistic message below CONTEXT_MAX", () => {
    const text = [
      "## Plan",
      "I read `src/app.ts` and the **loader**. Both call `init()` twice.",
      "```ts",
      "init(); init();",
      "```",
      "- Remove the second call in the loader to stop the double start",
      "- Keep the one in app.ts because tests rely on it being there",
      "That keeps behaviour the same. It also removes a race that showed up in CI last week. I verified it with the full suite and it passes on every platform we build for.",
      "Should I go ahead and remove the second call?",
    ].join("\n");
    const context = said(text, {
      pendingQuestion: {
        requestId: 9,
        questions: [
          {
            id: "q",
            prompt: "Should I go ahead and remove the second call?",
            multiSelect: false,
            allowCustom: false,
            options: [{ id: "y", label: "Yes" }],
          },
        ],
      },
    });
    expect(Array.from(context).length).toBeLessThanOrEqual(300);
    expect(context).not.toMatch(/[`*#]|init\(\); init/);
    expect(context.endsWith("passes on every platform we build for.")).toBe(
      true,
    );
  });
});

describe("forwardPayload", () => {
  it("is undefined without a pending question", () => {
    expect(
      forwardPayload(asking({ pendingQuestion: undefined }), "/p", 300),
    ).toBe(undefined);
  });

  it("maps the question and builds the header", () => {
    const session = asking({
      linkedWorkItem: { kind: "issue", repo: "a/b", number: 12, url: "u" },
      blocks: [block("assistant", "Need input")],
    });
    setWorkflowSessionContext(session.id, {
      workflowName: "Triage",
      nodeTitle: "Fix",
      itemLabel: "Crash",
      round: 2,
    });
    cleanup.push(session.id);
    expect(forwardPayload(session, "/work/acme", 90)).toEqual({
      project: "/work/acme",
      sessionId: session.id,
      requestId: 7,
      topicTitle: "[#12] Fix the login",
      header:
        "acme · Fix the login · #12\nWorkflow Triage · Fix · Crash · round 2",
      context: "Need input",
      delaySecs: 90,
      questions: prompt.questions,
    });
  });

  it("header without ticket or workflow is one line", () => {
    expect(forwardPayload(asking(), "/work/acme", 300)?.header).toBe(
      "acme · Fix the login",
    );
  });
});

describe("forwardDelayMs", () => {
  it("uses the project's delay and falls back to 5 minutes when unknown", () => {
    expect(forwardDelayMs({ delaySecs: 30 })).toBe(30_000);
    expect(forwardDelayMs({ delaySecs: 0 })).toBe(0);
    expect(forwardDelayMs(undefined)).toBe(FORWARD_DELAY_MS);
    expect(forwardDelayMs({})).toBe(FORWARD_DELAY_MS);
    expect(FORWARD_DELAY_MS).toBe(300_000);
  });
});

describe("dueForwards", () => {
  const fiveMinutes = () => FORWARD_DELAY_MS;

  it("due only after the delay and only once", () => {
    const session = asking();
    const key = `${session.id}#7`;
    const first = new Map([[key, 1_000]]);
    const sent = new Set<string>();
    expect(
      dueForwards(
        [session],
        first,
        sent,
        1_000 + FORWARD_DELAY_MS - 1,
        fiveMinutes,
      ),
    ).toEqual([]);
    expect(
      dueForwards(
        [session],
        first,
        sent,
        1_000 + FORWARD_DELAY_MS,
        fiveMinutes,
      ),
    ).toEqual([session]);
    sent.add(key);
    expect(
      dueForwards(
        [session],
        first,
        sent,
        1_000 + FORWARD_DELAY_MS,
        fiveMinutes,
      ),
    ).toEqual([]);
  });

  it("uses the delay of each session", () => {
    const quick = asking();
    const slow = asking();
    const first = new Map([
      [`${quick.id}#7`, 1_000],
      [`${slow.id}#7`, 1_000],
    ]);
    const delayMsFor = (session: Session) =>
      session.id === quick.id ? 30_000 : 600_000;
    expect(
      dueForwards([quick, slow], first, new Set(), 1_000 + 29_999, delayMsFor),
    ).toEqual([]);
    expect(
      dueForwards([quick, slow], first, new Set(), 1_000 + 30_000, delayMsFor),
    ).toEqual([quick]);
    expect(
      dueForwards([quick, slow], first, new Set(), 1_000 + 600_000, delayMsFor),
    ).toEqual([quick, slow]);
  });

  it("a zero delay is due as soon as the question was seen", () => {
    const session = asking();
    const first = new Map([[`${session.id}#7`, 1_000]]);
    expect(dueForwards([session], first, new Set(), 1_000, () => 0)).toEqual([
      session,
    ]);
    // Still never for a question that was not seen (disabled or unconfigured at first sight).
    expect(
      dueForwards([session], new Map(), new Set(), 1_000, () => 0),
    ).toEqual([]);
  });

  it("skips unseen, unforwardable and question-less sessions", () => {
    const hidden = asking({ ephemeral: true });
    const none = asking({ pendingQuestion: undefined });
    const unseen = asking();
    const first = new Map([
      [`${hidden.id}#7`, 0],
      [`${none.id}#7`, 0],
    ]);
    expect(
      dueForwards(
        [hidden, none, unseen],
        first,
        new Set(),
        FORWARD_DELAY_MS * 2,
        fiveMinutes,
      ),
    ).toEqual([]);
  });
});
