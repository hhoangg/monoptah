import { Storage } from "happy-dom";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { saveBlameColumn } from "../../settings/model/settings";
import {
  newChangesTab,
  newCommitTab,
  newFileTab,
  newReleaseNotesWorkspaceTab,
  newSessionChangesTab,
  newTerminalFile,
} from "../model/layout";
import { releaseNotesTitle } from "../../../app/model/releaseNotes";
import {
  appendProblems,
  surfaceTabMenuItems,
  surfaceTabPresentation,
} from "./SurfaceTabs";

describe("surfaceTabPresentation", () => {
  it("labels release notes from their version", () => {
    const file = newReleaseNotesWorkspaceTab({ version: "0.1.23" })
      .editorPanes[0]?.files[0];
    if (!file) throw new Error("expected release-note file");

    expect(surfaceTabPresentation(file)).toEqual({
      name: releaseNotesTitle("0.1.23"),
      label: releaseNotesTitle("0.1.23"),
      iconName: "CHANGELOG.md",
      tooltip: releaseNotesTitle("0.1.23"),
    });
  });

  it("labels the unified working-tree tab as Changes", () => {
    expect(
      surfaceTabPresentation(newChangesTab("/repo", "/repo/App.tsx")),
    ).toEqual({
      name: "Changes",
      label: "Changes",
      iconName: "CHANGES",
      tooltip: "Working tree changes",
    });
  });

  it("labels a session-scoped review distinctly", () => {
    expect(
      surfaceTabPresentation(
        newSessionChangesTab("/repo", "session-a", "/repo/App.tsx"),
      ),
    ).toEqual({
      name: "Session Changes",
      label: "Session Changes",
      iconName: "CHANGES",
      tooltip: "Changes captured for this session only",
    });
  });

  it("labels a commit tab from the subject", () => {
    expect(
      surfaceTabPresentation(
        newCommitTab("/repo", {
          sha: "abc1234deadbeef",
          shortSha: "abc1234",
          subject: "Fix the graph",
        }),
      ),
    ).toEqual({
      name: "Fix the graph",
      label: "Fix the graph",
      iconName: "CHANGES",
      tooltip: "abc1234 — Fix the graph",
    });
  });
});

describe("appendProblems", () => {
  it("leaves a clean file's tooltip alone", () => {
    expect(appendProblems("/repo/src/app.ts", 0)).toBe("/repo/src/app.ts");
  });

  it("singularises a lone problem", () => {
    expect(appendProblems("/repo/src/app.ts", 1)).toBe(
      "/repo/src/app.ts — 1 problem",
    );
  });

  it("pluralises the rest", () => {
    expect(appendProblems("/repo/src/app.ts", 4)).toBe(
      "/repo/src/app.ts — 4 problems",
    );
  });
});

describe("surfaceTabMenuItems", () => {
  beforeEach(() => {
    vi.stubGlobal("localStorage", new Storage());
  });
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  const labels = (items: ReturnType<typeof surfaceTabMenuItems>) =>
    items.flatMap((item) => (item.kind === "item" ? [item.label] : []));

  it("offers filesystem actions for regular and review file tabs", () => {
    const common = [
      "Open in Default App",
      expect.stringMatching(/Reveal|Containing Folder/),
      "Copy Path",
      "Copy Relative Path",
      "Copy File Name",
    ];
    expect(
      labels(surfaceTabMenuItems(newFileTab("/repo/src/app.ts", "/repo"))),
    ).toEqual([...common, "Show Git Blame", "Close", "Close Others"]);
    // Blame stays off in review tabs, so there is nothing to toggle there.
    expect(
      labels(surfaceTabMenuItems(newFileTab("/repo/src/app.ts", "/repo", true))),
    ).toEqual([...common, "Close", "Close Others"]);
  });

  it("labels the blame item by the current state of the column", () => {
    const file = newFileTab("/repo/src/app.ts", "/repo");
    expect(labels(surfaceTabMenuItems(file))).toContain("Show Git Blame");
    saveBlameColumn(true);
    const items = labels(surfaceTabMenuItems(file));
    expect(items).toContain("Hide Git Blame");
    expect(items).not.toContain("Show Git Blame");
  });

  it("leaves blame out of image and video tabs", () => {
    for (const path of ["/repo/logo.png", "/repo/demo.mp4"]) {
      const items = labels(surfaceTabMenuItems(newFileTab(path, "/repo")));
      expect(items.some((label) => /Git Blame/.test(label))).toBe(false);
    }
  });

  it("offers close actions when a tab has no real file", () => {
    for (const file of [
      newChangesTab("/repo"),
      newCommitTab("/repo", {
        sha: "abc1234deadbeef",
        shortSha: "abc1234",
        subject: "Fix the graph",
      }),
      newSessionChangesTab("/repo", "session-a"),
      newTerminalFile("/repo"),
    ]) {
      expect(surfaceTabMenuItems(file)).toEqual([
        { kind: "item", id: "close", label: "Close" },
        {
          kind: "item",
          id: "close-others",
          label: "Close Others",
          disabled: false,
        },
      ]);
    }
  });

  it("disables Close Others when there are no sibling tabs", () => {
    const items = surfaceTabMenuItems(
      newFileTab("/repo/src/app.ts", "/repo"),
      false,
    );
    expect(items.at(-1)).toEqual({
      kind: "item",
      id: "close-others",
      label: "Close Others",
      disabled: true,
    });
  });
});
