// @vitest-environment happy-dom
import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { DEFAULT_CONFIG } from "../model/config";
import { formatScore } from "../model/format";
import { currentPeriod } from "../model/period";
import type { ContributorsModel } from "../model/pipeline";
import { SCORING } from "../model/scoring";
import { priorOf, withConfig } from "../model/testFixtures";
import { FormulaSidebar, type FormulaSidebarProps } from "./FormulaSidebar";
import { NOW, leaderboardModel } from "./leaderboardFixture";

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
  props: Partial<FormulaSidebarProps> = {},
) =>
  act(async () => {
    root.render(
      createElement(FormulaSidebar, { model, onClose: vi.fn(), ...props }),
    );
  });

const panel = () =>
  container.querySelector('[role="complementary"]') as HTMLElement;
const text = () => panel().textContent ?? "";
const button = (label: string) =>
  Array.from(panel().querySelectorAll<HTMLButtonElement>("button")).find((b) =>
    (b.textContent ?? "").includes(label),
  );
const input = (label: string) =>
  panel().querySelector<HTMLInputElement>(`input[aria-label="${label}"]`)!;
const part = (key: string) =>
  panel().querySelector<HTMLElement>(`[data-part="${key}"]`)!;

function type(el: HTMLInputElement, value: string) {
  const setter = Object.getOwnPropertyDescriptor(
    HTMLInputElement.prototype,
    "value",
  )!.set!;
  setter.call(el, value);
  el.dispatchEvent(new Event("input", { bubbles: true }));
}

describe("FormulaSidebar formulas", () => {
  it("keeps each formula line whole and scrolls it sideways instead of wrapping", async () => {
    await render(leaderboardModel());
    const blocks = panel().querySelectorAll("pre");
    expect(blocks.length).toBeGreaterThan(0);
    for (const pre of blocks) {
      expect(pre.className).toContain("whitespace-pre");
      expect(pre.className).not.toContain("whitespace-pre-wrap");
      expect(pre.className).toContain("overflow-x-auto");
    }
  });
});

describe("FormulaSidebar breakdown", () => {
  it("is a labelled complementary region titled like the link that opens it", async () => {
    await render(leaderboardModel());
    expect(panel().getAttribute("aria-label")).toBe(
      "How the score is calculated",
    );
    expect(panel().querySelector("h3")?.textContent).toBe(
      "How the score is calculated",
    );
  });

  it("shows the viewer's score and the five weighted parts", async () => {
    const model = leaderboardModel();
    const me = model.leaderboard.rows.find((r) => r.isMe)!;
    await render(model);
    expect(text()).toContain(
      `Your score in Oct 2026: ${formatScore(me.score)}`,
    );
    for (const key of [
      "delivery",
      "care",
      "review",
      "consistency",
      "breadth",
    ] as const) {
      expect(part(key).textContent).toContain(formatScore(me.parts[key]));
    }
    expect(me.parts.delivery).toBeGreaterThan(0);
    expect(me.parts.review).toBe(0);
  });

  it("scales the bars to the largest part", async () => {
    const model = leaderboardModel();
    const me = model.leaderboard.rows.find((r) => r.isMe)!;
    await render(model);
    const width = (key: string) =>
      Number.parseFloat(
        part(key).querySelector<HTMLElement>("[style]")!.style.width,
      );
    const largest = Math.max(...Object.values(me.parts));
    expect(width("delivery")).toBeCloseTo(
      (me.parts.delivery / largest) * 100,
      3,
    );
    expect(width("consistency")).toBeCloseTo(
      (me.parts.consistency / largest) * 100,
      3,
    );
    expect(
      Math.max(
        ...["delivery", "care", "review", "consistency", "breadth"].map(width),
      ),
    ).toBeCloseTo(100, 3);
  });

  it("shows quality as a separate chip with the survival share", async () => {
    const model = leaderboardModel();
    const me = model.leaderboard.rows.find((r) => r.isMe)!;
    await render(model);
    expect(me.quality).toBeCloseTo(1.14, 2);
    expect(part("quality").textContent).toContain(`×${me.quality.toFixed(2)}`);
    expect(part("quality").textContent).toContain("survival 85% at 30 days");
    // Quality is inside the Delivery part already, so it has no bar of its own.
    expect(part("quality").querySelector("[style]")).toBeNull();
  });

  it("says so when there is no survival sample", async () => {
    const base = leaderboardModel();
    await render(leaderboardModel({ raw: { ...base.raw, survival: [] } }));
    expect(part("quality").textContent).toContain("×1.00");
    expect(part("quality").textContent).toContain("no 30-day data yet");
  });

  it("reads the weighted parts and marks a custom weight", async () => {
    const model = leaderboardModel({
      config: withConfig({
        weights: { ...DEFAULT_CONFIG.weights, consistency: 2 },
      }),
    });
    const me = model.leaderboard.rows.find((r) => r.isMe)!;
    const plain = leaderboardModel().leaderboard.rows.find((r) => r.isMe)!;
    expect(me.parts.consistency).toBeCloseTo(plain.parts.consistency * 2, 5);
    await render(model);
    expect(part("consistency").textContent).toContain(
      formatScore(me.parts.consistency),
    );
    expect(part("consistency").textContent).toContain("×2");
    expect(text()).toContain("Custom weights are active: Consistency ×2.");
  });

  it("explains when the viewer has no activity in the period", async () => {
    await render(leaderboardModel({ user: undefined }));
    expect(text()).toContain("Your score in Oct 2026");
    expect(text()).toContain("No activity for you in Oct 2026 yet");
    expect(panel().querySelector("[data-part]")).toBeNull();
  });

  it("names an older month by its label", async () => {
    await render(
      leaderboardModel({ period: priorOf(currentPeriod("month", NOW)) }),
    );
    expect(text()).toContain("Your score in Sep 2026:");
  });
});

