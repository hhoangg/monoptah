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
    // Contributors' own opener sets the view right after.
    if (near.includes("setContributorsView(next);")) return false;
    return !near.includes("setContributorsView(null);");
  });
  expect(missing.map((i) => i + 1)).toEqual([]);
});

it("opening Contributors closes every other view", () => {
  const start = lines.findIndex((l) => l.includes("const openContributors"));
  expect(start).toBeGreaterThan(-1);
  const body = lines.slice(start, start + 20).join("\n");
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
  expect(body).toContain("setContributorsView(next);");
});

it("the sidebar opens Contributors on the Ticket tab and keeps an open page as it is", () => {
  const body = handlerBody("onOpenContributors");
  expect(body).toContain(
    '(current) => current ?? { tab: "ticket", project: null }',
  );
  expect(app).toContain("onOpenContributors={onOpenContributors}");
  expect(app).toContain("contributorsActive={contributorsViewOpen}");
});

it("opening a project's Contributors shows its GitHub tab and rejects remote projects", () => {
  const body = handlerBody("onOpenProjectContributors");
  expect(body).toContain("isRemoteProjectPath(path)");
  expect(body).toContain('({ tab: "github", project: path })');
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
    expect(body, name).toContain("setContributorsView(null);");
  }
});

it("mounts ContributorsPage only while open, with its tab and project", () => {
  expect(app).toMatch(
    /\{contributorsView !== null \? \(\s*<ContributorsPage\s+tabbed=\{insetLayout\}\s+tab=\{contributorsView\.tab\}/,
  );
  expect(app).toContain("project={contributorsView.project}");
  expect(app).toContain("onClose={onLeaveContributors}");
  expect(app).toContain("onOpenSettings={onOpenClickUpSettings}");
  expect(app).toContain("ContributorsPage.preload");
  // Merged into one page: neither old view is mounted from App any more.
  expect(app).not.toMatch(/<ContributorsView[\s>]/);
  expect(app).not.toMatch(/<TicketContributorsView[\s>]/);
});

it("Settings remembers and restores the open Contributors page", () => {
  expect(app).toContain("contributors: contributorsViewRef.current,");
  expect(app).toContain("const contributors = returnView.contributors;");
});

it("the back button closes the Contributors page", () => {
  const start = lines.findIndex((l) => l.includes("const onRailBack"));
  const body = lines.slice(start, start + 50).join("\n");
  expect(body).toMatch(
    /if \(contributorsViewOpen\) \{\s*setContributorsView\(null\);\s*return;/,
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
    (i) => !lines[i + 1].includes("contributorsViewRef.current"),
  );
  expect(missingRef.map((i) => i + 1)).toEqual([]);
});

it("opening Contributors leaves workspace selection alone", () => {
  const start = lines.findIndex((l) => l.includes("const openContributors"));
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

it("removing a project unchooses it on the Contributors page and its Settings return", () => {
  const start = lines.findIndex((l) => l.includes("const onRemoveProject"));
  const body = lines.slice(start, start + 40).join("\n");
  expect(body).toMatch(
    /setContributorsView\(\(view\) =>\s*view\?\.project && sameProjectPath\(view\.project, normalized\)\s*\? \{ \.\.\.view, project: null \}\s*: view,/,
  );
  expect(body).toMatch(
    /sameProjectPath\(returnContributors\.project, normalized\)\s*\) \{\s*settingsReturnViewRef\.current\.contributors = \{\s*\.\.\.returnContributors,\s*project: null,/,
  );
});

it("closing Settings only restores a Contributors project that still exists", () => {
  const start = lines.findIndex((l) => l.includes("const onCloseSettings"));
  const body = lines.slice(start, start + 20).join("\n");
  expect(body).toContain(
    "!isKnownProject(contributors.project, projectCwdRef.current)",
  );
  expect(body).not.toContain("setContributorsView(returnView.contributors)");
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
  expect(body.indexOf("setContributorsView(null);")).toBeGreaterThan(submit);
  expect(body).toContain('setSidebarTab("sessions", project);');
  expect(body).toContain("await onSelectHistorySession(session.id);");
});

it("hands the Contributors page the session starter, which gets the page's project", () => {
  expect(app).toContain("onStartAgentSession={onStartContributorsSession}");
  expect(handlerBody("onStartContributorsSession")).toContain(
    "async (project: string, request: AskAgentRequest)",
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
  expect(body).toContain("contributorsViewRef.current !== null");
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
    /if \(!session \|\| session\.inboxAsk\) return;\s*setContributorsView\(null\);/,
  );
});

it("switching workspace from the sidebar closes the Contributors page", () => {
  const start = lines.findIndex((l) => l.includes("const onSelectWorkspace"));
  expect(start).toBeGreaterThan(-1);
  const body = lines.slice(start, start + 8).join("\n");
  expect(body).toContain("setContributorsView(null);");
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
    l.includes("{titleBarInBody ? workspaceTitleBar : null}"),
  );
  expect(titleBar).toBeGreaterThan(wrapper);
  expect(app).toContain("IS_MAC && compactRailActive && !chromeSurfaceOpen");
  // The inset layout keeps its tabs across the whole window, outside that
  // wrapper, so picking one of them has to close Contributors itself.
  const select = lines.findIndex((l) =>
    l.includes("const onSelectTitleTab = useCallback("),
  );
  expect(select).toBeGreaterThan(-1);
  expect(lines.slice(select, select + 40).join("\n")).toContain(
    "setContributorsView(null);",
  );
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
    "onOpenContributors={onOpenProjectContributors}",
  );
});

// A handler's source: from its `const name =` line to the next top-level
// `const` of the component body.
const handlerBody = (name: string) => {
  const start = lines.findIndex((l) => l.startsWith(`  const ${name} =`));
  expect(start, name).toBeGreaterThan(-1);
  const end = lines.findIndex((l, i) => i > start && /^ {2}const \w/.test(l));
  return lines.slice(start, end).join("\n");
};

it("opening a file from the sidebar closes the Contributors page", () => {
  // The Files tab stays visible beside the page, so an open that did not close
  // it would land in a workspace the page is hiding. It closes after the tab
  // guard, so an open that cannot proceed leaves the page alone.
  expect(handlerBody("onOpenFile")).toMatch(
    /if \(!tab\) return;\s*setContributorsView\(null\);/,
  );
});

it("opening a terminal from the sidebar closes the Contributors page", () => {
  // After the remote-project guard: a terminal that cannot open keeps the page.
  expect(handlerBody("onOpenTerminal")).toMatch(
    /isLocalProject\(workdir\)\)\s*return;\s*setContributorsView\(null\);/,
  );
});

it("opening a diff from the sidebar closes the Contributors page", () => {
  // After the path is resolved, so a failed lookup leaves the page alone.
  expect(handlerBody("onOpenDiff")).toMatch(
    /if \(resolved\) rememberOpenedFile\(diffCwd, resolved\);\s*setContributorsView\(null\);\s*setTabs\(/,
  );
});

it("opening all changes or a commit from the sidebar closes the Contributors page", () => {
  for (const name of ["onOpenAllChanges", "onOpenCommit"]) {
    expect(handlerBody(name), name).toMatch(
      /\) => \{\s*setContributorsView\(null\);\s*setTabs\(/,
    );
  }
});

it("the footer, project terminal dock and menu bar are not reachable while Contributors is open", () => {
  // Why onNewTerminal, onShowTerminal and onSelectProjectTerminal need no
  // close of their own: the footer is not rendered for the page, and the dock
  // and menu bar sit in the inert wrapper.
  const footerGate = lines.findIndex((l) => l.includes("const usageFooter ="));
  expect(footerGate).toBeGreaterThan(-1);
  expect(lines.slice(footerGate + 1, footerGate + 3).join("\n")).toContain(
    "chromeSurfaceOpen",
  );
  expect(lines.slice(footerGate, footerGate + 8).join("\n")).toContain(
    "<UsageFooter",
  );
});

// The new layout draws a tab strip, a Mono tab and a right panel around the
// workspace. The two fork views have no tab, so each of those has to treat
// them like upstream's app views.
const bodyAfter = (marker: string, length: number) => {
  const start = lines.findIndex((l) => l.includes(marker));
  expect(start, marker).toBeGreaterThan(-1);
  return lines.slice(start, start + length).join("\n");
};

it("opening Mail or Contributors puts a Mono's view away", () => {
  for (const name of ["onOpenMail", "openContributors"]) {
    expect(bodyAfter(`const ${name} =`, 14), name).toContain(
      "closeMonoView();",
    );
  }
});

it("closing the visible tab closes an open fork view before a Mono or app view", () => {
  const body = bodyAfter("const onCloseVisibleTab = useCallback", 14);
  const fork = body.indexOf("forkViewOpen");
  expect(fork).toBeGreaterThan(-1);
  expect(fork).toBeLessThan(body.indexOf("monoCovers"));
  for (const close of [
    "setMailViewOpen(false);",
    "setContributorsView(null);",
  ]) {
    expect(body).toContain(close);
  }
});

it("the right panel and Back treat an open fork view like an app view", () => {
  expect(app).toContain(
    "const forkViewOpen = mailViewOpen || contributorsViewOpen;",
  );
  expect(bodyAfter("const insetRightPanelShown =", 8)).toContain(
    "!forkViewOpen",
  );
  expect(bodyAfter("rightPanelToggleRef.current =", 6)).toContain(
    "!forkViewOpen",
  );
  expect(app).toContain(
    "rightPanelUnavailable={!!activeAppView || forkViewOpen}",
  );
  expect(app).toMatch(
    /canGoBack=\{!!monoViewId \|\| tabVisitNav\.canBack \|\| forkViewOpen\}/,
  );
});

it("the fork views drop their own header in the new layout", () => {
  for (const tag of ["MailView", "ContributorsPage"]) {
    const start = lines.findIndex((l) => l.includes(`<${tag}`));
    expect(start, tag).toBeGreaterThan(-1);
    expect(lines.slice(start, start + 6).join("\n"), tag).toContain(
      "tabbed={insetLayout}",
    );
  }
});
