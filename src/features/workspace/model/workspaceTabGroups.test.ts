import { describe, expect, it } from "vitest";
import {
  leafIds,
  newFileTab,
  newTerminalFile,
  newTab,
  splitPane,
  type WorkspaceTab,
} from "./layout";
import { planProjectReturn } from "../../projects/model/projectReturn";
import type { Session } from "../../sessions/model/session";
import {
  applyDetachPaneToTab,
  applyPlaceTabOnPane,
  applyPlaceSessionOnPane,
  filterTabsForProject,
  findOpenSessionTab,
  findTabForProject,
  openAddToChatSessionPane,
  planSessionPaneClose,
  planWorkspaceTabClose,
  ptyIdsOfPanes,
  replaceGroupInTabOrder,
  switchSessionInTab,
  workspaceTabProject,
  focusedWorkspaceTabCwd,
  workspaceTabCwd,
  tabInWorktree,
  workspaceTabWorktree,
} from "./workspaceTabGroups";

function session(id: string, cwd: string): Session {
  return {
    id,
    cwd,
    harness: "cursor",
    title: "",
    blocks: [],
    busy: false,
    model: "",
  };
}

function tab(id: string, sessionId: string): WorkspaceTab {
  return { ...newTab(sessionId), id };
}

function tuiSession(id: string): Session {
  return { ...session(id, "/repo"), surface: "tui" };
}

describe("focusedWorkspaceTabCwd", () => {
  it.each(["editor", "terminal"] as const)(
    "keeps a worktree-only %s tab under its owning project",
    (kind) => {
      const cwd = "/repo-worktrees/feature";
      const file = kind === "editor"
        ? newFileTab(`${cwd}/readme.md`, cwd, false, undefined, "/repo")
        : newTerminalFile(cwd, undefined, "/repo");
      const pane = { id: "surface", files: [file], activeFileId: file.id };
      const worktreeTab = {
        ...tab("tree-tab", pane.id),
        editorPanes: kind === "editor" ? [pane] : [],
        terminalPanes: kind === "terminal" ? [pane] : [],
      };
      expect(workspaceTabCwd(worktreeTab, [])).toBe("/repo");
      expect(focusedWorkspaceTabCwd(worktreeTab, [])).toBe("/repo");
      expect(planProjectReturn({
        tabs: [worktreeTab], sessions: [], memory: new Map(),
        activeTabId: "elsewhere", projectPath: "/repo",
      })).toMatchObject({ action: "activate", tabId: "tree-tab" });
    },
  );

  it.each(["editor", "terminal"] as const)(
    "uses the restored %s pane's project instead of the first chat's project",
    (kind) => {
      const sessions = [session("chat", "/alpha")];
      const file =
        kind === "editor"
          ? newFileTab("/beta/readme.md", "/beta")
          : newTerminalFile("/beta");
      const pane = { id: "surface", files: [file], activeFileId: file.id };
      const mixed: WorkspaceTab = {
        ...tab("mixed", "chat"),
        layout: splitPane(newTab("chat").layout, "chat", "right", pane.id),
        editorPanes: kind === "editor" ? [pane] : [],
        terminalPanes: kind === "terminal" ? [pane] : [],
      };
      const decision = planProjectReturn({
        tabs: [mixed],
        sessions,
        memory: new Map([["/beta", pane.id]]),
        activeTabId: mixed.id,
        projectPath: "/beta",
      });
      expect(decision).toEqual({
        action: "activate",
        tabId: mixed.id,
        paneId: pane.id,
      });
      if (decision.action !== "activate" || !decision.paneId) {
        throw new Error("Expected pane activation");
      }
      const focusedTab = { ...mixed, focusedId: decision.paneId };
      expect(workspaceTabCwd(focusedTab, sessions)).toBe("/alpha");
      expect(focusedWorkspaceTabCwd(focusedTab, sessions)).toBe("/beta");
      expect(mixed.focusedId).toBe("chat");
    },
  );

  it("prefers the focused chat over the first chat", () => {
    const mixed = {
      ...tab("mixed", "first"),
      layout: splitPane(newTab("first").layout, "first", "right", "second"),
      focusedId: "second",
    };
    expect(
      focusedWorkspaceTabCwd(mixed, [
        session("first", "/alpha"),
        session("second", "/beta"),
      ]),
    ).toBe("/beta");
  });

  it("retains the tab fallback when the focused pane has no cwd", () => {
    const mixed = { ...tab("mixed", "chat"), focusedId: "missing" };
    expect(focusedWorkspaceTabCwd(mixed, [session("chat", "/alpha")])).toBe(
      "/alpha",
    );
    expect(focusedWorkspaceTabCwd(mixed, [])).toBeNull();
  });
});

