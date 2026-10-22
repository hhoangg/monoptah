// @vitest-environment happy-dom
import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { ConfirmReset } from "./ConfirmReset";

let container: HTMLDivElement;
let root: Root;

beforeEach(() => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  vi.stubGlobal(
    "ResizeObserver",
    class {
      observe() {}
      disconnect() {}
    },
  );
  container = document.createElement("div");
  document.body.append(container);
  root = createRoot(container);
});

afterEach(() => {
  act(() => root.unmount());
  container.remove();
  vi.unstubAllGlobals();
});

type Option = {
  label: string;
  note?: string;
  disabled?: boolean;
  defaultChecked?: boolean;
};

async function render(
  onConfirm: (checked: boolean) => Promise<string | void>,
  option?: Option,
) {
  await act(async () =>
    root.render(
      createElement(
        ConfirmReset,
        {
          label: "Reset conversation",
          title: "Reset?",
          body: "Body",
          kept: "Kept things.",
          failure: "Could not reset.",
          onConfirm,
          option,
        },
        (open, ref) => createElement("button", { ref, onClick: open }, "Open"),
      ),
    ),
  );
}

const open = () =>
  act(() =>
    [...document.querySelectorAll("button")]
      .find((entry) => entry.textContent === "Open")!
      .click(),
  );
const checkbox = () =>
  document.querySelector<HTMLInputElement>(
    '[role="dialog"] input[type="checkbox"]',
  );
const confirm = () =>
  [
    ...document.querySelectorAll<HTMLButtonElement>('[role="dialog"] button'),
  ].find((entry) => entry.textContent === "Reset conversation")!;
const status = () => document.querySelector('[role="status"]');

it("shows no checkbox without an option and confirms with false", async () => {
  const onConfirm = vi.fn(async () => {});
  await render(onConfirm);
  open();
  expect(checkbox()).toBeNull();
  await act(async () => confirm().click());
  expect(onConfirm).toHaveBeenCalledExactlyOnceWith(false);
});

it("shows the option checked by default under the kept line and passes the choice", async () => {
  const onConfirm = vi.fn(async () => {});
  await render(onConfirm, {
    label: "Also delete the Claude Code session files on this computer",
    note: "They can't be recovered.",
  });
  open();
  const box = checkbox()!;
  expect(box.checked).toBe(true);
  const dialog = document.querySelector('[role="dialog"]')!;
  expect(dialog.textContent).toContain(
    "Also delete the Claude Code session files on this computer",
  );
  expect(dialog.textContent).toContain("They can't be recovered.");
  const text = dialog.textContent!;
  expect(text.indexOf("Kept things.")).toBeLessThan(
    text.indexOf("Also delete"),
  );
  await act(async () => confirm().click());
  expect(onConfirm).toHaveBeenLastCalledWith(true);
});

it("passes false after the box is unticked", async () => {
  const onConfirm = vi.fn(async () => {});
  await render(onConfirm, { label: "Also delete files" });
  open();
  act(() => checkbox()!.click());
  expect(checkbox()!.checked).toBe(false);
  await act(async () => confirm().click());
  expect(onConfirm).toHaveBeenCalledExactlyOnceWith(false);
});

it("starts unchecked and disabled, with its note, when the option is disabled", async () => {
  const onConfirm = vi.fn(async () => {});
  await render(onConfirm, {
    label: "Also delete the Cursor session files on this computer",
    note: "Monoptah can't delete Cursor session files yet.",
    disabled: true,
  });
  open();
  expect(checkbox()!.checked).toBe(false);
  expect(checkbox()!.disabled).toBe(true);
  expect(document.querySelector('[role="dialog"]')!.textContent).toContain(
    "Monoptah can't delete Cursor session files yet.",
  );
  await act(async () => confirm().click());
  expect(onConfirm).toHaveBeenCalledExactlyOnceWith(false);
});

it("honours defaultChecked false", async () => {
  await render(
    vi.fn(async () => {}),
    {
      label: "Also delete files",
      defaultChecked: false,
    },
  );
  open();
  expect(checkbox()!.checked).toBe(false);
});

it("resets the checkbox each time the dialog opens", async () => {
  await render(
    vi.fn(async () => {}),
    { label: "Also delete files" },
  );
  open();
  act(() => checkbox()!.click());
  expect(checkbox()!.checked).toBe(false);
  act(() =>
    [...document.querySelectorAll<HTMLButtonElement>('[role="dialog"] button')]
      .find((entry) => entry.textContent === "Cancel")!
      .click(),
  );
  open();
  expect(checkbox()!.checked).toBe(true);
});

it("closes and shows a returned message as a non-blocking status until reopened", async () => {
  const onConfirm = vi.fn(async () => "Conversation reset. Some files stayed.");
  await render(onConfirm, { label: "Also delete files" });
  open();
  expect(status()).toBeNull();
  await act(async () => confirm().click());
  expect(document.querySelector('[role="dialog"]')).toBeNull();
  expect(status()!.textContent).toBe("Conversation reset. Some files stayed.");
  expect(document.querySelector('[role="alert"]')).toBeNull();
  open();
  expect(status()).toBeNull();
});

it("closes without a status when nothing is returned", async () => {
  await render(vi.fn(async () => {}));
  open();
  await act(async () => confirm().click());
  expect(document.querySelector('[role="dialog"]')).toBeNull();
  expect(status()).toBeNull();
});

it("keeps the dialog open with the in-place error when the reset rejects", async () => {
  await render(vi.fn().mockRejectedValue(new Error("Storage unavailable")), {
    label: "Also delete files",
  });
  open();
  await act(async () => confirm().click());
  expect(document.querySelector('[role="alert"]')!.textContent).toContain(
    "Could not reset. Storage unavailable",
  );
  expect(status()).toBeNull();
});

it("checks the box by default when the option becomes enabled while open", async () => {
  const onConfirm = vi.fn(async () => {});
  const disabled = { label: "Also delete files", disabled: true };
  await render(onConfirm, disabled);
  open();
  expect(checkbox()!.checked).toBe(false);
  await render(onConfirm, { label: "Also delete files" });
  expect(checkbox()!.disabled).toBe(false);
  expect(checkbox()!.checked).toBe(true);
  await act(async () => confirm().click());
  expect(onConfirm).toHaveBeenLastCalledWith(true);
});

it("unchecks again when the option becomes disabled while open", async () => {
  const onConfirm = vi.fn(async () => {});
  await render(onConfirm, { label: "Also delete files" });
  open();
  expect(checkbox()!.checked).toBe(true);
  await render(onConfirm, { label: "Also delete files", disabled: true });
  expect(checkbox()!.checked).toBe(false);
  await act(async () => confirm().click());
  expect(onConfirm).toHaveBeenLastCalledWith(false);
});

it("respects an explicit uncheck across a prop change, and forgets it on reopen", async () => {
  const onConfirm = vi.fn(async () => {});
  await render(onConfirm, { label: "Also delete files" });
  open();
  act(() => checkbox()!.click());
  expect(checkbox()!.checked).toBe(false);
  await render(onConfirm, { label: "Also delete the other files" });
  expect(checkbox()!.checked).toBe(false);
  act(() =>
    [...document.querySelectorAll<HTMLButtonElement>('[role="dialog"] button')]
      .find((entry) => entry.textContent === "Cancel")!
      .click(),
  );
  open();
  expect(checkbox()!.checked).toBe(true);
});
