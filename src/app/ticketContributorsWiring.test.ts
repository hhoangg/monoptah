import { expect, it } from "vitest";
import app from "./App.tsx?raw";
import projectRail from "./shell/ProjectRail.tsx?raw";
import sidebar from "./shell/Sidebar.tsx?raw";

// App.tsx is too large to mount in a test, so the wiring is checked on the
// source: a missed opener leaves two views mounted at once.
const lines = app.split(/\r?\n/);
const indexesOf = (text: string) =>
  lines.flatMap((line, i) => (line.trim() === text ? [i] : []));

it("every site that closes Mail also closes Ticket Contributors", () => {
  const sites = indexesOf("setMailViewOpen(false);");
  expect(sites.length).toBeGreaterThan(15);
  const missing = sites.filter((i) => {
    // Mail's own close handler leaves everything else alone.
    if (
      lines
        .slice(i - 2, i)
        .join("\n")
        .includes("const onLeaveMail")
    )
      return false;
    const near = lines.slice(i - 6, i + 6).join("\n");
    // The back button closes one view per press and has its own test below.
    if (lines[i - 1].includes("if (mailViewOpen) {")) return false;
    // Ticket Contributors' own opener sets the flag right after.
    if (near.includes("setTicketContributorsOpen(true);")) return false;
    return !near.includes("setTicketContributorsOpen(false);");
  });
  expect(missing.map((i) => i + 1)).toEqual([]);
});

it("opening Ticket Contributors closes every other view", () => {
  const start = lines.findIndex((l) =>
    l.includes("const onOpenTicketContributors"),
  );
  expect(start).toBeGreaterThan(-1);
  const body = lines.slice(start, start + 18).join("\n");
  for (const setter of [
    "setFilePickerOpen",
    "setSettingsOpen",
    "setSearchViewOpen",
    "setInboxViewOpen",
    "setNotesViewOpen",
    "setAutomationsViewOpen",
    "setMailViewOpen",
  ]) {
    expect(body).toContain(`${setter}(false);`);
  }
  expect(body).toContain("setContributorsProject(null);");
  expect(body).toContain("setTicketContributorsOpen(true);");
  expect(app).toContain("onOpenTicketContributors={onOpenTicketContributors}");
});

it("every opener of another view closes Ticket Contributors", () => {
  for (const name of [
    "onOpenNotes",
    "onOpenAutomations",
    "onOpenSearch",
    "onOpenInbox",
    "onOpenMail",
    "onOpenContributors",
  ]) {
    const start = lines.findIndex((l) => l.includes(`const ${name} =`));
    expect(start, name).toBeGreaterThan(-1);
    const body = lines.slice(start, start + 20).join("\n");
    expect(body, name).toContain("setTicketContributorsOpen(false);");
  }
});

it("mounts TicketContributorsView only while open", () => {
  expect(app).toMatch(
    /\{ticketContributorsOpen \? \(\s*<TicketContributorsView/,
  );
  expect(app).toContain("onClose={onLeaveTicketContributors}");
  expect(app).toContain("onOpenSettings={onOpenClickUpSettings}");
});

it("opens Settings at the ClickUp section from the view", () => {
  expect(app).toMatch(
    /onOpenClickUpSettings = useCallback\(\s*\(\) => openSettings\("inbox", "clickup"\)/,
  );
});

it("Settings remembers and restores the open Ticket Contributors page", () => {
  expect(app).toContain(
    "ticketContributors: ticketContributorsOpenRef.current,",
  );
  expect(app).toContain(
    "setTicketContributorsOpen(returnView.ticketContributors);",
  );
});

it("the back button closes the Ticket Contributors page", () => {
  const start = lines.findIndex((l) => l.includes("const onRailBack"));
  const body = lines.slice(start, start + 50).join("\n");
  expect(body).toMatch(
    /if \(ticketContributorsOpen\) \{\s*setTicketContributorsOpen\(false\);\s*return;/,
  );
});

it("every list of open views that names Mail also names Ticket Contributors", () => {
  const flagLine = /^\s*!?mailViewOpen( \|\||,| &&|;)?$/;
  const refLine = /^\s*!?mailViewOpenRef\.current( \|\||( &&))?$/;
  const flagSites = lines.flatMap((line, i) =>
    flagLine.test(line) ? [i] : [],
  );
  const refSites = lines.flatMap((line, i) => (refLine.test(line) ? [i] : []));
  expect(flagSites.length).toBeGreaterThan(8);
  expect(refSites.length).toBeGreaterThan(3);
  const missingFlag = flagSites.filter(
    (i) =>
      !lines[i - 1].includes("ticketContributorsOpen") &&
      !lines[i + 1].includes("ticketContributorsOpen"),
  );
  expect(missingFlag.map((i) => i + 1)).toEqual([]);
  const missingRef = refSites.filter(
    (i) =>
      !lines[i - 1].includes("ticketContributorsOpenRef.current") &&
      !lines[i + 1].includes("ticketContributorsOpenRef.current"),
  );
  expect(missingRef.map((i) => i + 1)).toEqual([]);
});

it("the entry is in the icon row, the compact rail and the project rail, ungated", () => {
  expect(sidebar).toMatch(
    /\{onOpenTicketContributors \? \(\s*<IconButton\s+label="Ticket Contributors"/,
  );
  expect(sidebar).toMatch(
    /\{onOpenTicketContributors \? \(\s*<CompactRailAction\s+label="Ticket Contributors"/,
  );
  expect(projectRail).toMatch(
    /\{onOpenTicketContributors \? \(\s*<RailAction\s+label="Ticket Contributors"/,
  );
  // Unlike Mail, no setting hides it.
  expect(app).not.toContain("ticketContributorsEnabled");
});
