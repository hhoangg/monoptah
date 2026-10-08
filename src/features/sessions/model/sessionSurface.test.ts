// @vitest-environment happy-dom
import { beforeEach, describe, expect, it } from "vitest";
import { saveProviderSurface } from "../../providers/model/providerSurface";
import { sessionInWorktree } from "../../source-control/model/worktrees";
import {
  newContinuationSession,
  newDefaultSession,
  newHandoffSession,
  newNoteSession,
  newSession,
} from "./session";

const tree = {
  path: "/repo-worktrees/feature",
  branch: "feature",
  head: "abc",
  isMain: false,
  locked: false,
  prunable: false,
  missing: false,
  dirty: false,
  unpushed: 0,
  sessionIds: [],
};

beforeEach(() => {
  localStorage.clear();
});

describe("new session surface", () => {
  it("is chat when the provider has no preference", () => {
    expect(newSession("claude", "/repo").surface).toBe("chat");
    expect(newDefaultSession("/repo").surface).toBe("chat");
  });

  it("inherits the provider's preference when the caller does not pin one", () => {
    saveProviderSurface("claude", "tui");
    expect(newSession("claude", "/repo").surface).toBe("tui");
    expect(newSession("codex", "/repo").surface).toBe("chat");
  });

  it("lets a caller pin chat over a terminal preference", () => {
    saveProviderSurface("claude", "tui");
    expect(
      newSession("claude", "/repo", undefined, undefined, undefined, {
        surface: "chat",
      }).surface,
    ).toBe("chat");
  });

  it("carries the pinned surface through newDefaultSession", () => {
    saveProviderSurface(newDefaultSession("/repo").harness, "tui");
    const pinned = newDefaultSession("/repo", undefined, { surface: "chat" });
    expect(pinned.surface).toBe("chat");
    expect(newDefaultSession("/repo").surface).toBe("tui");
  });

  it("never starts a terminal for a project on another machine", () => {
    saveProviderSurface("claude", "tui");
    expect(newSession("claude", "remote://env/repo").surface).toBe("chat");
  });

  it("keeps a project on another machine in chat when a terminal is requested", () => {
    const session = newSession(
      "claude",
      "remote://env/repo",
      undefined,
      undefined,
      undefined,
      { surface: "tui" },
    );
    expect(session.surface).toBe("chat");
  });
});

describe("sessions that carry a composer card", () => {
  it("opens a note in chat even when the preference is a terminal", () => {
    const harness = newDefaultSession("/repo").harness;
    saveProviderSurface(harness, "tui");
    const card = { id: "n1", slug: "n", title: " Plan ", body: "text" };
    const session = newNoteSession(card, "/repo");
    expect(session.surface).toBe("chat");
    expect(session.noteCard).toBe(card);
    expect(session.title).toBe("Plan");
    // Ordinary new sessions still honour the preference.
    expect(newDefaultSession("/repo").surface).toBe("tui");
  });

  it("opens a handoff in chat even when the preference is a terminal", () => {
    saveProviderSurface("codex", "tui");
    const card = { from: "claude" as const, to: "codex" as const, brief: "b" };
    const session = newHandoffSession(
      "codex",
      "/repo",
      "gpt-5",
      "supervised",
      card,
    );
    expect(session.surface).toBe("chat");
    expect(session.handoffCard).toBe(card);
    expect(newSession("codex", "/repo").surface).toBe("tui");
  });
});

describe("sessions that continue another session", () => {
  it("keeps a terminal session's surface after the preference goes back to chat", () => {
    saveProviderSurface("claude", "tui");
    const source = newSession("claude", "/repo");
    expect(source.surface).toBe("tui");
    saveProviderSurface("claude", "chat");
    const next = newContinuationSession(source);
    expect(next.surface).toBe("tui");
    expect(next.id).not.toBe(source.id);
    expect(next.harness).toBe(source.harness);
    expect(next.model).toBe(source.model);
    expect(next.cwd).toBe(source.cwd);
  });

  it("keeps a chat session in chat after the preference becomes a terminal", () => {
    const source = newSession("claude", "/repo");
    saveProviderSurface("claude", "tui");
    expect(newContinuationSession(source).surface).toBe("chat");
    const { surface: _surface, ...legacy } = source;
    expect(newContinuationSession(legacy).surface).toBe("chat");
  });

  it("keeps the surface when a session moves to another worktree", () => {
    saveProviderSurface("claude", "tui");
    const source = {
      ...newSession("claude", "/repo"),
      blocks: [{ id: "u", role: "user" as const, text: "Build" }],
    };
    saveProviderSurface("claude", "chat");
    const moved = sessionInWorktree(source, tree);
    expect(moved.id).not.toBe(source.id);
    expect(moved.surface).toBe("tui");
  });

  it("never carries a terminal surface into a remote project", () => {
    const source = {
      ...newSession("claude", "/repo"),
      surface: "tui" as const,
    };
    expect(
      newContinuationSession({ ...source, cwd: "remote://env/repo" }).surface,
    ).toBe("chat");
  });
});
