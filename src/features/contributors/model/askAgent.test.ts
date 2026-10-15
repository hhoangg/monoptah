import { describe, expect, it } from "vitest";
import { buildContributorsModel } from "./pipeline";
import { DEFAULT_CONFIG } from "./config";
import {
  leaderboardModel,
  leaderboardRaw,
  NOW,
} from "../ui/leaderboardFixture";
import { file, ident, makeCommit, oct } from "./testFixtures";
import {
  DEFAULT_ASK_OPTIONS,
  askContextChips,
  askSubtitle,
  buildAskRequest,
  buildAskSnapshot,
  personNameOf,
} from "./askAgent";
import type { AskOptions } from "./askAgent";

const model = leaderboardModel();
const ana = model.leaderboard.rows.find((r) => r.name === "Ana")!;
const bob = model.leaderboard.rows.find((r) => r.name === "Bob")!;

type Obj = Record<string, unknown>;
type Snap = {
  people: Obj[];
  focus: Obj;
  scope: { kind: string; personId?: string };
  stats: { rows: Obj[] };
  prs: Obj[];
  commits: Obj[];
  formula: unknown;
  notes: string[];
  project: string;
  branch: string;
};

const options = (extra: Partial<AskOptions> = {}): AskOptions => ({
  ...DEFAULT_ASK_OPTIONS,
  ...extra,
});

const snap = (
  extra: Partial<AskOptions> = {},
  scope = { kind: "team" } as const,
) =>
  buildAskSnapshot({
    model,
    project: "/work/monoptah",
    scope,
    options: options(extra),
    now: NOW,
  }) as unknown as Snap;

describe("askSubtitle", () => {
  it("describes each scope", () => {
    expect(askSubtitle({ kind: "team" })).toBe("about the whole team");
    expect(askSubtitle({ kind: "me", personId: "x" }, "Ana")).toBe(
      "about your score",
    );
    expect(askSubtitle({ kind: "person", personId: "x" }, "Bob")).toBe(
      "about Bob",
    );
  });
});

describe("personNameOf", () => {
  it("resolves the focused person and is undefined for a team", () => {
    expect(personNameOf(model, { kind: "team" })).toBeUndefined();
    expect(
      personNameOf(model, { kind: "person", personId: bob.personId }),
    ).toBe("Bob");
  });
});

describe("askContextChips", () => {
  it("lists project, branch, season, people and the three data parts for a team", () => {
    const chips = askContextChips(model, "/work/monoptah", { kind: "team" });
    expect(chips.map((c) => c.id)).toEqual([
      "project",
      "branch",
      "season",
      "people",
      "scores",
      "formula",
      "ai",
    ]);
    expect(chips[0].label).toBe("monoptah");
    expect(chips[1].label).toBe(model.raw.branch);
    expect(chips[2].label).toBe(`${model.season.label} season`);
    expect(chips[3].label).toMatch(/^4 people · \d+ identities$/);
    expect(chips[4].label).toBe("Scores + breakdown");
    expect(chips[5].label).toBe("Scoring formula");
    expect(chips[6].label).toBe("AI-assisted stats");
  });

  it("only the data chips are removable", () => {
    const chips = askContextChips(model, "/work/monoptah", { kind: "team" });
    expect(Object.fromEntries(chips.map((c) => [c.id, c.removable]))).toEqual({
      project: false,
      branch: false,
      season: false,
      people: true,
      scores: true,
      formula: true,
      ai: true,
    });
  });

  it("swaps the people chip for the focused person", () => {
    const person = askContextChips(model, "/work/monoptah", {
      kind: "person",
      personId: bob.personId,
    });
    expect(person.map((c) => c.id)).toEqual([
      "project",
      "branch",
      "season",
      "focus",
      "people",
      "scores",
      "formula",
      "ai",
    ]);
    expect(person.find((c) => c.id === "focus")).toMatchObject({
      label: "Bob (focus)",
      removable: false,
    });
    expect(person.find((c) => c.id === "people")).toMatchObject({
      label: expect.stringMatching(/^Whole team: 4 people · \d+ identities$/),
      removable: true,
    });
    const me = askContextChips(model, "/work/monoptah", {
      kind: "me",
      personId: ana.personId,
    });
    expect(me.find((c) => c.id === "focus")?.label).toBe("Ana (you)");
  });
});

