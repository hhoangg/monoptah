import { beforeEach, describe, expect, it } from "vitest";
import type { FolderCard, SessionCard } from "@monoptah/protocol";
import { FOLDER_COLLAPSE_KEY, canEditFolders, folderColor, isRemoteProject, folderOf, folderedIds, groupSessions, isCollapsed, matchesQuery, projectFolders } from "./folders";

const session = (id: string, title = id): SessionCard => ({ id, projectId: "p", title, harness: "claude", model: "m", branch: "main", updatedAt: 1 });
const folder = (id: string, sessionIds: string[], extra: Partial<FolderCard> = {}): FolderCard => ({ id, projectId: "p", name: id, sessionIds, ...extra });

beforeEach(() => localStorage.clear());

describe("folder grouping", () => {
  it("keeps the desktop's folder and member order, drops unknown members and empty folders", () => {
    const known = new Map([session("a"), session("b"), session("c")].map((s) => [s.id, s] as const));
    const groups = groupSessions([folder("f1", ["c", "gone", "a"]), folder("f2", ["gone"]), folder("f3", ["b"])], known, () => true);
    expect(groups.map((g) => [g.folder.id, g.sessions.map((s) => s.id)])).toEqual([
      ["f1", ["c", "a"]],
      ["f3", ["b"]],
    ]);
  });

  it("leaves out the members a filter rejects, and the folder when none is left", () => {
    const known = new Map([session("a", "Alpha"), session("b", "Beta")].map((s) => [s.id, s] as const));
    const groups = groupSessions([folder("f1", ["a", "b"]), folder("f2", ["b"])], known, (s) => matchesQuery(s, "alp"));
    expect(groups.map((g) => [g.folder.id, g.sessions.map((s) => s.id)])).toEqual([["f1", ["a"]]]);
  });

  it("finds a project's folders and the folder of a session", () => {
    const all = [folder("f1", ["a"]), { ...folder("f2", ["b"]), projectId: "q" }];
    expect(projectFolders(all, "p").map((f) => f.id)).toEqual(["f1"]);
    expect(projectFolders(undefined, "p")).toEqual([]);
    expect(folderOf(all, "b")?.id).toBe("f2");
    expect(folderOf(all, "zz")).toBeUndefined();
    expect([...folderedIds(all)].sort()).toEqual(["a", "b"]);
  });

  it("matches a query on title, model and branch like sessions.list", () => {
    expect(matchesQuery(session("a", "Fix the parser"), "PARSER")).toBe(true);
    expect(matchesQuery(session("a"), "main")).toBe(true);
    expect(matchesQuery(session("a"), "nope")).toBe(false);
    expect(matchesQuery(session("a"), "  ")).toBe(true);
  });

  it("colors a folder from the desktop palette or a valid custom hex only", () => {
    expect(folderColor({})).toBeUndefined();
    expect(folderColor({ colorIndex: 4 })).toBe("hsl(142 55% 50%)");
    expect(folderColor({ colorIndex: 99 })).toBeUndefined();
    expect(folderColor({ colorIndex: 4, customColor: "#aabbcc" })).toBe("#aabbcc");
    expect(folderColor({ customColor: "red" })).toBeUndefined();
  });
});

describe("collapse state", () => {
  it("starts from the desktop's state, follows this device's choice, and yields when the desktop changes it", () => {
    const open = { id: "f", collapsed: false };
    expect(isCollapsed(open, {})).toBe(false);
    expect(isCollapsed({ id: "f", collapsed: true }, {})).toBe(true);
    expect(isCollapsed({ id: "f" }, {})).toBe(false);
    // This device folded it while the desktop had it open.
    const choices = { f: { desktop: false, local: true } };
    expect(isCollapsed(open, choices)).toBe(true);
    // The desktop folded it too, then unfolded it: its state wins again.
    expect(isCollapsed({ id: "f", collapsed: true }, choices)).toBe(true);
    expect(isCollapsed({ id: "f", collapsed: false }, { f: { desktop: true, local: false } })).toBe(false);
    expect(FOLDER_COLLAPSE_KEY.startsWith("monoptah.")).toBe(true);
  });
});

describe("who may edit folders", () => {
  it("needs a desktop that sends folders and a local project", () => {
    expect(canEditFolders([], "%2Fwork%2Fapp")).toBe(true);
    expect(canEditFolders(undefined, "%2Fwork%2Fapp")).toBe(false);
    expect(canEditFolders([], "remote-0123456789abcdef")).toBe(false);
    expect(canEditFolders([], "remote%3A%2F%2Fbox%2Fapp")).toBe(false);
    expect(isRemoteProject("REMOTE%3a%2f%2fbox")).toBe(true);
    expect(isRemoteProject("%2Fwork%2Fremote-app")).toBe(false);
    expect(isRemoteProject("%E0%A4%A")).toBe(false);
  });
});