describe("findOpenSessionTab", () => {
  it("does not focus a ghost tab whose session has been parked", () => {
    const ghost = tab("ghost-tab", "parked-session");
    expect(findOpenSessionTab([ghost], [], "parked-session")).toBeUndefined();
    expect(
      findOpenSessionTab(
        [ghost],
        [session("parked-session", "/workspace")],
        "parked-session",
      ),
    ).toBe(ghost);
  });
});

describe("switchSessionInTab", () => {
  it("replaces the focused session while keeping the tab and its other panes", () => {
    const split = {
      ...tab("current-tab", "current"),
      layout: splitPane(newTab("current").layout, "current", "right", "other"),
    };
    const [next] = switchSessionInTab(
      [split],
      split.id,
      "current",
      "target",
      [],
    )!.tabs;
    expect(next.id).toBe(split.id);
    expect(leafIds(next.layout)).toEqual(["target", "other"]);
    expect(next.focusedId).toBe("target");
  });

  it("focuses a session already in the current tab", () => {
    const split = {
      ...tab("current-tab", "current"),
      layout: splitPane(newTab("current").layout, "current", "right", "target"),
    };
    const switched = switchSessionInTab(
      [split],
      split.id,
      "current",
      "target",
      [tuiSession("current"), tuiSession("target")],
    )!;
    const [next] = switched.tabs;
    // Nothing leaves its tab, so no view remounts.
    expect(switched.movedPtyIds).toEqual([]);
    expect(leafIds(next.layout)).toEqual(["current", "target"]);
    expect(next.focusedId).toBe("target");
  });

  it("swaps sessions across tabs instead of mounting one twice", () => {
    const current = tab("current-tab", "current");
    const other = tab("other-tab", "target");
    const next = switchSessionInTab(
      [current, other],
      current.id,
      "current",
      "target",
      [],
    )!.tabs;
    expect(next.map((entry) => entry.id)).toEqual(["current-tab", "other-tab"]);
    expect(next.map((entry) => leafIds(entry.layout))).toEqual([
      ["target"],
      ["current"],
    ]);
    expect(next.map((entry) => entry.focusedId)).toEqual(["target", "current"]);
  });

  it("reports the terminal sessions on both sides of a cross-tab swap", () => {
    const current = tab("current-tab", "current");
    const other = tab("other-tab", "target");
    const switched = switchSessionInTab(
      [current, other],
      current.id,
      "current",
      "target",
      [tuiSession("current"), tuiSession("target")],
    )!;
    // Both leaves land in the other tab's tree and remount.
    expect(switched.movedPtyIds).toEqual(["tui:current", "tui:target"]);
  });

  it("reports only the terminal side of a cross-tab swap with a chat", () => {
    const current = tab("current-tab", "current");
    const other = tab("other-tab", "target");
    const switched = switchSessionInTab(
      [current, other],
      current.id,
      "current",
      "target",
      [session("current", "/repo"), tuiSession("target")],
    )!;
    expect(switched.movedPtyIds).toEqual(["tui:target"]);
  });

  it("reports nothing when the focused terminal is evicted, not moved", () => {
    // The target is open in no tab, so the focused session lands nowhere: its
    // process must be killed by the unmounting view, never handed off.
    const current = tab("current-tab", "current");
    const switched = switchSessionInTab(
      [current],
      current.id,
      "current",
      "target",
      [tuiSession("current"), tuiSession("target")],
    )!;
    expect(leafIds(switched.tabs[0].layout)).toEqual(["target"]);
    expect(switched.movedPtyIds).toEqual([]);
  });

  it("ignores a switch after the focused session has changed", () => {
    const current = tab("current-tab", "new-focus");
    expect(
      switchSessionInTab([current], current.id, "old-focus", "target", []),
    ).toBeNull();
  });
});

