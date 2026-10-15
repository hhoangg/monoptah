// @vitest-environment happy-dom
import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { formatScore } from "../model/format";
import type { LeaderboardRow } from "../model/leaderboard";
import type { ContributorsModel } from "../model/pipeline";
import { seasonChoices } from "../model/range";
import { makeRaw } from "../model/testFixtures";
import { leaderboardModel, NOW } from "./leaderboardFixture";
import { LeaderboardTab, type LeaderboardTabProps } from "./LeaderboardTab";

let container: HTMLDivElement;
let root: Root;

beforeEach(() => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  vi.stubGlobal("requestAnimationFrame", (cb: (t: number) => void) => {
    cb(0);
    return 1;
  });
  vi.stubGlobal("cancelAnimationFrame", () => {});
  container = document.createElement("div");
  document.body.append(container);
  root = createRoot(container);
});
afterEach(async () => {
  await act(async () => root.unmount());
  container.remove();
  vi.unstubAllGlobals();
});

const render = (
  model: ContributorsModel,
  props: Partial<LeaderboardTabProps> = {},
) =>
  act(async () => {
    root.render(createElement(LeaderboardTab, { model, ...props }));
  });

const rows = () =>
  Array.from(container.querySelectorAll<HTMLElement>("tbody tr"));
const cells = (row: HTMLElement) =>
  Array.from(row.querySelectorAll("td")).map((c) => c.textContent ?? "");
const nameOf = (row: HTMLElement) =>
  row.querySelector("td:nth-child(2) .font-medium")?.textContent;
const byText = (selector: string, text: string, scope: ParentNode = document) =>
  Array.from(scope.querySelectorAll<HTMLElement>(selector)).find((el) =>
    (el.textContent ?? "").includes(text),
  );

/** The fixture board with one row's movement replaced. */
function withMovement(model: ContributorsModel, index: number, m: number) {
  const next: LeaderboardRow[] = model.leaderboard.rows.map((row, i) =>
    i === index ? { ...row, movement: m } : row,
  );
  return {
    ...model,
    leaderboard: { rows: next, podium: next.slice(0, 3) },
  };
}

describe("LeaderboardTab ranking", () => {
  it("lists the board in rank order with the table columns", async () => {
    const model = leaderboardModel();
    await render(model);
    const heads = Array.from(container.querySelectorAll("thead th")).map((h) =>
      (h.textContent ?? "").trim(),
    );
    expect(heads).toEqual([
      "Rank",
      "Contributor",
      "Score",
      "Commits",
      "PRs (commits)",
      "AI-assisted",
      "Streak",
      "Badges",
      "Actions",
    ]);
    expect(rows().map(nameOf)).toEqual(["Ana", "Bob", "Cat", "Dan"]);
    expect(model.leaderboard.rows.map((r) => r.name)).toEqual([
      "Ana",
      "Bob",
      "Cat",
      "Dan",
    ]);
  });

  it("shows score, commits, PRs with their commits, AI share and streak from the row", async () => {
    const model = leaderboardModel();
    await render(model);
    const [ana] = model.leaderboard.rows;
    const text = cells(rows()[0]);
    expect(text[2]).toBe(formatScore(ana.score));
    expect(text[3]).toBe(String(ana.mainCommits));
    expect(text[4]).toBe(`${ana.mergedPrs} (${ana.branchCommits})`);
    expect(ana.mergedPrs).toBe(1);
    expect(ana.branchCommits).toBe(3);
    expect(text[5]).toBe(`${Math.round(ana.aiAssistedPct)}%`);
    expect(text[6]).toBe(`${ana.streak}d`);
  });

  it("names the season and states the motivation note", async () => {
    await render(leaderboardModel());
    const text = container.textContent ?? "";
    expect(text).toContain("Ranking");
    expect(text).toContain("season: Oct 2026 · resets monthly");
    expect(text).toContain(
      "Rankings are for motivation, not performance review.",
    );
  });

  it("left-aligns the badges under their header in the table", async () => {
    await render(leaderboardModel());
    const list = rows()[0].querySelector("ul");
    expect(list?.className).toContain("justify-start");
    expect(list?.className).not.toContain("justify-center");
    const podium = container.querySelector("[data-podium] ul");
    expect(podium?.className).toContain("justify-center");
  });

  it("shows the badges of a row", async () => {
    const model = leaderboardModel();
    await render(model);
    const ana = model.leaderboard.rows[0];
    expect(ana.badges.map((b) => b.id)).toContain("builder");
    const shown = Array.from(rows()[0].querySelectorAll("[data-badge]")).map(
      (b) => b.textContent,
    );
    expect(shown).toEqual(ana.badges.map((b) => b.label));
  });

  it("paints each avatar with the shared color of the person", async () => {
    const base = leaderboardModel();
    // Colors that no hash would pick, so only the shared map can explain them.
    const colors = Object.fromEntries(
      base.leaderboard.rows.map((r, i) => [r.personId, `#10203${i}`]),
    );
    const model = { ...base, colors };
    await render(model);
    const probe = document.createElement("span");
    for (const [i, row] of model.leaderboard.rows.entries()) {
      probe.style.backgroundColor = colors[row.personId];
      const avatar = rows()[i].querySelector<HTMLElement>(
        "td span.rounded-full[aria-hidden]",
      );
      expect(avatar?.style.backgroundColor).toBe(probe.style.backgroundColor);
    }
    const podium = Array.from(
      container.querySelectorAll<HTMLElement>(
        "[data-podium] span.rounded-full[aria-hidden]",
      ),
    );
    expect(podium).toHaveLength(3);
    for (const [i, avatar] of podium.entries()) {
      probe.style.backgroundColor = colors[model.leaderboard.rows[i].personId];
      expect(avatar.style.backgroundColor).toBe(probe.style.backgroundColor);
    }
  });

  it("gives the four people on the board four different colors", async () => {
    const model = leaderboardModel();
    await render(model);
    const colors = rows().map(
      (r) =>
        r.querySelector<HTMLElement>("td span.rounded-full[aria-hidden]")!.style
          .backgroundColor,
    );
    expect(new Set(colors).size).toBe(4);
  });
});

