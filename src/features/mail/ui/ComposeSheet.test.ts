// @vitest-environment happy-dom
import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import * as mail from "../model/mail";
import { contentIdFor } from "../model/mailCompose";
import type {
  MailMailbox,
  MessageRow,
  OutboxRow,
  SourceStatus,
  ThreadRow,
} from "../model/mailTypes";
import { ComposeSheet, FromPicker } from "./ComposeSheet";
import { senderGroups } from "../model/mailReply";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(async (cmd: string) =>
    cmd === "read_file_base64" ? "aGVsbG8=" : null,
  ),
}));
vi.mock("@tauri-apps/api/webview", () => ({
  getCurrentWebview: () => ({ onDragDropEvent: async () => () => undefined }),
}));
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(async () => () => undefined),
}));
vi.mock("../model/mail", () => ({
  mailStatus: vi.fn(),
  mailSendableMailboxes: vi.fn(),
  mailIdentities: vi.fn(),
  mailThread: vi.fn(),
  mailOutboxSave: vi.fn(),
  mailOutboxSend: vi.fn(),
  mailOutboxCheck: vi.fn(),
  mailOutboxRearm: vi.fn(),
  mailOutboxList: vi.fn(),
  mailOutboxDelete: vi.fn(),
}));

const m = vi.mocked(mail);

const mailbox = (id: number, address: string): MailMailbox =>
  ({
    id,
    accountId: 1,
    address,
    domain: address.split("@")[1],
    sortOrder: id,
    color: null,
    isSendable: true,
  }) as MailMailbox;

const SOURCE = {
  id: 1,
  label: "byptah-mail",
  sesRegion: "us-east-1",
  enabled: true,
  sortOrder: 1,
  lastError: null,
  mailboxes: [mailbox(1, "owner@byptah.com"), mailbox(2, "support@byptah.com")],
} as unknown as SourceStatus;

const RECEIVED = "RECEIVED-MARKER";

const message = (over: Partial<MessageRow> = {}): MessageRow =>
  ({
    id: 7,
    direction: "inbound",
    subject: "Order",
    fromName: "Ann",
    fromAddr: "ann@example.org",
    replyToAddr: null,
    to: [
      { name: null, addr: "owner@byptah.com" },
      { name: null, addr: "bob@example.org" },
    ],
    cc: [],
    bcc: [],
    dateUtc: 1_700_000_000,
    snippet: "",
    bodyText: `${RECEIVED} plain`,
    bodyHtml: `<p>${RECEIVED} html</p><script>alert(1)</script>`,
    ...over,
  }) as MessageRow;

const THREAD = { id: 5, mailboxAddress: "owner@byptah.com" } as ThreadRow;

const row = (over: Partial<OutboxRow> = {}): OutboxRow =>
  ({
    id: 41,
    kind: "new",
    fromAddr: "owner@byptah.com",
    to: [{ name: null, addr: "ann@example.org" }],
    cc: [],
    bcc: [],
    subject: "Hello",
    bodyJson: null,
    attachments: [],
    quotedHtml: null,
    replyToMessageId: null,
    threadId: null,
    state: "draft",
    lastError: null,
    ...over,
  }) as OutboxRow;

let container: HTMLDivElement;
let root: Root;
const onClose = vi.fn();

beforeEach(() => {
  (
    globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }
  ).IS_REACT_ACT_ENVIRONMENT = true;
  vi.stubGlobal(
    "ResizeObserver",
    class {
      observe() {}
      unobserve() {}
      disconnect() {}
    },
  );
  // ProseMirror asks for layout that happy-dom does not compute.
  Range.prototype.getClientRects = () => [] as unknown as DOMRectList;
  Range.prototype.getBoundingClientRect = () => new DOMRect();
  for (const fn of Object.values(m))
    (fn as ReturnType<typeof vi.fn>).mockReset();
  m.mailStatus.mockResolvedValue({ sources: [SOURCE] } as never);
  m.mailSendableMailboxes.mockResolvedValue([
    { ...mailbox(1, "owner@byptah.com"), isDefault: true },
  ] as never);
  m.mailIdentities.mockResolvedValue([]);
  m.mailOutboxSave.mockResolvedValue(41);
  m.mailOutboxCheck.mockResolvedValue([]);
  m.mailOutboxList.mockResolvedValue([]);
  onClose.mockReset();
  container = document.createElement("div");
  document.body.append(container);
  root = createRoot(container);
});