describe("openAddToChatSessionPane", () => {
  it("opens and focuses a chat beside the focused file pane", () => {
    const file = newFileTab("/workspace/readme.md", "/workspace");
    const pane = { id: "editor", files: [file], activeFileId: file.id };
    const fileOnly: WorkspaceTab = {
      ...newTab(pane.id),
      id: "file-tab",
      editorPanes: [pane],
      diffFocused: true,
    };

    const opened = openAddToChatSessionPane({
      tab: fileOnly,
      sessions: [],
      sessionId: "new-chat",
    });

    expect(leafIds(opened!.layout)).toEqual([pane.id, "new-chat"]);
    expect(opened?.focusedId).toBe("new-chat");
    expect(opened?.diffFocused).toBe(false);
    expect(opened?.editorPanes).toEqual([pane]);
  });

  it("leaves add-to-chat routing to an existing session pane", () => {
    const chatTab = tab("chat-tab", "existing-chat");
    expect(
      openAddToChatSessionPane({
        tab: chatTab,
        sessions: [session("existing-chat", "/workspace")],
        sessionId: "unused-chat",
      }),
    ).toBeNull();
  });

  it("splits beside unmapped leaves whose sessions are not mounted", () => {
    const ghostTab = tab("ghost-tab", "ghost-leaf");
    const opened = openAddToChatSessionPane({
      tab: ghostTab,
      sessions: [],
      sessionId: "new-chat",
    });

    expect(leafIds(opened!.layout)).toEqual(["ghost-leaf", "new-chat"]);
    expect(opened?.focusedId).toBe("new-chat");
    expect(opened?.diffFocused).toBe(false);
  });
});

describe("workspaceTabProject", () => {
  it("reads project from the tab session cwd", () => {
    const workspace = tab("t1", "s1");
    const sessions = [session("s1", "/Users/me/agent-terminal")];
    expect(workspaceTabProject(workspace, sessions)).toBe("agent-terminal");
  });
});

describe("findTabForProject", () => {
  it("matches a tab by project path, ignoring trailing slashes", () => {
    const tabs = [tab("t1", "s1"), tab("t2", "s2")];
    const sessions = [
      session("s1", "/tmp/alpha"),
      session("s2", "/tmp/beta"),
    ];
    expect(findTabForProject(tabs, sessions, "/tmp/beta/")?.id).toBe("t2");
  });

  it("returns undefined when no open tab belongs to the project", () => {
    const tabs = [tab("t1", "s1")];
    const sessions = [session("s1", "/tmp/alpha")];
    expect(findTabForProject(tabs, sessions, "/tmp/beta")).toBeUndefined();
  });
});

describe("filterTabsForProject", () => {
  it("keeps only tabs that belong to the project", () => {
    const tabs = [tab("t1", "s1"), tab("t2", "s2"), tab("t3", "s3")];
    const sessions = [
      session("s1", "/tmp/alpha"),
      session("s2", "/tmp/beta"),
      session("s3", "/tmp/beta"),
    ];
    expect(
      filterTabsForProject(tabs, sessions, "/tmp/beta").map((tab) => tab.id),
    ).toEqual(["t2", "t3"]);
  });
});

