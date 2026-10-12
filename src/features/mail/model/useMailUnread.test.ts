// @vitest-environment happy-dom
import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { MAIL_MESSAGES_CHANGED_EVENT } from "./mail";
import { useMailUnread } from "./useMailUnread";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));

let container: HTMLDivElement;
let root: Root;
let unreadTotal = 0;
const handlers = new Map<string, () => void>();

function Probe({ enabled }: { enabled: boolean }) {
  return createElement("span", null, String(useMailUnread(enabled)));
}
const show = async (enabled: boolean) =>
  act(async () => root.render(createElement(Probe, { enabled })));
const statusCalls = () =>
  vi.mocked(invoke).mock.calls.filter(([c]) => c === "mail_status").length;

beforeEach(() => {
  (
    globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }
  ).IS_REACT_ACT_ENVIRONMENT = true;
  unreadTotal = 4;
  handlers.clear();
  vi.mocked(invoke).mockReset();
  vi.mocked(invoke).mockImplementation((async () => ({
    sources: [],
    unreadTotal,
    syncIntervalSecs: 3600,
  })) as never);
  vi.mocked(listen).mockReset();
  vi.mocked(listen).mockImplementation((async (
    event: string,
    handler: () => void,
  ) => {
    handlers.set(event, handler);
    return () => handlers.delete(event);
  }) as never);
  container = document.createElement("div");
  document.body.append(container);
  root = createRoot(container);
});

afterEach(() => {
  act(() => root.unmount());
  container.remove();
});

it("reads the unread total Rust sums over enabled sources", async () => {
  await show(true);
  expect(container.textContent).toBe("4");
});

it("reads nothing and listens to nothing while Mail is off", async () => {
  await show(false);
  expect(container.textContent).toBe("0");
  expect(statusCalls()).toBe(0);
  expect(listen).not.toHaveBeenCalled();
});

it("re-reads when messages change, and drops to 0 when Mail is turned off", async () => {
  await show(true);
  unreadTotal = 9;
  expect(handlers.has(MAIL_MESSAGES_CHANGED_EVENT)).toBe(true);
  await act(async () => handlers.get(MAIL_MESSAGES_CHANGED_EVENT)!());
  expect(container.textContent).toBe("9");
  await show(false);
  expect(container.textContent).toBe("0");
  expect(handlers.size).toBe(0);
});

it("keeps the last count when a read fails", async () => {
  await show(true);
  vi.mocked(invoke).mockRejectedValue("boom");
  expect(handlers.has(MAIL_MESSAGES_CHANGED_EVENT)).toBe(true);
  await act(async () => handlers.get(MAIL_MESSAGES_CHANGED_EVENT)!());
  expect(container.textContent).toBe("4");
});
