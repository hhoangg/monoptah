// @vitest-environment happy-dom
import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("../model/api", () => ({
  readConfig: vi.fn(),
  readMailmap: vi.fn(),
  writeConfig: vi.fn(),
  collectRaw: vi.fn(),
  listBranches: vi.fn(),
  gitUser: vi.fn(),
}));

vi.mock("../../../app/shell/WindowControls", () => ({
  WindowControls: () => null,
}));
// Stands in for the Identities tab so a config write can be driven from it.
vi.mock("./IdentitiesTab", async () => {
  const { createElement: h } = await import("react");
  return {
    IdentitiesTab: (props: {
      model: { unmapped: unknown[] };
      config: { weights: Record<string, number> };
      onChangeConfig: (next: unknown) => void;
      locked?: boolean;
    }) =>
      h(
        "button",
        {
          "data-locked": props.locked ? "true" : "false",
          onClick: () =>
            props.onChangeConfig({
              ...props.config,
              weights: { ...props.config.weights, care: 2 },
            }),
        },
        `${props.model.unmapped.length} unmapped identities`,
      ),
  };
});

import {
  collectRaw,
  gitUser,
  listBranches,
  readConfig,
  readMailmap,
  writeConfig,
} from "../model/api";
import { resetContributorsCache } from "../hooks/useContributors";
import { REMOTE_PROJECT_PREFIX } from "../../projects/model/recents";
import { RANGE_KEY } from "../model/prefs";
import { formatDay, seasonChoices } from "../model/range";
import { previousSeason } from "../model/season";
import { ANA, BOB, file, makeCommit, makeRaw } from "../model/testFixtures";
import { deferUnhandledEscape } from "../../workspace/model/tabKeys";
import { ContributorsView } from "./ContributorsView";

let container: HTMLDivElement;
let root: Root;
let counter = 0;

const recent = (daysAgo: number) =>
  new Date(Date.now() - daysAgo * 86_400_000).toISOString();

const liveRaw = (extra: Parameters<typeof makeRaw>[0] = {}) =>
  makeRaw({
    since: recent(70),
    commits: [
      makeCommit("a1", ANA, recent(1), { files: [file("src/a.ts", 10)] }),
      makeCommit("b1", BOB, recent(2), { files: [file("src/b.ts", 5)] }),
    ],
    ...extra,
  });

// A CI account no person lists: the only identity that counts as unmapped.
const MACHINE = { name: "Build Box", email: "ci@build-box" };
const withMachine = () =>
  liveRaw({
    commits: [
      makeCommit("a1", ANA, recent(1), { files: [file("src/a.ts", 10)] }),
      makeCommit("b1", BOB, recent(2), { files: [file("src/b.ts", 5)] }),
      makeCommit("m1", MACHINE, recent(3), { files: [file("src/m.ts", 2)] }),
    ],
  });

type ViewProps = Parameters<typeof ContributorsView>[0];

async function mount(props: Partial<ViewProps> = {}) {
  counter += 1;
  await act(async () => {
    root.render(
      createElement(ContributorsView, {
        project: `/work/project-${counter}`,
        besideRail: true,
        onClose: vi.fn(),
        ...props,
      }),
    );
  });
  // Let the lazy tab chunks and the mocked loads settle.
  await act(async () => {});
}

// A lazily imported tab resolves after an unknown number of ticks, which grows
// under load, so tests wait for what they need to see instead of a fixed delay.
async function waitFor(check: () => boolean, what: string, timeoutMs = 5000) {
  const end = Date.now() + timeoutMs;
  while (!check()) {
    if (Date.now() > end) throw new Error(`timed out waiting for ${what}`);
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 5));
    });
  }
}

const LEADERBOARD_MARKER = "Rankings are for motivation";
const IDENTITIES_MARKER = "unmapped identities";

/** Clicks a tab and waits for its lazily loaded content to show. */
async function openTab(label: string, marker: string) {
  await act(async () => byText('[role="tab"]', label)?.click());
  await waitFor(
    () => (container.textContent ?? "").includes(marker),
    `the ${label} tab content`,
  );
}

const byText = (selector: string, text: string) =>
  Array.from(container.querySelectorAll<HTMLElement>(selector)).find((el) =>
    (el.textContent ?? "").includes(text),
  );

const rangeButton = (label: string) =>
  Array.from(
    container.querySelectorAll<HTMLElement>(
      '[role="radiogroup"][aria-label="Range"] button',
    ),
  ).find((b) => b.textContent === label);

const rangeInputs = () => {
  const start = container.querySelector<HTMLInputElement>(
    'input[type="date"][aria-label="Start date"]',
  );
  const end = container.querySelector<HTMLInputElement>(
    'input[type="date"][aria-label="End date"]',
  );
  return start && end ? { start, end } : null;
};

