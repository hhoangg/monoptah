import { describe, expect, it } from "vitest";
import type { SlashCommand } from "@monoptah/protocol";
import { rankCommands, scoreCommand, slashTokenAt } from "./slashToken";

describe("slashTokenAt", () => {
  it("opens on a bare slash and on a word that starts with one", () => {
    expect(slashTokenAt("/", 1)).toEqual({ start: 0, end: 1, query: "" });
    expect(slashTokenAt("/pl", 3)).toEqual({ start: 0, end: 3, query: "pl" });
    expect(slashTokenAt("fix this /re", 12)).toEqual({ start: 9, end: 12, query: "re" });
  });

  it("covers the whole word when the caret is in the middle of it", () => {
    expect(slashTokenAt("/compact now", 4)).toEqual({ start: 0, end: 8, query: "com" });
  });

  it("stays closed for paths, URLs, emails, quotes and text after the word", () => {
    expect(slashTokenAt("src/main.rs", 11)).toBeNull();
    expect(slashTokenAt("see /usr/bin", 12)).toBeNull();
    expect(slashTokenAt("/a.b", 4)).toBeNull();
    expect(slashTokenAt("https://x.dev", 13)).toBeNull();
    expect(slashTokenAt("> /plan", 7)).toBeNull();
    expect(slashTokenAt("/plan now", 9)).toBeNull();
    expect(slashTokenAt("/plan", 99)).toBeNull();
  });
});

describe("rankCommands", () => {
  const row = (id: string, kind: "builtin" | "skill", label: string, detail = ""): SlashCommand => ({ id, kind, label, detail, icon: id, available: true, mode: false, active: false });
  const rows = [row("draft", "builtin", "Draft"), row("plan", "builtin", "Plan mode"), row("compact", "builtin", "Compact context"), row("plan-review", "skill", "plan-review", "Review a plan"), row("lint", "skill", "lint", "Lint the project")];

  it("keeps the desktop's order for a bare slash, built-ins first", () => {
    expect(rankCommands(rows, "").map((r) => r.id)).toEqual(["draft", "plan", "compact", "lint", "plan-review"]);
  });

  it("filters and ranks by what is typed", () => {
    expect(rankCommands(rows, "pl").map((r) => r.id)).toEqual(["plan", "plan-review"]);
    expect(rankCommands(rows, "lnt").map((r) => r.id)).toEqual(["lint"]);
    expect(rankCommands(rows, "zzz")).toEqual([]);
  });

  it("scores an exact name over a prefix over a substring over a fuzzy match", () => {
    const exact = scoreCommand("plan", "plan") ?? 0;
    const prefix = scoreCommand("pl", "plan") ?? 0;
    const inner = scoreCommand("an", "plan") ?? 0;
    const fuzzy = scoreCommand("pn", "plan") ?? 0;
    expect(exact).toBeGreaterThan(prefix);
    expect(prefix).toBeGreaterThan(inner);
    expect(inner).toBeGreaterThan(fuzzy);
    expect(scoreCommand("x", "plan")).toBeNull();
  });
});