afterEach(() => {
  act(() => root.unmount());
  container.remove();
  vi.unstubAllGlobals();
});

const settle = async () => {
  for (let i = 0; i < 10; i++) await act(async () => await Promise.resolve());
  await act(async () => await new Promise((r) => setTimeout(r, 20)));
};

const open = async (request: Parameters<typeof ComposeSheet>[0]["request"]) => {
  await act(async () => {
    root.render(createElement(ComposeSheet, { request, onClose }));
  });
  // The editor is a lazy chunk; the first test pays for importing it.
  for (let i = 0; i < 60 && !editorEl(); i++) {
    await act(async () => await new Promise((r) => setTimeout(r, 50)));
  }
  expect(editorEl()).not.toBeNull();
  await settle();
};

const body = () => document.body;
const editorEl = () => document.querySelector<HTMLElement>(".ProseMirror");
const button = (name: RegExp | string) =>
  [...document.querySelectorAll<HTMLButtonElement>("button")].find((b) =>
    typeof name === "string"
      ? b.textContent?.trim() === name || b.getAttribute("aria-label") === name
      : name.test(b.textContent ?? "") ||
        name.test(b.getAttribute("aria-label") ?? ""),
  );
const click = async (el: Element | undefined) => {
  expect(el).toBeDefined();
  await act(async () =>
    el!.dispatchEvent(new MouseEvent("click", { bubbles: true })),
  );
  await settle();
};

it("never hands the editor a received message: the original sits outside it, as text", async () => {
  await open({ kind: "reply_all", message: message(), thread: THREAD });
  const pm = editorEl();
  expect(pm).not.toBeNull();
  expect(pm!.textContent).not.toContain(RECEIVED);
  expect(pm!.innerHTML).not.toContain("script");
  // The quote is shown, but as a text node outside the contenteditable.
  expect(body().textContent).toContain(`${RECEIVED} plain`);
  expect(pm!.closest("[contenteditable]")).toBe(pm);
  expect(body().querySelector("script")).toBeNull();
});

it("fills a reply-all from the message without the sending identity", async () => {
  await open({ kind: "reply_all", message: message(), thread: THREAD });
  const chips = [...body().querySelectorAll("span")].map((s) => s.textContent);
  expect(chips.some((t) => t?.includes("ann@example.org"))).toBe(true);
  expect(chips.some((t) => t?.includes("bob@example.org"))).toBe(true);
  expect(
    chips.some((t) => t?.includes("owner@byptah.com") && t.includes("Remove")),
  ).toBe(false);
  expect(
    (body().querySelector("#mail-subject") as HTMLInputElement).value,
  ).toBe("Re: Order");
});

it("reopens a draft from body_json", async () => {
  const doc = {
    type: "doc",
    content: [
      {
        type: "paragraph",
        content: [
          { type: "text", text: "Saved words", marks: [{ type: "bold" }] },
        ],
      },
      {
        type: "bulletList",
        content: [
          {
            type: "listItem",
            content: [
              { type: "paragraph", content: [{ type: "text", text: "item" }] },
            ],
          },
        ],
      },
    ],
  };
  await open({ kind: "draft", row: row({ bodyJson: doc }) });
  const pm = editorEl()!;
  expect(pm.querySelector("strong")?.textContent).toBe("Saved words");
  expect(pm.querySelector("ul li")?.textContent).toBe("item");
  expect(
    (body().querySelector("#mail-subject") as HTMLInputElement).value,
  ).toBe("Hello");
});

