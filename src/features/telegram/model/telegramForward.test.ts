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
  forwardPayload,
  isForwardable,
  questionContext,
  sessionTicketId,
  topicTitle,
} from "./telegramForward";
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
afterEach(() => {
  for (const id of cleanup.splice(0)) clearWorkflowSessionContext(id);
});

describe("isForwardable", () => {
  it("excludes orchestration workers, inbox ask, ephemeral, hidden, tui, mono chats, remote, codex auto-resolving", () => {
    expect(isForwardable(asking())).toBe(true);
    expect(isForwardable(asking({ orchestrationLeadId: "lead" }))).toBe(false);
    expect(isForwardable(asking({ inboxAsk: {} as Session["inboxAsk"] }))).toBe(
      false,
    );
    expect(isForwardable(asking({ ephemeral: true }))).toBe(false);
    expect(isForwardable(asking({ sidebarHidden: true }))).toBe(false);
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

describe("sessionTicketId", () => {
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
  it("context takes last assistant text tail 600", () => {
    const session = asking({
      blocks: [
        block("assistant", "old reply"),
        block("assistant", "z".repeat(700) + "END"),
        block("tool", "tool output"),
        block("assistant", "internal note", { internal: true }),
        block("user", "user text"),
      ],
    });
    const context = questionContext(session);
    expect(context).toHaveLength(600);
    expect(context.endsWith("zzzEND")).toBe(true);
    expect(questionContext(asking())).toBe("");
  });

  it("counts characters, not UTF-16 units", () => {
    const session = asking({ blocks: [block("assistant", "😀".repeat(700))] });
    expect(Array.from(questionContext(session))).toHaveLength(600);
  });
});

describe("forwardPayload", () => {
  it("is undefined without a pending question", () => {
    expect(forwardPayload(asking({ pendingQuestion: undefined }), "/p")).toBe(
      undefined,
    );
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
    expect(forwardPayload(session, "/work/acme")).toEqual({
      project: "/work/acme",
      sessionId: session.id,
      requestId: 7,
      topicTitle: "[#12] Fix the login",
      header:
        "acme · Fix the login · #12\nWorkflow Triage · Fix · Crash · round 2",
      context: "Need input",
      questions: prompt.questions,
    });
  });

  it("header without ticket or workflow is one line", () => {
    expect(forwardPayload(asking(), "/work/acme")?.header).toBe(
      "acme · Fix the login",
    );
  });
});

describe("dueForwards", () => {
  it("due only after 5 minutes and only once", () => {
    const session = asking();
    const key = `${session.id}#7`;
    const first = new Map([[key, 1_000]]);
    const sent = new Set<string>();
    expect(
      dueForwards([session], first, sent, 1_000 + FORWARD_DELAY_MS - 1),
    ).toEqual([]);
    expect(
      dueForwards([session], first, sent, 1_000 + FORWARD_DELAY_MS),
    ).toEqual([session]);
    sent.add(key);
    expect(
      dueForwards([session], first, sent, 1_000 + FORWARD_DELAY_MS),
    ).toEqual([]);
  });

  it("skips unseen, unforwardable and question-less sessions", () => {
    const hidden = asking({ sidebarHidden: true });
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
      ),
    ).toEqual([]);
  });
});
