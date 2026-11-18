// Copied from src/features/sessions/model/userQuestion.test.ts at ca73ac2b
import type { QuestionPrompt } from "@monoptah/protocol";
import { describe, expect, it } from "vitest";
import {
  CUSTOM_OPTION_ID,
  buildQuestionReply,
  displayOptions,
  nextSelection,
  questionAnswersComplete,
  questionCountdown,
  questionIsComplete,
  questionPromptTitle,
  selectedAnswerLabels,
  type Question,
} from "./userQuestion";

function question(overrides: Partial<Question> & { id: string; prompt: string }): Question {
  return { multiSelect: false, allowCustom: true, options: [], ...overrides };
}

describe("questionPromptTitle", () => {
  it("prefers the header, then the prompt, then a default", () => {
    expect(questionPromptTitle([question({ id: "a", prompt: "Which?", header: "Format" })])).toBe("Format");
    expect(questionPromptTitle([question({ id: "a", prompt: "Which?" })])).toBe("Which?");
    expect(questionPromptTitle([])).toBe("Question");
  });
});

describe("question answers", () => {
  const questions = [
    question({ id: "file", prompt: "Which file?", options: [{ id: "a.ts", label: "a.ts" }, { id: "b.ts", label: "b.ts" }] }),
  ];
  const q = questions[0]!;

  it("requires a selection before continue", () => {
    expect(questionAnswersComplete(questions, {}, {})).toBe(false);
    expect(questionAnswersComplete(questions, { [q.id]: ["a.ts"] }, {})).toBe(true);
    expect(questionAnswersComplete([], {}, {})).toBe(false);
  });

  it("requires custom text when Other is selected", () => {
    expect(questionAnswersComplete(questions, { [q.id]: [CUSTOM_OPTION_ID] }, {})).toBe(false);
    expect(questionAnswersComplete(questions, { [q.id]: [CUSTOM_OPTION_ID] }, { [q.id]: "   " })).toBe(false);
    expect(questionAnswersComplete(questions, { [q.id]: [CUSTOM_OPTION_ID] }, { [q.id]: "c.ts" })).toBe(true);
  });

  it("accepts custom text alone only when custom answers are allowed", () => {
    expect(questionIsComplete(q, {}, { [q.id]: "c.ts" })).toBe(true);
    expect(questionIsComplete({ ...q, allowCustom: false }, {}, { [q.id]: "c.ts" })).toBe(false);
  });

  it("maps selected ids back to labels and custom text", () => {
    expect(selectedAnswerLabels(q, { kind: "answered", answers: { [q.id]: ["b.ts"] } })).toEqual(["b.ts"]);
    expect(
      selectedAnswerLabels(q, { kind: "answered", answers: { [q.id]: [CUSTOM_OPTION_ID] }, custom: { [q.id]: "c.ts" } }),
    ).toEqual(["c.ts"]);
  });
});

describe("custom option", () => {
  it("appends Other when custom text is allowed and no Other option exists", () => {
    const q = question({ id: "q", prompt: "?", options: [{ id: "a", label: "A" }] });
    expect(displayOptions(q).map((o) => o.id)).toEqual(["a", CUSTOM_OPTION_ID]);
    expect(displayOptions({ ...q, allowCustom: false }).map((o) => o.id)).toEqual(["a"]);
    const withOther = question({ id: "q", prompt: "?", options: [{ id: "x", label: "other" }] });
    expect(displayOptions(withOther).map((o) => o.id)).toEqual(["x"]);
  });

  it("treats an option labelled Other as the custom option", () => {
    const q = question({ id: "q", prompt: "?", multiSelect: true, options: [{ id: "a", label: "A" }, { id: "x", label: "Other" }] });
    expect(nextSelection(q, ["a"], "x")).toEqual(["a", "x"]);
    expect(nextSelection(q, ["a", "x"], CUSTOM_OPTION_ID)).toEqual(["a", CUSTOM_OPTION_ID]);
    expect(questionIsComplete(q, { q: ["x"] }, {})).toBe(false);
    expect(questionIsComplete(q, { q: ["x"] }, { q: "mine" })).toBe(true);
  });

  it("single-select replaces and multi-select toggles", () => {
    const single = question({ id: "q", prompt: "?", options: [{ id: "a", label: "A" }, { id: "b", label: "B" }] });
    expect(nextSelection(single, ["a"], "b")).toEqual(["b"]);
    const multi = { ...single, multiSelect: true };
    expect(nextSelection(multi, ["a"], "b")).toEqual(["a", "b"]);
    expect(nextSelection(multi, ["a", "b"], "a")).toEqual(["b"]);
  });
});

describe("buildQuestionReply", () => {
  const language = question({ id: "lang", prompt: "Language?", header: "Language", options: [{ id: "ts", label: "TypeScript" }, { id: "js", label: "JavaScript" }] });
  const files = question({ id: "files", prompt: "Which files?", multiSelect: true, options: [{ id: "a.ts", label: "a.ts" }, { id: "b.ts", label: "b.ts" }] });
  const questions = [language, files];

  it("skips the whole prompt when no question was answered", () => {
    expect(buildQuestionReply(questions, {}, {})).toEqual({ kind: "skipped" });
    expect(questionIsComplete(language, {}, {})).toBe(false);
  });

  it("keeps answers for questions that were filled and drops skipped ones", () => {
    expect(buildQuestionReply(questions, { files: ["a.ts", "b.ts"] }, {})).toEqual({
      kind: "answered",
      answers: { files: ["a.ts", "b.ts"] },
    });
  });

  it("includes every completed question and trims custom text", () => {
    expect(
      buildQuestionReply(questions, { lang: [CUSTOM_OPTION_ID], files: ["a.ts"] }, { lang: "  Python " }),
    ).toEqual({
      kind: "answered",
      answers: { lang: [CUSTOM_OPTION_ID], files: ["a.ts"] },
      custom: { lang: "Python" },
    });
  });

  it("produces replies that match the protocol shape", async () => {
    const { QuestionReplySchema } = await import("@monoptah/protocol");
    expect(QuestionReplySchema.safeParse(buildQuestionReply(questions, {}, {})).success).toBe(true);
    expect(QuestionReplySchema.safeParse(buildQuestionReply(questions, { files: ["a.ts"] }, {})).success).toBe(true);
  });
});

describe("questionCountdown", () => {
  it("is none without a deadline, optional beyond 60 s and counting inside it", () => {
    expect(questionCountdown(undefined, 0)).toEqual({ kind: "none" });
    expect(questionCountdown(61_000, 0)).toEqual({ kind: "optional" });
    expect(questionCountdown(60_000, 0)).toEqual({ kind: "counting", seconds: 60 });
    expect(questionCountdown(10_500, 0)).toEqual({ kind: "counting", seconds: 11 });
    expect(questionCountdown(1_000, 5_000)).toEqual({ kind: "counting", seconds: 0 });
  });
});

describe("types", () => {
  it("accept a protocol prompt", () => {
    const prompt: QuestionPrompt = { requestId: 1, questions: [question({ id: "q", prompt: "?" })] };
    expect(prompt.questions).toHaveLength(1);
  });
});
