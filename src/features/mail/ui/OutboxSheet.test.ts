// @vitest-environment happy-dom
import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { mailOutboxDelete } from "../model/mail";
import type { OutboxRow, OutboxState } from "../model/mailTypes";
import { OutboxSheet } from "./OutboxSheet";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("../model/mail", () => ({ mailOutboxDelete: vi.fn() }));

const row = (id: number, state: OutboxState, over = {}): OutboxRow =>
  ({
    id,
    state,
    subject: `Subject ${id}`,
    to: [{ name: null, addr: "a@x.com" }],
    lastError: null,
    ...over,
  }) as unknown as OutboxRow;

let container: HTMLDivElement;
let root: Root;
const onOpen = vi.fn();
const onChanged = vi.fn();

beforeEach(() => {
  (
    globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }
  ).IS_REACT_ACT_ENVIRONMENT = true;
  vi.mocked(mailOutboxDelete).mockReset();
  onOpen.mockReset();
  onChanged.mockReset();
  container = document.createElement("div");
  document.body.append(container);
  root = createRoot(container);
});

afterEach(() => {
  act(() => root.unmount());
  container.remove();
  document.body.innerHTML = "";
});

const render = (rows: OutboxRow[]) =>
  act(() =>
    root.render(
      createElement(OutboxSheet, {
        rows,
        error: null,
        onOpen,
        onChanged,
        onClose: vi.fn(),
      }),
    ),
  );

const button = (name: RegExp) =>
  [...document.body.querySelectorAll("button")].find((b) =>
    name.test(b.getAttribute("aria-label") ?? b.textContent ?? ""),
  ) as HTMLButtonElement | undefined;

const click = async (b: HTMLButtonElement | undefined) => {
  expect(b).toBeDefined();
  await act(async () => b!.click());
};

describe("OutboxSheet", () => {
  it("shows an unconfirmed row apart from a draft and a failure, with a check and no Send", () => {
    render([
      row(1, "draft"),
      row(2, "failed", { lastError: "SES said no" }),
      row(3, "unconfirmed"),
    ]);
    const rows = [...document.body.querySelectorAll("[data-outbox-row]")];
    expect(rows.map((r) => r.getAttribute("data-outbox-row"))).toEqual([
      "draft",
      "failed",
      "unconfirmed",
    ]);
    const unconfirmed = rows[2];
    expect(unconfirmed.textContent).toContain("May have been sent");
    expect(unconfirmed.textContent).toContain("Check recipient");
    expect(unconfirmed.textContent).not.toMatch(/Send again|Retry/);
    expect(unconfirmed.querySelector("span")!.className).not.toBe(
      rows[1].querySelector("span")!.className,
    );
    expect(rows[1].textContent).toContain("SES said no");
  });

  it("hands the row to the composer as it is", async () => {
    const r = row(3, "unconfirmed");
    render([r]);
    await click(button(/Check recipient/));
    expect(onOpen).toHaveBeenCalledWith(r);
  });

  it("cannot delete a row that is being sent", async () => {
    render([row(5, "sending")]);
    const del = button(/Delete/)!;
    expect(del.disabled).toBe(true);
    await click(del);
    expect(mailOutboxDelete).not.toHaveBeenCalled();
    expect(button(/Check recipient|Edit|Review|Open/)).toBeUndefined();
  });

  it("shows the backend's refusal in plain words when a sending row slips through", async () => {
    vi.mocked(mailOutboxDelete).mockRejectedValue(
      "This message is being sent and cannot be deleted yet.",
    );
    render([row(6, "draft")]);
    await click(button(/Delete/));
    expect(document.body.querySelector("[role=alert]")!.textContent).toBe(
      "This message is being sent and cannot be deleted yet.",
    );
    expect(onChanged).toHaveBeenCalled();
  });

  it("asks before forgetting an unconfirmed row", async () => {
    render([row(7, "unconfirmed")]);
    await click(button(/^Delete/));
    expect(mailOutboxDelete).not.toHaveBeenCalled();
    await click(button(/Delete anyway/));
    expect(mailOutboxDelete).toHaveBeenCalledWith(7);
  });

  it("deletes a plain draft at once", async () => {
    render([row(8, "draft")]);
    await click(button(/^Delete/));
    expect(mailOutboxDelete).toHaveBeenCalledWith(8);
  });
});
