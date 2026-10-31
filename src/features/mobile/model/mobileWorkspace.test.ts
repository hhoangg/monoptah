// @vitest-environment happy-dom
import { beforeEach, describe, expect, it } from "vitest";
import { saveProviderSurface } from "../../providers/model/providerSurface";
import { setProjectDefaultProvider } from "../../sessions/model/projectProviders";
import { mobileNewSession, mobileQuestionReply } from "./mobileWorkspace";

beforeEach(() => localStorage.clear());

it("starts a phone session as a chat even when its provider opens in a terminal", () => {
  setProjectDefaultProvider("/project", "claude", "claude:opus-5");
  saveProviderSurface("claude", "tui");

  const session = mobileNewSession("/project", "auto");

  expect(session.harness).toBe("claude");
  expect(session.surface).toBe("chat");
  expect(session.runtimeMode).toBe("auto");
});

const prompt = {
  requestId: 1,
  questions: [
    {
      id: "colour",
      prompt: "Which colour?",
      multiSelect: false,
      allowCustom: true,
      options: [
        { id: "red", label: "Red" },
        { id: "blue", label: "Blue" },
      ],
    },
    {
      id: "size",
      prompt: "Which sizes?",
      multiSelect: true,
      allowCustom: false,
      options: [
        { id: "s", label: "Small" },
        { id: "l", label: "Large" },
      ],
    },
  ],
};

describe("mobile question replies", () => {
  it("keeps the questions and options asked, and a deliberate partial answer", () => {
    expect(
      mobileQuestionReply(
        { kind: "answered", answers: { colour: ["red"], size: ["s", "xl"], other: ["x"] } },
        prompt,
      ),
    ).toEqual({ kind: "answered", answers: { colour: ["red"], size: ["s"] } });
    expect(
      mobileQuestionReply(
        { kind: "answered", answers: { colour: ["__custom__"] }, custom: { colour: " Teal " } },
        prompt,
      ),
    ).toEqual({ kind: "answered", answers: { colour: ["__custom__"] }, custom: { colour: "Teal" } });
  });

  it("skips only when asked to, rejecting answers that leave nothing answered", () => {
    expect(mobileQuestionReply({ kind: "skipped" }, prompt)).toEqual({ kind: "skipped" });
    expect(mobileQuestionReply({ kind: "answered", answers: { colour: ["green"] } }, prompt)).toBeUndefined();
    expect(
      mobileQuestionReply(
        { kind: "answered", answers: { colour: ["__custom__"] }, custom: { colour: "x".repeat(10_001) } },
        prompt,
      ),
    ).toBeUndefined();
    expect(mobileQuestionReply({ kind: "maybe" }, prompt)).toBeUndefined();
    expect(mobileQuestionReply(null, prompt)).toBeUndefined();
  });
});
