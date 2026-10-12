// @vitest-environment happy-dom
import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { MailView } from "./MailView";
import { MAIL_PREF_KEYS } from "../model/mailPrefs";
import type { MessageRow, ThreadRow } from "../model/mailTypes";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(async () => () => undefined),
}));
vi.mock("../../../app/shell/WindowControls", () => ({
  WindowControls: () => null,
}));
// The real composer pulls in the editor; the request it is given is what matters.
vi.mock("./ComposeSheet", async () => {
  const { createElement: h } = await import("react");
  return {
    preloadMailEditor: () => undefined,
    ComposeSheet: ({
      request,
    }: {
      request: { kind: string; message?: { id: number } };
    }) =>
      h("div", {
        "data-compose": request.kind,
        "data-compose-message": request.message?.id,
      }),
  };
});

const row = (id: number): ThreadRow => ({
  id,
  accountId: 1,
  mailboxId: 1,
  mailboxAddress: "owner@a.com",
  domain: "a.com",
  subject: `Subject ${id}`,
  snippet: "",
  participants: [{ name: "Ann", addr: "ann@x.com" }],
  firstMessageAt: 1_700_000_000,
  lastMessageAt: 1_700_000_000 - id,
  messageCount: 1,
  unreadCount: 0,
  hasAttachments: false,
  isStarred: false,
});

const message = (id: number, dateUtc: number): MessageRow =>
  ({
    id,
    direction: "inbound",
    subject: "S",
    fromName: null,
    fromAddr: "ann@x.com",
    to: [],
    cc: [],
    bcc: [],
    dateUtc,
    snippet: "",
    bodyText: `body ${id}`,
    bodyHtml: null,
    remoteImagesBlocked: 0,
    hasAttachments: false,
    isRead: true,
    isStarred: false,
    state: "active",
    attachments: [],
  }) as unknown as MessageRow;

const status = {
  sources: [
    {
      id: 1,
      label: "Main",
      enabled: true,
      sortOrder: 1,
      lastError: null,
      unread: 0,
      mailboxes: [
        {
          id: 1,
          accountId: 1,
          address: "owner@a.com",
          domain: "a.com",
          s3Prefix: "owner/",
          displayName: "",
          color: null,
          sortOrder: 1,
          isSendable: true,
          unread: 0,
          sync: null,
        },
      ],
    },
  ],
  unreadTotal: 0,
  syncIntervalSecs: 3600,
};

let container: HTMLDivElement;
let root: Root;
const calls = (cmd: string) =>
  vi.mocked(invoke).mock.calls.filter(([c]) => c === cmd);

beforeEach(() => {
  (
    globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }
  ).IS_REACT_ACT_ENVIRONMENT = true;
  localStorage.clear();
  vi.mocked(invoke).mockReset();
  vi.mocked(invoke).mockImplementation((async (cmd: string) => {
    switch (cmd) {
      case "mail_status":
        return status;
      case "mail_threads":
        return { rows: [row(1), row(2), row(3)], nextCursor: null };
      case "mail_folders":
      case "mail_outbox_list":
        return [];
      case "mail_thread":
        return {
          thread: row(2),
          imagePolicy: "ask",
          restorable: false,
          // Newest first on purpose: the array order must not decide.
          messages: [message(21, 300), message(20, 100)],
        };
      case "mail_delete":
        return {
          removed: 1,
          kept: 0,
          threadsRemoved: 1,
          filesLeft: 0,
          errors: [],
        };
      default:
        return null;
    }
  }) as never);
  container = document.createElement("div");
  document.body.append(container);
  root = createRoot(container);
});

afterEach(() => {
  act(() => root.unmount());
  container.remove();
});

const render = async () => {
  await act(async () =>
    root.render(
      createElement(MailView, { besideRail: true, onClose: vi.fn() }),
    ),
  );
};
const press = async (
  key: string,
  target: Element = document.body,
  init: KeyboardEventInit = {},
) =>
  act(async () => {
    target.dispatchEvent(
      new KeyboardEvent("keydown", { key, bubbles: true, ...init }),
    );
  });
const rowEl = (id: number) =>
  container.querySelector<HTMLElement>(`[data-thread-id="${id}"]`)!;
const cursorId = () =>
  container.querySelector("[data-cursor]")?.getAttribute("data-thread-id") ??
  null;
const openId = () =>
  container
    .querySelector("[aria-current='true'][data-thread-id]")
    ?.getAttribute("data-thread-id") ?? null;

it("j and k move the cursor, and focus follows it", async () => {
  await render();
  await press("j");
  expect(cursorId()).toBe("1");
  await press("j");
  await press("j");
  expect(cursorId()).toBe("3");
  expect(document.activeElement).toBe(rowEl(3));
  await press("j");
  expect(cursorId()).toBe("3");
  await press("k");
  expect(cursorId()).toBe("2");
});

