import { describe, expect, it } from "vitest";
import type { Turn } from "@monoptah/protocol";
import { latestSessionError, sessionErrorContent } from "./sessionError";

describe("sessionErrorContent", () => {
  it("special-cases a removed provider account", () => {
    const content = sessionErrorContent("Request failed: removed provider account acct-1");
    expect(content.title).toBe("Account unavailable");
    expect(content.recovery).toBeTruthy();
  });
  it("passes any other error through", () => {
    expect(sessionErrorContent("rate limited")).toEqual({ title: "Couldn't continue", message: "rate limited", recovery: undefined });
  });
});

describe("latestSessionError", () => {
  const turn = (items: Turn["items"]): Turn => ({ id: `t${Math.random()}`, items });
  it("finds the error of the latest turn only", () => {
    const turns = [turn([{ type: "notice", id: "old", text: "old", error: true }]), turn([{ type: "text", id: "a", text: "ok" }])];
    expect(latestSessionError(turns)).toBeUndefined();
    const withError = [...turns, turn([{ type: "notice", id: "n1", text: "first", error: true }, { type: "notice", id: "n2", text: "second", error: true }])];
    expect(latestSessionError(withError)?.id).toBe("n2");
  });
  it("ignores plain notices", () => {
    expect(latestSessionError([turn([{ type: "notice", id: "n", text: "Waiting for approval" }])])).toBeUndefined();
  });
});
