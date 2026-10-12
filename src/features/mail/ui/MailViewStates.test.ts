// @vitest-environment happy-dom
import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { MailView } from "./MailView";
import { MAIL_PREF_KEYS } from "../model/mailPrefs";
import { SEARCH_DEBOUNCE_MS } from "../model/mailQuery";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(async () => () => undefined),
}));
vi.mock("../../../app/shell/WindowControls", () => ({
  WindowControls: () => null,
}));

const mailbox = (id: number, address: string) => ({
  id,
  accountId: 1,
  address,
  domain: address.split("@")[1],
  s3Prefix: "p/",
  displayName: "",
  color: null,
  sortOrder: id,
  isSendable: true,
  unread: 0,
  sync: null,
});
const source = (over: Record<string, unknown> = {}) => ({
  id: 1,
  label: "Main",
  enabled: true,
  sortOrder: 1,
  lastError: null,
  unread: 0,
  mailboxes: [mailbox(1, "owner@a.com")],
  ...over,
});

let sources: ReturnType<typeof source>[];
let threadRows: unknown[];
let threadFailure: string | null;
let folders: unknown[];

let container: HTMLDivElement;
let root: Root;

beforeEach(() => {
  (
    globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }
  ).IS_REACT_ACT_ENVIRONMENT = true;
  localStorage.clear();
  sources = [source()];
  threadRows = [];
  threadFailure = null;
  folders = [];
  vi.mocked(invoke).mockReset();
  vi.mocked(invoke).mockImplementation((async (cmd: string) => {
    switch (cmd) {
      case "mail_status":
        return { sources, unreadTotal: 0, syncIntervalSecs: 3600 };
      case "mail_threads":
        return { rows: threadRows, nextCursor: null };
      case "mail_folders":
        return folders;
      case "mail_outbox_list":
        return [];
      case "mail_thread":
        if (threadFailure !== null) throw new Error(threadFailure);
        return {
          thread: { ...(threadRows[0] as object) },
          imagePolicy: "ask",
          restorable: false,
          messages: [],
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
const threads = () =>
  container.querySelector<HTMLElement>('[data-mail-pane="threads"]')!
    .textContent ?? "";

it("says there is no source when none is configured", async () => {
  sources = [];
  await render();
  expect(threads()).toContain("No mail source yet");
  expect(threads()).toContain("Add an AWS source in Settings");
  expect(threads()).not.toContain("No mail yet");
});

it("says every source is off when all are disabled", async () => {
  sources = [source({ enabled: false })];
  await render();
  expect(threads()).toContain("Every mail source is turned off");
});

it("says the sources have no mailboxes when none was added", async () => {
  sources = [source({ mailboxes: [] })];
  await render();
  expect(threads()).toContain("Your sources have no mailboxes");
});

it("does not call a configured, empty mailbox a setup problem", async () => {
  await render();
  expect(threads()).toContain(
    "No mail yet. New mail appears after the next sync.",
  );
  expect(threads()).not.toContain("No mail source yet");
});

it("names the mailbox when that one mailbox is empty", async () => {
  localStorage.setItem(MAIL_PREF_KEYS.selectedMailbox, "1");
  await render();
  expect(threads()).toContain("Nothing in owner@a.com yet.");
});

it("keeps a missing source out of the way of a search that matches nothing", async () => {
  sources = [];
  await render();
  const box = container.querySelector<HTMLInputElement>(
    'input[type="search"]',
  )!;
  await act(async () => {
    Object.getOwnPropertyDescriptor(
      HTMLInputElement.prototype,
      "value",
    )!.set!.call(box, "invoice ");
    box.dispatchEvent(new Event("input", { bubbles: true }));
    await new Promise((r) => setTimeout(r, SEARCH_DEBOUNCE_MS + 50));
  });
  expect(threads()).toContain("No threads match.");
  expect(threads()).not.toContain("No mail source yet");
});

it("says a smart folder is empty", async () => {
  folders = [
    {
      id: 7,
      name: "Buyers",
      color: null,
      sortOrder: 1,
      matchMode: "all",
      hideFromMailbox: false,
      countUnread: true,
      rules: [],
      threadCount: 0,
      unreadCount: 0,
    },
  ];
  localStorage.setItem(MAIL_PREF_KEYS.selectedFolder, "7");
  await render();
  expect(threads()).toContain("Nothing in this folder yet.");
});

it("says the Archive is empty", async () => {
  localStorage.setItem(MAIL_PREF_KEYS.selectedFolder, "archived");
  await render();
  expect(threads()).toContain("Nothing archived.");
});

it("shows a failing source as an alert and still lists its mail", async () => {
  sources = [source({ lastError: "AccessDenied: s3:ListBucket" })];
  threadRows = [
    {
      id: 1,
      accountId: 1,
      mailboxId: 1,
      mailboxAddress: "owner@a.com",
      domain: "a.com",
      subject: "Still here",
      snippet: "",
      participants: [],
      firstMessageAt: 1,
      lastMessageAt: 1,
      messageCount: 1,
      unreadCount: 0,
      hasAttachments: false,
      isStarred: false,
    },
  ];
  await render();
  const alert = container.querySelector(
    '[data-mail-pane="threads"] [role="alert"]',
  );
  expect(alert).not.toBeNull();
  expect(alert?.textContent).toContain(
    "Main could not sync: AccessDenied: s3:ListBucket. Mail already downloaded",
  );
  expect(threads()).toContain("Still here");
});

it("does not alarm about a source that is switched off", async () => {
  sources = [source({ enabled: false, lastError: "old failure" })];
  await render();
  expect(threads()).not.toContain("old failure");
});

it("says a conversation could not be opened, and tries again on request", async () => {
  threadRows = [
    {
      id: 1,
      accountId: 1,
      mailboxId: 1,
      mailboxAddress: "owner@a.com",
      domain: "a.com",
      subject: "Broken",
      snippet: "",
      participants: [],
      firstMessageAt: 1,
      lastMessageAt: 1,
      messageCount: 1,
      unreadCount: 0,
      hasAttachments: false,
      isStarred: false,
    },
  ];
  threadFailure = "message 4 failed to parse";
  await render();
  await act(async () =>
    container.querySelector<HTMLElement>("[data-thread-id='1']")!.click(),
  );
  const reader = container.querySelector<HTMLElement>(
    '[data-mail-pane="reader"]',
  )!;
  expect(reader.querySelector('[role="alert"]')).not.toBeNull();
  expect(reader.querySelector('[role="alert"]')?.textContent).toContain(
    "Could not open this conversation: message 4 failed to parse",
  );
  threadFailure = null;
  const retry = [...reader.querySelectorAll("button")].find(
    (b) => b.textContent === "Try again",
  )!;
  await act(async () => retry.click());
  expect(reader.querySelector('[role="alert"]')).toBeNull();
});
