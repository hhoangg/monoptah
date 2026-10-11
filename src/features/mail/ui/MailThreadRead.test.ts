// @vitest-environment happy-dom
import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { MailThreadRead } from "./MailThreadRead";
import { mailMark, mailMessageRaw, mailStar, mailThread } from "../model/mail";
import type { MessageRow, ThreadDetail } from "../model/mailTypes";

vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(async () => () => undefined),
  emit: vi.fn(async () => undefined),
}));
vi.mock("@tauri-apps/plugin-opener", () => ({
  openUrl: vi.fn(),
  revealItemInDir: vi.fn(),
}));
vi.mock("../model/mail", () => ({
  MAIL_MESSAGES_CHANGED_EVENT: "mail://messages-changed",
  mailThread: vi.fn(),
  mailMessageRaw: vi.fn(),
  mailMark: vi.fn(async () => undefined),
  mailStar: vi.fn(async () => undefined),
  mailAttachmentOpen: vi.fn(),
  mailAttachmentBytes: vi.fn(),
  mailRemoteImage: vi.fn(),
}));

const msg = (id: number, over: Partial<MessageRow> = {}): MessageRow => ({
  id,
  direction: "inbound",
  messageIdHdr: null,
  inReplyTo: null,
  subject: "Quarterly plan",
  fromName: `Sender ${id}`,
  fromAddr: `s${id}@x.com`,
  replyToAddr: null,
  to: [{ name: null, addr: "me@a.com" }],
  cc: [],
  bcc: [],
  dateUtc: 1_700_000_000 + id * 3600,
  snippet: `<b>snippet ${id}</b>`,
  bodyText: `body text ${id}`,
  bodyHtml: null,
  remoteImagesBlocked: 0,
  hasAttachments: false,
  sizeBytes: null,
  spf: "softfail",
  dkim: "pass",
  dmarc: null,
  isRead: false,
  isStarred: false,
  state: "active",
  attachments: [],
  ...over,
});

const detail = (messages: MessageRow[]): ThreadDetail => ({
  thread: {
    id: 5,
    accountId: 1,
    mailboxId: 1,
    mailboxAddress: "me@a.com",
    domain: "a.com",
    subject: "Quarterly plan",
    snippet: "",
    participants: [],
    firstMessageAt: 1_700_000_000,
    lastMessageAt: 1_700_000_000,
    messageCount: messages.length,
    unreadCount: 1,
    hasAttachments: false,
    isStarred: false,
  },
  imagePolicy: "ask",
  messages,
});

let container: HTMLDivElement;
let root: Root;

beforeEach(() => {
  (
    globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }
  ).IS_REACT_ACT_ENVIRONMENT = true;
  container = document.createElement("div");
  document.body.append(container);
  root = createRoot(container);
  vi.mocked(mailThread).mockReset();
  vi.mocked(mailMark).mockClear();
  vi.mocked(mailStar).mockClear();
});

afterEach(() => {
  act(() => root.unmount());
  container.remove();
});

const render = async (id: number | null = 5) => {
  await act(async () => {
    root.render(createElement(MailThreadRead, { threadId: id }));
  });
};

it("opens the newest message and collapses older ones, in any array order", async () => {
  // Newest first on purpose: the reader must not trust the order it is given.
  vi.mocked(mailThread).mockResolvedValue(detail([msg(3), msg(1), msg(2)]));
  await render();
  expect(container.textContent).toContain("body text 3");
  expect(container.textContent).not.toContain("body text 1");
  const heads = [...container.querySelectorAll("article")].map((a) =>
    a.getAttribute("aria-label"),
  );
  expect(heads).toEqual([
    "Message from Sender 1",
    "Message from Sender 2",
    "Message from Sender 3",
  ]);
  const older = container.querySelector("article button") as HTMLButtonElement;
  await act(async () => older.click());
  expect(container.textContent).toContain("body text 1");
});

it("renders the snippet as text and dates from epoch seconds", async () => {
  vi.mocked(mailThread).mockResolvedValue(detail([msg(1), msg(2)]));
  await render();
  expect(container.textContent).toContain("<b>snippet 1</b>");
  expect(container.querySelector("b")).toBeNull();
  expect(container.textContent).not.toContain("1970");
});

it("shows auth verdicts as written, including softfail and none", async () => {
  vi.mocked(mailThread).mockResolvedValue(detail([msg(1)]));
  await render();
  expect(container.textContent).toContain("SPF softfail");
  expect(container.textContent).toContain("DKIM pass");
  expect(container.textContent).toContain("DMARC none");
});

it("stars and marks the whole thread", async () => {
  vi.mocked(mailThread).mockResolvedValue(detail([msg(1)]));
  await render();
  await act(async () =>
    (container.querySelector('[aria-label="Star"]') as HTMLElement).click(),
  );
  expect(mailStar).toHaveBeenCalledWith([5], true);
  const mark = [...container.querySelectorAll("button")].find((b) =>
    b.textContent?.includes("Mark as read"),
  )!;
  await act(async () => mark.click());
  expect(mailMark).toHaveBeenLastCalledWith([5], true);
});

it("marks an unread thread read once when it opens", async () => {
  vi.mocked(mailThread).mockResolvedValue(detail([msg(1)]));
  await render();
  expect(mailMark).toHaveBeenCalledTimes(1);
  expect(mailMark).toHaveBeenCalledWith([5], true);
});

it("does not mark an already-read thread", async () => {
  vi.mocked(mailThread).mockResolvedValue(detail([msg(1, { isRead: true })]));
  await render();
  expect(mailMark).not.toHaveBeenCalled();
});

it("shows the raw source as text in a dialog", async () => {
  vi.mocked(mailThread).mockResolvedValue(detail([msg(1)]));
  vi.mocked(mailMessageRaw).mockResolvedValue("Received: <b>x</b>\r\n\r\nbody");
  await render();
  const open = [...container.querySelectorAll("button")].find((b) =>
    b.textContent?.includes("View source"),
  )!;
  await act(async () => open.click());
  expect(mailMessageRaw).toHaveBeenCalledWith(1);
  expect(document.body.textContent).toContain("Received: <b>x</b>");
  expect(document.body.querySelector("pre b")).toBeNull();
});

it("shows permerror as a warning, not as a forgery", async () => {
  vi.mocked(mailThread).mockResolvedValue(
    detail([msg(1, { spf: "permerror", dkim: "fail" })]),
  );
  await render();
  const tone = (label: string) =>
    [...container.querySelectorAll("span span")].find(
      (s) => s.textContent === label,
    )!.className;
  expect(tone("permerror")).toContain("amber");
  expect(tone("fail")).toContain("red");
});

it("prompts when nothing is selected", async () => {
  await render(null);
  expect(container.textContent).toContain("Select a message");
  expect(mailThread).not.toHaveBeenCalled();
});