describe("planWorkspaceTabClose", () => {
  const sessions = [
    session("m1", "/projects/monocode"),
    session("r1", "/projects/ruler"),
    session("m2", "/projects/monocode"),
  ];
  const tabs = [tab("tm1", "m1"), tab("tr1", "r1"), tab("tm2", "m2")];

  it("uses the global neighbor in workspace scope", () => {
    expect(
      planWorkspaceTabClose({
        tabs,
        sessions,
        closingTabId: "tm2",
        scope: "workspace",
      }),
    ).toEqual({ action: "close", nextActiveTabId: "tr1" });
  });

  it("prefers the previous same-project tab in project scope", () => {
    expect(
      planWorkspaceTabClose({
        tabs,
        sessions,
        closingTabId: "tm2",
        scope: "project",
      }),
    ).toEqual({ action: "close", nextActiveTabId: "tm1" });
  });

  it("uses the next same-project tab when none exists to the left", () => {
    expect(
      planWorkspaceTabClose({
        tabs,
        sessions,
        closingTabId: "tm1",
        scope: "project",
      }),
    ).toEqual({ action: "close", nextActiveTabId: "tm2" });
  });

  it("keeps the last tab of a project instead of jumping to another", () => {
    expect(
      planWorkspaceTabClose({
        tabs: tabs.slice(0, 2),
        sessions,
        closingTabId: "tm1",
        scope: "project",
      }),
    ).toEqual({ action: "keep" });
  });

  it("still jumps across projects in workspace scope when a project is emptied", () => {
    expect(
      planWorkspaceTabClose({
        tabs: tabs.slice(0, 2),
        sessions,
        closingTabId: "tm1",
        scope: "workspace",
      }),
    ).toEqual({ action: "close", nextActiveTabId: "tr1" });
  });

  it("uses the global neighbor for a projectless tab", () => {
    const projectlessSessions = [
      ...sessions,
      session("blank1", "~"),
      session("blank2", "~"),
    ];
    expect(
      planWorkspaceTabClose({
        tabs: [tab("projectless", "blank1"), tabs[1]],
        sessions: projectlessSessions,
        closingTabId: "projectless",
        scope: "project",
      }),
    ).toEqual({ action: "close", nextActiveTabId: "tr1" });
    expect(
      planWorkspaceTabClose({
        tabs: [tab("blank1", "blank1"), tabs[1], tab("blank2", "blank2")],
        sessions: projectlessSessions,
        closingTabId: "blank2",
        scope: "project",
      }),
    ).toEqual({ action: "close", nextActiveTabId: "tr1" });
  });

  it("keeps the sole tab or an unknown tab", () => {
    expect(
      planWorkspaceTabClose({
        tabs: [tabs[0]],
        sessions,
        closingTabId: "tm1",
        scope: "workspace",
      }),
    ).toEqual({ action: "keep" });
    expect(
      planWorkspaceTabClose({
        tabs,
        sessions,
        closingTabId: "missing",
        scope: "project",
      }),
    ).toEqual({ action: "keep" });
  });
});

describe("applyPlaceSessionOnPane", () => {
  const sessions = [
    session("m1", "/projects/monocode"),
    session("m2", "/projects/monocode"),
    session("r1", "/projects/ruler"),
  ];

  function replacementFrom(seed: Session | undefined): Session {
    return session("replacement", seed?.cwd ?? "/tmp/fallback");
  }

  it("splits the target pane toward the drop edge", () => {
    const next = applyPlaceSessionOnPane({
      tabs: [tab("tm1", "m1")],
      sessions,
      sessionId: "m2",
      targetId: "m1",
      edge: "right",
      replaceTarget: false,
      scope: "workspace",
      createReplacement: replacementFrom,
    });
    expect(next?.activeTabId).toBe("tm1");
    expect(next?.tabs[0]?.focusedId).toBe("m2");
    expect(leafIds(next!.tabs[0]!.layout)).toEqual(["m1", "m2"]);
  });

  it("replaces a blank target instead of splitting it", () => {
    const blank = session("blank", "/projects/monocode");
    const next = applyPlaceSessionOnPane({
      tabs: [tab("tm1", "blank")],
      sessions: [...sessions, blank],
      sessionId: "m2",
      targetId: "blank",
      edge: "right",
      replaceTarget: true,
      scope: "workspace",
      createReplacement: replacementFrom,
    });
    expect(leafIds(next!.tabs[0]!.layout)).toEqual(["m2"]);
    expect(next?.sessions.map((entry) => entry.id)).toEqual([
      "m1",
      "m2",
      "r1",
    ]);
  });

  it("relocates a session from another tab and closes that tab", () => {
    const next = applyPlaceSessionOnPane({
      tabs: [tab("tm1", "m1"), tab("tm2", "m2")],
      sessions,
      sessionId: "m2",
      targetId: "m1",
      edge: "left",
      replaceTarget: false,
      scope: "workspace",
      createReplacement: replacementFrom,
    });
    expect(next?.tabs.map((entry) => entry.id)).toEqual(["tm1"]);
    expect(leafIds(next!.tabs[0]!.layout)).toEqual(["m2", "m1"]);
    expect(next?.movedPtyIds).toEqual([]);
  });

  it("reports the terminal session it relocates from another tab", () => {
    const withTui = [...sessions, tuiSession("t1")];
    const next = applyPlaceSessionOnPane({
      tabs: [tab("tm1", "m1"), tab("tt1", "t1")],
      sessions: withTui,
      sessionId: "t1",
      targetId: "m1",
      edge: "right",
      replaceTarget: false,
      scope: "workspace",
      createReplacement: replacementFrom,
    });
    expect(next?.movedPtyIds).toEqual(["tui:t1"]);
  });

  it("reports nothing for a terminal session already in the target tab", () => {
    const withTui = [...sessions, tuiSession("t1")];
    const grouped: WorkspaceTab = {
      ...tab("tm1", "m1"),
      layout: splitPane(newTab("m1").layout, "m1", "right", "t1"),
    };
    const next = applyPlaceSessionOnPane({
      tabs: [grouped],
      sessions: withTui,
      sessionId: "t1",
      targetId: "m1",
      edge: "left",
      replaceTarget: false,
      scope: "workspace",
      createReplacement: replacementFrom,
    });
    expect(next?.movedPtyIds).toEqual([]);
  });

  it("keeps the last tab of a project and fills it with a replacement", () => {
    const next = applyPlaceSessionOnPane({
      tabs: [tab("tr1", "r1"), tab("tm1", "m1")],
      sessions,
      sessionId: "m1",
      targetId: "r1",
      edge: "right",
      replaceTarget: false,
      scope: "project",
      createReplacement: replacementFrom,
    });
    expect(next?.tabs.map((entry) => entry.id)).toEqual(["tr1", "tm1"]);
    expect(leafIds(next!.tabs[0]!.layout)).toEqual(["r1", "m1"]);
    expect(next?.tabs[1]?.focusedId).toBe("replacement");
  });
});

