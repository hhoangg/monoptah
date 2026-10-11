// @vitest-environment happy-dom
import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { MailView } from "./MailView";
import { MAIL_PREF_KEYS, readingPaneSide } from "../model/mailPrefs";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(async (cmd: string) =>
    cmd === "mail_threads" ? { rows: [], nextCursor: null } : null,
  ),
}));
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(async () => () => undefined),
}));
vi.mock("../../../app/shell/WindowControls", () => ({
  WindowControls: () => null,
}));

let container: HTMLDivElement;
let root: Root;

beforeEach(() => {
  (
    globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }
  ).IS_REACT_ACT_ENVIRONMENT = true;
  localStorage.clear();
  container = document.createElement("div");
  document.body.append(container);
  root = createRoot(container);
});

afterEach(() => {
  act(() => root.unmount());
  container.remove();
});

const render = () =>
  act(() =>
    root.render(
      createElement(MailView, { besideRail: true, onClose: vi.fn() }),
    ),
  );

const pane = (name: string) =>
  container.querySelector<HTMLElement>(`[data-mail-pane="${name}"]`);

it("renders the folder, list and reading panes", () => {
  render();
  expect(pane("folders")).not.toBeNull();
  expect(pane("threads")).not.toBeNull();
  expect(pane("reader")).not.toBeNull();
});

it("starts the list at the persisted width", () => {
  localStorage.setItem(MAIL_PREF_KEYS.listWidth, "420");
  render();
  expect(pane("threads")!.style.width).toBe("420px");
});

it("drops the reading pane when it is off", () => {
  localStorage.setItem(MAIL_PREF_KEYS.readingPaneSide, "off");
  render();
  expect(pane("reader")).toBeNull();
  expect(container.querySelector('[role="separator"]')).toBeNull();
});

it("clears the list width when the reading pane moves to the bottom", () => {
  render();
  expect(pane("threads")!.style.width).not.toBe("");
  act(() => {
    localStorage.setItem(MAIL_PREF_KEYS.readingPaneSide, "bottom");
    readingPaneSide.save("bottom");
  });
  expect(pane("threads")!.style.width).toBe("");
  act(() => readingPaneSide.save("right"));
  expect(pane("threads")!.style.width).not.toBe("");
});

it("stacks the reading pane under the list when it is at the bottom", () => {
  localStorage.setItem(MAIL_PREF_KEYS.readingPaneSide, "bottom");
  render();
  expect(pane("reader")).not.toBeNull();
  expect(container.querySelector('[role="separator"]')).toBeNull();
});

const folderRow = (id: number) => ({
  id,
  name: `Folder ${id}`,
  color: null,
  sortOrder: id,
  matchMode: "all",
  hideFromMailbox: false,
  countUnread: true,
  rules: [],
  threadCount: 0,
  unreadCount: 0,
});

const threadScopes = () =>
  vi
    .mocked(invoke)
    .mock.calls.filter(([cmd]) => cmd === "mail_threads")
    .map(([, args]) => (args as { query: { scope: unknown } }).query.scope);

const withFolders = (ids: number[]) => {
  // Calls from earlier tests in this file would otherwise count.
  vi.mocked(invoke).mockClear();
  vi.mocked(invoke).mockImplementation((async (cmd: string) =>
    cmd === "mail_threads"
      ? { rows: [], nextCursor: null }
      : cmd === "mail_folders"
        ? ids.map(folderRow)
        : null) as never);
};

it("lists the selected smart folder by its numeric id", async () => {
  withFolders([7]);
  localStorage.setItem(MAIL_PREF_KEYS.selectedFolder, "7");
  await act(async () => render());
  expect(threadScopes()).toEqual([{ kind: "folder", folderId: 7 }]);
});

it("falls back to all mail for a folder that no longer exists", async () => {
  withFolders([7]);
  localStorage.setItem(MAIL_PREF_KEYS.selectedFolder, "9");
  await act(async () => render());
  expect(threadScopes().at(-1)).toEqual({ kind: "all" });
});

it("treats a damaged folder pref as no folder, not as NaN", async () => {
  withFolders([7]);
  localStorage.setItem(MAIL_PREF_KEYS.selectedFolder, "abc");
  await act(async () => render());
  expect(JSON.stringify(threadScopes())).not.toContain("null");
  expect(threadScopes().at(-1)).toEqual({ kind: "all" });
});
