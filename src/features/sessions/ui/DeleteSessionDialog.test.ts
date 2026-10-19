// @vitest-environment happy-dom
import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  DeleteSessionDialog,
  providerFilesKind,
} from "./DeleteSessionDialog";

let container: HTMLDivElement;
let root: Root;

beforeEach(() => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  container = document.createElement("div");
  document.body.append(container);
  root = createRoot(container);
});
afterEach(async () => {
  await act(async () => root.unmount());
  container.remove();
  vi.unstubAllGlobals();
});

const button = (text: string) =>
  [...document.querySelectorAll<HTMLButtonElement>("button")].find(
    (node) => node.textContent === text,
  )!;
const checkboxes = () =>
  [...document.querySelectorAll<HTMLInputElement>('input[type="checkbox"]')];

describe("DeleteSessionDialog", () => {
  it("checks both options by default and reports them", async () => {
    const onClose = vi.fn();
    await act(async () =>
      root.render(
        createElement(DeleteSessionDialog, {
          title: "Fix bug",
          unusedWorktrees: ["/repo-worktrees/feature"],
          providerFiles: "claude",
          onClose,
        }),
      ),
    );
    expect(checkboxes().map((box) => box.checked)).toEqual([true, true]);
    expect(document.body.textContent).toContain(
      "Also delete the Claude Code session files on this computer",
    );
    expect(document.body.textContent).toContain("Delete session?");
    await act(async () => button("Delete session").click());
    expect(onClose).toHaveBeenLastCalledWith({
      confirmed: true,
      deleteWorktree: true,
      deleteProviderFiles: true,
    });
    await act(async () => checkboxes()[0].click());
    await act(async () => button("Delete session").click());
    expect(onClose).toHaveBeenLastCalledWith({
      confirmed: true,
      deleteWorktree: true,
      deleteProviderFiles: false,
    });
  });

  it("hides the options that do not apply and never reports them", async () => {
    const onClose = vi.fn();
    await act(async () =>
      root.render(
        createElement(DeleteSessionDialog, { title: "Fix bug", onClose }),
      ),
    );
    expect(checkboxes()).toHaveLength(0);
    await act(async () => button("Delete session").click());
    expect(onClose).toHaveBeenLastCalledWith({
      confirmed: true,
      deleteWorktree: false,
      deleteProviderFiles: false,
    });
  });

  it("names Codex, and the generic harness when a selection mixes both", async () => {
    await act(async () =>
      root.render(
        createElement(DeleteSessionDialog, {
          title: "Fix bug",
          providerFiles: "codex",
          onClose: vi.fn(),
        }),
      ),
    );
    expect(document.body.textContent).toContain(
      "Also delete the Codex session files on this computer",
    );
    await act(async () =>
      root.render(
        createElement(DeleteSessionDialog, {
          title: "",
          count: 3,
          providerFiles: "mixed",
          onClose: vi.fn(),
        }),
      ),
    );
    expect(document.body.textContent).toContain(
      "Also delete the harness session files on this computer",
    );
  });

  it("asks once for several sessions and cancels without a choice", async () => {
    const onClose = vi.fn();
    await act(async () =>
      root.render(
        createElement(DeleteSessionDialog, {
          title: "",
          count: 3,
          unusedWorktrees: ["/a", "/b"],
          onClose,
        }),
      ),
    );
    expect(document.body.textContent).toContain("Delete 3 sessions?");
    expect(document.body.textContent).toContain(
      "3 sessions will be permanently deleted.",
    );
    expect(document.body.textContent).toContain(
      "Also delete unused worktrees",
    );
    await act(async () => button("Cancel").click());
    expect(onClose).toHaveBeenLastCalledWith({
      confirmed: false,
      deleteWorktree: false,
      deleteProviderFiles: false,
    });
    await act(async () => button("Delete 3 sessions").click());
    expect(onClose).toHaveBeenLastCalledWith({
      confirmed: true,
      deleteWorktree: true,
      deleteProviderFiles: false,
    });
  });
});

describe("providerFilesKind", () => {
  it("only applies to Claude Code and Codex sessions that have provider files", () => {
    expect(
      providerFilesKind({ harness: "claude", providerSessionId: "p1" }),
    ).toBe("claude");
    expect(
      providerFilesKind({ harness: "codex", providerSessionId: "p1" }),
    ).toBe("codex");
    expect(providerFilesKind({ harness: "claude" })).toBeUndefined();
    expect(
      providerFilesKind({ harness: "cursor", providerSessionId: "p1" }),
    ).toBeUndefined();
  });
});