/** Types a value into a controlled input the way a user edit reaches React. */
async function setDay(input: HTMLInputElement, value: string) {
  const set = Object.getOwnPropertyDescriptor(
    HTMLInputElement.prototype,
    "value",
  )!.set!;
  await act(async () => {
    set.call(input, value);
    input.dispatchEvent(new Event("input", { bubbles: true }));
  });
}

beforeEach(() => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  localStorage.clear();
  resetContributorsCache();
  container = document.createElement("div");
  document.body.append(container);
  root = createRoot(container);
  vi.mocked(readConfig).mockResolvedValue(null);
  vi.mocked(readMailmap).mockResolvedValue(null);
  vi.mocked(writeConfig).mockResolvedValue(undefined);
  vi.mocked(listBranches).mockResolvedValue({
    current: "main",
    branches: ["main", "dev"],
  });
  vi.mocked(gitUser).mockResolvedValue({
    name: "Ana",
    email: "ana@example.com",
  });
  vi.mocked(collectRaw).mockImplementation(async () => liveRaw());
});

afterEach(async () => {
  await act(async () => root.unmount());
  container.remove();
  vi.clearAllMocks();
  vi.unstubAllGlobals();
});

describe("ContributorsView", () => {
  it("is a labelled region with the project crumb", async () => {
    await mount({ project: "/work/monoptah" });
    const region = container.querySelector('[role="region"]');
    expect(region?.getAttribute("aria-label")).toBe("Contributors");
    expect(region?.textContent).toContain("monoptah");
  });

  it("shows a loading state before the data arrives", async () => {
    vi.mocked(collectRaw).mockReturnValueOnce(new Promise(() => {}));
    await mount();
    expect(container.querySelector('[role="status"]')?.textContent).toContain(
      "Reading the git history",
    );
    expect(container.querySelector('[role="tablist"]')).toBeNull();
  });

  it("renders the three tabs with the unmapped count on Identities", async () => {
    vi.mocked(collectRaw).mockImplementation(async () => withMachine());
    await mount();
    const tabs = Array.from(container.querySelectorAll('[role="tab"]'));
    expect(tabs.map((t) => t.textContent)).toEqual([
      "Stats",
      "Leaderboard",
      "Identities1 unmapped",
    ]);
    expect(tabs[0].getAttribute("aria-selected")).toBe("true");
    expect(container.textContent).toContain("Commit activity");
  });

  it("shows no unmapped badge when only ordinary authors are present", async () => {
    await mount();
    const tab = byText('[role="tab"]', "Identities");
    expect(tab?.textContent).toBe("Identities");
  });

  it("switches tabs and shows the tab content", async () => {
    vi.mocked(collectRaw).mockImplementation(async () => withMachine());
    await mount();
    await openTab("Leaderboard", LEADERBOARD_MARKER);
    expect(
      byText('[role="tab"]', "Leaderboard")?.getAttribute("aria-selected"),
    ).toBe("true");
    expect(container.textContent).toContain(
      "Rankings are for motivation, not performance review.",
    );
    expect(container.textContent).not.toContain("Commit activity");

    await openTab("Identities", IDENTITIES_MARKER);
    expect(container.textContent).toContain("1 unmapped identities");
  });

  it("moves between tabs with the arrow keys", async () => {
    await mount();
    const list = container.querySelector('[role="tablist"]') as HTMLElement;
    await act(async () => {
      list.dispatchEvent(
        new KeyboardEvent("keydown", { key: "ArrowRight", bubbles: true }),
      );
    });
    expect(
      byText('[role="tab"]', "Leaderboard")?.getAttribute("aria-selected"),
    ).toBe("true");
  });

  it("shows the GitHub notice with the reason and retries with a forced refresh", async () => {
    vi.mocked(collectRaw).mockImplementation(async () =>
      liveRaw({
        ghAvailable: false,
        ghNote: "GitHub CLI (`gh`) is not installed.",
      }),
    );
    await mount();
    const notice = container.querySelector('[role="status"]');
    expect(notice?.textContent).toContain(
      "Review points are not counted because GitHub data is unavailable: GitHub CLI (`gh`) is not installed.",
    );
    await act(async () => byText("button", "Retry")?.click());
    expect(vi.mocked(collectRaw).mock.calls.at(-1)?.[1].force).toBe(true);
  });

  it("shows a collector note as information when GitHub data is available", async () => {
    vi.mocked(collectRaw).mockImplementation(async () =>
      liveRaw({
        ghAvailable: true,
        ghNote:
          "GitHub returned no pull requests for this period, so review points may be missing.",
      }),
    );
    await mount();
    const notice = container.querySelector('[role="status"]');
    expect(notice?.textContent).toContain(
      "GitHub returned no pull requests for this period, so review points may be missing.",
    );
    expect(notice?.textContent).not.toContain("not counted");
  });

  it("hides the GitHub notice when GitHub data is available", async () => {
    await mount();
    expect(container.textContent).not.toContain("GitHub data is unavailable");
  });

  it("shows the error with a Retry that reloads", async () => {
    vi.mocked(collectRaw).mockRejectedValueOnce("not a git repository");
    await mount();
    expect(container.querySelector('[role="alert"]')?.textContent).toContain(
      "not a git repository",
    );
    await act(async () => byText('[role="alert"] button', "Retry")?.click());
    expect(container.querySelector('[role="alert"]')).toBeNull();
    expect(container.querySelector('[role="tablist"]')).not.toBeNull();
  });

  it("clears the old error and shows the loading state while retrying", async () => {
    vi.mocked(collectRaw).mockRejectedValueOnce("not a git repository");
    await mount();
    expect(container.querySelector('[role="alert"]')).not.toBeNull();
    let release!: () => void;
    vi.mocked(collectRaw).mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          release = () => resolve(liveRaw());
        }),
    );
    await act(async () => byText('[role="alert"] button', "Retry")?.click());
    expect(container.querySelector('[role="alert"]')).toBeNull();
    expect(container.querySelector('[role="status"]')?.textContent).toContain(
      "Reading the git history",
    );
    await act(async () => release());
    expect(container.querySelector('[role="tablist"]')).not.toBeNull();
  });

  it("shows an empty state when there are no commits in the range", async () => {
    vi.mocked(collectRaw).mockImplementation(async () =>
      makeRaw({ since: recent(70) }),
    );
    await mount();
    expect(container.textContent).toContain("No commits in this range");
  });

  it("fires the team scope from the Ask Agent button, which is absent without a handler", async () => {
    const onAskAgent = vi.fn();
    await mount({ onAskAgent });
    await act(async () => byText("button", "Ask Agent")?.click());
    expect(onAskAgent).toHaveBeenCalledWith({ kind: "team" });
  });

  it("omits Ask Agent without a handler", async () => {
    await mount();
    expect(byText("button", "Ask Agent")).toBeUndefined();
  });

  it("offers only 7d, 30d and Custom", async () => {
    await mount();
    const labels = Array.from(
      container.querySelectorAll(
        '[role="radiogroup"][aria-label="Range"] button',
      ),
    ).map((b) => b.textContent);
    expect(labels).toEqual(["7d", "30d", "Custom"]);
    expect(rangeInputs()).toBeNull();
  });

  it("changes and remembers the range", async () => {
    await mount();
    const week = rangeButton("7d");
    await act(async () => week?.click());
    expect(localStorage.getItem(RANGE_KEY)).toBe("7d");
    expect(week?.getAttribute("aria-checked")).toBe("true");
  });

  it("falls back to 30d for a saved range that no longer exists", async () => {
    localStorage.setItem(RANGE_KEY, "90d");
    await mount();
    expect(rangeButton("30d")?.getAttribute("aria-checked")).toBe("true");
  });

  describe("custom range", () => {
    const today = () => formatDay(new Date());
    const daysAgo = (n: number) => {
      const d = new Date();
      return formatDay(
        new Date(d.getFullYear(), d.getMonth(), d.getDate() - n),
      );
    };

    it("starts from the days the current preset shows", async () => {
      await mount();
      await act(async () => rangeButton("Custom")?.click());
      expect(rangeButton("Custom")?.getAttribute("aria-checked")).toBe("true");
      const { start, end } = rangeInputs()!;
      expect(start.value).toBe(daysAgo(29));
      expect(end.value).toBe(today());
      expect(localStorage.getItem(RANGE_KEY)).toBe(
        `custom:${daysAgo(29)}:${today()}`,
      );
    });

    it("loads the data of the picked days", async () => {
      await mount();
      await act(async () => rangeButton("Custom")?.click());
      const before = vi.mocked(collectRaw).mock.calls.length;
      await setDay(rangeInputs()!.start, daysAgo(200));
      expect(vi.mocked(collectRaw).mock.calls.length).toBeGreaterThan(before);
      const since = Date.parse(
        vi.mocked(collectRaw).mock.calls.at(-1)![1].since,
      );
      expect(since).toBeLessThanOrEqual(Date.parse(daysAgo(200)));
    });

    it("keeps the span within 30 days when the start is moved further back", async () => {
      await mount();
      await act(async () => rangeButton("Custom")?.click());
      await setDay(rangeInputs()!.start, daysAgo(90));
      const { start, end } = rangeInputs()!;
      expect(start.value).toBe(daysAgo(90));
      expect(end.value).toBe(daysAgo(61));
      expect(localStorage.getItem(RANGE_KEY)).toBe(
        `custom:${daysAgo(90)}:${daysAgo(61)}`,
      );
    });

    it("keeps the span within 30 days when the end is moved forward", async () => {
      localStorage.setItem(RANGE_KEY, `custom:${daysAgo(90)}:${daysAgo(80)}`);
      await mount();
      await setDay(rangeInputs()!.end, daysAgo(10));
      const { start, end } = rangeInputs()!;
      expect(end.value).toBe(daysAgo(10));
      expect(start.value).toBe(daysAgo(39));
    });

    it("does not allow an end after today", async () => {
      await mount();
      await act(async () => rangeButton("Custom")?.click());
      expect(rangeInputs()!.end.max).toBe(today());
      expect(rangeInputs()!.start.max).toBe(today());
      const tomorrow = new Date();
      tomorrow.setDate(tomorrow.getDate() + 1);
      await setDay(rangeInputs()!.end, formatDay(tomorrow));
      expect(rangeInputs()!.end.value).toBe(today());
    });

    it("moves the other end when the picked start is after the end", async () => {
      localStorage.setItem(RANGE_KEY, `custom:${daysAgo(20)}:${daysAgo(10)}`);
      await mount();
      await setDay(rangeInputs()!.start, daysAgo(5));
      expect(rangeInputs()!.start.value).toBe(daysAgo(5));
      expect(rangeInputs()!.end.value).toBe(daysAgo(5));
    });

    it("ignores a cleared date", async () => {
      localStorage.setItem(RANGE_KEY, `custom:${daysAgo(20)}:${daysAgo(10)}`);
      await mount();
      await setDay(rangeInputs()!.start, "");
      expect(rangeInputs()!.start.value).toBe(daysAgo(20));
    });

    it("clamps a saved range that is longer than 30 days", async () => {
      localStorage.setItem(RANGE_KEY, `custom:${daysAgo(100)}:${daysAgo(5)}`);
      await mount();
      const { start, end } = rangeInputs()!;
      expect(end.value).toBe(daysAgo(5));
      expect(start.value).toBe(daysAgo(34));
    });

    it("goes back to a preset and hides the date inputs", async () => {
      await mount();
      await act(async () => rangeButton("Custom")?.click());
      await act(async () => rangeButton("7d")?.click());
      expect(rangeInputs()).toBeNull();
      expect(localStorage.getItem(RANGE_KEY)).toBe("7d");
    });

    it("shows how many days the previous-period comparison covers", async () => {
      localStorage.setItem(RANGE_KEY, `custom:${daysAgo(9)}:${today()}`);
      await mount();
      expect(container.textContent).toContain("vs prev 10d");
    });
  });

  it("toggles merge identities and remembers it", async () => {
    await mount();
    const toggle = container.querySelector(
      'button[role="switch"][aria-label="Merge identities"]',
    ) as HTMLButtonElement;
    expect(toggle.getAttribute("aria-checked")).toBe("true");
    await act(async () => toggle.click());
    expect(toggle.getAttribute("aria-checked")).toBe("false");
    expect(localStorage.getItem("monocode.contributors.mergeIdentities")).toBe(
      "false",
    );
  });

  it("offers the branches and collects the chosen one", async () => {
    await mount();
    const trigger = container.querySelector(
      'button[aria-label="Branch: main"]',
    ) as HTMLButtonElement;
    await act(async () => trigger.click());
    const dev = Array.from(
      document.body.querySelectorAll('[role="option"]'),
    ).find((o) => o.textContent?.includes("dev")) as HTMLElement;
    await act(async () => dev.click());
    expect(vi.mocked(collectRaw).mock.calls.at(-1)?.[1].branch).toBe("dev");
  });

  it("shows a config save failure from the Identities tab and clears it on success", async () => {
    await mount();
    vi.mocked(writeConfig).mockRejectedValueOnce("disk full");
    await openTab("Identities", IDENTITIES_MARKER);
    await act(async () => byText("button", "unmapped identities")?.click());
    expect(container.querySelector('[role="alert"]')?.textContent).toContain(
      "Could not save .monoptah/contributors.json: disk full",
    );
    await act(async () => byText("button", "unmapped identities")?.click());
    expect(container.querySelector('[role="alert"]')).toBeNull();
  });
});

