// @vitest-environment happy-dom
import { afterEach, expect, it } from "vitest";
import { mailKeyAction } from "./mailKeys";

const press = (
  key: string,
  over: Partial<Parameters<typeof mailKeyAction>[0]> = {},
) =>
  mailKeyAction({
    key,
    ctrlKey: false,
    metaKey: false,
    altKey: false,
    repeat: false,
    isComposing: false,
    defaultPrevented: false,
    target: document.body,
    ...over,
  });

afterEach(() => {
  document.body.innerHTML = "";
});

it("maps the documented keys", () => {
  expect(
    ["j", "k", "Enter", "r", "a", "e", "#", "/"].map((k) => press(k)),
  ).toEqual([
    "next",
    "prev",
    "open",
    "reply",
    "reply_all",
    "archive",
    "delete",
    "search",
  ]);
});

it("takes none of them with Cmd, Ctrl or Alt held", () => {
  for (const mod of ["metaKey", "ctrlKey", "altKey"] as const) {
    expect(press("e", { [mod]: true })).toBeNull();
  }
});

it("ignores unmapped keys and capital letters", () => {
  expect(press("x")).toBeNull();
  expect(press("J")).toBeNull();
});

it("leaves keys alone while typing, in a dialog or in an editor", () => {
  for (const html of [
    "<input>",
    "<textarea></textarea>",
    '<div contenteditable="true"></div>',
    '<div role="dialog"><span></span></div>',
    '<div class="cm-editor"><span></span></div>',
    '<div role="menu"><button>Item</button></div>',
    '<div role="listbox"><div role="option"></div></div>',
  ]) {
    document.body.innerHTML = html;
    const target = document.body.firstElementChild!;
    expect(
      press("e", { target: target.firstElementChild ?? target }),
    ).toBeNull();
  }
});

it("does not take Enter from a focused button, which already activates", () => {
  document.body.innerHTML = "<button>Compose</button>";
  const target = document.body.querySelector("button")!;
  expect(press("Enter", { target })).toBeNull();
  // But a letter pressed there is still Mail's.
  expect(press("j", { target })).toBe("next");
});

it("lets only j and k repeat", () => {
  expect(press("j", { repeat: true })).toBe("next");
  expect(press("k", { repeat: true })).toBe("prev");
  expect(press("e", { repeat: true })).toBeNull();
  expect(press("#", { repeat: true })).toBeNull();
});

it("ignores a key an inner handler already consumed, and IME composition", () => {
  expect(press("e", { defaultPrevented: true })).toBeNull();
  expect(press("e", { isComposing: true })).toBeNull();
});
