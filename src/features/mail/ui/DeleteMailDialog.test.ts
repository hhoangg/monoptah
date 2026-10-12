// @vitest-environment happy-dom
import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { DeleteMailDialog, type DeleteReport } from "./DeleteMailDialog";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

const clean: DeleteReport = {
  removed: 4,
  kept: 0,
  threadsRemoved: 1,
  filesLeft: 0,
  errors: [],
};

let container: HTMLDivElement;
let root: Root;
const onClose = vi.fn();
const onDone = vi.fn();

beforeEach(() => {
  (
    globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }
  ).IS_REACT_ACT_ENVIRONMENT = true;
  container = document.createElement("div");
  document.body.append(container);
  root = createRoot(container);
  vi.mocked(invoke).mockReset();
  vi.mocked(invoke).mockResolvedValue(clean);
  onClose.mockClear();
  onDone.mockClear();
});

afterEach(() => {
  act(() => root.unmount());
  container.remove();
});

const render = async (messageCount = 4, alreadyArchived = false) => {
  await act(async () => {
    root.render(
      createElement(DeleteMailDialog, {
        threadIds: [5],
        subject: "Order #1182",
        messageCount,
        mailbox: "owner@a.com",
        alreadyArchived,
        onClose,
        onDone,
      }),
    );
  });
};

const radio = (index: number) =>
  document.body.querySelectorAll<HTMLInputElement>('input[type="radio"]')[
    index
  ];
const confirm = () =>
  document.body.querySelector<HTMLButtonElement>('button[type="submit"]')!;
const type = async (text: string) => {
  const input = document.getElementById("delete-confirm") as HTMLInputElement;
  const set = Object.getOwnPropertyDescriptor(
    HTMLInputElement.prototype,
    "value",
  )!.set!;
  await act(async () => {
    set.call(input, text);
    input.dispatchEvent(new Event("input", { bubbles: true }));
  });
};
const submit = async () =>
  act(async () => {
    document.body
      .querySelector("form")!
      .dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
  });

it("offers archive and a delete on S3, with archive chosen", async () => {
  await render();
  expect(document.body.textContent).toContain("Archive locally");
  expect(document.body.textContent).toContain("Delete on S3 as well");
  expect(radio(0).checked).toBe(true);
  expect(radio(1).checked).toBe(false);
  expect(confirm().textContent).toBe("Archive");
});

it("archives without hard: true, and without a typed word", async () => {
  await render();
  await act(async () => confirm().click());
  expect(invoke).toHaveBeenCalledTimes(1);
  expect(invoke).toHaveBeenCalledWith("mail_delete", {
    threadIds: [5],
    hard: false,
  });
  expect(onDone).toHaveBeenCalledWith(clean, false);
  expect(onClose).toHaveBeenCalled();
});

it("passes hard: true only after delete on S3 is chosen and the word typed", async () => {
  await render();
  await act(async () => radio(1).click());
  expect(confirm().textContent).toContain("Delete from S3");
  expect(confirm().disabled).toBe(true);

  // Submitting the form (Enter in the field) must not get round the gate.
  await type("del");
  await submit();
  await type("");
  await submit();
  expect(invoke).not.toHaveBeenCalled();

  await type(" Delete ");
  expect(confirm().disabled).toBe(false);
  await act(async () => confirm().click());
  expect(invoke).toHaveBeenCalledTimes(1);
  expect(invoke).toHaveBeenCalledWith("mail_delete", {
    threadIds: [5],
    hard: true,
  });
});

it("going back to archive after typing the word archives", async () => {
  await render();
  await act(async () => radio(1).click());
  await type("delete");
  await act(async () => radio(0).click());
  await act(async () => confirm().click());
  expect(invoke).toHaveBeenCalledWith("mail_delete", {
    threadIds: [5],
    hard: false,
  });
});