describe("ContributorsView leaderboard", () => {
  beforeEach(() => {
    vi.stubGlobal("requestAnimationFrame", (cb: (t: number) => void) => {
      cb(0);
      return 1;
    });
    vi.stubGlobal("cancelAnimationFrame", () => {});
  });

  const openLeaderboard = async () => {
    await mount();
    await openTab("Leaderboard", LEADERBOARD_MARKER);
  };

  it("offers the current month and five before, and loads earlier data for an older pick", async () => {
    await openLeaderboard();
    const before = vi.mocked(collectRaw).mock.calls.length;
    const trigger = container.querySelector(
      'button[aria-label^="Season:"]',
    ) as HTMLButtonElement;
    await act(async () => trigger.click());
    const options = Array.from(
      document.body.querySelectorAll<HTMLElement>('[role="option"]'),
    );
    expect(options).toHaveLength(6);
    const oldest = options[5];
    await act(async () => oldest.click());
    const calls = vi.mocked(collectRaw).mock.calls;
    expect(calls.length).toBe(before + 1);
    const picked = seasonChoices(new Date())[5];
    expect(Date.parse(calls.at(-1)![1].since)).toBeLessThanOrEqual(
      previousSeason(picked).start.getTime(),
    );
    expect(
      container.querySelector('button[aria-label^="Season:"]')?.textContent,
    ).toBe(picked.label);
  });

  it("opens the formula sidebar over the page body, outside the scrolling content", async () => {
    await openLeaderboard();
    await act(async () =>
      byText("button", "How is the score calculated?")?.click(),
    );
    const panel = container.querySelector('[role="complementary"]');
    expect(panel).not.toBeNull();
    expect(panel?.parentElement?.className).toContain("pointer-events-none");
    expect(panel?.closest("[class*='overflow-y-auto']")).toBe(panel);
    expect(panel?.textContent).toContain("Your score");
    await act(async () => {
      document.dispatchEvent(
        new KeyboardEvent("keydown", { key: "Escape", bubbles: true }),
      );
    });
    expect(container.querySelector('[role="complementary"]')).toBeNull();
  });

  it("writes edited weights to the config file", async () => {
    await openLeaderboard();
    await act(async () =>
      byText("button", "How is the score calculated?")?.click(),
    );
    await act(async () => byText("button", "Customize weights")?.click());
    const care = container.querySelector(
      'input[aria-label="Care weight"]',
    ) as HTMLInputElement;
    await act(async () => {
      Object.getOwnPropertyDescriptor(
        HTMLInputElement.prototype,
        "value",
      )!.set!.call(care, "2");
      care.dispatchEvent(new Event("input", { bubbles: true }));
    });
    await act(async () => byText("button", "Save")?.click());
    const written = vi.mocked(writeConfig).mock.calls.at(-1)!;
    expect(JSON.parse(written[1]).weights.care).toBe(2);
  });

  it("shows a failed write once, at the page level, even with the sidebar open", async () => {
    await openLeaderboard();
    await act(async () =>
      byText("button", "How is the score calculated?")?.click(),
    );
    await act(async () => byText("button", "Customize weights")?.click());
    const care = container.querySelector(
      'input[aria-label="Care weight"]',
    ) as HTMLInputElement;
    vi.mocked(writeConfig).mockRejectedValueOnce("disk full");
    await act(async () => {
      Object.getOwnPropertyDescriptor(
        HTMLInputElement.prototype,
        "value",
      )!.set!.call(care, "3");
      care.dispatchEvent(new Event("input", { bubbles: true }));
    });
    await act(async () => byText("button", "Save")?.click());
    const alerts = container.querySelectorAll('[role="alert"]');
    expect(alerts).toHaveLength(1);
    expect(alerts[0].textContent).toContain("disk full");
  });

  it("shows a failed write from the Identities tab once", async () => {
    await mount();
    vi.mocked(writeConfig).mockRejectedValueOnce("disk full");
    await openTab("Identities", IDENTITIES_MARKER);
    await act(async () => byText("button", "unmapped identities")?.click());
    expect(container.querySelectorAll('[role="alert"]')).toHaveLength(1);
  });
});