describe("LeaderboardTab movement", () => {
  it("shows up, down, unchanged and new with the previous-season explanation", async () => {
    const model = leaderboardModel();
    expect(model.leaderboard.rows.map((r) => r.movement)).toEqual([
      1,
      -1,
      0,
      null,
    ]);
    await render(model);
    const glyphs = rows().map((r) => {
      const el = r.querySelector<HTMLElement>("[title]");
      return [
        el?.querySelector("[aria-hidden]")?.textContent,
        el?.querySelector(".sr-only")?.textContent,
        el?.className,
      ] as const;
    });
    expect(glyphs[0][0]).toBe("▲1");
    expect(glyphs[0][1]).toBe("Up 1 place vs previous season");
    expect(glyphs[0][2]).toContain("text-diff-add-fg");
    expect(glyphs[1][0]).toBe("▼1");
    expect(glyphs[1][1]).toBe("Down 1 place vs previous season");
    expect(glyphs[1][2]).toContain("text-diff-del-fg");
    expect(glyphs[2][0]).toBe("·");
    expect(glyphs[2][1]).toBe("Same rank as the previous season");
    expect(glyphs[3][0]).toBe("·");
    expect(glyphs[3][1]).toBe("Not ranked in the previous season");
  });

  it("pluralizes a bigger move", async () => {
    await render(withMovement(leaderboardModel(), 1, 2));
    const el = rows()[1].querySelector<HTMLElement>("[title]");
    expect(el?.querySelector("[aria-hidden]")?.textContent).toBe("▲2");
    expect(el?.getAttribute("title")).toBe("Up 2 places vs previous season");
    expect(el?.querySelector(".sr-only")?.textContent).toBe(
      "Up 2 places vs previous season",
    );
    expect(el?.hasAttribute("aria-label")).toBe(false);
  });
});

describe("LeaderboardTab you chip", () => {
  it("marks only the git user's row and tints it", async () => {
    await render(leaderboardModel());
    const marked = rows().filter((r) => (r.textContent ?? "").includes("you"));
    expect(marked).toHaveLength(1);
    expect(marked[0].textContent).toContain("Ana");
    expect(marked[0].className).toContain("bg-accent/8");
    expect(rows()[1].className).not.toContain("bg-accent/8");
  });

  it("shows no chip when the git user is not on the board", async () => {
    await render(leaderboardModel({ user: undefined }));
    expect(rows().some((r) => (r.textContent ?? "").includes("you"))).toBe(
      false,
    );
  });
});

