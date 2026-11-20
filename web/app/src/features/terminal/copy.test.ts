import { TERMINAL_EXIT_REASONS } from "@monoptah/protocol";
import { describe, expect, it } from "vitest";
import { LOCAL_EXIT_REASONS, endedText, openFailedText, OPEN_FAILED_FALLBACK, wrongCodeNotice } from "./copy";
import { clockText, durationText, limitsHint } from "./time";

describe("endedText", () => {
  it("has its own sentence for every exit reason the desktop sends and the two the web adds", () => {
    const texts = new Set([...TERMINAL_EXIT_REASONS, ...LOCAL_EXIT_REASONS].map((reason) => endedText({ reason })));
    expect(texts.size).toBe(TERMINAL_EXIT_REASONS.length + LOCAL_EXIT_REASONS.length);
    for (const text of texts) expect(text).not.toMatch(/_|undefined/);
  });
  it("says the exit code and the idle limit", () => {
    expect(endedText({ reason: "exit", code: 2 })).toContain("code 2");
    expect(endedText({ reason: "exit" })).toBe("the shell exited.");
    expect(endedText({ reason: "idle", idleMinutes: 15 })).toContain("15 minutes");
    expect(endedText({ reason: "idle", idleMinutes: 1 })).toContain("1 minute.");
  });
  it("never repeats a reason it does not know", () => {
    expect(endedText({ reason: "<b>boom</b>" })).toBe("the terminal ended.");
  });
});

describe("openFailedText", () => {
  it("covers the desktop codes of terminal.open and falls back for the rest", () => {
    for (const code of ["read_only", "forbidden", "disabled", "not_selected", "busy", "not_found", "unsupported"]) expect(openFailedText(code)).not.toBe(OPEN_FAILED_FALLBACK);
    expect(openFailedText("disabled")).toContain("Allow web terminal");
    expect(openFailedText("toString")).toBe(OPEN_FAILED_FALLBACK);
    expect(openFailedText(undefined)).toBe(OPEN_FAILED_FALLBACK);
  });
  it("counts the tries left", () => {
    expect(wrongCodeNotice(2)).toContain("2 tries left");
    expect(wrongCodeNotice(1)).toContain("1 try left");
  });
});

describe("time text", () => {
  it("formats durations and the countdown", () => {
    expect(durationText(45_000)).toBe("45 s");
    expect(durationText(15 * 60_000)).toBe("15 min");
    expect(durationText(8 * 3_600_000)).toBe("8 h");
    expect(durationText(7 * 3_600_000 + 52 * 60_000)).toBe("7 h 52 min");
    expect(clockText(45_000)).toBe("0:45");
    expect(clockText(61_000)).toBe("1:01");
    expect(clockText(-5)).toBe("0:00");
  });
  it("hints the idle and the total limit", () => {
    expect(limitsHint({ openedAt: 0, idleTimeoutMs: 900_000, maxDurationMs: 28_800_000 }, 0)).toBe("Closes after 15 min without typing, and in 8 h at the latest.");
    expect(limitsHint({ openedAt: 0, idleTimeoutMs: 0, maxDurationMs: 0 }, 0)).toBe("");
  });
});