describe("buildAskSnapshot", () => {
  it("keeps everything by default, with per-PR on and raw commits off", () => {
    const s = snap();
    expect(s.formula).toBeDefined();
    expect(s.people[0].parts).toBeDefined();
    expect(s.people[0].breakdown).toBeDefined();
    expect(s.people[0].aiAssistedPct).toBeDefined();
    expect(s.people[0].identities).toBeDefined();
    expect(s.prs.length).toBeGreaterThan(0);
    expect(s.commits).toBeUndefined();
    expect(s.project).toBe("monoptah");
    expect(s.branch).toBe(model.raw.branch);
  });

  it("drops the formula when that chip is removed", () => {
    const s = snap({ omit: ["formula"] });
    expect(s.formula).toBeUndefined();
    expect(s.people[0].parts).toBeDefined();
  });

  it("drops parts and breakdown when scores are removed, also on the focus", () => {
    const s = snap({ omit: ["scores"] }, {
      kind: "person",
      personId: bob.personId,
    } as never);
    for (const p of [...s.people, s.focus]) {
      expect(p.parts).toBeUndefined();
      expect(p.breakdown).toBeUndefined();
      expect(typeof p.score).toBe("number");
      expect(typeof p.rank).toBe("number");
    }
  });

  it("drops the AI-assisted fields everywhere", () => {
    const s = snap({ omit: ["ai"] });
    expect(s.people.every((p) => !("aiAssistedPct" in p))).toBe(true);
    expect(s.stats.rows.every((r) => !("aiAssistedPct" in r))).toBe(true);
    expect(s.prs.every((p) => !("aiAssisted" in p))).toBe(true);
  });

  it("drops identities when the people chip is removed", () => {
    const s = snap({ omit: ["people"] });
    expect(s.people.every((p) => !("identities" in p))).toBe(true);
    expect(s.stats.rows.every((r) => !("identities" in r))).toBe(true);
  });

  it("per-PR off removes prs; raw commits on adds commits", () => {
    expect(snap({ perPr: false }).prs).toBeUndefined();
    const s = snap({ rawCommits: true });
    expect(Array.isArray(s.commits)).toBe(true);
    expect(s.commits.length).toBeGreaterThan(0);
  });

  it("tells the agent what was left out and what the stats cover", () => {
    const s = snap({ omit: ["formula", "ai"] });
    const note = s.notes.find((n: string) => n.startsWith("Left out"));
    expect(note).toContain("scoring formula");
    expect(note).toContain("AI-assisted");
    expect(
      s.notes.some((n: string) => n.startsWith("The stats section covers")),
    ).toBe(true);
    expect(snap().notes.some((n: string) => n.startsWith("Left out"))).toBe(
      false,
    );
  });

  it("limits prs to the focused person", () => {
    const s = snap({}, { kind: "person", personId: ana.personId } as never);
    expect(s.prs.length).toBeGreaterThan(0);
    expect(s.prs.every((p) => p.personId === ana.personId)).toBe(true);
  });
});