describe("LeaderboardTab podium", () => {
  it("shows the top three in rank order with the first highlighted", async () => {
    const model = leaderboardModel();
    await render(model);
    const items = Array.from(
      container.querySelectorAll<HTMLElement>("[data-podium] > li"),
    );
    expect(items.map((i) => i.dataset.rank)).toEqual(["1", "2", "3"]);
    expect(items[0].className).toContain("border-accent/60");
    expect(items[1].className).not.toContain("border-accent/60");
    expect(items[0].textContent).toContain("Ana");
    expect(items[0].textContent).toContain("you");
    expect(items[1].textContent).toContain("Bob");
    expect(items[2].textContent).toContain("Cat");
  });

  it("places 2nd left, 1st in the middle and 3rd right on a wide page", async () => {
    await render(leaderboardModel());
    const items = Array.from(
      container.querySelectorAll<HTMLElement>("[data-podium] > li"),
    );
    expect(items[0].className).toContain("col-start-2");
    expect(items[1].className).toContain("col-start-1");
    expect(items[2].className).toContain("col-start-3");
  });

  it("shows score, PRs with commits, streak and badges", async () => {
    const model = leaderboardModel();
    await render(model);
    const [ana, bob] = model.leaderboard.rows;
    const first = container.querySelector<HTMLElement>("[data-podium] > li");
    expect(first?.textContent).toContain(formatScore(ana.score));
    expect(first?.textContent).toContain(
      `1 PR (3 commits) · ${ana.streak}d streak`,
    );
    expect(first?.textContent).toContain("Builder");
    const second = container.querySelectorAll("[data-podium] > li")[1];
    expect(second.textContent).toContain(`0 PRs (0 commits) · ${bob.streak}d`);
    expect(second.textContent).toContain("Reviewer");
  });

  it("labels the medals with text and uses a trophy glyph", async () => {
    await render(leaderboardModel());
    const items = Array.from(container.querySelectorAll("[data-podium] > li"));
    expect(
      items.map((i) => /(1st|2nd|3rd)/.exec(i.textContent ?? "")?.[0]),
    ).toEqual(["1st", "2nd", "3rd"]);
    expect(items[0].querySelector("svg")).not.toBeNull();
  });

  it("shows fewer places when fewer people are ranked", async () => {
    const raw = leaderboardModel().raw;
    const model = leaderboardModel({
      raw: {
        ...raw,
        commits: raw.commits.filter((c) => c.hash.startsWith("ana-o")),
        prs: [],
        reviews: [],
      },
    });
    await render(model);
    expect(container.querySelectorAll("[data-podium] > li")).toHaveLength(1);
    expect(rows()).toHaveLength(1);
  });
});

describe("LeaderboardTab ask", () => {
  it("asks about the person of the row", async () => {
    const onAsk = vi.fn();
    await render(leaderboardModel(), { onAskAgent: onAsk });
    const bob = leaderboardModel().leaderboard.rows[1];
    const button = container.querySelector(
      'button[aria-label="Ask Agent about Bob"]',
    ) as HTMLButtonElement;
    await act(async () => button.click());
    expect(onAsk).toHaveBeenCalledTimes(1);
    expect(onAsk).toHaveBeenCalledWith({
      kind: "person",
      personId: bob.personId,
    });
  });

  it("has no Ask buttons without a handler", async () => {
    await render(leaderboardModel());
    expect(
      container.querySelector('button[aria-label^="Ask Agent"]'),
    ).toBeNull();
  });
});

