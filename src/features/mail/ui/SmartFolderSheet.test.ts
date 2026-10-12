// @vitest-environment happy-dom
import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { SmartFolderSheet } from "./SmartFolderSheet";
import {
  mailFolderDelete,
  mailFolderPreview,
  mailFolderUpsert,
} from "../model/mail";
import { SEARCH_DEBOUNCE_MS } from "../model/mailQuery";
import type { FolderRow, Rule } from "../model/mailTypes";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("../model/mail", () => ({
  mailFolderPreview: vi.fn(),
  mailFolderUpsert: vi.fn(),
  mailFolderDelete: vi.fn(),
}));

const rule = (
  field: Rule["field"],
  op: Rule["op"],
  value: Rule["value"] = "",
  negate = false,
): Rule => ({ field, op, value, negate });

const savedFolder = (over: Partial<FolderRow> = {}): FolderRow => ({
  id: 4,
  name: "Buyers",
  color: null,
  sortOrder: 2,
  matchMode: "all",
  hideFromMailbox: false,
  countUnread: true,
  rules: [rule("from", "ends_with", "@gmail.com")],
  threadCount: 0,
  unreadCount: 0,
  ...over,
});

const suggestions = { mailboxes: [], domains: [], sources: [] };

let container: HTMLDivElement;
let root: Root;
const onClose = vi.fn();
const onSaved = vi.fn();
const onDeleted = vi.fn();

beforeEach(() => {
  (
    globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }
  ).IS_REACT_ACT_ENVIRONMENT = true;
  vi.useFakeTimers();
  for (const fn of [
    mailFolderPreview,
    mailFolderUpsert,
    mailFolderDelete,
    onClose,
    onSaved,
    onDeleted,
  ])
    vi.mocked(fn).mockReset();
  vi.mocked(mailFolderPreview).mockResolvedValue({ count: 38 });
  vi.mocked(mailFolderUpsert).mockResolvedValue(savedFolder());
  vi.mocked(mailFolderDelete).mockResolvedValue(undefined);
  container = document.createElement("div");
  document.body.append(container);
  root = createRoot(container);
});

afterEach(() => {
  act(() => root.unmount());
  container.remove();
  vi.useRealTimers();
});

const open = async (
  props: Partial<Parameters<typeof SmartFolderSheet>[0]> = {},
) => {
  await act(async () =>
    root.render(
      createElement(SmartFolderSheet, {
        folder: null,
        suggestions,
        onClose,
        onSaved,
        onDeleted,
        ...props,
      }),
    ),
  );
  await settle();
};
const settle = () =>
  act(async () => {
    await vi.advanceTimersByTimeAsync(SEARCH_DEBOUNCE_MS);
  });
const button = (text: string) =>
  [...document.body.querySelectorAll("button")].find(
    (b) => b.textContent?.trim() === text,
  ) as HTMLButtonElement;
const field = (label: string) =>
  document.body.querySelector<HTMLInputElement>(`[aria-label='${label}']`)!;
const typeInto = async (el: HTMLInputElement, value: string) => {
  await act(async () => {
    Object.getOwnPropertyDescriptor(
      HTMLInputElement.prototype,
      "value",
    )!.set!.call(el, value);
    el.dispatchEvent(new Event("input", { bubbles: true }));
  });
  await settle();
};

it("runs the live count in the editor's match mode", async () => {
  await open({ folder: savedFolder({ matchMode: "all" }) });
  expect(mailFolderPreview).toHaveBeenLastCalledWith(
    [rule("from", "ends_with", "@gmail.com")],
    "all",
  );
  expect(document.body.textContent).toContain("38");
  await act(async () => button("Any condition").click());
  await settle();
  // The mode changes the count, so the preview must be asked in the new one.
  expect(mailFolderPreview).toHaveBeenLastCalledWith(
    [rule("from", "ends_with", "@gmail.com")],
    "any",
  );
});