it("says a delete is irreversible unless the bucket is versioned", async () => {
  await render();
  const text = document.body.textContent ?? "";
  expect(text).toContain("cannot be undone");
  expect(text).toContain("unless versioning is turned on");
  expect(text).not.toMatch(/recover|restore|undo it/i);
  expect(text).not.toContain("MonoCode");
});

it("stays open and shows what S3 refused when part of the delete failed", async () => {
  vi.mocked(invoke).mockResolvedValue({
    removed: 3,
    kept: 1,
    threadsRemoved: 0,
    filesLeft: 0,
    // What the Rust side makes of S3's MissingContentMD5 code.
    errors: ["S3 returned an error (MissingContentMD5)."],
  } satisfies DeleteReport);
  await render();
  await act(async () => radio(1).click());
  await type("delete");
  await act(async () => confirm().click());
  expect(onClose).not.toHaveBeenCalled();
  expect(document.body.textContent).toContain("1 could not be deleted");
  expect(document.body.textContent).toContain(
    "S3 returned an error (MissingContentMD5).",
  );
});

it("says the mail is gone but cached files remain, instead of closing quietly", async () => {
  vi.mocked(invoke).mockResolvedValue({
    ...clean,
    filesLeft: 2,
  } satisfies DeleteReport);
  await render();
  await act(async () => radio(1).click());
  await type("delete");
  await act(async () => confirm().click());
  expect(onClose).not.toHaveBeenCalled();
  expect(document.body.textContent).toContain("The mail is gone");
  expect(document.body.textContent).toContain("2 messages");
});

it("points at the Archive view and does not call archiving one-way", async () => {
  await render();
  const text = document.body.textContent ?? "";
  expect(text).toContain("stays under Archive");
  expect(text).not.toContain("not yet browsable");
});

it("shows the error when the command itself fails, and does not close", async () => {
  vi.mocked(invoke).mockRejectedValue("MissingContentMD5: header required");
  await render();
  await act(async () => radio(1).click());
  await type("delete");
  await act(async () => confirm().click());
  expect(onClose).not.toHaveBeenCalled();
  expect(onDone).not.toHaveBeenCalled();
  expect(document.body.textContent).toContain("MissingContentMD5");
});

it("says 'message', not 'messages', for a one-message thread", async () => {
  await render(1);
  const text = document.body.textContent ?? "";
  expect(text).toContain(
    "removes the message from Monoptah (1 in this thread)",
  );
  expect(text).not.toContain("the messages from Monoptah (1");
});

it("keeps 'messages' for a longer thread", async () => {
  await render(4);
  expect(document.body.textContent).toContain(
    "removes the messages from Monoptah (4 in this thread)",
  );
});

it("announces the files-left sentence, not only the count deleted", async () => {
  vi.mocked(invoke).mockResolvedValue({
    ...clean,
    filesLeft: 2,
  } satisfies DeleteReport);
  await render();
  await act(async () => radio(1).click());
  await type("delete");
  await act(async () => confirm().click());
  const alerts = document.body.querySelectorAll('[role="alert"]');
  expect(alerts).toHaveLength(1);
  expect(alerts[0]!.textContent).toContain(
    "could not be removed from this computer",
  );
});

it("on an archived thread offers only the delete, still behind the typed word", async () => {
  await render(4, true);
  expect(document.body.textContent).not.toContain("Archive locally");
  expect(document.body.textContent).toContain("already archived");
  expect(radio(0).checked).toBe(true);
  expect(document.body.querySelectorAll('input[type="radio"]')).toHaveLength(1);
  expect(confirm().textContent).toContain("Delete from S3");
  expect(confirm().disabled).toBe(true);
  // Enter in the form must not delete before the word is typed.
  await submit();
  expect(invoke).not.toHaveBeenCalled();
  await type("delete");
  await act(async () => confirm().click());
  expect(invoke).toHaveBeenCalledWith("mail_delete", {
    threadIds: [5],
    hard: true,
  });
});