describe("buildAskRequest", () => {
  const base = {
    model,
    project: "/work/monoptah",
    scope: { kind: "team" } as const,
    now: NOW,
  };

  it("builds the file, the prompt and the snapshot json", () => {
    const req = buildAskRequest({
      ...base,
      question: "  Who drove the work?  ",
      options: options(),
      harness: "claude",
    });
    expect(req.question).toBe("Who drove the work?");
    expect(req.fileName).toBe(`contributors-${model.season.id}.json`);
    expect(req.harness).toBe("claude");
    expect(req.scope).toEqual({ kind: "team" });
    expect(req.prompt).toContain("Question: Who drove the work?");
    expect(req.prompt).toContain(req.fileName);
    expect(req.prompt).toContain('"monoptah"');
    expect(JSON.parse(req.snapshotJson).season.id).toBe(model.season.id);
  });

  it("expresses allowTools in the prompt", () => {
    const on = buildAskRequest({ ...base, question: "q", options: options() });
    const off = buildAskRequest({
      ...base,
      question: "q",
      options: options({ allowTools: false }),
    });
    expect(on.prompt).toContain("read-only git and gh");
    expect(off.prompt).toContain("Do not run commands");
    expect(on.harness).toBeUndefined();
  });

  it("names the focused person in the prompt", () => {
    const req = buildAskRequest({
      ...base,
      scope: { kind: "person", personId: bob.personId },
      question: "q",
      options: options(),
    });
    expect(req.prompt).toContain("Bob");
  });
});

describe("anonymised snapshot (people chip removed)", () => {
  // Eve has no display name, so her name falls back to her e-mail address.
  const EVE = ident("", "eve@example.com");
  const withEve = buildContributorsModel({
    raw: leaderboardRaw({
      commits: [
        ...leaderboardRaw().commits,
        makeCommit("eve-1", EVE, oct(3), {
          files: [file("src/features/a.ts", 5)],
        }),
      ],
    }),
    config: DEFAULT_CONFIG,
    mailmap: [],
    merge: true,
    range: "30d",
    now: NOW,
    user: { name: "Ana", email: "ana@example.com" },
  });
  const rowOf = (name: string) =>
    withEve.leaderboard.rows.find((r) => r.name === name)!;

  const build = (
    scope: Parameters<typeof buildAskSnapshot>[0]["scope"],
    extra: Partial<AskOptions> = {},
  ) =>
    buildAskSnapshot({
      model: withEve,
      project: "/work/monoptah",
      scope,
      options: options({ omit: ["people"], rawCommits: true, ...extra }),
      now: NOW,
    }) as unknown as Snap;

  it("contains no e-mail address anywhere, for every scope", () => {
    const personId = rowOf("Bob").personId;
    for (const scope of [
      { kind: "team" },
      { kind: "person", personId },
      { kind: "me", personId: rowOf("Ana").personId },
    ] as const) {
      expect(JSON.stringify(build(scope))).not.toContain("@");
    }
    // Sanity: the same data does contain addresses when the chip is kept.
    const kept = buildAskSnapshot({
      model: withEve,
      project: "/work/monoptah",
      scope: { kind: "team" },
      options: options({ rawCommits: true }),
      now: NOW,
    });
    expect(JSON.stringify(kept)).toContain("@");
  });

  it("numbers people person-1, person-2 ... in rank order and keeps real names", () => {
    const s = build({ kind: "team" });
    const rows = withEve.leaderboard.rows;
    expect(s.people.map((p) => p.personId)).toEqual(
      rows.map((_, i) => `person-${i + 1}`),
    );
    const names = s.people.map((p) => p.name);
    for (const row of rows) {
      if (!row.name.includes("@")) expect(names).toContain(row.name);
    }
  });

  it("replaces a display name that is an e-mail address by the alias", () => {
    const s = build({ kind: "team" });
    const index = withEve.leaderboard.rows.findIndex(
      (r) => r.personId === "eve@example.com",
    );
    expect(index).toBeGreaterThanOrEqual(0);
    expect(s.people[index]).toMatchObject({
      personId: `person-${index + 1}`,
      name: `person-${index + 1}`,
    });
  });

  it("keeps scope, focus, prs, stats and commits consistent", () => {
    const bobRow = rowOf("Bob");
    const bobAlias = `person-${withEve.leaderboard.rows.indexOf(bobRow) + 1}`;
    const s = build({ kind: "person", personId: bobRow.personId });
    expect(s.scope).toEqual({ kind: "person", personId: bobAlias });
    expect(s.focus).toMatchObject({ personId: bobAlias, name: "Bob" });
    expect(s.people.find((p) => p.personId === bobAlias)?.name).toBe("Bob");
    expect(s.stats.rows.map((r) => r.personId)).toContain(bobAlias);
    expect(
      s.stats.rows.every((r) => /^person-\d+$/.test(String(r.personId))),
    ).toBe(true);
    expect(s.prs.every((p) => p.personId === bobAlias)).toBe(true);
    expect(s.commits.length).toBeGreaterThan(0);
    for (const c of s.commits) {
      expect(String(c.author)).not.toMatch(/[<>]/);
    }
  });

  it("gives a commit by an unnamed author the alias instead of an empty name", () => {
    const s = build({ kind: "team" });
    const index = withEve.leaderboard.rows.findIndex(
      (r) => r.personId === "eve@example.com",
    );
    const eve = s.commits.find((c) => c.person === `person-${index + 1}`);
    expect(eve).toBeDefined();
    expect(eve?.author).toBe(`person-${index + 1}`);
  });

  it("anonymises the prompt too", () => {
    const bobRow = rowOf("Bob");
    const req = buildAskRequest({
      model: withEve,
      project: "/work/monoptah",
      scope: { kind: "person", personId: bobRow.personId },
      question: "q",
      options: options({ omit: ["people"] }),
      now: NOW,
    });
    expect(req.prompt).not.toContain("@");
    expect(req.prompt).toContain("Bob");
    expect(req.scope).toEqual({ kind: "person", personId: bobRow.personId });
    expect(JSON.parse(req.snapshotJson).scope.personId).toMatch(/^person-\d+$/);
  });

  it("explains the aliases to the agent", () => {
    const s = build({ kind: "team" });
    expect(s.notes.join("\n")).toContain("person-1");
  });
});

