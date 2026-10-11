// @vitest-environment happy-dom
import { act, createElement, type ReactNode } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  MailThreadList,
  useMailThreadList,
  formatMailDate,
  scopeSpansDomains,
  type ListBase,
} from "./MailThreadList";
import { mailThreads } from "../model/mail";
import { listen } from "@tauri-apps/api/event";
import { SEARCH_DEBOUNCE_MS } from "../model/mailQuery";
import type { Rule, ThreadRow, ThreadScope } from "../model/mailTypes";

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
const folder: ThreadScope = { kind: "folder", folderId: 7 };

const baseOf = (scope: ThreadScope): ListBase =>
  scope.kind === "mailbox"
    ? { kind: "mailbox", mailboxId: scope.mailboxId, address: "owner@a.com" }
    : scope.kind === "folder"
      ? { kind: "folder", folderId: scope.folderId }
      : { kind: "all" };
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
  const list = useMailThreadList(baseOf(scope));
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

it("resets the window on a scope change even when the first fetch fails", async () => {
  const rows = Array.from({ length: 70 }, (_, i) => thread(i + 1));
  vi.mocked(mailThreads)
    .mockResolvedValueOnce({ rows, nextCursor: null })
    .mockRejectedValueOnce(new Error("boom"))
    .mockResolvedValue({ rows, nextCursor: null });
  await render(all, twoDomains);
  await act(async () => intersect!());
  expect(rowCount()).toBe(64);
  await render(oneBox, twoDomains);
  // The fetch failed, yet the window no longer carries the old scope's size.
  expect(rowCount()).toBe(32);
});

it("exposes rows as list items holding buttons", async () => {
  vi.mocked(mailThreads).mockResolvedValue({
    rows: [thread(1)],
    nextCursor: null,
  });
  await render(all, oneDomain);
  expect(
    container.querySelector(
      "ul[role='list'][aria-label='Threads'] > li > button[data-thread-id]",
    ),
  ).not.toBeNull();
  // role="listitem" on the button would hide that it is clickable.
  expect(container.querySelector("button[role]")).toBeNull();
});

// ── server-side filters ────────────────────────────────────────────

describe("search box and chips", () => {
  const rule = (
    field: Rule["field"],
    op: Rule["op"],
    value: Rule["value"] = "",
    negate = false,
  ): Rule => ({ field, op, value, negate });

  const typeInto = async (el: HTMLInputElement, value: string) =>
    act(async () => {
      Object.getOwnPropertyDescriptor(
        HTMLInputElement.prototype,
        "value",
      )!.set!.call(el, value);
      el.dispatchEvent(new Event("input", { bubbles: true }));
    });
  const box = () =>
    container.querySelector<HTMLInputElement>(
      "input[aria-label='Search mail']",
    )!;
  const pause = (ms = SEARCH_DEBOUNCE_MS) =>
    act(async () => {
      await vi.advanceTimersByTimeAsync(ms);
    });
  const lastScope = () => vi.mocked(mailThreads).mock.calls.at(-1)![0].scope;
  const calls = () => vi.mocked(mailThreads).mock.calls.length;

  beforeEach(() => {
    vi.useFakeTimers();
    vi.mocked(mailThreads).mockResolvedValue({ rows: [], nextCursor: null });
  });
  afterEach(() => vi.useRealTimers());

  it("waits for a pause in typing before it queries", async () => {
    await render(all, twoDomains);
    expect(calls()).toBe(1);
    await typeInto(box(), "inv");
    await pause(SEARCH_DEBOUNCE_MS - 1);
    await typeInto(box(), "invo");
    await pause(SEARCH_DEBOUNCE_MS - 1);
    // Two keystrokes and a nearly-elapsed pause: still nothing sent.
    expect(calls()).toBe(1);
    await pause(1);
    expect(calls()).toBe(2);
    expect(lastScope()).toEqual({
      kind: "rules",
      rules: [rule("body", "contains_prefix", "invo")],
      matchMode: "all",
    });
  });

  it("prefix-matches only the last word, and only once it is long enough", async () => {
    await render(all, twoDomains);
    await typeInto(box(), "from:bob inv");
    await pause();
    expect(lastScope()).toEqual({
      kind: "rules",
      rules: [
        rule("from", "contains", "bob"),
        rule("body", "contains_prefix", "inv"),
      ],
      matchMode: "all",
    });
    await typeInto(box(), "from:bob in");
    await pause();
    expect(lastScope()).toEqual({
      kind: "rules",
      rules: [rule("from", "contains", "bob"), rule("body", "contains", "in")],
      matchMode: "all",
    });
  });

  it("keeps a chip an exact phrase", async () => {
    await render(all, twoDomains);
    const chip = [...container.querySelectorAll("button")].find(
      (b) => b.textContent === "Content",
    )!;
    await act(async () => chip.click());
    await typeInto(
      container.querySelector<HTMLInputElement>(
        "input[aria-label='Filter by content']",
      )!,
      "invoice",
    );
    await pause();
    expect(lastScope()).toEqual({
      kind: "rules",
      rules: [rule("body", "contains", "invoice")],
      matchMode: "all",
    });
  });

  it("refuses a bare wildcard instead of listing the whole mailbox", async () => {
    await render(all, twoDomains);
    await typeInto(box(), "-%");
    await pause();
    // No query went out, so nothing replaced the list.
    expect(calls()).toBe(1);
    expect(container.querySelector("[role='status']")?.textContent).toContain(
      "%",
    );
  });

  it("keeps the last valid query while the text is not valid", async () => {
    await render(all, twoDomains);
    await typeInto(box(), "invoice is:unread");
    await pause();
    expect(calls()).toBe(2);
    // Half-way through retyping the flag, the token is dropped by the parser.
    // Running that would flash every invoice, read or not.
    await typeInto(box(), "invoice is:unre");
    await pause();
    expect(calls()).toBe(2);
    expect(container.querySelector("[role='status']")?.textContent).toContain(
      "is:unre",
    );
    await typeInto(box(), "invoice is:starred");
    await pause();
    expect(lastScope()).toEqual({
      kind: "rules",
      rules: [rule("body", "contains", "invoice"), rule("is_starred", "true")],
      matchMode: "all",
    });
  });

  it("scopes a filter to the open mailbox and drops back when cleared", async () => {
    await render(oneBox, twoDomains);
    await typeInto(box(), "invoice ");
    await pause();
    expect(lastScope()).toEqual({
      kind: "rules",
      rules: [
        rule("mailbox", "is", "owner@a.com"),
        rule("body", "contains", "invoice"),
      ],
      matchMode: "all",
    });
    await typeInto(box(), "");
    await pause();
    expect(lastScope()).toEqual({ kind: "mailbox", mailboxId: 1 });
  });

  it("filters the whole mailbox, not the pages loaded so far", async () => {
    vi.mocked(mailThreads).mockImplementation(async ({ scope }) => ({
      rows:
        scope.kind === "rules"
          ? [thread(900, { subject: "Needle far down the mailbox" })]
          : [thread(1)],
      nextCursor: null,
    }));
    await render(all, twoDomains);
    await typeInto(box(), "needle ");
    await pause();
    expect(rowCount()).toBe(1);
    expect(container.textContent).toContain("Needle far down");
    expect(container.textContent).not.toContain("loaded so far");
  });

  it("keeps the text when the list remounts", async () => {
    await render(all, twoDomains);
    await typeInto(box(), "invoice");
    await render(all, twoDomains, {}, "stacked");
    expect(box().value).toBe("invoice");
  });

  it("saves the exact phrases, not the prefix used while typing", async () => {
    const onSaveAsFolder = vi.fn();
    await render(all, twoDomains, { onSaveAsFolder });
    await typeInto(box(), "from:bob invo");
    const save = [...container.querySelectorAll("button")].find(
      (b) => b.textContent === "Save as folder",
    )!;
    await act(async () => save.click());
    expect(onSaveAsFolder).toHaveBeenCalledWith(
      [rule("from", "contains", "bob"), rule("body", "contains", "invo")],
      "all",
    );
  });

  it("will not save an empty filter or one with a mistake", async () => {
    await render(all, twoDomains, { onSaveAsFolder: vi.fn() });
    const save = () =>
      [...container.querySelectorAll("button")].find(
        (b) => b.textContent === "Save as folder",
      ) as HTMLButtonElement;
    expect(save().disabled).toBe(true);
    await typeInto(box(), "-%");
    expect(save().disabled).toBe(true);
    await typeInto(box(), "bob");
    expect(save().disabled).toBe(false);
  });
});

