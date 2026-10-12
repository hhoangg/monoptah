// @vitest-environment happy-dom
import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
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
const webview = vi.hoisted(() => ({
  drop: null as null | ((e: { payload: unknown }) => void),
}));
vi.mock("@tauri-apps/api/webview", () => ({
  getCurrentWebview: () => ({
    onDragDropEvent: async (h: (e: { payload: unknown }) => void) => {
      webview.drop = h;
      return () => undefined;
    },
  }),
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
  vi.mocked(invoke).mockImplementation((async (cmd: string) =>
    cmd === "read_file_base64" ? "aGVsbG8=" : null) as never);
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

const lastSave = () => m.mailOutboxSave.mock.calls.at(-1)![0];

it("Quote inline is off for an original with no plain text, and the quote still goes out", async () => {
  // A reopened forward: only the sanitised HTML survives on the row.
  await open({
    kind: "draft",
    row: row({
      kind: "forward",
      quotedHtml: "<p>FORWARDED-ORIGINAL</p>",
      replyToMessageId: 7,
      threadId: null,
    }),
  });
  const inline = button("Quote inline") as HTMLButtonElement;
  expect(inline.disabled).toBe(true);
  await click(inline);
  // React drops clicks on a disabled button, so call the handler itself to
  // exercise the guard inside quoteInline.
  const props = Object.keys(inline).find((k) => k.startsWith("__reactProps"))!;
  await act(async () =>
    (inline as never as Record<string, { onClick: () => void }>)[
      props
    ].onClick(),
  );
  m.mailOutboxSend.mockRejectedValue("stop here");
  await click(button("Send"));
  expect(lastSave().quotedHtml).toBe("<p>FORWARDED-ORIGINAL</p>");
  expect(lastSave().bodyHtml).toContain("FORWARDED-ORIGINAL");
  expect(editorEl()!.textContent).toBe("");
});

it("Quote inline moves the text into the document and stops quoting it twice", async () => {
  await open({ kind: "reply", message: message(), thread: THREAD });
  await click(button("Quote inline"));
  expect(editorEl()!.textContent).toContain(`${RECEIVED} plain`);
  m.mailOutboxSend.mockRejectedValue("stop here");
  await click(button("Send"));
  expect(lastSave().quotedHtml).toBeNull();
});

const setValue = async (el: HTMLInputElement, value: string) => {
  await act(async () => {
    Object.getOwnPropertyDescriptor(
      HTMLInputElement.prototype,
      "value",
    )!.set!.call(el, value);
    el.dispatchEvent(new Event("input", { bubbles: true }));
  });
};

it("without a From, autosave says what to do and closing does not silently discard", async () => {
  m.mailSendableMailboxes.mockResolvedValue([]);
  await open({ kind: "new" });
  vi.useFakeTimers();
  try {
    await setValue(
      body().querySelector<HTMLInputElement>("#mail-subject")!,
      "typed words",
    );
    await act(async () => {
      vi.advanceTimersByTime(2000);
    });
  } finally {
    vi.useRealTimers();
  }
  expect(m.mailOutboxSave).not.toHaveBeenCalled();
  expect(body().textContent).toContain(
    "Choose a From address before this can be saved.",
  );
  expect(body().textContent).not.toContain("not being saved");
  // Closing warns first; only the second attempt leaves.
  await click(button("Close"));
  expect(onClose).not.toHaveBeenCalled();
  expect(body().textContent).toContain("Close again to discard");
  await click(button("Close"));
  expect(onClose).toHaveBeenCalledTimes(1);
});

it("a refused save re-reads the row, so a sent message stops offering Send", async () => {
  m.mailOutboxSave.mockRejectedValue(
    "This message was already sent or is being sent, so it cannot be changed.",
  );
  m.mailOutboxList.mockResolvedValue([row({ state: "sent" })]);
  await open({ kind: "draft", row: row() });
  expect(button("Send")).toBeDefined();
  await click(button("Save draft"));
  expect(button("Send")).toBeUndefined();
});

it("Cmd+Enter cannot send a row the UI withholds Send from", async () => {
  await open({ kind: "draft", row: row({ state: "unconfirmed" }) });
  await act(async () => {
    for (const mod of [{ metaKey: true }, { ctrlKey: true }]) {
      editorEl()!.dispatchEvent(
        new KeyboardEvent("keydown", {
          key: "Enter",
          bubbles: true,
          cancelable: true,
          ...mod,
        }),
      );
    }
  });
  await settle();
  expect(m.mailOutboxSave).not.toHaveBeenCalled();
  expect(m.mailOutboxCheck).not.toHaveBeenCalled();
  expect(m.mailOutboxSend).not.toHaveBeenCalled();
});

it("says when a reopened draft's files are gone, before Send", async () => {
  const { invoke } = await import("@tauri-apps/api/core");
  const files = [
    {
      path: "/tmp/gone.pdf",
      filename: "gone.pdf",
      mime: "application/pdf",
      cid: null,
    },
    {
      path: "/tmp/here.pdf",
      filename: "here.pdf",
      mime: "application/pdf",
      cid: null,
    },
  ];
  vi.mocked(invoke).mockImplementation((async (cmd: string) =>
    cmd === "inspect_paths" ? [{ path: "/tmp/here.pdf" }] : null) as never);
  await open({ kind: "draft", row: row({ attachments: files }) });
  expect(body().textContent).toContain(
    "gone.pdf is no longer on this computer",
  );
  expect(body().textContent).not.toContain("here.pdf is no longer");
});

it("the missing-file warning survives pressing Send, and Send does not go ahead", async () => {
  const { invoke } = await import("@tauri-apps/api/core");
  vi.mocked(invoke).mockImplementation((async (cmd: string) =>
    cmd === "inspect_paths" ? [] : null) as never);
  await open({
    kind: "draft",
    row: row({
      attachments: [
        {
          path: "/tmp/gone.pdf",
          filename: "gone.pdf",
          mime: "application/pdf",
          cid: null,
        },
      ],
    }),
  });
  expect(body().textContent).toContain(
    "gone.pdf is no longer on this computer",
  );
  await click(button("Send"));
  expect(body().textContent).toContain(
    "gone.pdf is no longer on this computer",
  );
  expect(m.mailOutboxSend).not.toHaveBeenCalled();
  // Removing the file clears its warning.
  await click(button("Remove gone.pdf"));
  expect(body().textContent).not.toContain("no longer on this computer");
});

it("a file that vanishes while the sheet is open is caught on Send", async () => {
  const { invoke } = await import("@tauri-apps/api/core");
  const present = [{ path: "/tmp/a.pdf" }];
  vi.mocked(invoke).mockImplementation((async (cmd: string) =>
    cmd === "inspect_paths" ? present : null) as never);
  await open({
    kind: "draft",
    row: row({
      attachments: [
        {
          path: "/tmp/a.pdf",
          filename: "a.pdf",
          mime: "application/pdf",
          cid: null,
        },
      ],
    }),
  });
  expect(body().textContent).not.toContain("no longer on this computer");
  present.length = 0;
  await click(button("Send"));
  expect(body().textContent).toContain("a.pdf is no longer on this computer");
  expect(m.mailOutboxSend).not.toHaveBeenCalled();
});

it("the discard warning is not weakened by typing again", async () => {
  m.mailSendableMailboxes.mockResolvedValue([]);
  await open({ kind: "new" });
  vi.useFakeTimers();
  try {
    const subject = body().querySelector<HTMLInputElement>("#mail-subject")!;
    await setValue(subject, "one");
    // Not `click`: its settle() waits on a real timer, which is faked here.
    await act(async () => {
      button("Close")!.dispatchEvent(
        new MouseEvent("click", { bubbles: true }),
      );
      await Promise.resolve();
    });
    expect(body().textContent).toContain("Close again to discard");
    await setValue(subject, "one two");
    await act(async () => {
      vi.advanceTimersByTime(2000);
    });
  } finally {
    vi.useRealTimers();
  }
  expect(body().textContent).toContain("Close again to discard");
});

it("Save draft without a From gives the same instruction as autosave", async () => {
  m.mailSendableMailboxes.mockResolvedValue([]);
  await open({ kind: "new" });
  await click(button("Save draft"));
  expect(m.mailOutboxSave).not.toHaveBeenCalled();
  expect(body().textContent).toContain(
    "Choose a From address before this can be saved.",
  );
});

const SENT = {
  outboxId: 41,
  sesMessageId: "x",
  messageId: 1,
  threadId: 1,
  archive: { stored: true, reason: null },
  bookkeepingError: null,
};

const pdf = (cid: string | null = null, path = "/tmp/a.pdf") => ({
  path,
  filename: path.split("/").pop()!,
  mime: "application/pdf",
  cid,
});

const noFiles = () =>
  vi
    .mocked(invoke)
    .mockImplementation((async (cmd: string) =>
      cmd === "inspect_paths" ? [] : null) as never);

it("deleting a missing inline image from the body lets Send through", async () => {
  noFiles();
  m.mailOutboxSend.mockResolvedValue(SENT);
  const cid = contentIdFor("41", "att-1");
  await open({
    kind: "draft",
    row: row({
      bodyJson: {
        type: "doc",
        content: [
          { type: "image", attrs: { src: "blob:x", "data-mono-att": "att-1" } },
        ],
      },
      attachments: [
        { path: "/tmp/p.png", filename: "p.png", mime: "image/png", cid },
      ],
    }),
  });
  expect(body().textContent).toContain("p.png is no longer on this computer");
  await click(button("Send"));
  expect(m.mailOutboxSend).not.toHaveBeenCalled();
  await act(async () =>
    (
      editorEl() as unknown as {
        editor: { commands: { clearContent(): void } };
      }
    ).editor.commands.clearContent(),
  );
  await settle();
  expect(body().textContent).not.toContain("no longer on this computer");
  await click(button("Send"));
  expect(m.mailOutboxSend).toHaveBeenCalledWith(41);
});

it("attaching a file again clears its stale missing warning", async () => {
  let exists = false;
  vi.mocked(invoke).mockImplementation((async (cmd: string) =>
    cmd === "inspect_paths"
      ? exists
        ? [{ path: "/tmp/a.pdf", name: "a.pdf", size: 3, isDir: false }]
        : []
      : null) as never);
  await open({ kind: "draft", row: row({ attachments: [pdf()] }) });
  expect(body().textContent).toContain("a.pdf is no longer on this computer");
  await click(button("Remove a.pdf"));
  exists = true;
  await act(async () =>
    webview.drop!({ payload: { type: "drop", paths: ["/tmp/a.pdf"] } }),
  );
  await settle();
  expect(button("Remove a.pdf")).toBeDefined();
  expect(body().textContent).not.toContain("no longer on this computer");
});

it("Send shows a missing file and the empty recipient together", async () => {
  noFiles();
  await open({
    kind: "draft",
    row: row({ to: [], attachments: [pdf()] }),
  });
  await click(button("Send"));
  expect(body().textContent).toContain("a.pdf is no longer on this computer");
  expect(body().textContent).toContain("Add at least one recipient in To.");
  expect(m.mailOutboxSend).not.toHaveBeenCalled();
});

it("two clicks on Send in one tick send once", async () => {
  m.mailOutboxSend.mockResolvedValue(SENT);
  await open({ kind: "draft", row: row() });
  const send = button("Send")!;
  // No await between the clicks: both handlers hold the same stale `busy`.
  await act(async () => {
    send.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    send.dispatchEvent(new MouseEvent("click", { bubbles: true }));
  });
  await settle();
  expect(m.mailOutboxSend).toHaveBeenCalledTimes(1);
  expect(m.mailOutboxCheck).toHaveBeenCalledTimes(1);
});

const inlineDoc = {
  type: "doc",
  content: [
    { type: "image", attrs: { src: "blob:x", "data-mono-att": "att-1" } },
  ],
};
const inlineImage = () => ({
  path: "/tmp/p.png",
  filename: "p.png",
  mime: "image/png",
  cid: contentIdFor("41", "att-1"),
});

it("tells an attached file to be removed and attached again", async () => {
  noFiles();
  await open({ kind: "draft", row: row({ attachments: [pdf()] }) });
  const alert = body().querySelector('[role="alert"]')!.textContent;
  expect(alert).toBe(
    "a.pdf is no longer on this computer. Remove it and attach again, or this message cannot be sent.",
  );
});

it("tells an inline image to be deleted from the message, not removed", async () => {
  noFiles();
  await open({
    kind: "draft",
    row: row({ bodyJson: inlineDoc, attachments: [inlineImage()] }),
  });
  const alert = body().querySelector('[role="alert"]')!.textContent;
  expect(alert).toBe(
    "p.png is no longer on this computer. Delete the image from the message, or this message cannot be sent.",
  );
  // There is no Remove button for it; the copy must not point at one.
  expect(alert).not.toContain("Remove");
});

it("says both when a file and an inline image are missing", async () => {
  noFiles();
  await open({
    kind: "draft",
    row: row({
      bodyJson: inlineDoc,
      attachments: [pdf(), inlineImage()],
    }),
  });
  const alert = body().querySelector('[role="alert"]')!.textContent;
  expect(alert).toContain("Remove the missing files and attach them again");
  expect(alert).toContain("delete the image from the message");
});

it("clears a missing warning when the same Windows file is attached with the other separator", async () => {
  let exists = false;
  vi.mocked(invoke).mockImplementation((async (cmd: string) =>
    cmd === "inspect_paths"
      ? exists
        ? [{ path: "C:/mail/a.pdf", name: "a.pdf", size: 3, isDir: false }]
        : []
      : null) as never);
  await open({
    kind: "draft",
    row: row({
      attachments: [
        {
          path: "C:\\mail\\a.pdf",
          filename: "a.pdf",
          mime: "application/pdf",
          cid: null,
        },
      ],
    }),
  });
  expect(body().textContent).toContain("a.pdf is no longer on this computer");
  exists = true;
  await act(async () =>
    webview.drop!({ payload: { type: "drop", paths: ["C:/mail/a.pdf"] } }),
  );
  await settle();
  expect(body().textContent).not.toContain("no longer on this computer");
});
