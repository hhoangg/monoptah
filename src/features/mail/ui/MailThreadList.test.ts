// @vitest-environment happy-dom
import { act, createElement, type ReactNode } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import {
  MailThreadList,
  useMailThreadList,
  filterThreads,
  formatMailDate,
  NO_FILTERS,
  scopeSpansDomains,
} from "./MailThreadList";
import { mailThreads } from "../model/mail";
import { listen } from "@tauri-apps/api/event";
import type { ThreadRow, ThreadScope } from "../model/mailTypes";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(async () => () => undefined),
}));
vi.mock("../model/mail", () => ({
  MAIL_MESSAGES_CHANGED_EVENT: "mail://messages-changed",
  mailThreads: vi.fn(),
}));

const thread = (id: number, over: Partial<ThreadRow> = {}): ThreadRow => ({
  id,
  accountId: 1,
  mailboxId: 1,
  mailboxAddress: "owner@a.com",
  domain: "a.com",
  subject: `Subject ${id}`,
  snippet: `Snippet ${id}`,
  participants: [{ name: "Linh", addr: "linh@x.com" }],
  firstMessageAt: 1_700_000_000,
  lastMessageAt: 1_700_000_000 - id,
  messageCount: 1,
  unreadCount: 0,
  hasAttachments: false,
  isStarred: false,
  ...over,
});

const all: ThreadScope = { kind: "all" };
const oneBox: ThreadScope = { kind: "mailbox", mailboxId: 1 };
const twoDomains = [{ address: "owner@a.com" }, { address: "x@b.com" }];
const oneDomain = [{ address: "owner@a.com" }, { address: "y@a.com" }];

let container: HTMLDivElement;
let root: Root;
let intersect: (() => void) | null;

beforeEach(() => {
  (
    globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }
  ).IS_REACT_ACT_ENVIRONMENT = true;
  localStorage.clear();
  intersect = null;
  class FakeObserver {
    constructor(cb: (e: { isIntersecting: boolean }[]) => void) {
      intersect = () => cb([{ isIntersecting: true }]);
    }
    observe() {}
    disconnect() {
      intersect = null;
    }
  }
  vi.stubGlobal("IntersectionObserver", FakeObserver);
  vi.mocked(mailThreads).mockReset();
  container = document.createElement("div");
  document.body.append(container);
  root = createRoot(container);
});

afterEach(() => {
  act(() => root.unmount());
  container.remove();
  vi.unstubAllGlobals();
});

// Stands in for MailView: list state above, the keyed list below.
function Harness({
  scope,
  mailboxes,
  listKey,
  extra,
}: {
  scope: ThreadScope;
  mailboxes: { address: string }[];
  listKey: string;
  extra: Record<string, unknown>;
}): ReactNode {
  const list = useMailThreadList(scope, false);
  return createElement(MailThreadList, {
    key: listKey,
    list,
    scope,
    mailboxes,
    selectedId: null,
    onSelect: vi.fn(),
    ...extra,
  });
}

const render = async (
  scope: ThreadScope,
  mailboxes: { address: string }[],
  props: Record<string, unknown> = {},
  listKey = "beside",
) => {
  await act(async () =>
    root.render(
      createElement(Harness, { scope, mailboxes, listKey, extra: props }),
    ),
  );
};

const rowCount = () => container.querySelectorAll("[data-thread-id]").length;

it("grows the window on scroll and stops at the end", async () => {
  const rows = Array.from({ length: 70 }, (_, i) => thread(i + 1));
  vi.mocked(mailThreads).mockResolvedValue({ rows, nextCursor: null });
  await render(all, twoDomains);
  expect(rowCount()).toBe(32);
  await act(async () => intersect!());
  expect(rowCount()).toBe(64);
  await act(async () => intersect!());
  expect(rowCount()).toBe(70);
  // Nothing left to window or fetch: the sentinel is gone.
  expect(intersect).toBeNull();
  expect(mailThreads).toHaveBeenCalledTimes(1);
});