it("moving the cursor opens nothing; Enter opens the cursor's thread", async () => {
  await render();
  await press("j");
  await press("j");
  expect(openId()).toBeNull();
  expect(calls("mail_thread")).toHaveLength(0);
  await press("Enter");
  expect(openId()).toBe("2");
  expect(calls("mail_thread")[0]![1]).toEqual({ threadId: 2 });
});

it("Enter on a focused button is left to the button", async () => {
  await render();
  await press("j");
  await press("Enter", container.querySelector("button")!);
  expect(openId()).toBeNull();
});

it("r replies to the newest message of the cursor's thread", async () => {
  await render();
  await press("j");
  await press("j");
  await press("r");
  expect(calls("mail_thread")[0]![1]).toEqual({ threadId: 2 });
  const compose = container.querySelector("[data-compose]")!;
  expect(compose.getAttribute("data-compose")).toBe("reply");
  // Message 21 is the newest by date even though it comes first in the array.
  expect(compose.getAttribute("data-compose-message")).toBe("21");
});

it("a replies to all", async () => {
  await render();
  await press("j");
  await press("a");
  expect(
    container.querySelector("[data-compose]")?.getAttribute("data-compose"),
  ).toBe("reply_all");
});

it("e archives the cursor's thread without asking, and moves on", async () => {
  await render();
  await press("j");
  await press("e");
  expect(calls("mail_delete")).toEqual([
    ["mail_delete", { threadIds: [1], hard: false }],
  ]);
  expect(document.querySelector('[role="dialog"]')).toBeNull();
  expect(cursorId()).toBe("2");
});

it("e does nothing in the Archive, where everything is already archived", async () => {
  localStorage.setItem(MAIL_PREF_KEYS.selectedFolder, "archived");
  await render();
  await press("j");
  await press("e");
  expect(calls("mail_delete")).toHaveLength(0);
});

it("# asks first; it never deletes on the keypress", async () => {
  await render();
  await press("j");
  await press("#", document.body, { shiftKey: true });
  expect(document.querySelector('[role="dialog"]')).not.toBeNull();
  expect(document.body.textContent).toContain("Delete 1 thread?");
  expect(calls("mail_delete")).toHaveLength(0);
});

it("ignores every key while a dialog is open", async () => {
  await render();
  await press("j");
  await press("#", document.body, { shiftKey: true });
  await press("e");
  await press("j");
  expect(calls("mail_delete")).toHaveLength(0);
  expect(cursorId()).toBe("1");
});

it("/ focuses the search box", async () => {
  await render();
  await press("/");
  expect(document.activeElement).toBe(
    container.querySelector('input[type="search"]'),
  );
});

it("leaves / alone where there is no search box", async () => {
  localStorage.setItem(MAIL_PREF_KEYS.selectedFolder, "archived");
  await render();
  const event = new KeyboardEvent("keydown", {
    key: "/",
    bubbles: true,
    cancelable: true,
  });
  await act(async () => {
    document.body.dispatchEvent(event);
  });
  expect(event.defaultPrevented).toBe(false);
});

it("does not take letters typed into the search box", async () => {
  await render();
  await press("/");
  const box = container.querySelector('input[type="search"]')!;
  await press("e", box);
  await press("j", box);
  expect(calls("mail_delete")).toHaveLength(0);
  expect(cursorId()).toBeNull();
});

it("stops listening when Mail closes, so no other view inherits the keys", async () => {
  await render();
  await press("j");
  act(() => root.unmount());
  // Unmounted, a leaked listener would still hold row 1 as its target.
  await press("e");
  expect(calls("mail_delete")).toHaveLength(0);
  root = createRoot(container);
});

it("with the reading pane off, Enter opens in place of the list and Escape returns", async () => {
  localStorage.setItem(MAIL_PREF_KEYS.readingPaneSide, "off");
  await render();
  await press("j");
  await press("Enter");
  expect(container.querySelector('[data-mail-pane="threads"]')).toBeNull();
  expect(container.querySelector('[data-mail-pane="reader"]')).not.toBeNull();
  await press("Escape");
  expect(container.querySelector('[data-mail-pane="threads"]')).not.toBeNull();
  expect(container.querySelector('[data-mail-pane="reader"]')).toBeNull();
  expect(document.activeElement).toBe(rowEl(1));
});

it("says so when a key's action fails, instead of doing nothing", async () => {
  vi.mocked(invoke).mockImplementation((async (cmd: string) => {
    if (cmd === "mail_threads") return { rows: [row(1)], nextCursor: null };
    if (cmd === "mail_thread") throw new Error("thread went away");
    if (cmd === "mail_delete") throw new Error("S3 said no");
    return cmd === "mail_status" ? status : [];
  }) as never);
  await render();
  await press("j");
  await press("r");
  const alert = () =>
    container.querySelector('[data-mail-pane="threads"] [role="alert"]');
  expect(alert()?.textContent).toBe("thread went away");
  expect(container.querySelector("[data-compose]")).toBeNull();
  await press("e");
  expect(alert()?.textContent).toBe("S3 said no");
});