it("does not count while typing, only after a pause", async () => {
  await open({ folder: savedFolder() });
  const before = vi.mocked(mailFolderPreview).mock.calls.length;
  await act(async () => {
    const el = field("From value");
    Object.getOwnPropertyDescriptor(
      HTMLInputElement.prototype,
      "value",
    )!.set!.call(el, "@x.com");
    el.dispatchEvent(new Event("input", { bubbles: true }));
    await vi.advanceTimersByTimeAsync(SEARCH_DEBOUNCE_MS - 1);
  });
  expect(mailFolderPreview).toHaveBeenCalledTimes(before);
  await settle();
  expect(mailFolderPreview).toHaveBeenCalledTimes(before + 1);
});

it("reads a day count that came back from the database as text", async () => {
  // Saved as a JSON number, read back as the string "30".
  await open({
    folder: savedFolder({ rules: [rule("date", "within_days", "30")] }),
  });
  expect(field("Date value").value).toBe("30");
  expect(document.body.querySelector("[role='alert']")).toBeNull();
  expect(button("Save folder").disabled).toBe(false);
});

it("labels contains_any on a mailbox 'is any of'", async () => {
  await open({
    folder: savedFolder({
      rules: [
        rule("mailbox", "contains_any", ["a@x.com", "b@x.com"]),
        rule("subject", "contains_any", ["order"]),
      ],
    }),
  });
  const text = document.body.textContent!;
  expect(text).toContain("is any of");
  expect(text).toContain("contains any of");
  // One of each: not "contains any of" on the mailbox row.
  expect(text.match(/is any of/g)).toHaveLength(1);
  expect(text.match(/contains any of/g)).toHaveLength(1);
});

it("holds a bare wildcard back from the count and from saving", async () => {
  await open({
    folder: savedFolder({ rules: [rule("subject", "contains", "%", true)] }),
  });
  expect(document.body.querySelector("[role='alert']")?.textContent).toContain(
    "word",
  );
  expect(button("Save folder").disabled).toBe(true);
  expect(mailFolderPreview).not.toHaveBeenCalled();
});

it("saves the folder with its rules and options", async () => {
  await open({
    folder: null,
    initial: {
      rules: [rule("from", "contains", "bob")],
      matchMode: "all",
    },
  });
  expect(button("Save folder").disabled).toBe(true);
  await typeInto(
    document.getElementById("smart-folder-name") as HTMLInputElement,
    "Bobs",
  );
  expect(button("Save folder").disabled).toBe(false);
  await act(async () => button("Save folder").click());
  expect(mailFolderUpsert).toHaveBeenCalledWith(
    {
      id: null,
      name: "Bobs",
      color: null,
      sortOrder: null,
      matchMode: "all",
      hideFromMailbox: false,
      countUnread: true,
    },
    [rule("from", "contains", "bob")],
  );
  expect(onSaved).toHaveBeenCalled();
});

it("says plainly what hiding from the mailbox does", async () => {
  await open({ folder: savedFolder() });
  const text = document.body.textContent!;
  expect(text).toContain("Hide matches from All mail and mailboxes");
  expect(text).toContain("leave All mail and their mailbox");
});

it("deletes only after a second press", async () => {
  await open({ folder: savedFolder() });
  await act(async () => button("Delete").click());
  expect(mailFolderDelete).not.toHaveBeenCalled();
  await act(async () => button("Delete folder").click());
  expect(mailFolderDelete).toHaveBeenCalledWith(4);
  expect(onDeleted).toHaveBeenCalledWith(4);
});

it("shows the server's reason when saving fails", async () => {
  vi.mocked(mailFolderUpsert).mockRejectedValue(
    new Error("A folder named Buyers already exists."),
  );
  await open({ folder: savedFolder() });
  await act(async () => button("Save folder").click());
  expect(document.body.textContent).toContain("already exists");
  expect(onSaved).not.toHaveBeenCalled();
});

const toggle = () =>
  document.body.querySelector<HTMLButtonElement>(
    "[role='switch'][aria-label='Hide matches from All mail and mailboxes']",
  )!;

