// @vitest-environment happy-dom
import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { buildContributorsModel } from "../model/pipeline";
import type { ContributorsModel } from "../model/pipeline";
import {
  ANA,
  BOB,
  file,
  ident,
  makeCommit,
  makePr,
  makeRaw,
  oct,
  withConfig,
} from "../model/testFixtures";
import { currentPeriod, syncWindow } from "../model/period";
import { CommitChart, StatsTab } from "./StatsTab";

const NOW = new Date(2026, 9, 20, 10);
const MONTH = currentPeriod("month", NOW);
const sinceOf = (period = MONTH) => syncWindow(period, NOW).from.toISOString();

function model(
  overrides: Partial<Parameters<typeof buildContributorsModel>[0]> = {},
) {
  const raw = makeRaw({
    since: sinceOf(overrides.period),
    commits: [
      makeCommit("p1", ANA, new Date(2026, 9, 1, 9).toISOString(), {
        files: [file("src/a.ts", 12000, 1000)],
      }),
      makeCommit("early", ANA, new Date(2026, 8, 5, 9).toISOString(), {
        files: [file("src/a.ts", 4)],
      }),
      makeCommit("a1", ANA, oct(14), { files: [file("src/a.ts", 20, 5)] }),
      makeCommit("a2", ident("Ana", "ana@laptop.local"), oct(15), {
        files: [file("src/b.ts", 10)],
      }),
      makeCommit("b1", BOB, oct(19), { files: [file("src/c.ts", 7)] }),
      makeCommit("b2", BOB, oct(19, 14), { files: [file("src/c.ts", 3)] }),
    ],
    prs: [makePr("1", ANA, oct(16))],
  });
  return buildContributorsModel({
    raw,
    config: withConfig({
      people: [
        {
          id: "ana",
          name: "Ana Sol",
          identities: ["ana@example.com", "ana@laptop.local"],
        },
      ],
    }),
    mailmap: [],
    merge: true,
    period: MONTH,
    now: NOW,
    ...overrides,
  });
}

let container: HTMLDivElement;
let root: Root;
const render = (m: ContributorsModel, onAskAgent?: (s: unknown) => void) =>
  act(async () => {
    root.render(createElement(StatsTab, { model: m, onAskAgent }));
  });

beforeEach(() => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  container = document.createElement("div");
  document.body.append(container);
  root = createRoot(container);
});
afterEach(async () => {
  await act(async () => root.unmount());
  container.remove();
  vi.unstubAllGlobals();
});