describe("people chip counts", () => {
  it("counts people seen only in the stats range too", () => {
    const rows = model.leaderboard.rows.length;
    const people = askContextChips(model, "/work/monoptah", {
      kind: "team",
    }).find((c) => c.id === "people")!;
    const union = new Set([
      ...model.leaderboard.rows.map((r) => r.personId),
      ...model.stats.rows.map((r) => r.personId),
    ]).size;
    expect(union).toBeGreaterThanOrEqual(rows);
    expect(people.label.startsWith(`${union} `)).toBe(true);
  });

  it("says what removing it does", () => {
    const people = askContextChips(model, "/work/monoptah", {
      kind: "team",
    }).find((c) => c.id === "people")!;
    expect(people.hint).toContain("person-1");
    expect(people.hint).toContain("e-mail");
  });
});

describe("prompt wording follows the chips", () => {
  const prompt = (extra: Partial<AskOptions>) =>
    buildAskRequest({
      model,
      project: "/work/monoptah",
      scope: { kind: "team" },
      question: "q",
      options: options(extra),
      now: NOW,
    }).prompt;

  it("lists everything by default", () => {
    const text = prompt({});
    expect(text).toContain("scores and ranks");
    expect(text).toContain("per-dimension breakdowns");
    expect(text).toContain("identity mapping");
    expect(text).toContain("the scoring formula");
    expect(text).toContain("per-PR details");
    expect(text).toContain("AI-assisted shares");
    expect(text).not.toContain("raw commit list");
  });

  it("drops what was removed and adds what was switched on", () => {
    const text = prompt({
      perPr: false,
      rawCommits: true,
      omit: ["scores", "formula", "people", "ai"],
    });
    expect(text).toContain("scores and ranks");
    expect(text).toContain("the raw commit list");
    for (const gone of [
      "per-dimension breakdowns",
      "identity mapping",
      "scoring formula",
      "per-PR details",
      "AI-assisted",
    ]) {
      expect(text).not.toContain(gone);
    }
  });
});