describe("LeaderboardTab season", () => {
  const seasons = seasonChoices(NOW).map(({ id, label }) => ({ id, label }));

  it("shows the selected season and reports a new pick", async () => {
    const onSeason = vi.fn();
    await render(leaderboardModel(), {
      seasons,
      season: "2026-10",
      onSeasonChange: onSeason,
    });
    const trigger = container.querySelector(
      'button[aria-label="Season: Oct 2026"]',
    ) as HTMLButtonElement;
    expect(trigger).not.toBeNull();
    await act(async () => trigger.click());
    const options = Array.from(
      document.querySelectorAll<HTMLElement>('[role="option"]'),
    ).map((o) => o.textContent);
    expect(options).toEqual([
      "Oct 2026",
      "Sep 2026",
      "Aug 2026",
      "Jul 2026",
      "Jun 2026",
      "May 2026",
    ]);
    await act(async () =>
      document.querySelectorAll<HTMLElement>('[role="option"]')[1].click(),
    );
    expect(onSeason).toHaveBeenCalledWith("2026-09");
  });

  it("has no selector without a season list", async () => {
    await render(leaderboardModel());
    expect(container.querySelector('button[aria-label^="Season"]')).toBeNull();
  });

  it("titles the board with the scored season", async () => {
    await render(leaderboardModel({ seasonId: "2026-09" }), {
      seasons,
      season: "2026-09",
      onSeasonChange: vi.fn(),
    });
    expect(container.textContent).toContain(
      "season: Sep 2026 · resets monthly",
    );
    expect(rows().map(nameOf)).toEqual(["Bob", "Ana", "Cat"]);
  });
});

describe("LeaderboardTab empty", () => {
  it("explains an empty season instead of showing an empty table", async () => {
    await render(leaderboardModel({ raw: makeRaw() }));
    expect(container.textContent).toContain("No scored activity in Oct 2026");
    expect(container.querySelector("table")).toBeNull();
    expect(container.querySelector("[data-podium]")).toBeNull();
    expect(container.textContent).toContain("How is the score calculated?");
  });
});

describe("LeaderboardTab summary line", () => {
  it("states the formula without a Review caveat when GitHub data exists", async () => {
    await render(leaderboardModel());
    const text = container.textContent ?? "";
    expect(text).toContain(
      "Score = Delivery × Quality + Care + Review + Consistency + Breadth.",
    );
    expect(text).not.toContain("Review is not counted");
  });

  it("says Review is not counted when GitHub data is unavailable", async () => {
    const model = leaderboardModel({
      raw: { ...leaderboardModel().raw, ghAvailable: false },
    });
    await render(model);
    expect(container.textContent).toContain(
      "Breadth. Review is not counted (GitHub data unavailable).",
    );
  });
});

describe("LeaderboardTab formula sidebar", () => {
  const link = () =>
    byText(
      "button",
      "How is the score calculated?",
      container,
    ) as HTMLButtonElement;

  it("opens from the link and closes with Escape, returning focus to the link", async () => {
    await render(leaderboardModel());
    expect(document.querySelector('[role="complementary"]')).toBeNull();
    await act(async () => link().click());
    const panel = document.querySelector<HTMLElement>('[role="complementary"]');
    expect(panel?.getAttribute("aria-label")).toBe(
      "How the score is calculated",
    );
    expect(link().getAttribute("aria-expanded")).toBe("true");
    expect(link().getAttribute("aria-controls")).toBe(panel?.id);
    expect(document.activeElement).toBe(panel);

    await act(async () => {
      document.dispatchEvent(
        new KeyboardEvent("keydown", { key: "Escape", bubbles: true }),
      );
    });
    expect(document.querySelector('[role="complementary"]')).toBeNull();
    expect(document.activeElement).toBe(link());
  });

  it("closes with the X button", async () => {
    await render(leaderboardModel());
    await act(async () => link().click());
    await act(async () =>
      (
        document.querySelector(
          '[role="complementary"] button[aria-label="Close"]',
        ) as HTMLButtonElement
      ).click(),
    );
    expect(document.querySelector('[role="complementary"]')).toBeNull();
    expect(document.activeElement).toBe(link());
  });

  it("slides into the page body host when one is given", async () => {
    const host = document.createElement("div");
    document.body.append(host);
    await render(leaderboardModel(), { sidebarHost: host });
    await act(async () => link().click());
    expect(host.querySelector('[role="complementary"]')).not.toBeNull();
    expect(container.querySelector('[role="complementary"]')).toBeNull();
    host.remove();
  });

  it("shows the viewer's score from the leaderboard row", async () => {
    const model = leaderboardModel();
    await render(model);
    await act(async () => link().click());
    const me = model.leaderboard.rows[0];
    expect(
      document.querySelector('[role="complementary"]')?.textContent,
    ).toContain(`Your score this season: ${formatScore(me.score)}`);
  });
});
