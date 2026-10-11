// @vitest-environment happy-dom
import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { MailView } from "./MailView";
import { MAIL_PREF_KEYS } from "../model/mailPrefs";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
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

it("stacks the reading pane under the list when it is at the bottom", () => {
  localStorage.setItem(MAIL_PREF_KEYS.readingPaneSide, "bottom");
  render();
  expect(pane("reader")).not.toBeNull();
  expect(container.querySelector('[role="separator"]')).toBeNull();
});
