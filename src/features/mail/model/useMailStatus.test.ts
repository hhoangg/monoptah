// @vitest-environment happy-dom
import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { MAIL_MESSAGES_CHANGED_EVENT, MAIL_SYNC_STATE_EVENT } from "./mail";
import type { SyncStateEvent } from "./mailTypes";
import { useMailStatus } from "./useMailStatus";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));

let container: HTMLDivElement;
let root: Root;
const handlers = new Map<string, (e: { payload: unknown }) => void>();
/** Each `mail_status` call parks here until the test settles it. */
let pending: { resolve: (unread: number) => void }[] = [];

function Probe() {
  const { status, progress } = useMailStatus();
  return createElement(
    "span",
    null,
    `${status?.unreadTotal ?? "none"}|${
      progress ? `${progress.done}/${progress.total}` : "idle"
    }`,
  );
}
const shown = () => container.textContent;
const reads = () =>
  vi.mocked(invoke).mock.calls.filter(([c]) => c === "mail_status").length;
const emit = (event: string, payload: unknown = null) =>
  act(async () => handlers.get(event)!({ payload }));
const sync = (over: Partial<SyncStateEvent>) =>
  emit(MAIL_SYNC_STATE_EVENT, {
    accountId: 1,
    mailboxId: 7,
    phase: "running",
    done: 0,
    total: 0,
    error: null,
    ...over,
  });
const settle = (index: number, unread: number) =>
  act(async () => pending[index]!.resolve(unread));

beforeEach(async () => {
  (
    globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }
  ).IS_REACT_ACT_ENVIRONMENT = true;
  handlers.clear();
  pending = [];
  vi.mocked(invoke).mockReset();
  vi.mocked(invoke).mockImplementation(
    (() =>
      new Promise((resolve) =>
        pending.push({
          resolve: (unreadTotal: number) =>
            resolve({ sources: [], unreadTotal, syncIntervalSecs: 3600 }),
        }),
      )) as never,
  );
  vi.mocked(listen).mockReset();
  vi.mocked(listen).mockImplementation((async (
    event: string,
    handler: (e: { payload: unknown }) => void,
  ) => {
    handlers.set(event, handler);
    return () => handlers.delete(event);
  }) as never);
  container = document.createElement("div");
  document.body.append(container);
  root = createRoot(container);
  await act(async () => root.render(createElement(Probe)));
  await settle(0, 1);
});

afterEach(() => {
  act(() => root.unmount());
  container.remove();
});

it("reads once at start and shows it", () => {
  expect(reads()).toBe(1);
  expect(shown()).toBe("1|idle");
});

it("does not read status for every object a sync fetches", async () => {
  for (let done = 0; done < 50; done += 1) {
    await sync({ done, total: 214 });
  }
  expect(reads()).toBe(1);
  expect(shown()).toBe("1|49/214");
});

it("reads when a mailbox or a source finishes, not before", async () => {
  await sync({ done: 3, total: 10 });
  expect(reads()).toBe(1);
  await sync({ phase: "done", done: 10, total: 10 });
  expect(reads()).toBe(2);
  expect(shown()).toBe("1|idle");
});

it("keeps one read in flight and runs one more for everything that landed meanwhile", async () => {
  await emit(MAIL_MESSAGES_CHANGED_EVENT);
  expect(reads()).toBe(2);
  // Ten more events while the second read is out: still two reads started.
  for (let i = 0; i < 10; i += 1) await emit(MAIL_MESSAGES_CHANGED_EVENT);
  expect(reads()).toBe(2);
  await settle(1, 5);
  // The queued read starts only now, after the first was applied.
  expect(reads()).toBe(3);
  expect(shown()).toBe("5|idle");
  await settle(2, 9);
  expect(shown()).toBe("9|idle");
  expect(reads()).toBe(3);
});

it("shows progress across mailboxes and clears a source's leftovers when it reports", async () => {
  await sync({ mailboxId: 7, done: 10, total: 100 });
  await sync({ accountId: 2, mailboxId: 8, done: 5, total: 20 });
  expect(shown()).toBe("1|15/120");
  // The source-level event follows its mailboxes; a mailbox that never said
  // "done" (a failed listing) must not stay "syncing".
  await sync({ mailboxId: null, phase: "error" });
  expect(shown()).toBe("1|5/20");
  await sync({ accountId: 2, mailboxId: null, phase: "done" });
  expect(shown()).toBe("1|idle");
});

it("ignores a read that lands after the view is gone", async () => {
  await emit(MAIL_MESSAGES_CHANGED_EVENT);
  await act(async () => root.unmount());
  const before = reads();
  await settle(1, 99);
  expect(reads()).toBe(before);
  root = createRoot(container);
});