describe("applyPlaceTabOnPane", () => {
  const sessions = [
    session("target", "/projects/monocode"),
    session("source", "/projects/monocode"),
    session("other", "/projects/monocode"),
  ];

  it("turns a separate tab into a split beside the target pane", () => {
    const next = applyPlaceTabOnPane({
      tabs: [tab("target-tab", "target"), tab("source-tab", "source")],
      sessions,
      sourceTabId: "source-tab",
      targetId: "target",
      edge: "left",
      replaceTarget: false,
    });

    expect(next?.activeTabId).toBe("target-tab");
    expect(next?.tabs.map((entry) => entry.id)).toEqual(["target-tab"]);
    expect(leafIds(next!.tabs[0]!.layout)).toEqual(["source", "target"]);
    expect(next?.tabs[0]?.focusedId).toBe("source");
  });

  it("reports every terminal of the dragged tab, and none of the target's", () => {
    const file = newTerminalFile("/projects/monocode");
    const pane = { id: "term", files: [file], activeFileId: file.id };
    const source: WorkspaceTab = {
      ...tab("source-tab", "source"),
      layout: splitPane(newTab("source").layout, "source", "down", pane.id),
      terminalPanes: [pane],
    };
    const next = applyPlaceTabOnPane({
      tabs: [tab("target-tab", "target"), source],
      sessions: [tuiSession("target"), tuiSession("source")],
      sourceTabId: source.id,
      targetId: "target",
      edge: "right",
      replaceTarget: false,
    });
    expect(next?.movedPtyIds).toEqual(["tui:source", file.id]);
  });

  it("keeps every pane and the nested layout of the dragged tab", () => {
    const file = newFileTab(
      "/projects/monocode/readme.md",
      "/projects/monocode",
    );
    const pane = { id: "editor", files: [file], activeFileId: file.id };
    const source: WorkspaceTab = {
      ...tab("source-tab", "source"),
      layout: splitPane(newTab("source").layout, "source", "down", pane.id),
      focusedId: pane.id,
      editorPanes: [pane],
    };
    const next = applyPlaceTabOnPane({
      tabs: [tab("target-tab", "target"), source],
      sessions,
      sourceTabId: source.id,
      targetId: "target",
      edge: "right",
      replaceTarget: false,
    });

    expect(leafIds(next!.tabs[0]!.layout)).toEqual([
      "target",
      "source",
      "editor",
    ]);
    expect(next?.tabs[0]?.layout).toMatchObject({
      type: "split",
      dir: "right",
      children: [
        { type: "leaf", id: "target" },
        { type: "split", dir: "down" },
      ],
    });
    expect(next?.tabs[0]?.editorPanes).toEqual([pane]);
    expect(next?.focusedId).toBe("editor");
  });

  it("replaces a blank target without leaving its session mounted", () => {
    const blank = session("blank", "/projects/monocode");
    const next = applyPlaceTabOnPane({
      tabs: [tab("target-tab", "blank"), tab("source-tab", "source")],
      sessions: [...sessions, blank],
      sourceTabId: "source-tab",
      targetId: blank.id,
      edge: "bottom",
      replaceTarget: true,
    });

    expect(leafIds(next!.tabs[0]!.layout)).toEqual(["source"]);
    expect(next?.sessions.some((entry) => entry.id === blank.id)).toBe(false);
  });
});

