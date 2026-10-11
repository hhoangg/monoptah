// @vitest-environment happy-dom
import { act, createElement, useState } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import {
  FRAME_CSP,
  MAX_FRAME_HEIGHT,
  MailMessageBody,
  canTogglePlain,
  buildFrameDocument,
  cidReferences,
  linkifyText,
  remoteImageUrls,
} from "./MailMessageBody";
import { mailAttachmentBytes, mailRemoteImage } from "../model/mail";
import type { MessageRow } from "../model/mailTypes";

vi.mock("@tauri-apps/plugin-opener", () => ({
  openUrl: vi.fn(async () => undefined),
}));
vi.mock("../model/mail", () => ({
  mailRemoteImage: vi.fn(),
  mailAttachmentBytes: vi.fn(),
}));

const PNG = new Uint8Array([0x89, 0x50, 0x4e, 0x47, 0, 0, 0, 0, 0, 0]).buffer;
const REMOTE = "https://cdn.example.com/a.png?x=1&y=2";

const message = (over: Partial<MessageRow> = {}): MessageRow => ({
  id: 7,
  direction: "inbound",
  messageIdHdr: null,
  inReplyTo: null,
  subject: "Hi",
  fromName: "Linh",
  fromAddr: "linh@x.com",
  replyToAddr: null,
  to: [],
  cc: [],
  bcc: [],
  dateUtc: 1_700_000_000,
  snippet: "hello",
  bodyText: "hello text",
  bodyHtml: `<p>hi</p><img src="" data-mono-src="${REMOTE}">`,
  remoteImagesBlocked: 1,
  hasAttachments: false,
  sizeBytes: null,
  spf: null,
  dkim: null,
  dmarc: null,
  isRead: false,
  isStarred: false,
  state: "active",
  attachments: [],
  ...over,
});

let container: HTMLDivElement;
let root: Root;
let blobCount: number;

beforeEach(() => {
  (
    globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }
  ).IS_REACT_ACT_ENVIRONMENT = true;
  container = document.createElement("div");
  document.body.append(container);
  root = createRoot(container);
  blobCount = 0;
  URL.createObjectURL = vi.fn(() => `blob:app/${++blobCount}`);
  URL.revokeObjectURL = vi.fn();
  vi.mocked(mailRemoteImage).mockReset();
  vi.mocked(mailRemoteImage).mockResolvedValue(PNG);
  vi.mocked(mailAttachmentBytes).mockReset();
  vi.mocked(mailAttachmentBytes).mockResolvedValue(PNG);
});

afterEach(() => {
  act(() => root.unmount());
  container.remove();
});

// Stands in for ThreadBody, which owns the per-message allow-state.
function Harness(props: Parameters<typeof MailMessageBody>[0]) {
  const [allowed, setAllowed] = useState(false);
  return createElement(MailMessageBody, {
    ...props,
    messageAllowed: allowed,
    onAllowMessage: () => setAllowed(true),
  });
}

const render = async (
  msg: MessageRow,
  over: Partial<Parameters<typeof MailMessageBody>[0]> = {},
) => {
  await act(async () => {
    root.render(
      createElement(Harness, {
        message: msg,
        messageAllowed: false,
        onAllowMessage: vi.fn(),
        imagePolicy: "ask",
        senderAllowed: false,
        senderLabel: "linh@x.com",
        onAllowSender: vi.fn(),
        ...over,
      }),
    );
  });
};

const frame = () => container.querySelector("iframe");
const button = (label: string) =>
  [...container.querySelectorAll("button")].find((b) =>
    b.textContent?.includes(label),
  );

it("never grants scripts to the frame", async () => {
  await render(message());
  const sandbox = frame()!.getAttribute("sandbox")!;
  expect(sandbox).not.toContain("allow-scripts");
  expect(sandbox.split(/\s+/)).toEqual(["allow-same-origin"]);
  expect(frame()!.getAttribute("srcdoc")).toContain(`content="${FRAME_CSP}"`);
});

it("shows how many remote images were blocked and loads none", async () => {
  await render(message());
  expect(container.textContent).toContain("1 remote image blocked");
  expect(mailRemoteImage).not.toHaveBeenCalled();
  expect(frame()!.getAttribute("srcdoc")).not.toContain("https://");
});

it("fetches images through Rust and puts only a blob URL in the frame", async () => {
  await render(message());
  await act(async () => button("Show images")!.click());
  expect(mailRemoteImage).toHaveBeenCalledWith(7, REMOTE);
  const srcdoc = frame()!.getAttribute("srcdoc")!;
  expect(srcdoc).toContain('src="blob:app/1"');
  expect(srcdoc).not.toContain("cdn.example.com");
  expect(container.textContent).not.toContain("remote image blocked");
});

it("loads images at once for an allowed sender, and never when off", async () => {
  await render(message(), { senderAllowed: true });
  expect(mailRemoteImage).toHaveBeenCalledTimes(1);
  await render(message({ id: 8 }), { imagePolicy: "never" });
  expect(mailRemoteImage).toHaveBeenCalledTimes(1);
  expect(button("Show images")).toBeUndefined();
  expect(container.textContent).toContain("turned off");
});