describe("ContributorsView with an unreadable config file", () => {
  const BANNER =
    /^\.monoptah\/contributors\.json could not be read \(Invalid JSON: .+\)\. Fix the file in your repo, then reload\. Editing is disabled until then\.$/;
  const banner = () =>
    Array.from(container.querySelectorAll<HTMLElement>('[role="alert"]')).find(
      (el) => (el.textContent ?? "").includes("could not be read"),
    );

  beforeEach(() => {
    vi.stubGlobal("requestAnimationFrame", (cb: (t: number) => void) => {
      cb(0);
      return 1;
    });
    vi.stubGlobal("cancelAnimationFrame", () => {});
    vi.mocked(readConfig).mockResolvedValue('{ "people": [ ,, ] }');
  });

  it("shows a banner with the reason and a Reload button, on every tab", async () => {
    await mount();
    expect((banner()?.textContent ?? "").replace(/Reload$/, "").trim()).toMatch(
      BANNER,
    );
    expect(
      Array.from(banner()!.querySelectorAll("button")).map(
        (b) => b.textContent,
      ),
    ).toEqual(["Reload"]);
    await openTab("Leaderboard", LEADERBOARD_MARKER);
    expect(banner()).toBeDefined();
    await openTab("Identities", IDENTITIES_MARKER);
    expect(banner()).toBeDefined();
    // The page still renders from the defaults.
    expect(container.querySelector('[role="tablist"]')).not.toBeNull();
  });

  it("reloads from disk and drops the banner once the file is fixed", async () => {
    await mount();
    expect(banner()).toBeDefined();
    const reads = vi.mocked(readConfig).mock.calls.length;
    vi.mocked(readConfig).mockResolvedValue(null);
    await act(async () => byText('[role="alert"] button', "Reload")?.click());
    expect(vi.mocked(readConfig).mock.calls.length).toBe(reads + 1);
    expect(vi.mocked(collectRaw).mock.calls.at(-1)?.[1].force).toBe(true);
    expect(banner()).toBeUndefined();
  });

  it("locks the Identities tab and the weights editor and writes nothing", async () => {
    await mount();
    await openTab("Identities", IDENTITIES_MARKER);
    const identities = byText("button", "unmapped identities")!;
    expect(identities.getAttribute("data-locked")).toBe("true");
    // A write attempted anyway is refused without a second alert.
    await act(async () => identities.click());
    expect(writeConfig).not.toHaveBeenCalled();
    expect(
      Array.from(container.querySelectorAll('[role="alert"]')).filter(
        (el) => !(el.textContent ?? "").includes("could not be read"),
      ),
    ).toEqual([]);

    await openTab("Leaderboard", LEADERBOARD_MARKER);
    await act(async () =>
      byText("button", "How is the score calculated?")?.click(),
    );
    expect(
      (byText("button", "Customize weights") as HTMLButtonElement).disabled,
    ).toBe(true);
  });

  it("does not show the banner for a readable file", async () => {
    vi.mocked(readConfig).mockResolvedValue(null);
    await mount();
    expect(banner()).toBeUndefined();
    await openTab("Identities", IDENTITIES_MARKER);
    expect(
      byText("button", "unmapped identities")?.getAttribute("data-locked"),
    ).toBe("false");
  });
});