describe("applyDetachPaneToTab", () => {
  it("turns one chat pane into a separate tab at the drop position", () => {
    const source: WorkspaceTab = {
      ...tab("source-tab", "first"),
      layout: splitPane(newTab("first").layout, "first", "right", "second"),
      focusedId: "second",
    };
    const target = tab("target-tab", "third");
    const next = applyDetachPaneToTab({
      tabs: [source, target],
      sessions: [session("first", "/repo"), session("second", "/repo")],
      paneId: "second",
      targetTabId: target.id,
      position: "before",
      createTabId: () => "detached-tab",
    });

    expect(next?.activeTabId).toBe("detached-tab");
    expect(next?.focusedId).toBe("second");
    expect(next?.tabs.map((entry) => entry.id)).toEqual([
      "source-tab",
      "detached-tab",
      "target-tab",
    ]);
    expect(leafIds(next!.tabs[0]!.layout)).toEqual(["first"]);
    expect(leafIds(next!.tabs[1]!.layout)).toEqual(["second"]);
    expect(next?.movedPtyIds).toEqual([]);
  });

  it("reports the terminal session it detaches", () => {
    const source: WorkspaceTab = {
      ...tab("source-tab", "first"),
      layout: splitPane(newTab("first").layout, "first", "right", "second"),
    };
    const next = applyDetachPaneToTab({
      tabs: [source, tab("target-tab", "third")],
      sessions: [tuiSession("first"), tuiSession("second")],
      paneId: "second",
      targetTabId: "target-tab",
      position: "after",
    });
    // Only the pane that leaves the split, not the one that stays.
    expect(next?.movedPtyIds).toEqual(["tui:second"]);
  });

  it.each(["editor", "terminal"] as const)(
    "moves the complete %s pane metadata into the new tab",
    (kind) => {
      const file =
        kind === "editor"
          ? newFileTab("/projects/monocode/readme.md", "/projects/monocode")
          : newTerminalFile("/projects/monocode");
      const pane = { id: `${kind}-pane`, files: [file], activeFileId: file.id };
      const source: WorkspaceTab = {
        ...tab("source-tab", "chat"),
        layout: splitPane(newTab("chat").layout, "chat", "down", pane.id),
        editorPanes: kind === "editor" ? [pane] : [],
        terminalPanes: kind === "terminal" ? [pane] : [],
      };
      const next = applyDetachPaneToTab({
        tabs: [source, tab("target-tab", "other")],
        sessions: [],
        paneId: pane.id,
        targetTabId: "target-tab",
        position: "after",
        createTabId: () => "detached-tab",
      });

      expect(next?.tabs.map((entry) => entry.id)).toEqual([
        "source-tab",
        "target-tab",
        "detached-tab",
      ]);
      expect(next?.tabs[0]?.editorPanes).toEqual([]);
      expect(next?.tabs[0]?.terminalPanes).toEqual([]);
      expect(next?.tabs[2]?.editorPanes).toEqual(
        kind === "editor" ? [pane] : [],
      );
      expect(next?.tabs[2]?.terminalPanes).toEqual(
        kind === "terminal" ? [pane] : [],
      );
      expect(next?.movedPtyIds).toEqual(kind === "terminal" ? [file.id] : []);
    },
  );

  it("does not detach the only pane in a tab", () => {
    expect(
      applyDetachPaneToTab({
        tabs: [tab("source-tab", "only")],
        sessions: [],
        paneId: "only",
        targetTabId: "source-tab",
        position: "after",
      }),
    ).toBeNull();
  });
});

describe("replaceGroupInTabOrder", () => {
  it("swaps a contiguous slice of ids", () => {
    expect(replaceGroupInTabOrder(["a", "b", "c", "d"], 1, 2, ["d", "c"])).toEqual([
      "a",
      "d",
      "c",
      "d",
    ]);
  });
});