describe("FormulaSidebar period wording", () => {
  afterEach(() => vi.useRealTimers());

  it("names the model's period, whatever the clock says", async () => {
    vi.useFakeTimers({ toFake: ["Date"] });
    vi.setSystemTime(new Date(2027, 0, 3));
    await render(leaderboardModel());
    expect(text()).toContain("Your score in Oct 2026:");
  });

  it("says the last 7 days for the 7d period", async () => {
    await render(leaderboardModel({ period: currentPeriod("7d", NOW) }));
    expect(text()).toContain("Your score in the last 7 days");
  });

  it("names All time by its label", async () => {
    const all = currentPeriod("all", NOW, new Date(2026, 8, 1));
    await render(leaderboardModel({ period: all }));
    expect(text()).toContain("Your score over All time:");
    await render(leaderboardModel({ period: all, user: undefined }));
    expect(text()).toContain("No activity for you over All time yet");
  });

  it("names a quarter and a year by their labels", async () => {
    await render(leaderboardModel({ period: currentPeriod("quarter", NOW) }));
    expect(text()).toContain("Your score in Q4 2026");
    await render(leaderboardModel({ period: currentPeriod("year", NOW) }));
    expect(text()).toContain("Your score in 2026");
  });

  it("uses the same wording for a missing viewer", async () => {
    await render(
      leaderboardModel({
        user: undefined,
        period: currentPeriod("quarter", NOW),
      }),
    );
    expect(text()).toContain("No activity for you in Q4 2026 yet");
  });
});

describe("FormulaSidebar copy", () => {
  it("says that Consistency grows with the period and Breadth is capped", async () => {
    await render(leaderboardModel());
    expect(text()).toContain(
      "Consistency grows with the length of the period; Breadth is capped, so in long periods it weighs less.",
    );
  });

  it("renders the numbers from the scoring constants", async () => {
    await render(leaderboardModel());
    const t = text();
    expect(t).toContain(
      `iteration = ${SCORING.ITER_K} × log2(1 + commits in PR)  (max ${SCORING.ITER_CAP})`,
    );
    expect(t).toContain(
      `PR points = ${SCORING.PR_BASE} + ${SCORING.SIZE_K} × size + iteration     (max ${SCORING.PR_CAP})`,
    );
    expect(t).toContain(
      `Quality: ×${SCORING.QUALITY_MIN} to ×${SCORING.QUALITY_MAX}`,
    );
    expect(t).toContain(
      `quality = ${SCORING.QUALITY_BASE} + ${SCORING.QUALITY_SLOPE} × survival`,
    );
    expect(t).toContain("+20% if it adds or updates tests");
    expect(t).toContain("+10% if it updates docs");
    expect(t).toContain("+15% if it deletes more than it adds");
    expect(t).toContain("+15% if it fixes a bug");
    expect(t).toContain(
      `${SCORING.REVIEW_PR} points per PR you review for someone else, +${SCORING.REVIEW_COMMENT} per meaningful comment (max ${SCORING.REVIEW_COMMENT_CAP} per PR)`,
    );
    expect(t).toContain(
      `${SCORING.ACTIVE_DAY} points per active day, plus ${SCORING.STREAK_K} per day of streak (max ${SCORING.STREAK_CAP})`,
    );
    expect(t).toContain(
      `${SCORING.MODULE_PTS} points per module where you changed at least ${SCORING.MODULE_MIN_EFF} lines (max ${SCORING.MODULE_CAP} modules)`,
    );
  });

  it("has the formula, every dimension section, the AI note and the callout", async () => {
    await render(leaderboardModel());
    const t = text();
    expect(t).toContain(
      "Score = Delivery × Quality + Care\n      + Review + Consistency + Breadth",
    );
    for (const heading of [
      "The formula",
      "Delivery: merged pull requests",
      "Care: bonus on a PR",
      "Review",
      "Consistency",
      "Breadth",
      "AI agents",
      "What is ignored",
    ]) {
      expect(
        Array.from(panel().querySelectorAll("h4")).map((h) => h.textContent),
      ).toContain(heading);
    }
    expect(t).toContain("There is no AI bonus or penalty");
    expect(t).toContain(
      "This board is for motivation, not performance review.",
    );
  });

  it("lists the active ignore globs of the project", async () => {
    await render(
      leaderboardModel({
        config: withConfig({ ignore: ["vendor/**", "*.snap"] }),
      }),
    );
    const globs = Array.from(
      panel().querySelectorAll("[data-ignore-globs] code"),
    ).map((c) => c.textContent);
    expect(globs).toEqual(["vendor/**", "*.snap"]);
  });

  it("says when nothing is ignored", async () => {
    await render(leaderboardModel({ config: withConfig({ ignore: [] }) }));
    expect(panel().querySelector("[data-ignore-globs]")).toBeNull();
    expect(text()).toContain("No file patterns are ignored for this project.");
  });

  it("explains in the Review section when GitHub data is unavailable", async () => {
    const base = leaderboardModel();
    await render(
      leaderboardModel({
        raw: {
          ...base.raw,
          ghAvailable: false,
          ghNote: "GitHub CLI (`gh`) is not installed.",
        },
      }),
    );
    expect(text()).toContain(
      "Review points are not counted because GitHub data is unavailable: GitHub CLI (`gh`) is not installed.",
    );
  });

  it("has no GitHub notice when review data exists", async () => {
    await render(leaderboardModel());
    expect(text()).not.toContain("GitHub data is unavailable");
  });
});