it("cannot hide matches for a clock-relative folder, and says why", async () => {
  await open({
    folder: savedFolder({
      hideFromMailbox: true,
      rules: [rule("date", "within_days", "7")],
    }),
  });
  expect(toggle().disabled).toBe(true);
  expect(toggle().getAttribute("aria-checked")).toBe("false");
  expect(document.body.textContent).toContain("Not available with a");
  // A stale flag from before the rule was added must not be saved.
  await act(async () => button("Save folder").click());
  expect(vi.mocked(mailFolderUpsert).mock.calls[0]![0].hideFromMailbox).toBe(
    false,
  );
});

it("still hides matches when no rule is clock-relative", async () => {
  await open({ folder: savedFolder() });
  expect(toggle().disabled).toBe(false);
  expect(document.body.textContent).not.toContain("Not available with a");
  await act(async () => toggle().click());
  await act(async () => button("Save folder").click());
  expect(vi.mocked(mailFolderUpsert).mock.calls[0]![0].hideFromMailbox).toBe(
    true,
  );
});

it("opens wide and centred: the width sits on the positioned wrapper", async () => {
  await open({ folder: savedFolder() });
  const wrapper =
    document.body.querySelector("[role='dialog']")!.parentElement!;
  expect(wrapper.className).toContain("760px");
  expect(wrapper.className).not.toContain("560px");
  expect(wrapper.className).toContain("-translate-x-1/2");
});

it("starts a new condition as contains, so typing a name matches", async () => {
  await open({ folder: null, initial: null });
  await typeInto(
    document.getElementById("smart-folder-name") as HTMLInputElement,
    "Bobs",
  );
  await typeInto(field("From value"), "bob");
  await act(async () => button("Save folder").click());
  expect(mailFolderUpsert).toHaveBeenCalledWith(expect.anything(), [
    rule("from", "contains", "bob"),
  ]);
});

it("stops adding conditions at the backend cap", async () => {
  await open({
    folder: savedFolder({
      rules: Array.from({ length: 50 }, (_, i) =>
        rule("from", "contains", `p${i}`),
      ),
    }),
  });
  expect(button("Add condition").disabled).toBe(true);
});

it("ties the disabled switch to the reason it is disabled", async () => {
  await open({
    folder: savedFolder({
      hideFromMailbox: true,
      rules: [rule("date", "within_days", "7")],
    }),
  });
  const id = toggle().getAttribute("aria-describedby");
  expect(id).toBe("hide-blocked-reason");
  expect(document.getElementById(id!)?.textContent).toContain(
    "Not available with a",
  );
});

it("does not describe the switch by a reason that is not shown", async () => {
  await open({ folder: savedFolder() });
  expect(toggle().hasAttribute("aria-describedby")).toBe(false);
});

const hundred = Array.from({ length: 100 }, (_, i) => `v${i}@x.com`);
const pressEnter = (el: HTMLInputElement) =>
  act(async () => {
    el.dispatchEvent(
      new KeyboardEvent("keydown", { key: "Enter", bubbles: true }),
    );
  });

it("keeps the token it could not add once the list is full, and says so", async () => {
  await open({
    folder: savedFolder({ rules: [rule("from", "contains_any", hundred)] }),
  });
  const input = field("From value");
  await typeInto(input, "extra@x.com");
  await pressEnter(input);
  expect(input.value).toBe("extra@x.com");
  expect(document.body.textContent).toContain("At most 100 values");
  expect(
    document.body.querySelectorAll(
      "[aria-label^='Remove '][aria-label$='@x.com']",
    ),
  ).toHaveLength(100);
});

it("still clears the draft after adding below the limit", async () => {
  await open({
    folder: savedFolder({ rules: [rule("from", "contains_any", ["a@x.com"])] }),
  });
  const input = field("From value");
  await typeInto(input, "b@x.com");
  await pressEnter(input);
  expect(input.value).toBe("");
  expect(
    document.body.querySelectorAll(
      "[aria-label^='Remove '][aria-label$='@x.com']",
    ),
  ).toHaveLength(2);
});
