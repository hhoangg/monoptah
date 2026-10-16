import { expect, it } from "vitest";
import app from "./App.tsx?raw";
import footer from "./shell/UsageFooter.tsx?raw";

// App.tsx is too large to mount in a test, so the wiring is checked on the
// source: a missed opener leaves two views mounted at once.
const lines = app.split(/\r?\n/);
const indexesOf = (text: string) =>
  lines.flatMap((line, i) => (line.trim() === text ? [i] : []));

it("every site that closes Mail also closes Contributors", () => {
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
    // Contributors' own opener sets the project right after.
    if (near.includes("setContributorsProject(path);")) return false;
    return !near.includes("setContributorsProject(null);");
  });
  expect(missing.map((i) => i + 1)).toEqual([]);
});

it("opening Contributors closes every other view and rejects remote projects", () => {
  const start = lines.findIndex((l) => l.includes("const onOpenContributors"));
  expect(start).toBeGreaterThan(-1);
  const body = lines.slice(start, start + 28).join("\n");
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
  expect(body).toContain("isRemoteProjectPath(path)");
  expect(body).toContain("setContributorsProject(path);");
  expect(app).toContain("onOpenContributors={onOpenContributors}");
});

it("every opener of another view closes Contributors", () => {
  for (const name of [
    "onOpenNotes",
    "onOpenAutomations",
    "onOpenSearch",
    "onOpenInbox",
    "onOpenMail",
  ]) {
    const start = lines.findIndex((l) => l.includes(`const ${name} =`));
    expect(start, name).toBeGreaterThan(-1);
    const body = lines.slice(start, start + 16).join("\n");
    expect(body, name).toContain("setContributorsProject(null);");
  }
});