describe("FormulaSidebar behaviour", () => {
  it("takes the focus when it opens", async () => {
    await render(leaderboardModel());
    expect(document.activeElement).toBe(panel());
  });

  it("closes with Escape and with the X button, and only once", async () => {
    const onClose = vi.fn();
    await render(leaderboardModel(), { onClose });
    await act(async () => {
      document.dispatchEvent(
        new KeyboardEvent("keydown", { key: "Escape", bubbles: true }),
      );
    });
    expect(onClose).toHaveBeenCalledTimes(1);
    await act(async () =>
      panel()
        .querySelector<HTMLButtonElement>('button[aria-label="Close"]')!
        .click(),
    );
    expect(onClose).toHaveBeenCalledTimes(2);
  });

  it("ignores other keys and stops listening after it unmounts", async () => {
    const onClose = vi.fn();
    await render(leaderboardModel(), { onClose });
    await act(async () => {
      document.dispatchEvent(
        new KeyboardEvent("keydown", { key: "a", bubbles: true }),
      );
    });
    await act(async () => root.render(null));
    await act(async () => {
      document.dispatchEvent(
        new KeyboardEvent("keydown", { key: "Escape", bubbles: true }),
      );
    });
    expect(onClose).not.toHaveBeenCalled();
  });
});

describe("FormulaSidebar Ask Agent", () => {
  it("asks about the viewer's score", async () => {
    const onAsk = vi.fn();
    const model = leaderboardModel();
    await render(model, { onAskAgent: onAsk });
    await act(async () => button("Ask Agent about my score")!.click());
    expect(onAsk).toHaveBeenCalledWith({
      kind: "me",
      personId: model.meId,
    });
  });

  it("is disabled when the viewer is not on the board", async () => {
    const onAsk = vi.fn();
    await render(leaderboardModel({ user: undefined }), { onAskAgent: onAsk });
    const ask = button("Ask Agent about my score")!;
    expect(ask.disabled).toBe(true);
    await act(async () => ask.click());
    expect(onAsk).not.toHaveBeenCalled();
  });

  it("is hidden without a handler", async () => {
    await render(leaderboardModel());
    expect(button("Ask Agent about my score")).toBeUndefined();
  });
});

