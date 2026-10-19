import { expect, it } from "vitest";
import app from "./App.tsx?raw";
import menu from "./shell/useProjectMenu.tsx?raw";
import projectRail from "./shell/ProjectRail.tsx?raw";
import sidebar from "./shell/Sidebar.tsx?raw";

// App.tsx is too large to mount in a test, so the wiring is checked on the
// source: a missed opener leaves two views mounted at once.

it("the Ticket Contributors state and view are gone: one page, two tabs", () => {
  for (const text of [app, sidebar, projectRail]) {
    expect(text).not.toMatch(/ticketContributors/i);
    expect(text).not.toContain("Ticket Contributors");
  }
});

it("opens Settings at the ClickUp section from the page", () => {
  expect(app).toMatch(
    /onOpenClickUpSettings = useCallback\(\s*\(\) => openSettings\("inbox", "clickup"\)/,
  );
});

it("Settings remembers and restores the open Contributors page with its tab and project", () => {
  expect(app).toContain("contributors: contributorsViewRef.current,");
  expect(app).toMatch(
    /setContributorsView\(\s*contributors !== null &&[\s\S]*?: contributors,\s*\);/,
  );
});

it("the entry is in the icon row, the compact rail and the project rail, ungated", () => {
  expect(sidebar).toMatch(
    /\{onOpenContributors \? \(\s*<IconButton\s+label="Contributors"/,
  );
  expect(sidebar).toMatch(
    /\{onOpenContributors \? \(\s*<CompactRailAction\s+label="Contributors"/,
  );
  expect(sidebar).toMatch(
    /label: "Contributors",\s*icon: ChartBreakoutSquare,/,
  );
  expect(projectRail).toMatch(
    /\{onOpenContributors \? \(\s*<RailAction\s+label="Contributors"/,
  );
  // Unlike Mail, no setting hides it.
  expect(app).not.toContain("contributorsEnabled");
});

it("the project menu has no Contributors item or opener", () => {
  expect(menu).not.toMatch(/ontributors/);
  expect(sidebar).not.toContain("canOpenContributors");
  // No caller hands the menu an opener either.
  const calls = [sidebar, projectRail].flatMap(
    (text) => text.match(/useProjectMenu\(\{[\s\S]*?\}\)/g) ?? [],
  );
  expect(calls.length).toBeGreaterThanOrEqual(3);
  for (const call of calls) expect(call).not.toMatch(/ontributors/);
});