describe("worktree tab scope", () => {
  const main = session("main", "/repo");
  const feature = { ...session("feature", "/repo"), worktreeCwd: "/trees/a" };
  const sessions = [main, feature];

  it("places a session tab in its working copy", () => {
    expect(workspaceTabWorktree(tab("t1", "main"), sessions)).toBe("/repo");
    expect(workspaceTabWorktree(tab("t2", "feature"), sessions)).toBe(
      "/trees/a",
    );
  });

  it("shows a tab only in its own worktree", () => {
    expect(tabInWorktree(tab("t1", "main"), sessions, "/repo")).toBe(true);
    expect(tabInWorktree(tab("t1", "main"), sessions, "/trees/a")).toBe(false);
    expect(tabInWorktree(tab("t2", "feature"), sessions, "/trees/a")).toBe(
      true,
    );
  });

  it("shows tabs without a working copy everywhere", () => {
    expect(tabInWorktree(tab("t3", "unknown"), sessions, "/trees/a")).toBe(
      true,
    );
  });
});

describe("workspaceTabWorktree", () => {
  const main = session("main", "/repo");
  const feature = { ...session("feature", "/repo"), worktreeCwd: "/trees/a" };
  const sessions = [main, feature];

  it("uses the session's working copy", () => {
    expect(workspaceTabWorktree(tab("t1", "main"), sessions)).toBe("/repo");
    expect(workspaceTabWorktree(tab("t2", "feature"), sessions)).toBe(
      "/trees/a",
    );
  });

  it("scopes tabs to one worktree and keeps unowned tabs everywhere", () => {
    expect(tabInWorktree(tab("t1", "main"), sessions, "/repo")).toBe(true);
    expect(tabInWorktree(tab("t2", "feature"), sessions, "/repo")).toBe(false);
    expect(tabInWorktree(tab("t2", "feature"), sessions, "/trees/a")).toBe(
      true,
    );
    expect(tabInWorktree(tab("t3", "unknown"), sessions, "/trees/a")).toBe(
      true,
    );
  });

  it("closes to a tab in the same worktree, or keeps the last one", () => {
    const other = { ...session("other", "/repo"), worktreeCwd: "/trees/a" };
    const all = [main, feature, other];
    const tabs = [tab("t1", "main"), tab("t2", "feature"), tab("t3", "other")];
    const worktreeOf = (entry: WorkspaceTab) => workspaceTabWorktree(entry, all);
    expect(
      planWorkspaceTabClose({
        tabs,
        sessions: all,
        closingTabId: "t2",
        scope: "project",
        worktreeOf,
      }),
    ).toEqual({ action: "close", nextActiveTabId: "t3" });
    expect(
      planWorkspaceTabClose({
        tabs,
        sessions: all,
        closingTabId: "t1",
        scope: "project",
        worktreeOf,
      }),
    ).toEqual({ action: "keep" });
  });
});