it("does not promise that showing images hides the user", async () => {
  await render(message());
  expect(container.textContent).toContain("IP address");
});

it("skips relative and protocol-relative image sources", () => {
  const html =
    '<img data-mono-src="/x.png"><img data-mono-src="//evil/x.png">' +
    `<img data-mono-src="${REMOTE}"><img data-mono-src="${REMOTE}">`;
  expect(remoteImageUrls(html)).toEqual([REMOTE]);
});

it("resolves cid references in img src and in style", async () => {
  const html =
    '<img src="cid:logo@x"><div style="background:url(cid:bg@x) no-repeat"></div>';
  expect(cidReferences(html).sort()).toEqual(["bg@x", "logo@x"]);
  const out = buildFrameDocument(
    html,
    new Map(),
    new Map([
      ["logo@x", "blob:app/1"],
      ["bg@x", "blob:app/2"],
    ]),
  );
  expect(out).toContain('src="blob:app/1"');
  expect(out).toContain("url(blob:app/2)");
  expect(out).not.toContain("cid:");
});

it("strips any other image source and url() from the frame document", () => {
  const out = buildFrameDocument(
    '<img src="https://evil/x.png"><img src="cid:nope"><div style="background:url(https://evil/y)"></div>',
    new Map(),
    new Map(),
  );
  expect(out).not.toContain("evil");
  expect(out).not.toContain("cid:");
});

it("fetches inline cid images from attachments without asking", async () => {
  const msg = message({
    bodyHtml: '<img src="cid:logo@x">',
    remoteImagesBlocked: 0,
    attachments: [
      {
        id: 31,
        messageId: 7,
        partIndex: 1,
        filename: "logo.png",
        mimeType: "image/png",
        sizeBytes: 10,
        contentId: "<logo@x>",
        isInline: true,
        downloaded: false,
      },
    ],
  });
  await render(msg);
  expect(mailAttachmentBytes).toHaveBeenCalledWith(31);
  expect(frame()!.getAttribute("srcdoc")).toContain('src="blob:app/1"');
});

it("sizes the frame from the document height, up to a maximum", async () => {
  await render(message());
  const el = frame()!;
  const doc = el.contentDocument!;
  let height = 300;
  Object.defineProperty(doc.body, "scrollHeight", {
    configurable: true,
    get: () => height,
  });
  await act(async () => {
    el.dispatchEvent(new Event("load"));
  });
  expect(el.style.height).toBe("300px");
  // Late-loading images re-measure.
  height = 5000;
  await act(async () => {
    doc.dispatchEvent(new Event("load"));
  });
  expect(el.style.height).toBe(`${MAX_FRAME_HEIGHT}px`);
});

it("falls back to text for HTML-less mail and says so when empty", async () => {
  await render(message({ bodyHtml: null }));
  expect(frame()).toBeNull();
  expect(container.textContent).toContain("hello text");
  await render(message({ bodyHtml: null, bodyText: null }));
  expect(container.textContent).toContain("no content");
});

it("renders plain text as text, not markup", async () => {
  await render(
    message({ bodyHtml: null, bodyText: "<img src=x onerror=alert(1)> hi" }),
  );
  expect(container.querySelector("img")).toBeNull();
  expect(container.textContent).toContain("<img src=x");
});

it("linkifies URLs without their trailing punctuation", () => {
  expect(linkifyText("see https://a.example/x, ok")).toEqual([
    "see ",
    { url: "https://a.example/x" },
    ", ok",
  ]);
});

it("counts only the images it can load and says so for the rest", async () => {
  await render(
    message({
      bodyHtml: `<img data-mono-src="${REMOTE}"><img data-mono-src="/rel.png">`,
      remoteImagesBlocked: 2,
    }),
  );
  expect(container.textContent).toContain("2 remote images blocked");
  expect(container.textContent).toContain("1 of them cannot be loaded");
  await render(
    message({
      bodyHtml: '<img data-mono-src="/rel.png">',
      remoteImagesBlocked: 1,
    }),
  );
  expect(container.textContent).toContain("None of them can be loaded");
  expect(button("Show images")).toBeUndefined();
});

it("labels the sender action with its real lifetime", async () => {
  await render(message());
  expect(button("this session")).toBeDefined();
  expect(container.textContent).not.toContain("Always show");
});

it("removes base, meta, style and link from the frame document", () => {
  const out = buildFrameDocument(
    '<base href="https://evil/"><meta http-equiv="refresh" content="0;url=https://evil"><link rel="stylesheet" href="data:text/css,p{}"><style>p{}</style><p>x</p>',
    new Map(),
    new Map(),
  );
  const body = out.slice(out.indexOf("<body>"));
  expect(body).not.toMatch(/base|meta|link|style/);
  expect(out).not.toContain("evil");
});

it("offers the plain-text toggle only when both bodies have content", () => {
  expect(canTogglePlain(message())).toBe(true);
  expect(canTogglePlain(message({ bodyText: null }))).toBe(false);
  expect(canTogglePlain(message({ bodyText: "  " }))).toBe(false);
  expect(canTogglePlain(message({ bodyHtml: null }))).toBe(false);
});