describe("a smart folder", () => {
  it("lists its members with no filters on top", async () => {
    vi.mocked(mailThreads).mockResolvedValue({
      rows: [thread(5)],
      nextCursor: null,
    });
    const onEditFolder = vi.fn();
    await render(folder, twoDomains, { onEditFolder });
    expect(mailThreads).toHaveBeenCalledWith({
      scope: { kind: "folder", folderId: 7 },
      limit: 100,
    });
    expect(
      container.querySelector("input[aria-label='Search mail']"),
    ).toBeNull();
    const edit = [...container.querySelectorAll("button")].find((b) =>
      b.textContent?.includes("Edit folder"),
    )!;
    await act(async () => edit.click());
    expect(onEditFolder).toHaveBeenCalled();
  });

  it("shows mail that arrives after it was saved", async () => {
    vi.mocked(mailThreads)
      .mockResolvedValueOnce({
        rows: [thread(1, { subject: "stored invoice" })],
        nextCursor: null,
      })
      .mockResolvedValueOnce({
        rows: [
          thread(2, { subject: "later invoice" }),
          thread(1, { subject: "stored invoice" }),
        ],
        nextCursor: null,
      });
    let fire: (() => void) | undefined;
    vi.mocked(listen).mockImplementation((async (
      _event: string,
      cb: () => void,
    ) => {
      fire = cb;
      return () => undefined;
    }) as unknown as typeof listen);
    await render(folder, twoDomains);
    expect(container.textContent).toContain("stored invoice");
    expect(container.textContent).not.toContain("later invoice");
    // Sync files the new message and announces it; the list reads the folder again.
    await act(async () => fire!());
    expect(container.textContent).toContain("later invoice");
    expect(mailThreads).toHaveBeenLastCalledWith({
      scope: { kind: "folder", folderId: 7 },
      limit: 100,
    });
  });

  it("drops an in-flight page when the scope moves on", async () => {
    const cursor = { lastMessageAt: 1, id: 32 };
    let resolveOld!: (v: { rows: ThreadRow[]; nextCursor: null }) => void;
    vi.mocked(mailThreads)
      .mockResolvedValueOnce({
        rows: Array.from({ length: 32 }, (_, i) => thread(i + 1)),
        nextCursor: cursor,
      })
      .mockImplementationOnce(
        () => new Promise((resolve) => (resolveOld = resolve)),
      )
      .mockResolvedValueOnce({ rows: [], nextCursor: null });
    await render(all, twoDomains);
    await act(async () => intersect!());
    await render(folder, twoDomains);
    await act(async () =>
      resolveOld({ rows: [thread(900)], nextCursor: null }),
    );
    expect(rowCount()).toBe(0);
  });
});