describe("planSessionPaneClose", () => {
  const sessions = [
    session("a", "/repo"),
    session("b", "/repo"),
    session("c", "/repo"),
  ];

  it("closes a pane in a tab that is not the active one", () => {
    const split: WorkspaceTab = {
      ...tab("t2", "b"),
      layout: splitPane(newTab("b").layout, "b", "right", "c"),
    };
    const tabs = [tab("t1", "a"), split];
    const plan = planSessionPaneClose({
      tabs,
      sessions,
      sessionId: "c",
      scope: "workspace",
    });
    expect(plan).toMatchObject({ action: "remove-pane", tabId: "t2" });
    if (plan.action !== "remove-pane") throw new Error("Expected remove-pane");
    expect(leafIds(plan.layout)).toEqual(["b"]);
    // The plan never touches the sessions, so nothing is archived.
    expect(sessions.map((s) => s.id)).toEqual(["a", "b", "c"]);
  });

  it("drops a background tab when its last pane closes", () => {
    const plan = planSessionPaneClose({
      tabs: [tab("t1", "a"), tab("t2", "b")],
      sessions,
      sessionId: "b",
      scope: "workspace",
    });
    expect(plan).toEqual({ action: "close-tab", tabId: "t2" });
  });

  it("resets the only tab to a blank session instead of closing it", () => {
    const plan = planSessionPaneClose({
      tabs: [tab("t1", "a")],
      sessions,
      sessionId: "a",
      scope: "workspace",
    });
    expect(plan).toEqual({ action: "clear-tab", tabId: "t1" });
  });

  describe("project scope, as the app runs it", () => {
    const tuiSession = (id: string, cwd: string): Session => ({
      ...session(id, cwd),
      surface: "tui",
    });
    const worktreeOf = (entry: WorkspaceTab) =>
      leafIds(entry.layout).includes("wt")
        ? "/repo-wt"
        : leafIds(entry.layout).includes("wt2")
          ? "/repo-wt2"
          : null;

    it("clears the only tab when a late TUI exit leaves nothing else", () => {
      const all = [tuiSession("a", "/repo")];
      const plan = planSessionPaneClose({
        tabs: [tab("t1", "a")],
        sessions: all,
        sessionId: "a",
        scope: "project",
        worktreeOf,
      });
      expect(plan).toEqual({ action: "clear-tab", tabId: "t1" });
      // Planning never edits the sessions, so no replacement is spawned here;
      // the app builds a blank chat for a clear-tab plan.
      expect(all).toHaveLength(1);
      expect(all[0].surface).toBe("tui");
    });

    it("closes a tab that has a same-project sibling", () => {
      const plan = planSessionPaneClose({
        tabs: [tab("t1", "a"), tab("t2", "b")],
        sessions,
        sessionId: "b",
        scope: "project",
        worktreeOf,
      });
      expect(plan).toEqual({ action: "close-tab", tabId: "t2" });
    });

    it("clears a background tab whose project has no other tab", () => {
      const other = [session("a", "/repo"), tuiSession("b", "/other")];
      const plan = planSessionPaneClose({
        tabs: [tab("t1", "a"), tab("t2", "b")],
        sessions: other,
        sessionId: "b",
        scope: "project",
        worktreeOf,
      });
      expect(plan).toEqual({ action: "clear-tab", tabId: "t2" });
    });

    it("clears a worktree tab when the only sibling is in another worktree", () => {
      const all = [session("wt2", "/repo"), tuiSession("wt", "/repo")];
      const plan = planSessionPaneClose({
        tabs: [tab("t1", "wt2"), tab("t2", "wt")],
        sessions: all,
        sessionId: "wt",
        scope: "project",
        worktreeOf,
      });
      expect(plan).toEqual({ action: "clear-tab", tabId: "t2" });
    });
  });

  it("does nothing when no tab holds the session, or the session is gone", () => {
    const tabs = [tab("t1", "a")];
    expect(
      planSessionPaneClose({ tabs, sessions, sessionId: "c", scope: "workspace" }),
    ).toEqual({ action: "none" });
    expect(
      planSessionPaneClose({
        tabs,
        sessions: [],
        sessionId: "a",
        scope: "workspace",
      }),
    ).toEqual({ action: "none" });
  });
});

describe("ptyIdsOfPanes", () => {
  const tui = { ...session("tui", "/repo"), surface: "tui" as const };

  it("names the PTY of a terminal session and of every terminal file moved", () => {
    const first = newTerminalFile("/repo");
    const second = newTerminalFile("/repo");
    const pane = { id: "term", files: [first, second], activeFileId: first.id };
    const grouped: WorkspaceTab = {
      ...tab("t1", "tui"),
      layout: splitPane(newTab("tui").layout, "tui", "right", "term"),
      terminalPanes: [pane],
    };
    expect(
      ptyIdsOfPanes(grouped, ["tui", "term"], [tui, session("chat", "/repo")]),
    ).toEqual(["tui:tui", first.id, second.id]);
  });

  it("names the terminal files of a pane kept in editorPanes", () => {
    // A terminal can sit in an editor pane, as the project dock's does.
    const file = newTerminalFile("/repo");
    const grouped: WorkspaceTab = {
      ...tab("t1", "chat"),
      editorPanes: [{ id: "dock", files: [file], activeFileId: file.id }],
    };
    expect(ptyIdsOfPanes(grouped, ["dock"], [])).toEqual([file.id]);
  });

  it("ignores editor files that are not terminals", () => {
    const doc = newFileTab("/repo/readme.md", "/repo");
    const grouped: WorkspaceTab = {
      ...tab("t1", "chat"),
      editorPanes: [{ id: "editor", files: [doc], activeFileId: doc.id }],
    };
    expect(ptyIdsOfPanes(grouped, ["editor"], [])).toEqual([]);
  });

  it("names nothing for a chat and only the panes asked for", () => {
    const file = newTerminalFile("/repo");
    const grouped: WorkspaceTab = {
      ...tab("t1", "chat"),
      terminalPanes: [{ id: "term", files: [file], activeFileId: file.id }],
    };
    expect(ptyIdsOfPanes(grouped, ["chat"], [session("chat", "/repo")])).toEqual([]);
  });
});
