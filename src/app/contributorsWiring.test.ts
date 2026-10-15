import { expect, it } from "vitest";
import app from "./App.tsx?raw";

// App.tsx is too large to mount in a test, so the wiring is checked on the
// source: a missed opener leaves two views mounted at once.
const lines = app.split("\n");
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
