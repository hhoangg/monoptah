// @vitest-environment happy-dom
import { EditorView } from "@codemirror/view";
import type * as TauriCore from "@tauri-apps/api/core";
import { Storage } from "happy-dom";
import { act, createElement, type ComponentProps } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { GitBlame, GitBlameHunk } from "../../../platform/tauri/fs";
import { notifyGitChanged } from "../../../platform/tauri/fs";
import {
  saveBlameColumn,
  saveInlineBlame,
} from "../../settings/model/settings";
import { blameForLine } from "../editor/editorBlame";
import { invalidateWatchedFiles } from "../model/fileWatch";
import { FileEditor } from "./FileEditor";

type PendingBlame = {
  args: Record<string, unknown>;
  resolve: (blame: GitBlame) => void;
  reject: (error: unknown) => void;
};

const bridge = vi.hoisted(() => ({
  invoke: (_command: string, _args?: Record<string, unknown>): unknown =>
    undefined,
}));
// Keep the editor real; replace only native transport.
vi.mock("@tauri-apps/api/core", async (original) => ({
  ...(await original<typeof TauriCore>()),
  invoke: async (command: string, args?: Record<string, unknown>) =>
    bridge.invoke(command, args),
}));

const CWD = "/repo";
const PATH = "/repo/src/notes.txt";
const CONTENT = "one\ntwo\nthree\n";

function hunk(sha: string, startLine: number, lineCount: number): GitBlameHunk {
  return {
    sha: sha.padEnd(40, "0"),
    shortSha: sha.slice(0, 7),
    author: `author-${sha}`,
    authorTime: 1_700_000_000,
    summary: `summary ${sha}`,
    uncommitted: false,
    startLine,
    lineCount,
  };
}

const OLD: GitBlame = { hunks: [hunk("aaa1111", 1, 4)] };
const NEW: GitBlame = { hunks: [hunk("bbb2222", 1, 4)] };

