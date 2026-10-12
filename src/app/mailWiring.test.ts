import { expect, it } from "vitest";
import app from "./App.tsx?raw";

// App.tsx is too large to mount in a test, so the wiring is checked on the
// source: a missed opener leaves two views mounted at once.
const lines = app.split("\n");
const indexesOf = (text: string) =>
  lines.flatMap((line, i) => (line.trim() === text ? [i] : []));

it("every opener that closes Automations also closes Mail", () => {
  const sites = indexesOf("setAutomationsViewOpen(false);");
  expect(sites.length).toBeGreaterThan(15);
  const missing = sites.filter((i) => {
    const near = lines.slice(i - 6, i + 6).join("\n");
    // Exceptions: the view's own close handlers, and Mail's opener (below).
    if (
      lines
        .slice(i - 12, i)
        .join("\n")
        .includes("const onOpenMail")
    )
      return false;
    if (near.includes("const onLeaveAutomations")) return false;
    if (near.includes("if (automationsViewOpen) {")) return false;
    return !near.includes("setMailViewOpen(false);");
  });
  expect(missing.map((i) => i + 1)).toEqual([]);
});

it("opening Mail closes every other view and is gated on the flag", () => {
  const start = lines.findIndex((l) => l.includes("const onOpenMail"));
  const body = lines.slice(start, start + 16).join("\n");
  for (const setter of [
    "setFilePickerOpen",
    "setSettingsOpen",
    "setSearchViewOpen",
    "setInboxViewOpen",
    "setNotesViewOpen",
    "setAutomationsViewOpen",
  ]) {
    expect(body).toContain(`${setter}(false);`);
  }
  expect(body).toContain("if (!loadMailEnabled()) return;");
  expect(app).toContain("onOpenMail={mailEnabled ? onOpenMail : undefined}");
});

it("every opener of another view closes Mail", () => {
  for (const name of [
    "onOpenNotes",
    "onOpenAutomations",
    "onOpenSearch",
    "onOpenInbox",
  ]) {
    const start = lines.findIndex((l) => l.includes(`const ${name} =`));
    expect(start, name).toBeGreaterThan(-1);
    const body = lines.slice(start, start + 14).join("\n");
    expect(body, name).toContain("setMailViewOpen(false);");
  }
});

it("mounts MailView only while open", () => {
  expect(app).toMatch(/\{mailViewOpen \? \(\s*<MailView/);
});

it("feeds the rail's unread badge, and only while Mail is enabled", () => {
  expect(app).toContain("useMailUnread(mailEnabled)");
  expect(app).toContain("mailUnread={mailUnread}");
});
