import { describe, expect, it } from "vitest";
import {
  buildSessionList,
  ungroupedSessions,
  type SessionFolder,
} from "../../features/sessions/model/sessionFolders";
import type { SessionSummary } from "../../features/sessions/data/sessionStore";
import { groupStartedSessions, startedSessionCaller } from "./Sidebar";

const rows = (...ids: string[]) => ids.map((id) => ({ id }));
const summary = (id: string) => ({ id, updatedAt: 1 }) as SessionSummary;
const folder = (sessionIds: string[]): SessionFolder => ({
  id: "ticket",
  name: "Ticket",
  sessionIds,
  collapsed: false,
});

// Mirrors Sidebar: started sessions are filtered out of the list that
// folders and the loose list are built from, then hung under their caller.
function sidebarList(ids: string[], folders: SessionFolder[]) {
  const visible = ids.map(summary);
  const started = groupStartedSessions(visible);
  const listed = visible.filter((session) => !started.childIds.has(session.id));
  const entries = buildSessionList(
    listed,
    folders,
    ungroupedSessions(listed, folders),
  );
  return { started, entries };
}
const listedIds = (entries: ReturnType<typeof buildSessionList>) =>
  entries.flatMap((entry) =>
    entry.kind === "session"
      ? [entry.session.id]
      : entry.sessions.map((session) => session.id),
  );

describe("groupStartedSessions", () => {
  it("hangs a session started through the app CLI under its caller", () => {
    const { byParent, childIds } = groupStartedSessions(
      rows(
        "4f1c-9a2b",
        "app-4f1c-9a2b-review-ui",
        "app-4f1c-9a2b-review_api-2",
        "other",
      ),
    );
    expect(byParent.get("4f1c-9a2b")?.map((row) => row.id)).toEqual([
      "app-4f1c-9a2b-review-ui",
      "app-4f1c-9a2b-review_api-2",
    ]);
    expect([...childIds].sort()).toEqual([
      "app-4f1c-9a2b-review-ui",
      "app-4f1c-9a2b-review_api-2",
    ]);
  });

  it("joins a launch's own launches to the top-level session", () => {
    const { byParent } = groupStartedSessions(
      rows("lead", "app-lead-a", "app-app-lead-a-b"),
    );
    expect(byParent.get("lead")?.map((row) => row.id)).toEqual([
      "app-lead-a",
      "app-app-lead-a-b",
    ]);
    expect(byParent.has("app-lead-a")).toBe(false);
  });

  it("leaves a launch whose caller is not listed where it is", () => {
    const { byParent, childIds } = groupStartedSessions(
      rows("app-mono-session-1-task", "plain"),
    );
    expect(byParent.size).toBe(0);
    expect(childIds.size).toBe(0);
  });

  it("finds a launch's caller only among the given sessions", () => {
    const ids = new Set(["ab3c-98dc", "plain"]);
    expect(startedSessionCaller("app-ab3c-98dc-3a64-6702", ids)).toBe(
      "ab3c-98dc",
    );
    expect(startedSessionCaller("app-mono-1-task", ids)).toBeUndefined();
    expect(startedSessionCaller("plain", ids)).toBeUndefined();
  });

  it("nests a reviewer started under a dev when the Mono's chat is not listed", () => {
    const { byParent, childIds } = groupStartedSessions(
      rows("app-mono-1-dev", "app-app-mono-1-dev-rev"),
    );
    expect(byParent.get("app-mono-1-dev")?.map((row) => row.id)).toEqual([
      "app-app-mono-1-dev-rev",
    ]);
    expect([...childIds]).toEqual(["app-app-mono-1-dev-rev"]);
  });

  it("lists the reviewer once, under the dev, when both are in the same folder", () => {
    const dev = "app-mono-1-dev";
    const rev = "app-app-mono-1-dev-rev";
    const { started, entries } = sidebarList([dev, rev], [folder([dev, rev])]);
    expect(entries.map((entry) => entry.kind)).toEqual(["folder"]);
    expect(listedIds(entries)).toEqual([dev]);
    expect(started.byParent.get(dev)?.map((row) => row.id)).toEqual([rev]);
  });

  it("lists the reviewer once, under the dev, when only the dev is in the folder", () => {
    const dev = "app-mono-1-dev";
    const rev = "app-app-mono-1-dev-rev";
    const { started, entries } = sidebarList([dev, rev], [folder([dev])]);
    expect(listedIds(entries)).toEqual([dev]);
    expect(started.byParent.get(dev)?.map((row) => row.id)).toEqual([rev]);
  });
});