it("keeps the window and filters when the list remounts", async () => {
  const rows = Array.from({ length: 70 }, (_, i) => thread(i + 1));
  vi.mocked(mailThreads).mockResolvedValue({ rows, nextCursor: null });
  await render(all, twoDomains);
  await act(async () => intersect!());
  expect(rowCount()).toBe(64);
  await render(all, twoDomains, {}, "stacked");
  expect(rowCount()).toBe(64);
  expect(mailThreads).toHaveBeenCalledTimes(1);
});

it("fetches the next page only once the window reaches the loaded rows", async () => {
  const first = Array.from({ length: 32 }, (_, i) => thread(i + 1));
  const second = Array.from({ length: 5 }, (_, i) => thread(100 + i));
  const cursor = { lastMessageAt: 1, id: 32 };
  vi.mocked(mailThreads)
    .mockResolvedValueOnce({ rows: first, nextCursor: cursor })
    .mockResolvedValueOnce({ rows: second, nextCursor: null });
  await render(all, twoDomains);
  expect(rowCount()).toBe(32);
  await act(async () => intersect!());
  expect(mailThreads).toHaveBeenLastCalledWith(
    expect.objectContaining({ cursor }),
  );
  expect(rowCount()).toBe(37);
  expect(intersect).toBeNull();
});

it("shows the domain chip when the scope spans domains and hides it otherwise", async () => {
  vi.mocked(mailThreads).mockResolvedValue({
    rows: [thread(1)],
    nextCursor: null,
  });
  await render(all, twoDomains);
  expect(container.querySelector("[data-chip='domain']")?.textContent).toBe(
    "a.com",
  );
  await render(all, oneDomain);
  expect(container.querySelector("[data-chip='domain']")).toBeNull();
  await render(oneBox, twoDomains);
  expect(container.querySelector("[data-chip='domain']")).toBeNull();
  expect(container.querySelector("[data-chip='mailbox']")?.textContent).toBe(
    "owner@",
  );
});

it("scopeSpansDomains counts distinct domains", () => {
  expect(scopeSpansDomains(all, twoDomains)).toBe(true);
  expect(scopeSpansDomains(all, oneDomain)).toBe(false);
  expect(scopeSpansDomains(oneBox, twoDomains)).toBe(false);
});

it("renders unread, star, attachment and message count", async () => {
  vi.mocked(mailThreads).mockResolvedValue({
    rows: [
      thread(1, {
        unreadCount: 2,
        isStarred: true,
        hasAttachments: true,
        messageCount: 4,
      }),
    ],
    nextCursor: null,
  });
  await render(all, oneDomain);
  const text = container.textContent!;
  expect(container.querySelector("[aria-label='Unread']")).not.toBeNull();
  expect(container.querySelector("[aria-label='Starred']")).not.toBeNull();
  expect(
    container.querySelector("[aria-label='Has attachments']"),
  ).not.toBeNull();
  expect(text).toContain("4");
  expect(text).toContain("Linh");
});

it("filters loaded rows by content", () => {
  const rows = [thread(1), thread(2, { subject: "Invoice" })];
  expect(filterThreads(rows, NO_FILTERS)).toHaveLength(2);
  expect(
    filterThreads(rows, { ...NO_FILTERS, content: "invoice" }).map((r) => r.id),
  ).toEqual([2]);
});