it("rebinds an inline image of a reopened draft to its attachment", async () => {
  const cid = contentIdFor("41", "att-1");
  const doc = {
    type: "doc",
    content: [
      {
        type: "image",
        attrs: { src: "blob:dead", alt: "pic", "data-mono-att": "att-1" },
      },
    ],
  };
  URL.createObjectURL = vi.fn(() => "blob:fresh");
  await open({
    kind: "draft",
    row: row({
      bodyJson: doc,
      attachments: [
        { path: "/tmp/pic.png", filename: "pic.png", mime: "image/png", cid },
      ],
    }),
  });
  const img = editorEl()!.querySelector("img")!;
  expect(img.getAttribute("src")).toBe("blob:fresh");
  expect(img.getAttribute("data-mono-att")).toBe("att-1");
});

it("sends in the fixed order, with the real outbox id as the draft id", async () => {
  const doc = {
    type: "doc",
    content: [
      { type: "image", attrs: { src: "blob:x", "data-mono-att": "a1" } },
    ],
  };
  const cid = contentIdFor("41", "a1");
  m.mailOutboxSend.mockResolvedValue({
    outboxId: 41,
    sesMessageId: "x",
    messageId: 1,
    threadId: 1,
    archive: { stored: true, reason: null },
    bookkeepingError: null,
  });
  await open({
    kind: "draft",
    row: row({
      bodyJson: doc,
      attachments: [
        { path: "/tmp/p.png", filename: "p.png", mime: "image/png", cid },
      ],
    }),
  });
  await click(button("Send"));
  const calls = [
    ...m.mailOutboxSave.mock.calls.map((c) => ["save", c[0]] as const),
  ];
  expect(m.mailOutboxSend).toHaveBeenCalledWith(41);
  expect(m.mailOutboxCheck).toHaveBeenCalledWith(41);
  const last = calls[calls.length - 1][1];
  expect(last.id).toBe(41);
  // The cid carries the real id; a placeholder would collide with the quote's.
  expect(last.bodyHtml).toContain(`cid:${cid}`);
  expect(last.attachments).toEqual([
    { path: "/tmp/p.png", filename: "p.png", mime: "image/png", cid },
  ]);
  const order = [
    m.mailOutboxSave.mock.invocationCallOrder.at(-1)!,
    m.mailOutboxCheck.mock.invocationCallOrder[0],
    m.mailOutboxSend.mock.invocationCallOrder[0],
  ];
  expect(order).toEqual([...order].sort((a, b) => a - b));
});

it("a brand-new message saves for an id before serializing with it", async () => {
  m.mailOutboxSave.mockResolvedValue(99);
  m.mailOutboxSend.mockRejectedValue({
    kind: "network",
    message: "down",
    retrySafe: true,
  });
  await open({ kind: "new" });
  const input = body().querySelector<HTMLInputElement>(
    'input[aria-label="To"]',
  )!;
  await act(async () => {
    const set = Object.getOwnPropertyDescriptor(
      HTMLInputElement.prototype,
      "value",
    )!.set!;
    set.call(input, "ann@example.org");
    input.dispatchEvent(new Event("input", { bubbles: true }));
  });
  await click(button("Send"));
  const [first, second] = m.mailOutboxSave.mock.calls.map((c) => c[0]);
  expect(first.id).toBeNull();
  expect(second.id).toBe(99);
  expect(second.bodyHtml).toBeTruthy();
  expect(second.to).toEqual([{ name: null, addr: "ann@example.org" }]);
});

it("blocks Send with the SES problem and does not send", async () => {
  m.mailOutboxCheck.mockResolvedValue([
    {
      kind: "sendingDisabled",
      message: "Sending is switched off for this account.",
      addresses: [],
    },
  ]);
  await open({ kind: "draft", row: row() });
  await click(button("Send"));
  expect(body().textContent).toContain(
    "Sending is switched off for this account.",
  );
  expect(m.mailOutboxSend).not.toHaveBeenCalled();
});

it("an unconfirmed row offers a check and a confirmation, never Send or Retry", async () => {
  await open({
    kind: "draft",
    row: row({ state: "unconfirmed", lastError: "connection dropped" }),
  });
  expect(body().textContent).toContain("Check the recipient first");
  const labels = [...body().querySelectorAll("button")].map((b) =>
    b.textContent?.trim(),
  );
  expect(labels).not.toContain("Send");
  expect(labels).not.toContain("Send again");
  expect(labels.some((l) => /retry/i.test(l ?? ""))).toBe(false);
  // Rearming takes two deliberate steps.
  await click(button("I checked the recipient"));
  expect(m.mailOutboxRearm).not.toHaveBeenCalled();
  m.mailOutboxList.mockResolvedValue([row({ state: "draft" })]);
  await click(button("Allow sending again"));
  expect(m.mailOutboxRearm).toHaveBeenCalledWith(41);
  expect(button("Send")).toBeDefined();
});