describe("FileEditor blame", () => {
  let root: Root;
  let container: HTMLDivElement;
  let pending: PendingBlame[];
  let written: string[];
  let disk: string;
  let lastProps: Partial<ComponentProps<typeof FileEditor>>;

  const calls = () => pending.length;

  beforeEach(() => {
    vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
    vi.stubGlobal("localStorage", new Storage());
    pending = [];
    written = [];
    disk = CONTENT;
    lastProps = {};
    container = document.createElement("div");
    document.body.append(container);
    root = createRoot(container);
    bridge.invoke = (command, args) => {
      if (command === "read_text_file") return disk;
      if (command === "stat_files") return [];
      if (command === "write_text_file") {
        written.push(String(args?.content));
        return;
      }
      if (command === "git_blame") {
        return new Promise<GitBlame>((resolve, reject) => {
          pending.push({ args: args ?? {}, resolve, reject });
        });
      }
      if (command === "git_diff_files") return { files: [] };
      if (command === "git_file_diff") {
        return { original: "", current: CONTENT, binary: false, tooLarge: false };
      }
      throw new Error(`Unexpected native command: ${command}`);
    };
  });

  afterEach(async () => {
    await act(async () => root.unmount());
    container.remove();
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
  });

  async function rerender(props: Partial<ComponentProps<typeof FileEditor>>) {
    lastProps = { ...lastProps, ...props };
    await act(async () =>
      root.render(
        createElement(FileEditor, {
          path: PATH,
          cwd: CWD,
          active: true,
          onDirtyChange: () => {},
          ...lastProps,
        }),
      ),
    );
  }

  async function render(props: Partial<ComponentProps<typeof FileEditor>> = {}) {
    await rerender(props);
    await act(async () =>
      vi.waitFor(() =>
        expect(container.querySelector(".cm-editor")).not.toBeNull(),
      ),
    );
    return EditorView.findFromDOM(
      container.querySelector<HTMLElement>(".cm-editor")!,
    )!;
  }

  async function answer(index: number, blame: GitBlame) {
    await act(async () => {
      pending[index].resolve(blame);
    });
  }

  async function settle() {
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 20));
    });
  }

  // Polls in short act() scopes: React only flushes the updates a timer or a
  // reload causes when an act scope ends, so waiting inside one never sees them.
  async function until(check: () => boolean) {
    for (let attempt = 0; attempt < 50; attempt++) {
      if (check()) return;
      await settle();
    }
    expect(check()).toBe(true);
  }

  async function save(view: EditorView) {
    const modifier = /Mac/.test(navigator.platform)
      ? { metaKey: true }
      : { ctrlKey: true };
    await act(async () => {
      view.contentDOM.dispatchEvent(
        new KeyboardEvent("keydown", {
          key: "s",
          bubbles: true,
          cancelable: true,
          ...modifier,
        }),
      );
    });
  }

  const shaOf = (view: EditorView, line: number) =>
    blameForLine(view.state, line)?.sha ?? null;

  it("requests blame with the repo-relative path when inline blame is on", async () => {
    const view = await render();
    await act(async () => vi.waitFor(() => expect(calls()).toBe(1)));
    expect(pending[0].args).toEqual({ cwd: CWD, relative: "src/notes.txt" });

    await answer(0, OLD);
    expect(shaOf(view, 1)).toBe(OLD.hunks[0].sha);
  });

  it("requests blame for the column alone", async () => {
    saveInlineBlame(false);
    saveBlameColumn(true);
    await render();
    await act(async () => vi.waitFor(() => expect(calls()).toBe(1)));
  });

  it("does not request blame when inline and column are both off", async () => {
    saveInlineBlame(false);
    saveBlameColumn(false);
    await render();
    await settle();
    notifyGitChanged();
    await settle();
    expect(calls()).toBe(0);
  });

  it("does not request blame for a file outside the repository", async () => {
    await render({ cwd: "/elsewhere" });
    await settle();
    expect(calls()).toBe(0);
  });

  it("does not request blame in a review (diff) tab", async () => {
    await render({ showDiff: true });
    await settle();
    notifyGitChanged();
    await settle();
    expect(calls()).toBe(0);
  });

  it("fetches again after the file is saved", async () => {
    const view = await render();
    await act(async () => vi.waitFor(() => expect(calls()).toBe(1)));
    await answer(0, OLD);

    await act(async () => view.dispatch({ changes: { from: 0, insert: "zero\n" } }));
    await save(view);
    await act(async () => vi.waitFor(() => expect(written).toHaveLength(1)));
    await act(async () => vi.waitFor(() => expect(calls()).toBe(2)));

    await answer(1, NEW);
    expect(shaOf(view, 1)).toBe(NEW.hunks[0].sha);
  });

  it("fetches again when git state changes and the editor is clean", async () => {
    const view = await render();
    await act(async () => vi.waitFor(() => expect(calls()).toBe(1)));
    await answer(0, OLD);

    notifyGitChanged();
    await act(async () => vi.waitFor(() => expect(calls()).toBe(2)));
    await answer(1, NEW);
    expect(shaOf(view, 1)).toBe(NEW.hunks[0].sha);
  });

  it("fetches again when the window regains focus", async () => {
    await render();
    await act(async () => vi.waitFor(() => expect(calls()).toBe(1)));
    await act(async () => {
      window.dispatchEvent(new Event("focus"));
    });
    await act(async () => vi.waitFor(() => expect(calls()).toBe(2)));
  });

  it("does not request blame while the editor has unsaved edits", async () => {
    const view = await render();
    await act(async () => vi.waitFor(() => expect(calls()).toBe(1)));
    await answer(0, OLD);

    await act(async () => view.dispatch({ changes: { from: 0, insert: "x" } }));
    notifyGitChanged();
    window.dispatchEvent(new Event("focus"));
    await settle();
    expect(calls()).toBe(1);
  });

  it("lets a newer response win over a stale one that arrives later", async () => {
    const view = await render();
    await act(async () => vi.waitFor(() => expect(calls()).toBe(1)));
    notifyGitChanged();
    await act(async () => vi.waitFor(() => expect(calls()).toBe(2)));

    await answer(1, NEW);
    await answer(0, OLD);
    expect(shaOf(view, 1)).toBe(NEW.hunks[0].sha);
  });

  it("discards a response when the document was edited while it was in flight", async () => {
    const view = await render();
    await act(async () => vi.waitFor(() => expect(calls()).toBe(1)));

    await act(async () => view.dispatch({ changes: { from: 0, insert: "x" } }));
    await answer(0, OLD);
    expect(shaOf(view, 1)).toBeNull();
    // Still dirty, so nothing is refetched until the file is saved.
    expect(calls()).toBe(1);

    await save(view);
    await act(async () => vi.waitFor(() => expect(calls()).toBe(2)));
    await answer(1, NEW);
    expect(shaOf(view, 1)).toBe(NEW.hunks[0].sha);
  });

  it("refetches when an edit was undone before the response arrived", async () => {
    const view = await render();
    await act(async () => vi.waitFor(() => expect(calls()).toBe(1)));

    await act(async () => view.dispatch({ changes: { from: 0, insert: "x" } }));
    await act(async () =>
      view.dispatch({ changes: { from: 0, to: 1, insert: "" } }),
    );
    await answer(0, OLD);
    await act(async () => vi.waitFor(() => expect(calls()).toBe(2)));
    await answer(1, NEW);
    expect(shaOf(view, 1)).toBe(NEW.hunks[0].sha);
  });

  it("treats an empty blame as no blame", async () => {
    const view = await render();
    await act(async () => vi.waitFor(() => expect(calls()).toBe(1)));
    await answer(0, OLD);
    expect(shaOf(view, 1)).toBe(OLD.hunks[0].sha);

    notifyGitChanged();
    await act(async () => vi.waitFor(() => expect(calls()).toBe(2)));
    await answer(1, { hunks: [] });
    expect(shaOf(view, 1)).toBeNull();
  });

  it("clears blame when the request fails", async () => {
    const view = await render();
    await act(async () => vi.waitFor(() => expect(calls()).toBe(1)));
    await answer(0, OLD);

    notifyGitChanged();
    await act(async () => vi.waitFor(() => expect(calls()).toBe(2)));
    await act(async () => {
      pending[1].reject(new Error("not a git repository"));
    });
    expect(shaOf(view, 1)).toBeNull();
  });

  it("fetches again when blame is switched back on after both were off", async () => {
    saveInlineBlame(false);
    saveBlameColumn(false);
    const view = await render();
    await settle();
    expect(calls()).toBe(0);

    await act(async () => {
      saveInlineBlame(true);
    });
    await act(async () => vi.waitFor(() => expect(calls()).toBe(1)));
    await answer(0, OLD);
    expect(shaOf(view, 1)).toBe(OLD.hunks[0].sha);
  });

  it("keeps its data when switching between inline and column", async () => {
    const view = await render();
    await act(async () => vi.waitFor(() => expect(calls()).toBe(1)));
    await answer(0, OLD);

    await act(async () => {
      saveBlameColumn(true);
    });
    await act(async () => {
      saveInlineBlame(false);
    });
    await settle();
    expect(calls()).toBe(1);
    expect(shaOf(view, 1)).toBe(OLD.hunks[0].sha);
  });

  it("fetches blame for the new document after the file changes on disk", async () => {
    const view = await render();
    await act(async () => vi.waitFor(() => expect(calls()).toBe(1)));
    await answer(0, OLD);

    disk = "uno\ndos\ntres\ncuatro\n";
    invalidateWatchedFiles([PATH]);
    await until(() => calls() === 2);
    // The request goes out only once the editor shows the new content.
    expect(view.state.doc.toString()).toBe(disk);

    await answer(1, { hunks: [hunk("ccc3333", 1, 4)] });
    expect(shaOf(view, 4)).toBe(hunk("ccc3333", 1, 4).sha);
  });

  describe("in a tab that is not shown", () => {
    it("does not fetch on mount, git changes or focus", async () => {
      await render({ active: false, shown: false });
      await settle();
      notifyGitChanged();
      window.dispatchEvent(new Event("focus"));
      await settle();
      expect(calls()).toBe(0);
    });

    it("fetches once when it is shown after a git change", async () => {
      const view = await render({ active: false, shown: false });
      notifyGitChanged();
      notifyGitChanged();
      window.dispatchEvent(new Event("focus"));
      await settle();
      expect(calls()).toBe(0);

      await rerender({ shown: true });
      await act(async () => vi.waitFor(() => expect(calls()).toBe(1)));
      await settle();
      expect(calls()).toBe(1);
      await answer(0, OLD);
      expect(shaOf(view, 1)).toBe(OLD.hunks[0].sha);
    });

    it("does not refetch when shown again and nothing changed", async () => {
      await render();
      await act(async () => vi.waitFor(() => expect(calls()).toBe(1)));
      await answer(0, OLD);

      await rerender({ shown: false });
      await rerender({ shown: true });
      await settle();
      expect(calls()).toBe(1);
    });

    it("marks blame stale when a hidden tab is saved", async () => {
      const view = await render();
      await act(async () => vi.waitFor(() => expect(calls()).toBe(1)));
      await answer(0, OLD);

      await act(async () => view.dispatch({ changes: { from: 0, insert: "z\n" } }));
      await rerender({ shown: false });
      await save(view);
      await act(async () => vi.waitFor(() => expect(written).toHaveLength(1)));
      await settle();
      expect(calls()).toBe(1);

      await rerender({ shown: true });
      await act(async () => vi.waitFor(() => expect(calls()).toBe(2)));
    });
  });

  describe("in a pane that is shown but not focused", () => {
    it("fetches on mount", async () => {
      await render({ active: false, shown: true });
      await act(async () => vi.waitFor(() => expect(calls()).toBe(1)));
    });

    it("refreshes on git changes and window focus", async () => {
      const view = await render({ active: false, shown: true });
      await act(async () => vi.waitFor(() => expect(calls()).toBe(1)));
      await answer(0, OLD);

      notifyGitChanged();
      await act(async () => vi.waitFor(() => expect(calls()).toBe(2)));
      await answer(1, NEW);
      expect(shaOf(view, 1)).toBe(NEW.hunks[0].sha);

      await act(async () => {
        window.dispatchEvent(new Event("focus"));
      });
      await act(async () => vi.waitFor(() => expect(calls()).toBe(3)));
    });

    it("does not need focus to catch up after being hidden", async () => {
      await render({ active: false, shown: false });
      notifyGitChanged();
      await settle();
      expect(calls()).toBe(0);

      await rerender({ active: false, shown: true });
      await act(async () => vi.waitFor(() => expect(calls()).toBe(1)));
    });
  });

  it("collapses the blame column for a file without blame", async () => {
    saveInlineBlame(false);
    saveBlameColumn(true);
    await render();
    await act(async () => vi.waitFor(() => expect(calls()).toBe(1)));
    const editor = container.querySelector<HTMLElement>(".cm-editor")!;
    // Not loaded yet: the column keeps its width.
    expect(editor.classList.contains("cm-blameNone")).toBe(false);
    expect(container.querySelector(".cm-blameGutter")).not.toBeNull();

    await answer(0, { hunks: [] });
    expect(editor.classList.contains("cm-blameNone")).toBe(true);
  });

  it("opens a clicked commit in the repository of the file", async () => {
    saveInlineBlame(false);
    saveBlameColumn(true);
    const onOpenCommit = vi.fn();
    await render({ onOpenCommit });
    await act(async () => vi.waitFor(() => expect(calls()).toBe(1)));
    await answer(0, OLD);

    const cell = container.querySelector<HTMLElement>(
      ".cm-blameCell.cm-blameClickable",
    );
    expect(cell).not.toBeNull();
    await act(async () => cell!.click());
    expect(onOpenCommit).toHaveBeenCalledExactlyOnceWith(CWD, {
      sha: OLD.hunks[0].sha,
      shortSha: "aaa1111",
      subject: "summary aaa1111",
    });
  });
});