const pagedThreads = (
  second: () => Promise<unknown> | unknown = () => ({
    rows: [row(4), row(5)],
    nextCursor: null,
  }),
) =>
  vi.mocked(invoke).mockImplementation((async (
    cmd: string,
    args?: { query?: { cursor?: unknown } },
  ) => {
    if (cmd === "mail_threads") {
      return args?.query?.cursor
        ? second()
        : {
            rows: [row(1), row(2), row(3)],
            nextCursor: { lastMessageAt: 1, id: 3 },
          };
    }
    return cmd === "mail_status" ? status : [];
  }) as never);

it("j on the last loaded row moves onto the next page's first row, with no second key", async () => {
  pagedThreads();
  await render();
  for (let i = 0; i < 3; i++) await press("j");
  expect(cursorId()).toBe("3");
  await press("j");
  expect(cursorId()).toBe("4");
});

it("drops a pending j when the folder changes before the page lands", async () => {
  let land!: (page: unknown) => void;
  pagedThreads(() => new Promise((resolve) => (land = resolve)));
  await render();
  for (let i = 0; i < 4; i++) await press("j");
  expect(cursorId()).toBe("3");
  await act(async () =>
    container.querySelector<HTMLElement>("nav button[aria-current]")!.click(),
  );
  await act(async () => land({ rows: [row(4), row(5)], nextCursor: null }));
  expect(cursorId()).toBeNull();
});

it("e moves off the archived thread even when the cursor is empty and the open thread is the first row", async () => {
  let listed = [row(1), row(2), row(3)];
  vi.mocked(invoke).mockImplementation((async (
    cmd: string,
    args?: { threadIds?: number[]; hard?: boolean },
  ) => {
    if (cmd === "mail_threads") return { rows: listed, nextCursor: null };
    if (cmd === "mail_delete") {
      listed = listed.filter((r) => !args!.threadIds!.includes(r.id));
      return {
        removed: 1,
        kept: 0,
        threadsRemoved: 1,
        filesLeft: 0,
        errors: [],
      };
    }
    if (cmd === "mail_thread")
      return {
        thread: row(1),
        imagePolicy: "ask",
        restorable: false,
        messages: [],
      };
    return cmd === "mail_status" ? status : [];
  }) as never);
  await render();
  await act(async () => rowEl(1).click());
  await press("j");
  // Hard-deleting the cursor's thread leaves no cursor but keeps thread 1 open.
  await press("#", document.body, { shiftKey: true });
  await act(async () =>
    document
      .querySelectorAll<HTMLInputElement>('input[type="radio"]')[1]!
      .click(),
  );
  await act(async () => {
    const input = document.getElementById("delete-confirm") as HTMLInputElement;
    Object.getOwnPropertyDescriptor(
      HTMLInputElement.prototype,
      "value",
    )!.set!.call(input, "delete");
    input.dispatchEvent(new Event("input", { bubbles: true }));
  });
  await act(async () =>
    document.querySelector<HTMLButtonElement>('button[type="submit"]')!.click(),
  );
  expect(cursorId()).toBeNull();
  expect(openId()).toBe("1");
  await press("e");
  expect(calls("mail_delete").at(-1)).toEqual([
    "mail_delete",
    { threadIds: [1], hard: false },
  ]);
  expect(cursorId()).toBe("3");
});

describe("an archive that left something behind", () => {
  const archiveReturning = async (report: object) => {
    vi.mocked(invoke).mockImplementation((async (cmd: string) => {
      if (cmd === "mail_threads")
        return { rows: [row(1), row(2)], nextCursor: null };
      if (cmd === "mail_delete") return report;
      return cmd === "mail_status" ? status : [];
    }) as never);
    await render();
    await press("j");
    await press("e");
  };
  const alert = () =>
    container.querySelector('[data-mail-pane="threads"] [role="alert"]')
      ?.textContent;
  const done = { removed: 1, kept: 0, threadsRemoved: 1, errors: [] };

  it("says how many attachment files stayed on this computer", async () => {
    await archiveReturning({ ...done, filesLeft: 2 });
    expect(alert()).toBe(
      "Archived, but downloaded attachment files for 2 messages could not be removed from this computer.",
    );
  });

  it("uses the singular for one", async () => {
    await archiveReturning({ ...done, filesLeft: 1 });
    expect(alert()).toContain("files for 1 message could not");
  });

  it("shows the errors the report carries", async () => {
    await archiveReturning({
      ...done,
      filesLeft: 0,
      errors: ["Disk is read-only."],
    });
    expect(alert()).toBe("Disk is read-only.");
  });

  it("stays quiet when the archive was clean", async () => {
    await archiveReturning({ ...done, filesLeft: 0 });
    expect(alert()).toBeUndefined();
  });
});

it("an open menu owns the keyboard: e does not archive the mail behind it", async () => {
  await render();
  await press("j");
  const menu = document.createElement("div");
  menu.setAttribute("role", "menu");
  document.body.append(menu);
  await press("e");
  await press("j");
  menu.remove();
  expect(calls("mail_delete")).toHaveLength(0);
  expect(cursorId()).toBe("1");
});