describe("ContributorsView hardening", () => {
  it("says Contributors works on local projects for a remote path, and reads nothing", async () => {
    const calls = vi.mocked(collectRaw).mock.calls.length;
    await mount({ project: `${REMOTE_PROJECT_PREFIX}host/work/app` });
    expect(container.textContent).toContain(
      "Contributors works on local projects only",
    );
    expect(container.querySelector('[role="tablist"]')).toBeNull();
    expect(container.textContent).not.toContain("Reading the git history");
    expect(container.querySelector(".hidden")?.textContent).toContain(
      "Merge identities",
    );
    expect(vi.mocked(collectRaw).mock.calls.length).toBe(calls);
  });

  it("jumps to the first and last tab with Home and End", async () => {
    await mount();
    const list = container.querySelector('[role="tablist"]') as HTMLElement;
    await act(async () => {
      list.dispatchEvent(
        new KeyboardEvent("keydown", { key: "End", bubbles: true }),
      );
    });
    expect(
      byText('[role="tab"]', "Identities")?.getAttribute("aria-selected"),
    ).toBe("true");
    await act(async () => {
      list.dispatchEvent(
        new KeyboardEvent("keydown", { key: "Home", bubbles: true }),
      );
    });
    expect(byText('[role="tab"]', "Stats")?.getAttribute("aria-selected")).toBe(
      "true",
    );
  });

  it("keeps the page up and marks it busy while a new range loads", async () => {
    await mount();
    let release!: () => void;
    vi.mocked(collectRaw).mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          release = () => resolve(liveRaw());
        }),
    );
    const all = Array.from(
      container.querySelectorAll(
        '[role="radiogroup"][aria-label="Range"] button',
      ),
    ).find((b) => b.textContent === "7d") as HTMLElement;
    await act(async () => all.click());
    expect(container.querySelector('[role="tablist"]')).not.toBeNull();
    expect(
      container.querySelector('[role="tabpanel"]')?.getAttribute("aria-busy"),
    ).toBe("true");
    await act(async () => release());
    expect(
      container.querySelector('[role="tabpanel"]')?.getAttribute("aria-busy"),
    ).toBe("false");
  });
});