describe("FormulaSidebar weights editor", () => {
  it("is not offered without a config writer", async () => {
    await render(leaderboardModel());
    expect(button("Customize weights")).toBeUndefined();
    expect(text()).not.toContain("Saved in .monoptah/contributors.json");
  });

  it("expands to five number inputs with the current weights", async () => {
    await render(
      leaderboardModel({
        config: withConfig({
          weights: { ...DEFAULT_CONFIG.weights, care: 1.5 },
        }),
      }),
      { onChangeConfig: vi.fn() },
    );
    expect(panel().querySelector("[data-weights-editor]")).toBeNull();
    expect(text()).toContain("Saved in .monoptah/contributors.json");
    const toggle = button("Customize weights")!;
    expect(toggle.getAttribute("aria-expanded")).toBe("false");
    await act(async () => toggle.click());
    expect(toggle.getAttribute("aria-expanded")).toBe("true");
    const inputs = Array.from(
      panel().querySelectorAll<HTMLInputElement>("[data-weights-editor] input"),
    );
    expect(inputs.map((i) => i.getAttribute("aria-label"))).toEqual([
      "Delivery weight",
      "Care weight",
      "Review weight",
      "Consistency weight",
      "Breadth weight",
    ]);
    for (const i of inputs) {
      expect(i.type).toBe("number");
      expect(i.min).toBe("0");
      expect(i.max).toBe("5");
      expect(i.step).toBe("0.1");
    }
    expect(inputs.map((i) => i.value)).toEqual(["1", "1.5", "1", "1", "1"]);
    await act(async () => toggle.click());
    expect(panel().querySelector("[data-weights-editor]")).toBeNull();
  });

  it("saves the typed weights clamped to 0-5 and keeps the rest of the config", async () => {
    const onChange = vi.fn();
    const model = leaderboardModel({
      config: withConfig({ ignore: ["only/**"] }),
    });
    await render(model, { onChangeConfig: onChange });
    await act(async () => button("Customize weights")!.click());
    await act(async () => type(input("Delivery weight"), "9"));
    await act(async () => type(input("Care weight"), "-2"));
    await act(async () => type(input("Review weight"), "0.35"));
    await act(async () => type(input("Consistency weight"), ""));
    await act(async () => button("Save")!.click());
    expect(onChange).toHaveBeenCalledTimes(1);
    expect(onChange).toHaveBeenCalledWith({
      ...model.config,
      weights: {
        delivery: 5,
        care: 0,
        review: 0.35,
        consistency: 1,
        breadth: 1,
      },
    });
    expect(input("Delivery weight").value).toBe("5");
    expect(input("Care weight").value).toBe("0");
  });

  it("only saves when something changed", async () => {
    await render(leaderboardModel(), {
      onChangeConfig: vi.fn(),
    });
    await act(async () => button("Customize weights")!.click());
    expect(button("Save")!.disabled).toBe(true);
    await act(async () => type(input("Breadth weight"), "2"));
    expect(button("Save")!.disabled).toBe(false);
  });

  it("resets the inputs to the defaults and saves them on request", async () => {
    const onChange = vi.fn();
    const model = leaderboardModel({
      config: withConfig({
        weights: {
          delivery: 2,
          care: 0,
          review: 3,
          consistency: 1,
          breadth: 4,
        },
      }),
    });
    await render(model, { onChangeConfig: onChange });
    await act(async () => button("Customize weights")!.click());
    await act(async () => button("Reset to defaults")!.click());
    expect(onChange).not.toHaveBeenCalled();
    for (const label of [
      "Delivery weight",
      "Care weight",
      "Review weight",
      "Consistency weight",
      "Breadth weight",
    ]) {
      expect(input(label).value).toBe("1");
    }
    await act(async () => button("Save")!.click());
    expect(onChange).toHaveBeenCalledWith({
      ...model.config,
      weights: DEFAULT_CONFIG.weights,
    });
  });

  it("leaves write errors to the page, so one failure shows one alert", async () => {
    await render(leaderboardModel(), { onChangeConfig: vi.fn() });
    await act(async () => button("Customize weights")!.click());
    expect(panel().querySelector("[role=alert]")).toBeNull();
  });

  it("disables the weights editor while the config file is unreadable", async () => {
    const onChange = vi.fn();
    await render(leaderboardModel(), {
      onChangeConfig: onChange,
      editLocked: true,
    });
    const customize = button("Customize weights")!;
    expect(customize.disabled).toBe(true);
    await act(async () => customize.click());
    expect(panel().querySelector("[data-weights-editor]")).toBeNull();
    expect(onChange).not.toHaveBeenCalled();
  });

  it("locks an editor that was already open when the file became unreadable", async () => {
    const onChange = vi.fn();
    const model = leaderboardModel();
    await render(model, { onChangeConfig: onChange });
    await act(async () => button("Customize weights")!.click());
    await act(async () => type(input("Breadth weight"), "2"));
    await render(model, { onChangeConfig: onChange, editLocked: true });
    expect(input("Breadth weight").disabled).toBe(true);
    expect(button("Save")!.disabled).toBe(true);
    expect(button("Reset to defaults")!.disabled).toBe(true);
  });
});
