// @vitest-environment happy-dom
import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { listen } from "@tauri-apps/api/event";
import { mailOutboxList } from "./mail";
import { presentOutbox, useMailOutbox, visibleOutbox } from "./mailOutbox";
import type { OutboxRow, OutboxState } from "./mailTypes";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(async () => () => undefined),
}));
vi.mock("./mail", () => ({
  MAIL_OUTBOX_CHANGED_EVENT: "mail://outbox-changed",
  mailOutboxList: vi.fn(async () => []),
}));

const row = (id: number, state: OutboxState, updatedAt = id): OutboxRow =>
  ({ id, state, updatedAt, subject: `S${id}`, to: [] }) as unknown as OutboxRow;

describe("presentOutbox", () => {
  it("sends an unconfirmed row to the recipient check, never to Send", () => {
    const p = presentOutbox("unconfirmed");
    expect(p.openLabel).toBe("Check recipient");
    expect(p.tone).toBe("caution");
    expect(p.confirmDelete).toBe(true);
    expect(p.openLabel).not.toMatch(/send|retry/i);
  });

  it("looks different for a draft, a failure and an unconfirmed row", () => {
    const tones = (["draft", "failed", "unconfirmed"] as const).map(
      (s) => presentOutbox(s).label,
    );
    expect(new Set(tones).size).toBe(3);
    expect(presentOutbox("failed").tone).not.toBe(
      presentOutbox("unconfirmed").tone,
    );
  });

  it("cannot delete or open a row that is being sent", () => {
    const p = presentOutbox("sending");
    expect(p.canDelete).toBe(false);
    expect(p.canOpen).toBe(false);
  });
});

describe("visibleOutbox", () => {
  it("hides sent rows and puts unconfirmed first", () => {
    const out = visibleOutbox([
      row(1, "draft", 9),
      row(2, "sent"),
      row(3, "failed"),
      row(4, "unconfirmed", 1),
    ]);
    expect(out.map((r) => r.id)).toEqual([4, 3, 1]);
  });
});

describe("useMailOutbox", () => {
  let container: HTMLDivElement;
  let root: Root;
  let handlers: Record<string, () => void>;

  beforeEach(async () => {
    (
      globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }
    ).IS_REACT_ACT_ENVIRONMENT = true;
    handlers = {};
    vi.mocked(mailOutboxList).mockClear();
    vi.mocked(listen).mockImplementation((async (
      event: string,
      cb: () => void,
    ) => {
      handlers[event] = cb;
      return () => undefined;
    }) as unknown as typeof listen);
    container = document.createElement("div");
    document.body.append(container);
    root = createRoot(container);
    const Probe = () => {
      useMailOutbox();
      return null;
    };
    await act(async () => root.render(createElement(Probe)));
  });

  afterEach(() => {
    act(() => root.unmount());
    container.remove();
  });

  it("reloads on the outbox event and on no other", async () => {
    expect(mailOutboxList).toHaveBeenCalledTimes(1);
    // The composer's autosave must not reach the list through the event that
    // reloads the whole mail UI.
    expect(Object.keys(handlers)).toEqual(["mail://outbox-changed"]);
    await act(async () => handlers["mail://outbox-changed"]());
    expect(mailOutboxList).toHaveBeenCalledTimes(2);
  });
});