describe("ContributorsView Ask Agent dialog", () => {
  beforeEach(() => {
    vi.stubGlobal("requestAnimationFrame", (cb: (t: number) => void) => {
      cb(0);
      return 1;
    });
    vi.stubGlobal("cancelAnimationFrame", () => {});
    // Commits made right now always fall in the current season.
    vi.mocked(collectRaw).mockImplementation(async () =>
      liveRaw({
        commits: [
          makeCommit("a1", ANA, recent(0), { files: [file("src/a.ts", 10)] }),
          makeCommit("b1", BOB, recent(0), { files: [file("src/b.ts", 5)] }),
        ],
      }),
    );
  });

  const inDialog = (text: string) =>
    Array.from(
      document.querySelectorAll<HTMLElement>('[role="dialog"] button'),
    ).find((el) => (el.textContent ?? "").includes(text));
  const dialog = () => document.querySelector('[role="dialog"]');
  const startWith = (onStart: ReturnType<typeof vi.fn>) =>
    mount({ onStartAgentSession: onStart as never });
  const typeQuestion = async (text: string) => {
    const box = document.querySelector("textarea") as HTMLTextAreaElement;
    await act(async () => {
      Object.getOwnPropertyDescriptor(
        HTMLTextAreaElement.prototype,
        "value",
      )!.set!.call(box, text);
      box.dispatchEvent(new Event("input", { bubbles: true }));
    });
  };
  const clickStart = () => act(async () => inDialog("Start session")?.click());

  it("shows the Ask buttons when only onStartAgentSession is given", async () => {
    await startWith(vi.fn());
    expect(byText("button", "Ask Agent")).toBeDefined();
    expect(dialog()).toBeNull();
  });

  it("disables Ask Agent until the data is ready and never opens a dialog later", async () => {
    let release!: () => void;
    vi.mocked(collectRaw).mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          release = () => resolve(liveRaw());
        }),
    );
    const onAskAgent = vi.fn();
    await mount({ onAskAgent, onStartAgentSession: vi.fn() });
    const ask = byText("button", "Ask Agent") as HTMLButtonElement;
    expect(ask.disabled).toBe(true);
    await act(async () => ask.click());
    await act(async () => release());
    await waitFor(
      () => !(byText("button", "Ask Agent") as HTMLButtonElement).disabled,
      "the load to finish",
    );
    expect(dialog()).toBeNull();
    expect(onAskAgent).not.toHaveBeenCalled();
    expect((byText("button", "Ask Agent") as HTMLButtonElement).disabled).toBe(
      false,
    );
  });

  it("disables Ask Agent after a failed load and opens nothing on retry", async () => {
    vi.mocked(collectRaw).mockRejectedValueOnce(new Error("git failed"));
    await mount({ onStartAgentSession: vi.fn() });
    const ask = byText("button", "Ask Agent") as HTMLButtonElement;
    expect(ask.disabled).toBe(true);
    await act(async () => ask.click());
    await act(async () => byText("button", "Retry")?.click());
    await waitFor(
      () => container.querySelector('[role="alert"]') === null,
      "the retry to finish",
    );
    expect(dialog()).toBeNull();
  });

  it("opens the dialog for the team from the top bar and starts a team session", async () => {
    const onStart = vi.fn().mockResolvedValue(undefined);
    await startWith(onStart);
    await act(async () => byText("button", "Ask Agent")?.click());
    expect(dialog()?.textContent).toContain("about the whole team");
    await typeQuestion("What changed?");
    await clickStart();
    expect(onStart).toHaveBeenCalledTimes(1);
    const request = onStart.mock.calls[0][0];
    expect(request.scope).toEqual({ kind: "team" });
    expect(request.question).toBe("What changed?");
    expect(JSON.parse(request.snapshotJson).scope).toEqual({ kind: "team" });
    expect(dialog()).toBeNull();
  });

  it("keeps reporting the scope to onAskAgent as well", async () => {
    const onAskAgent = vi.fn();
    await mount({ onAskAgent, onStartAgentSession: vi.fn() });
    await act(async () => byText("button", "Ask Agent")?.click());
    expect(onAskAgent).toHaveBeenCalledWith({ kind: "team" });
    expect(dialog()).not.toBeNull();
  });

  it("does not open a dialog without onStartAgentSession", async () => {
    await mount({ onAskAgent: vi.fn() });
    await act(async () => byText("button", "Ask Agent")?.click());
    expect(dialog()).toBeNull();
  });

  it("opens a person dialog from a Stats row", async () => {
    const onStart = vi.fn().mockResolvedValue(undefined);
    await startWith(onStart);
    await act(async () =>
      (
        container.querySelector(
          'button[aria-label="Ask Agent about Ana"]',
        ) as HTMLElement
      ).click(),
    );
    expect(dialog()?.textContent).toContain("about Ana");
    await typeQuestion("q");
    await clickStart();
    const request = onStart.mock.calls[0][0];
    expect(request.scope.kind).toBe("person");
    expect(JSON.parse(request.snapshotJson).focus.name).toBe("Ana");
  });

  it("opens a person dialog from a Leaderboard row", async () => {
    await startWith(vi.fn());
    await openTab("Leaderboard", LEADERBOARD_MARKER);
    await act(async () =>
      (
        container.querySelector(
          'button[aria-label="Ask Agent about Bob"]',
        ) as HTMLElement
      ).click(),
    );
    expect(dialog()?.textContent).toContain("about Bob");
  });

  it("opens a 'me' dialog from the formula sidebar", async () => {
    const onStart = vi.fn().mockResolvedValue(undefined);
    await startWith(onStart);
    await openTab("Leaderboard", LEADERBOARD_MARKER);
    await act(async () =>
      byText("button", "How is the score calculated?")?.click(),
    );
    await act(async () =>
      byText("button", "Ask Agent about my score")?.click(),
    );
    expect(dialog()?.textContent).toContain("about your score");
    await typeQuestion("q");
    await clickStart();
    const request = onStart.mock.calls[0][0];
    expect(request.scope.kind).toBe("me");
    expect(JSON.parse(request.snapshotJson).focus.name).toBe("Ana");
  });

  it("Escape closes only the dialog when the formula sidebar is open", async () => {
    await startWith(vi.fn());
    await openTab("Leaderboard", LEADERBOARD_MARKER);
    await act(async () =>
      byText("button", "How is the score calculated?")?.click(),
    );
    await act(async () =>
      byText("button", "Ask Agent about my score")?.click(),
    );
    expect(dialog()).not.toBeNull();
    const escape = () =>
      act(async () => {
        document.body.dispatchEvent(
          new KeyboardEvent("keydown", { key: "Escape", bubbles: true }),
        );
      });
    await escape();
    expect(dialog()).toBeNull();
    expect(container.querySelector('[role="complementary"]')).not.toBeNull();
    await escape();
    expect(container.querySelector('[role="complementary"]')).toBeNull();
  });

  it("closes with Cancel and shows a failed start inside the dialog", async () => {
    const onStart = vi.fn().mockRejectedValue(new Error("Session failed"));
    await startWith(onStart);
    await act(async () => byText("button", "Ask Agent")?.click());
    await typeQuestion("q");
    await clickStart();
    expect(dialog()?.querySelector('[role="alert"]')?.textContent).toBe(
      "Session failed",
    );
    await act(async () => inDialog("Cancel")?.click());
    expect(dialog()).toBeNull();
  });
});