it("does not trust retrySafe: a form refusal on an unconfirmed row still offers no Retry", async () => {
  // Refusing an unconfirmed row is { kind: "form", retrySafe: true }.
  m.mailOutboxSend.mockRejectedValue({
    kind: "form",
    message: "Check the recipient first.",
    retrySafe: true,
  });
  m.mailOutboxList.mockResolvedValue([row({ state: "unconfirmed" })]);
  await open({ kind: "draft", row: row({ state: "draft" }) });
  await click(button("Send"));
  const labels = [...body().querySelectorAll("button")].map((b) =>
    b.textContent?.trim(),
  );
  expect(labels).not.toContain("Send");
  expect(labels).not.toContain("Send again");
  expect(labels).toContain("I checked the recipient");
});

it("fails closed when the row cannot be read after a call that may have reached SES", async () => {
  m.mailOutboxSend.mockRejectedValue({
    kind: "unconfirmed",
    message: "dropped",
    retrySafe: false,
  });
  m.mailOutboxList.mockRejectedValue("db busy");
  await open({ kind: "draft", row: row() });
  await click(button("Send"));
  expect(button("Send")).toBeUndefined();
  expect(body().textContent).toContain("Check the recipient first");
});

it("says so when a save is refused, instead of letting the person keep typing", async () => {
  m.mailOutboxSave.mockRejectedValue(
    "This message was already sent or is being sent, so it cannot be changed.",
  );
  await open({ kind: "draft", row: row() });
  await click(button("Save draft"));
  expect(body().textContent).toContain("not being saved");
  expect(body().textContent).toContain("already sent or is being sent");
});

it("does not listen for the mail change event while composing", async () => {
  const { listen } = await import("@tauri-apps/api/event");
  await open({ kind: "draft", row: row() });
  expect(vi.mocked(listen)).not.toHaveBeenCalled();
});

it("From picker: grouped by domain, unsendable entries disabled with their reason", async () => {
  const groups = senderGroups(
    [
      SOURCE,
      {
        ...SOURCE,
        id: 2,
        label: "acme",
        sortOrder: 2,
        mailboxes: [{ ...mailbox(9, "hello@acme.com"), accountId: 2 }],
      } as unknown as SourceStatus,
    ],
    [{ ...mailbox(1, "owner@byptah.com"), isDefault: false } as never],
    [
      {
        accountId: 1,
        identity: "support@byptah.com",
        kind: "email",
        status: "sending_disabled",
      },
    ],
  );
  const onChange = vi.fn();
  await act(async () => {
    root.render(
      createElement(FromPicker, {
        groups,
        value: "owner@byptah.com",
        onChange,
      }),
    );
  });
  await click(button(/^From:/));
  const headings = [...body().querySelectorAll('[role="group"]')].map((g) =>
    g.getAttribute("aria-label"),
  );
  expect(headings).toEqual(["byptah.com", "acme.com"]);
  const options = [...body().querySelectorAll<HTMLElement>('[role="option"]')];
  const byText = (t: string) =>
    options.find((o) => o.textContent?.includes(t))!;
  expect(byText("owner@byptah.com").getAttribute("aria-disabled")).toBe(
    "false",
  );
  expect(byText("support@byptah.com").getAttribute("aria-disabled")).toBe(
    "true",
  );
  expect(byText("support@byptah.com").textContent).toContain(
    "sending disabled by AWS",
  );
  expect(byText("hello@acme.com").textContent).toContain("not verified");
  await click(byText("support@byptah.com"));
  expect(onChange).not.toHaveBeenCalled();
  await click(byText("owner@byptah.com"));
  expect(onChange).toHaveBeenCalledWith("owner@byptah.com");
});