describe("StatsTab", () => {
  it("shows the four KPI cards from the model", async () => {
    const m = model();
    await render(m);
    const text = container.textContent ?? "";
    expect(text).toContain("Commits");
    expect(text).toContain(String(m.stats.kpis.commits.value));
    expect(text).toContain("Contributors");
    expect(text).toContain("from 3 identities");
    expect(text).toContain("Lines changed");
    expect(text).toContain("+12k");
    expect(text).toContain("PRs merged");
  });

  it("colors line additions and deletions with the diff tokens", async () => {
    await render(model());
    expect(
      Array.from(container.querySelectorAll(".text-diff-add-fg")).some((e) =>
        /^\+/.test(e.textContent ?? ""),
      ),
    ).toBe(true);
    expect(
      Array.from(container.querySelectorAll(".text-diff-del-fg")).some((e) =>
        /^-/.test(e.textContent ?? ""),
      ),
    ).toBe(true);
  });

  it("renders one row per contributor with commits, share and the identity chip", async () => {
    const m = model();
    await render(m);
    const rows = container.querySelectorAll("tbody tr");
    expect(rows).toHaveLength(m.stats.rows.length);
    const first = rows[0].textContent ?? "";
    expect(first).toContain("Ana Sol");
    expect(first).toContain("2 identities");
    expect(first).toContain(`${m.stats.rows[0].commits}`);
  });

  it("expands a row to its identities in a monospace list on click", async () => {
    await render(model());
    expect(container.querySelector("[data-identities]")).toBeNull();
    const row = container.querySelector("tbody tr") as HTMLElement;
    await act(async () => row.click());
    const sub = container.querySelector("[data-identities]");
    expect(sub?.textContent).toContain("ana@example.com");
    expect(sub?.textContent).toContain("ana@laptop.local");
    expect(sub?.querySelector("ul")?.className).toContain("font-mono");
    await act(async () => row.click());
    expect(container.querySelector("[data-identities]")).toBeNull();
  });

  it("calls onAskAgent with the person scope without expanding the row", async () => {
    const onAsk = vi.fn();
    await render(model(), onAsk);
    const ask = container.querySelector(
      'button[aria-label="Ask Agent about Ana Sol"]',
    ) as HTMLButtonElement;
    await act(async () => ask.click());
    expect(onAsk).toHaveBeenCalledWith({ kind: "person", personId: "ana" });
    expect(container.querySelector("[data-identities]")).toBeNull();
  });

  it("has no Ask buttons without a handler", async () => {
    await render(model());
    expect(
      container.querySelector('button[aria-label^="Ask Agent"]'),
    ).toBeNull();
  });

  it("says so when nobody committed in the period", async () => {
    await render(model({ raw: makeRaw() }));
    expect(container.textContent).toContain("No contributors in this period.");
  });

  it("shows the change against the previous period with an arrow, a sign and a color", async () => {
    const m = model();
    const delta = m.stats.kpis.commits.delta;
    await render(m);
    if (delta === null)
      throw new Error("fixture should cover the previous period");
    expect(container.textContent).toContain("vs Sep 2026");
    expect(container.textContent).toMatch(/[▲▼] [+-]\d+%/);
  });

  it("names the previous period of each kind", async () => {
    await render(model({ period: currentPeriod("7d", NOW) }));
    expect(container.textContent).toContain("vs prev 7d");
    await render(model({ period: currentPeriod("quarter", NOW) }));
    expect(container.textContent).toContain("vs Q3 2026");
    await render(model({ period: currentPeriod("year", NOW) }));
    expect(container.textContent).toContain("vs 2025");
  });

  it("hides every comparison for All time, which has no previous period", async () => {
    const all = currentPeriod("all", NOW, new Date(2026, 8, 1));
    const m = model({ period: all });
    expect(m.previousPeriod).toBeNull();
    await render(m);
    const text = container.textContent ?? "";
    expect(text).toContain("Commits");
    expect(text).not.toContain("vs ");
    expect(text).not.toMatch(/[▲▼]/);
    expect(text).not.toContain(" new");
  });
});

describe("CommitChart", () => {
  it("renders one bar group per day and stacks a segment per contributor", async () => {
    const m = model();
    await act(async () => {
      root.render(createElement(CommitChart, { stats: m.stats }));
    });
    const svg = container.querySelector("svg") as SVGElement;
    expect(svg.getAttribute("role")).toBe("img");
    expect(svg.getAttribute("viewBox")).toBe("0 0 1000 180");
    const days = container.querySelectorAll("g[data-bucket]");
    expect(days).toHaveLength(m.stats.buckets.length);
    const segments = container.querySelectorAll("rect[data-person]");
    const expected = m.stats.buckets.reduce(
      (n, w) => n + Object.values(w.byPerson).filter((c) => c > 0).length,
      0,
    );
    expect(segments).toHaveLength(expected);
  });

  it("summarizes the chart for screen readers", async () => {
    const m = model();
    await act(async () => {
      root.render(createElement(CommitChart, { stats: m.stats }));
    });
    const label =
      container.querySelector("svg")?.getAttribute("aria-label") ?? "";
    expect(label).toContain("Commit activity per day");
    expect(label).toContain(`${m.stats.buckets.length} days`);
    expect(label).toContain("busiest day");
  });

  it("scales the tallest week to the full height and the others proportionally", async () => {
    const m = model();
    await act(async () => {
      root.render(createElement(CommitChart, { stats: m.stats }));
    });
    const heights = Array.from(
      container.querySelectorAll("rect[data-person]"),
    ).map((r) => Number(r.getAttribute("height")));
    expect(Math.max(...heights)).toBeLessThanOrEqual(164);
    expect(Math.min(...heights)).toBeGreaterThan(0);
  });

  it("draws no segments for an empty period", async () => {
    const m = model({ raw: makeRaw() });
    await act(async () => {
      root.render(createElement(CommitChart, { stats: m.stats }));
    });
    expect(container.querySelectorAll("rect")).toHaveLength(0);
    expect(
      container.querySelector("svg")?.getAttribute("aria-label"),
    ).toContain("no commits");
  });
});

