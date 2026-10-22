// @vitest-environment happy-dom
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import {
  isProviderSessionId,
  monoProviderFiles,
  providerFilesProblemMessage,
  type ProviderSessionRef,
} from "./monoProviderFiles";
import { retiredProviderSessions, saveMonoRotation } from "./monoRotation";

const A = "11111111-1111-4111-8111-111111111111";
const B = "22222222-2222-4222-8222-222222222222";
const C = "33333333-3333-4333-8333-333333333333";

const claude = (id: string): ProviderSessionRef => ({ harness: "claude", id });
const codex = (id: string): ProviderSessionRef => ({ harness: "codex", id });

beforeEach(() => {
  const stored = new Map<string, string>();
  vi.stubGlobal("localStorage", {
    getItem: (key: string) => stored.get(key) ?? null,
    setItem: (key: string, value: string) => stored.set(key, value),
  });
});
afterEach(() => vi.unstubAllGlobals());

it("accepts only canonical UUID-shaped ids", () => {
  expect(isProviderSessionId(A)).toBe(true);
  expect(isProviderSessionId(A.toUpperCase())).toBe(true);
  for (const bad of [
    "../x",
    "memory",
    "",
    `${A}0`,
    A.slice(0, 35),
    "zzzzzzzz-zzzz-zzzz-zzzz-zzzzzzzzzzzz",
    A.replaceAll("-", "_"),
    `${A.slice(0, 35)}/`,
    undefined,
    42,
  ]) {
    expect(isProviderSessionId(bad)).toBe(false);
  }
});

it("offers the current Claude Code session and the retired ones, newest last", () => {
  const files = monoProviderFiles({ harness: "claude", providerSessionId: C }, [
    claude(A),
    claude(B),
  ]);
  expect(files.sessions).toEqual([claude(C), claude(A), claude(B)]);
  expect(files.supported).toBe(true);
  expect(files.label).toBe("Claude Code");
  expect(files.note).toBeUndefined();
});

it("dedupes a current session that was also retired", () => {
  const files = monoProviderFiles({ harness: "codex", providerSessionId: A }, [
    codex(A),
    codex(B),
    codex(B),
  ]);
  expect(files.sessions).toEqual([codex(A), codex(B)]);
  expect(files.label).toBe("Codex");
});

it("names both providers when both have files", () => {
  const files = monoProviderFiles({ harness: "codex", providerSessionId: A }, [
    claude(B),
  ]);
  expect(files.sessions).toEqual([codex(A), claude(B)]);
  expect(files.label).toBe("Claude Code and Codex");
});

it.each(["cursor", "opencode", "grok", "pi", "devin"])(
  "cannot delete %s files and says so",
  (harness) => {
    const files = monoProviderFiles({ harness, providerSessionId: A }, []);
    expect(files.sessions).toEqual([]);
    expect(files.supported).toBe(false);
    expect(files.note).toMatch(
      /^Monoptah can't delete .+ session files yet\.$/,
    );
  },
);

it("uses the harness display name in the unsupported note", () => {
  expect(
    monoProviderFiles({ harness: "cursor", providerSessionId: A }, []).note,
  ).toBe("Monoptah can't delete Cursor session files yet.");
  expect(
    monoProviderFiles({ harness: "opencode", providerSessionId: A }, []).note,
  ).toBe("Monoptah can't delete OpenCode session files yet.");
});

it("still offers retired sessions when the current harness is unsupported", () => {
  const files = monoProviderFiles({ harness: "cursor", providerSessionId: A }, [
    claude(B),
  ]);
  expect(files.sessions).toEqual([claude(B)]);
  expect(files.supported).toBe(true);
  expect(files.label).toBe("Claude Code");
  expect(files.note).toContain("Cursor");
  expect(files.note).toContain("not covered");
});

it("says there is nothing to delete yet when a supported harness has no ids", () => {
  const none = monoProviderFiles({ harness: "claude" }, []);
  expect(none.sessions).toEqual([]);
  expect(none.supported).toBe(false);
  expect(none.label).toBe("Claude Code");
  expect(none.note).toBe(
    "There are no Claude Code session files to delete yet.",
  );
  expect(monoProviderFiles({ harness: "codex" }, []).note).toBe(
    "There are no Codex session files to delete yet.",
  );
});

it("uses retired sessions when the current one has no id yet", () => {
  const files = monoProviderFiles({ harness: "codex" }, [codex(A)]);
  expect(files.sessions).toEqual([codex(A)]);
  expect(files.note).toBeUndefined();
});

it.each([
  "../x",
  "memory",
  "",
  `${A}0`,
  "zzzzzzzz-zzzz-zzzz-zzzz-zzzzzzzzzzzz",
])("refuses the malformed id %j", (id) => {
  const files = monoProviderFiles(
    { harness: "claude", providerSessionId: id },
    [claude(id), { harness: "cursor", id: A } as never],
  );
  expect(files.sessions).toEqual([]);
  expect(files.supported).toBe(false);
});

it("reads retired sessions from storage and ignores malformed entries", () => {
  saveMonoRotation("mono-1", {
    at: 1,
    reason: "context",
    earlier: [],
    retired: [
      claude(A),
      { harness: "cursor", id: B },
      { harness: "claude", id: "../x" },
      { harness: "codex" },
      "nope",
      null,
      codex(C),
      claude(A),
    ] as never,
  });
  expect(retiredProviderSessions("mono-1")).toEqual([claude(A), codex(C)]);
  expect(retiredProviderSessions("other")).toEqual([]);
});

it("reads nothing from a rotation without a retired list or broken storage", () => {
  saveMonoRotation("mono-1", { at: 1, reason: "context", earlier: [] });
  expect(retiredProviderSessions("mono-1")).toEqual([]);
  saveMonoRotation("mono-2", {
    at: 1,
    reason: "context",
    earlier: [],
    retired: "x" as never,
  });
  expect(retiredProviderSessions("mono-2")).toEqual([]);
});

it("words the leftover-files message for one and for several problems", () => {
  expect(providerFilesProblemMessage("Claude Code", 1)).toBe(
    "Conversation reset. Some Claude Code session files may not have been deleted (1 problem); you can remove them manually.",
  );
  expect(providerFilesProblemMessage("Codex", 3)).toBe(
    "Conversation reset. Some Codex session files may not have been deleted (3 problems); you can remove them manually.",
  );
});