describe("ContributorsView Escape", () => {
  beforeEach(() => {
    vi.stubGlobal("requestAnimationFrame", (cb: (t: number) => void) => {
      cb(0);
      return 1;
    });
    vi.stubGlobal("cancelAnimationFrame", () => {});
  });

  const press = async (target: EventTarget = document.body) => {
    const event = new KeyboardEvent("keydown", {
      key: "Escape",
      bubbles: true,
      cancelable: true,
    });
    await act(async () => {
      target.dispatchEvent(event);
    });
    return event;
  };
  const dialog = () => document.querySelector('[role="dialog"]');

  it("closes the page on a plain Escape and claims the key", async () => {
    const onClose = vi.fn();
    await mount({ onClose });
    const event = await press();
    expect(onClose).toHaveBeenCalledTimes(1);
    expect(event.defaultPrevented).toBe(true);
  });

  it("leaves Escape alone when another layer already took it", async () => {
    const onClose = vi.fn();
    await mount({ onClose });
    const claim = (e: Event) => e.preventDefault();
    document.addEventListener("keydown", claim);
    try {
      await press();
    } finally {
      document.removeEventListener("keydown", claim);
    }
    expect(onClose).not.toHaveBeenCalled();
  });

  it("does not stop a running agent turn: the deferred stop sees a claimed Escape", async () => {
    const stop = vi.fn();
    // What App registers, before the page mounts.
    const app = (e: KeyboardEvent) => deferUnhandledEscape(e, stop);
    window.addEventListener("keydown", app);
    try {
      await mount({ onClose: vi.fn() });
      await press();
      await new Promise((resolve) => setTimeout(resolve, 10));
      expect(stop).not.toHaveBeenCalled();
    } finally {
      window.removeEventListener("keydown", app);
    }
  });

  it("while a text field has focus, Escape leaves the field instead of closing the page", async () => {
    const onClose = vi.fn();
    await mount({ onClose });
    const field = document.createElement("input");
    container.append(field);
    field.focus();
    const event = await press(field);
    expect(onClose).not.toHaveBeenCalled();
    expect(event.defaultPrevented).toBe(true);
    expect(document.activeElement).not.toBe(field);
    await press();
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("the formula sidebar keeps priority, then the page closes", async () => {
    const onClose = vi.fn();
    await mount({ onClose });
    await openTab("Leaderboard", LEADERBOARD_MARKER);
    await act(async () =>
      byText("button", "How is the score calculated?")?.click(),
    );
    await press();
    expect(container.querySelector('[role="complementary"]')).toBeNull();
    expect(onClose).not.toHaveBeenCalled();
    await press();
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("the Ask Agent dialog keeps priority, then the page closes", async () => {
    const onClose = vi.fn();
    await mount({ onClose, onStartAgentSession: vi.fn() });
    await act(async () => byText("button", "Ask Agent")?.click());
    expect(dialog()).not.toBeNull();
    await press();
    expect(dialog()).toBeNull();
    expect(onClose).not.toHaveBeenCalled();
    await press();
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("returns focus to the control that opened the Ask Agent dialog", async () => {
    await mount({ onStartAgentSession: vi.fn() });
    const opener = byText("button", "Ask Agent") as HTMLButtonElement;
    await act(async () => opener.click());
    expect(dialog()).not.toBeNull();
    await press();
    expect(dialog()).toBeNull();
    expect(document.activeElement).toBe(opener);

    // A row button works the same, and so does Cancel.
    const row = container.querySelector(
      'button[aria-label="Ask Agent about Ana"]',
    ) as HTMLButtonElement;
    await act(async () => row.click());
    const cancel = Array.from(
      document.querySelectorAll<HTMLElement>('[role="dialog"] button'),
    ).find((el) => el.textContent?.includes("Cancel"));
    await act(async () => cancel?.click());
    expect(dialog()).toBeNull();
    expect(document.activeElement).toBe(row);
  });
});