describe("StatsTab details", () => {
  it("draws a baseline under the bars", async () => {
    const m = model();
    await act(async () => {
      root.render(createElement(CommitChart, { stats: m.stats }));
    });
    expect(container.querySelector("line[data-baseline]")).not.toBeNull();
  });

  it("never lets bars overlap when there are very many weeks", async () => {
    const m = model();
    const weeks = Array.from({ length: 1200 }, (_, i) => ({
      start: `w${i}`,
      label: `w${i}`,
      short: `w${i}`,
      byPerson: { ana: 1 },
    }));
    await act(async () => {
      root.render(
        createElement(CommitChart, {
          stats: { ...m.stats, buckets: weeks },
        }),
      );
    });
    const first = container.querySelector("rect") as SVGRectElement;
    expect(Number(first.getAttribute("width"))).toBeLessThan(1000 / 1200);
  });

  it("points the expand button at the identities row it reveals", async () => {
    await render(model());
    const toggle = container.querySelector(
      "tbody button[aria-expanded]",
    ) as HTMLElement;
    expect(toggle.getAttribute("aria-controls")).toBeNull();
    await act(async () => toggle.click());
    const id = toggle.getAttribute("aria-controls");
    expect(id).toBeTruthy();
    expect(container.querySelector(`[id="${id}"]`)).not.toBeNull();
  });
});