it("formats dates from epoch seconds, not milliseconds", () => {
  const at = (y: number, m: number, d: number, h = 0, min = 0) =>
    Math.floor(new Date(y, m, d, h, min).getTime() / 1000);
  const now = new Date(2026, 9, 11, 15, 0).getTime();
  expect(formatMailDate(at(2026, 9, 11, 11, 4), now)).toMatch(/11.04/);
  expect(formatMailDate(at(2026, 9, 10, 23, 59), now)).toBe("Yesterday");
  expect(formatMailDate(at(2026, 9, 8, 9, 0), now)).toBe(
    new Date(2026, 9, 8).toLocaleDateString([], { weekday: "short" }),
  );
  expect(formatMailDate(at(2026, 8, 2, 9, 0), now)).toBe(
    new Date(2026, 8, 2).toLocaleDateString([], {
      month: "short",
      day: "numeric",
    }),
  );
  expect(formatMailDate(at(2025, 8, 2, 9, 0), now)).toContain("2025");
  // The January 1970 bug this guards against.
  expect(formatMailDate(at(2026, 9, 11, 11, 4), now)).not.toContain("1970");
});

it("drops a page fetched under the previous scope", async () => {
  const cursor = { lastMessageAt: 1, id: 32 };
  const first = Array.from({ length: 32 }, (_, i) => thread(i + 1));
  let resolveOld!: (v: { rows: ThreadRow[]; nextCursor: null }) => void;
  vi.mocked(mailThreads)
    .mockResolvedValueOnce({ rows: first, nextCursor: cursor })
    .mockImplementationOnce(
      () => new Promise((resolve) => (resolveOld = resolve)),
    )
    .mockResolvedValueOnce({
      rows: [thread(500, { subject: "New scope" })],
      nextCursor: null,
    });
  await render(all, twoDomains);
  await act(async () => intersect!());
  await render(oneBox, twoDomains);
  await act(async () => resolveOld({ rows: [thread(900)], nextCursor: null }));
  expect(rowCount()).toBe(1);
  expect(container.textContent).toContain("New scope");
  expect(container.querySelector("[data-thread-id='900']")).toBeNull();
});

it("keeps the window across a background reload of the same scope", async () => {
  const rows = Array.from({ length: 70 }, (_, i) => thread(i + 1));
  vi.mocked(mailThreads).mockResolvedValue({ rows, nextCursor: null });
  let fire: (() => void) | undefined;
  vi.mocked(listen).mockImplementation((async (
    _event: string,
    cb: () => void,
  ) => {
    fire = cb;
    return () => undefined;
  }) as unknown as typeof listen);
  await render(all, twoDomains);
  await act(async () => intersect!());
  expect(rowCount()).toBe(64);
  await act(async () => fire!());
  expect(mailThreads).toHaveBeenCalledTimes(2);
  expect(rowCount()).toBe(64);
});

it("keeps loading pages while a filter hides every loaded row", async () => {
  const cursor = { lastMessageAt: 1, id: 32 };
  const page = (from: number) =>
    Array.from({ length: 32 }, (_, i) => thread(from + i));
  vi.mocked(mailThreads)
    .mockResolvedValueOnce({ rows: page(1), nextCursor: cursor })
    .mockResolvedValueOnce({
      rows: [thread(200, { subject: "Needle" })],
      nextCursor: null,
    });
  await render(all, twoDomains);
  const input = async () => {
    const chips = [...container.querySelectorAll("button")];
    await act(async () =>
      chips.find((b) => b.textContent === "Content")!.click(),
    );
    return container.querySelector<HTMLInputElement>("input")!;
  };
  const field = await input();
  await act(async () => {
    const set = Object.getOwnPropertyDescriptor(
      HTMLInputElement.prototype,
      "value",
    )!.set!;
    set.call(field, "needle");
    field.dispatchEvent(new Event("input", { bubbles: true }));
  });
  expect(rowCount()).toBe(0);
  expect(intersect).not.toBeNull();
  await act(async () => intersect!());
  expect(rowCount()).toBe(1);
  expect(container.textContent).toContain("loaded so far");
});

it("exposes rows as list items holding buttons", async () => {
  vi.mocked(mailThreads).mockResolvedValue({
    rows: [thread(1)],
    nextCursor: null,
  });
  await render(all, oneDomain);
  expect(
    container.querySelector("ul > li > button[data-thread-id]"),
  ).not.toBeNull();
});