it("mounts ContributorsView only while open, per project", () => {
  expect(app).toMatch(
    /\{contributorsProject !== null \? \(\s*<ContributorsView\s+key=\{contributorsProject\}\s+project=\{contributorsProject\}/,
  );
  expect(app).toContain("onClose={onLeaveContributors}");
  expect(app).toContain("ContributorsView.preload");
});

it("Settings remembers and restores the open Contributors page", () => {
  expect(app).toContain("contributors: contributorsProjectRef.current,");
  expect(app).toContain("const contributors = returnView.contributors;");
});

it("the back button closes the Contributors page", () => {
  const start = lines.findIndex((l) => l.includes("const onRailBack"));
  const body = lines.slice(start, start + 50).join("\n");
  expect(body).toMatch(
    /if \(contributorsViewOpen\) \{\s*setContributorsProject\(null\);\s*return;/,
  );
});

it("every list of open views that names Mail also names Contributors", () => {
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
      !lines[i].includes("contributors") &&
      !lines[i + 1].includes("contributorsViewOpen"),
  );
  expect(missingFlag.map((i) => i + 1)).toEqual([]);
  const missingRef = refSites.filter(
    (i) => !lines[i + 1].includes("contributorsProjectRef.current"),
  );
  expect(missingRef.map((i) => i + 1)).toEqual([]);
});

it("opening Contributors leaves workspace selection alone", () => {
  const start = lines.findIndex((l) => l.includes("const onOpenContributors"));
  const end = lines.findIndex(
    (l, i) => i > start && l.includes("const onLeaveContributors"),
  );
  const body = lines.slice(start, end).join("\n");
  for (const forbidden of [
    "onSelectProject",
    "setProjectCwd",
    "rememberProject",
    "setRecents",
    "setActiveTabId",
    "appendTab",
  ]) {
    expect(body, forbidden).not.toContain(forbidden);
  }
});

it("removing a project closes its Contributors page and its Settings return", () => {
  const start = lines.findIndex((l) => l.includes("const onRemoveProject"));
  const body = lines.slice(start, start + 40).join("\n");
  expect(body).toMatch(
    /contributorsProjectRef\.current !== null &&\s*sameProjectPath\(contributorsProjectRef\.current, normalized\)\s*\) \{\s*setContributorsProject\(null\);/,
  );
  expect(body).toMatch(
    /sameProjectPath\(returnContributors, normalized\)\s*\) \{\s*settingsReturnViewRef\.current\.contributors = null;/,
  );
});

it("closing Settings only restores a Contributors project that still exists", () => {
  const start = lines.findIndex((l) => l.includes("const onCloseSettings"));
  const body = lines.slice(start, start + 20).join("\n");
  expect(body).toContain("isKnownProject(contributors, projectCwdRef.current)");
  expect(body).not.toContain("setContributorsProject(returnView.contributors)");
});

it("starting a Contributors session creates a chat, submits and closes the page", () => {
  const start = lines.findIndex((l) =>
    l.includes("const onStartContributorsSession"),
  );
  expect(start).toBeGreaterThan(-1);
  const end = lines.findIndex(
    (l, i) => i > start && l.includes("const onOpenNotes"),
  );
  const body = lines.slice(start, end).join("\n");
  // A submitted prompt needs a chat, never a terminal session.
  expect(body).toContain('surface: "chat"');
  expect(body.match(/surface: "chat"/g)).toHaveLength(2);
  expect(body).toContain("isRemoteProjectPath(project)");
  expect(body).toContain('"write_attachment"');
  expect(body).toContain("utf8ToBase64(request.snapshotJson)");
  expect(body).toContain("attachmentsFromPaths([path])");
  expect(body).toContain("harnessSupportsAttachments(base.harness)");
  // The file is written before any session exists, so a failure leaves none.
  expect(body.indexOf("write_attachment")).toBeLessThan(
    body.indexOf("setSessions(withSession)"),
  );
  // A refused submit is an error and removes the session again.
  expect(body).toMatch(
    /if \(!onSubmit\(session\.id, prompt, attachments\)\) \{[\s\S]*?setSessions\(without\);[\s\S]*?throw new Error\(/,
  );
  // Only an accepted submit closes the page and opens the session.
  const submit = body.indexOf("onSubmit(session.id");
  expect(body.indexOf("setContributorsProject(null);")).toBeGreaterThan(submit);
  expect(body).toContain('setSidebarTab("sessions", project);');
  expect(body).toContain("await onSelectHistorySession(session.id);");
});

it("hands the Contributors page the session starter for its own project", () => {
  expect(app).toContain("onStartAgentSession={(request) =>");
  expect(app).toContain(
    "onStartContributorsSession(contributorsProject, request)",
  );
});

it("starts the Contributors session on the project's model for that provider", () => {
  const start = lines.findIndex((l) =>
    l.includes("const onStartContributorsSession"),
  );
  const end = lines.findIndex(
    (l, i) => i > start && l.includes("const onOpenNotes"),
  );
  const body = lines.slice(start, end).join("\n");
  expect(body).toMatch(
    /newSession\(\s*harness,\s*project,\s*sessionModelForHarness\(harness, project\),\s*runtimeMode,\s*undefined,\s*\{\s*surface: "chat"\s*\},?\s*\)/,
  );
  expect(app).toMatch(
    /import \{[^}]*\bsessionModelForHarness\b[^}]*\} from "\.\.\/features\/sessions\/model\/models";/,
  );
});

it("a running agent turn is never stopped by Escape while the Contributors page is open", () => {
  const start = lines.findIndex((l) => l.includes("const onEscape = ("));
  expect(start).toBeGreaterThan(-1);
  const body = lines.slice(start, start + 12).join("\n");
  expect(body).toContain("contributorsProjectRef.current !== null");
});

it("a failed navigation after an accepted submit is not reported as a failed start", () => {
  const start = lines.findIndex((l) =>
    l.includes("const onStartContributorsSession"),
  );
  const end = lines.findIndex(
    (l, i) => i > start && l.includes("const onOpenNotes"),
  );
  const body = lines.slice(start, end).join("\n");
  expect(body).toMatch(
    /try \{\s*await onSelectHistorySession\(session\.id\);\s*\} catch \{/,
  );
});

it("selecting a session from the sidebar closes the Contributors page", () => {
  // The session sidebar stays visible beside Contributors (it is hidden for
  // Notes, Mail, Search, Inbox and Settings), so its click has to close the
  // page itself or the chosen session opens underneath it.
  const start = lines.findIndex((l) =>
    l.includes("const onSelectHistorySession"),
  );
  expect(start).toBeGreaterThan(-1);
  const body = lines.slice(start, start + 12).join("\n");
  // After the load guard, so a session that cannot open leaves the page alone,
  // and before the first branch that returns with the session already shown.
  expect(body).toMatch(
    /if \(!session \|\| session\.inboxAsk\) return;\s*setContributorsProject\(null\);/,
  );
});

it("switching workspace from the sidebar closes the Contributors page", () => {
  const start = lines.findIndex((l) => l.includes("const onSelectWorkspace"));
  expect(start).toBeGreaterThan(-1);
  const body = lines.slice(start, start + 8).join("\n");
  expect(body).toContain("setContributorsProject(null);");
});

it("the title-bar tabs are inert while Contributors is open", () => {
  // activateTab does not close Contributors, so a tab click would open its
  // session underneath the page. It cannot happen: the workspace, title bar
  // included, sits in the wrapper that is hidden and inert for the page.
  const wrapper = lines.findIndex((l) => l.includes('? "hidden"'));
  expect(wrapper).toBeGreaterThan(-1);
  const head = lines.slice(wrapper - 12, wrapper + 25).join("\n");
  expect(head.match(/contributorsViewOpen/g)?.length).toBeGreaterThanOrEqual(3);
  const titleBar = lines.findIndex((l) =>
    l.includes("{compactTitleBar ? null : workspaceTitleBar}"),
  );
  expect(titleBar).toBeGreaterThan(wrapper);
  expect(app).toContain("IS_MAC && compactRailActive && !chromeSurfaceOpen");
  expect(app).toMatch(/const chromeSurfaceOpen =[^;]*contributorsViewOpen;/);
});

// The one-shot collector was replaced by the sync store. Built from parts so
// this file does not match itself.
const RETIRED = ["contributors", "collect"].join("_");
const RETIRED_FN = ["collect", "Raw"].join("");

it("the retired one-shot collect command is not invoked or registered anywhere", async () => {
  const sources = import.meta.glob("../features/contributors/**/*.{ts,tsx}", {
    query: "?raw",
    import: "default",
  });
  const hits: string[] = [];
  for (const [path, load] of Object.entries(sources)) {
    if (path.endsWith("contributorsWiring.test.ts")) continue;
    const text = (await load()) as string;
    if (text.includes(RETIRED) || text.includes(RETIRED_FN)) hits.push(path);
  }
  expect(hits).toEqual([]);
  expect(app).not.toContain(RETIRED);
  const lib = (await import("../../src-tauri/src/lib.rs?raw")).default;
  expect(lib).not.toContain(RETIRED);
  expect(lib).toContain("contributors::contributors_load");
  expect(lib).toContain("contributors::contributors_sync_start");
});

it("the footer sync indicator sits before the Terminal item and opens Contributors", () => {
  const group = footer.indexOf("ml-auto flex shrink-0 items-center gap-2");
  expect(group).toBeGreaterThan(-1);
  const indicator = footer.indexOf("<SyncIndicator", group);
  const chip = footer.indexOf("<RunningTerminalChip", group);
  expect(indicator).toBeGreaterThan(-1);
  expect(indicator).toBeLessThan(chip);
  const start = lines.findIndex((l) => l.includes("<UsageFooter"));
  expect(start).toBeGreaterThan(-1);
  expect(lines.slice(start, start + 30).join("\n")).toContain(
    "onOpenContributors={onOpenContributors}",
  );
});