describe("CommitChart readability", () => {
  const draw = async (m: ContributorsModel) => {
    await act(async () => {
      root.render(createElement(CommitChart, { stats: m.stats }));
    });
  };

  it("labels the x axis with the first and last bucket and no more than about eight", async () => {
    const m = model({ period: MONTH });
    await draw(m);
    const labels = Array.from(container.querySelectorAll("[data-axis]")).map(
      (e) => e.textContent,
    );
    const buckets = m.stats.buckets;
    expect(labels[0]).toBe(buckets[0].short);
    expect(labels[labels.length - 1]).toBe(buckets[buckets.length - 1].short);
    expect(labels.length).toBeLessThanOrEqual(10);
    expect(labels.length).toBeGreaterThanOrEqual(2);
  });

  it("labels days as 'Mon D'", async () => {
    await draw(model({ period: currentPeriod("7d", NOW) }));
    expect(container.querySelector("[data-axis]")?.textContent).toMatch(
      /^[A-Z][a-z]{2} \d{1,2}$/,
    );
  });

  it("shows the busiest bucket as the y max", async () => {
    const m = model();
    await draw(m);
    const busiest = Math.max(
      ...m.stats.buckets.map((b) =>
        Object.values(b.byPerson).reduce((a, c) => a + c, 0),
      ),
    );
    expect(container.querySelector("[data-ymax]")?.textContent).toBe(
      String(busiest),
    );
  });

  it("puts the bucket label and every person's count in the bar tooltip", async () => {
    const m = model();
    await draw(m);
    const bucket = m.stats.buckets.find(
      (b) => Object.keys(b.byPerson).length >= 1 && b.start >= "2026-10-01",
    );
    if (!bucket) throw new Error("fixture needs a busy bucket");
    const title =
      container.querySelector(`g[data-bucket="${bucket.start}"] > title`)
        ?.textContent ?? "";
    expect(title).toContain(bucket.label);
    for (const [id, count] of Object.entries(bucket.byPerson)) {
      const name = m.stats.legend.find((l) => l.personId === id)?.name;
      expect(title).toContain(`${name}: ${count}`);
    }
  });

  it("names the unit of the period in the summary and the card heading", async () => {
    const units = [
      ["month", "day"],
      ["7d", "day"],
      ["quarter", "week"],
      ["year", "month"],
    ] as const;
    for (const [kind, unit] of units) {
      const m = model({ period: currentPeriod(kind, NOW) });
      await draw(m);
      const label = container.querySelector("svg")?.getAttribute("aria-label");
      expect(label).toContain(`Commit activity per ${unit}`);
      expect(label).toContain(`${unit}s`);
      await render(m);
      expect(container.textContent).toContain(
        `per ${unit}, stacked by contributor`,
      );
    }
  });

  it("paints the table avatar with the row color from the shared map", async () => {
    const m = model();
    const recolored = {
      ...m,
      stats: {
        ...m.stats,
        rows: m.stats.rows.map((r) => ({ ...r, color: "#102030" })),
      },
    };
    await render(recolored);
    const probe = document.createElement("span");
    probe.style.backgroundColor = "#102030";
    const avatar = container.querySelector<HTMLElement>(
      "tbody tr [aria-hidden][style]",
    );
    expect(avatar?.style.backgroundColor).toBe(probe.style.backgroundColor);
  });

  it("colors a person the same in the table, legend and chart", async () => {
    const m = model();
    await render(m);
    const row = m.stats.rows[0];
    const hex = m.colors[row.personId];
    expect(row.color).toBe(hex);
    const styled = Array.from(container.querySelectorAll("[style]")).filter(
      (e) => (e.getAttribute("style") ?? "").toLowerCase().includes(hex),
    );
    // avatar, legend swatch and share bar
    expect(styled.length).toBeGreaterThanOrEqual(3);
    await act(async () => {
      root.render(createElement(CommitChart, { stats: m.stats }));
    });
    const fills = container.querySelectorAll(
      `rect[data-person="${row.personId}"]`,
    );
    expect(fills.length).toBeGreaterThan(0);
    for (const rect of fills) expect(rect.getAttribute("fill")).toBe(hex);
  });
});

describe("StatsTab legend", () => {
  const manyAuthors = (count: number) => {
    const people = Array.from({ length: count }, (_, i) => ({
      who: ident(`Person ${i + 1}`, `p${i + 1}@example.com`),
      commits: count - i,
    }));
    const raw = makeRaw({
      since: sinceOf(),
      commits: people.flatMap(({ who, commits }, i) =>
        Array.from({ length: commits }, (_, n) =>
          makeCommit(`c${i}-${n}`, who, oct(10 + (n % 8)), {
            files: [file(`src/f${i}.ts`, 3)],
          }),
        ),
      ),
    });
    return buildContributorsModel({
      raw,
      config: withConfig({}),
      mailmap: [],
      merge: true,
      period: MONTH,
      now: NOW,
    });
  };
  const legendItems = () =>
    Array.from(container.querySelectorAll("section ul")).find((ul) =>
      ul.textContent?.includes("Person 1"),
    );

  it("lists everyone when there are eight or fewer", async () => {
    await render(manyAuthors(8));
    const items = legendItems()!.querySelectorAll("li");
    expect(items).toHaveLength(8);
    expect(legendItems()!.textContent).not.toContain("more");
  });

  it("shows the top eight by commits, then a +N more chip that names the rest", async () => {
    await render(manyAuthors(12));
    const ul = legendItems()!;
    const items = Array.from(ul.querySelectorAll("li"));
    expect(items).toHaveLength(9);
    expect(items.slice(0, 8).map((li) => li.textContent)).toEqual(
      Array.from({ length: 8 }, (_, i) => `Person ${i + 1}`),
    );
    const more = items[8];
    expect(more.textContent).toBe("+4 more");
    expect(more.getAttribute("title")).toBe(
      "Person 9, Person 10, Person 11, Person 12",
    );
  });
});
