// @vitest-environment happy-dom
import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { MailView } from "./MailView";
import type { OutboxRow } from "../model/mailTypes";

const composer = vi.hoisted(() => ({
  requests: [] as unknown[],
  close: null as null | (() => void),
}));

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(async () => () => undefined),
}));
vi.mock("../../../app/shell/WindowControls", () => ({
  WindowControls: () => null,
}));
vi.mock("./ComposeSheet", () => ({
  preloadMailEditor: () => undefined,
  ComposeSheet: (p: { request: unknown; onClose: () => void }) => {
    composer.requests.push(p.request);
    composer.close = p.onClose;
    return null;
  },
}));

const unconfirmed = {
  id: 3,
  state: "unconfirmed",
  subject: "Quote",
  to: [{ name: null, addr: "a@x.com" }],
  lastError: null,
  updatedAt: 1,
} as unknown as OutboxRow;

let container: HTMLDivElement;
let root: Root;
let listCalls: number;
let outboxRows: OutboxRow[];

beforeEach(async () => {
  (
    globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }
  ).IS_REACT_ACT_ENVIRONMENT = true;
  localStorage.clear();
  composer.requests = [];
  listCalls = 0;
  outboxRows = [unconfirmed];
  vi.mocked(invoke).mockImplementation((async (cmd: string) => {
    if (cmd === "mail_threads") return { rows: [], nextCursor: null };
    if (cmd === "mail_outbox_list") {
      listCalls += 1;
      return outboxRows;
    }
    return null;
  }) as typeof invoke);
  container = document.createElement("div");
  document.body.append(container);
  root = createRoot(container);
  await act(async () =>
    root.render(
      createElement(MailView, { besideRail: true, onClose: vi.fn() }),
    ),
  );
});

afterEach(() => {
  act(() => root.unmount());
  container.remove();
  document.body.innerHTML = "";
});

const press = async (match: RegExp) => {
  const b = [...document.body.querySelectorAll("button")].find((x) =>
    match.test(x.getAttribute("aria-label") ?? x.textContent ?? ""),
  );
  expect(b).toBeDefined();
  await act(async () => b!.click());
};

it("flags unconfirmed mail on the Outbox button before it is opened", () => {
  const b = document.body.querySelector("button[aria-label^='Outbox']")!;
  expect(b.getAttribute("aria-label")).toBe("Outbox, 1 message, 1 to check");
});

it("opens a row in the existing draft entry point and reloads when the composer closes", async () => {
  await press(/^Outbox/);
  await press(/Check recipient/);
  expect(composer.requests).toEqual([{ kind: "draft", row: unconfirmed }]);
  const before = listCalls;
  // A save emits nothing; closing is how the list finds out.
  await act(async () => composer.close!());
  expect(listCalls).toBe(before + 1);
});

it("carries both the total and the to-check count, visibly and to a screen reader", async () => {
  const draft = (id: number) =>
    ({ ...unconfirmed, id, state: "draft" }) as OutboxRow;
  outboxRows = [unconfirmed, draft(4), draft(5), draft(6)];
  await act(async () => {
    root.unmount();
    root = createRoot(container);
    root.render(
      createElement(MailView, { besideRail: true, onClose: vi.fn() }),
    );
  });
  const b = document.body.querySelector("button[aria-label^='Outbox']")!;
  expect(b.getAttribute("aria-label")).toBe("Outbox, 4 messages, 1 to check");
  // What a sighted user reads must say the same two things.
  expect(b.textContent).toContain("4");
  expect(b.textContent).toContain("1 to check");
});

it("names only the total when nothing needs checking", async () => {
  outboxRows = [{ ...unconfirmed, state: "draft" } as OutboxRow];
  await act(async () => {
    root.unmount();
    root = createRoot(container);
    root.render(
      createElement(MailView, { besideRail: true, onClose: vi.fn() }),
    );
  });
  const b = document.body.querySelector("button[aria-label^='Outbox']")!;
  expect(b.getAttribute("aria-label")).toBe("Outbox, 1 message");
  expect(b.textContent).not.toContain("to check");
});
