// @vitest-environment happy-dom
import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { useIdentityDrag } from "./useIdentityDrag";
import type { IdentityDrag } from "./useIdentityDrag";

let container: HTMLDivElement;
let root: Root;
let latest: IdentityDrag | null = null;
const onDrop = vi.fn();
/** What `document.elementFromPoint` returns, per test. */
let hit: Element | null = null;

function Harness() {
  const { drag, start } = useIdentityDrag({ onDrop });
  latest = drag;
  return createElement(
    "div",
    null,
    createElement(
      "ul",
      { "data-person-drop": "ana" },
      createElement(
        "li",
        {
          "data-testid": "row",
          onPointerDown: (e: React.PointerEvent) =>
            start("ana@laptop.local", "ana", e),
        },
        "ana@laptop.local",
        createElement("button", { "data-testid": "inner" }, "Move to..."),
      ),
    ),
    createElement("section", { "data-person-drop": "bob" }, "Bob"),
    createElement("section", { id: "plain" }, "nothing"),
  );
}

function pointer(
  target: EventTarget,
  type: string,
  x: number,
  y: number,
  init: { button?: number } = {},
) {
  const event = new MouseEvent(type, {
    bubbles: true,
    cancelable: true,
    clientX: x,
    clientY: y,
    button: init.button ?? 0,
  });
  Object.defineProperty(event, "pointerId", { value: 1 });
  return act(async () => {
    target.dispatchEvent(event);
  });
}
const row = () => container.querySelector<HTMLElement>('[data-testid="row"]')!;
const bob = () => container.querySelector('[data-person-drop="bob"]')!;

beforeEach(async () => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  onDrop.mockClear();
  latest = null;
  hit = null;
  document.elementFromPoint = () => hit;
  container = document.createElement("div");
  document.body.append(container);
  root = createRoot(container);
  await act(async () => root.render(createElement(Harness)));
});
afterEach(async () => {
  await act(async () => root.unmount());
  container.remove();
  vi.unstubAllGlobals();
  document.body.style.cursor = "";
});

describe("useIdentityDrag", () => {
  it("does not start below the movement threshold, so a click stays a click", async () => {
    await pointer(row(), "pointerdown", 10, 10);
    await pointer(window, "pointermove", 12, 12);
    expect(latest).toBeNull();
    hit = bob();
    await pointer(window, "pointerup", 12, 12);
    expect(latest).toBeNull();
    expect(onDrop).not.toHaveBeenCalled();
  });

  it("starts after the threshold and follows the pointer", async () => {
    await pointer(row(), "pointerdown", 10, 10);
    await pointer(window, "pointermove", 20, 30);
    expect(latest).toEqual({
      key: "ana@laptop.local",
      x: 20,
      y: 30,
      overPersonId: null,
    });
    await pointer(window, "pointermove", 25, 35);
    expect(latest).toMatchObject({ x: 25, y: 35 });
    expect(document.body.style.cursor).toBe("grabbing");
  });

  it("reports the person under the pointer via data-person-drop", async () => {
    await pointer(row(), "pointerdown", 10, 10);
    hit = bob();
    await pointer(window, "pointermove", 40, 40);
    expect(latest?.overPersonId).toBe("bob");
    hit = container.querySelector("#plain");
    await pointer(window, "pointermove", 50, 50);
    expect(latest?.overPersonId).toBeNull();
  });

  it("drops on the person under the pointer and then resets", async () => {
    await pointer(row(), "pointerdown", 10, 10);
    hit = bob();
    await pointer(window, "pointermove", 40, 40);
    await pointer(window, "pointerup", 41, 41);
    expect(onDrop).toHaveBeenCalledTimes(1);
    expect(onDrop).toHaveBeenCalledWith("ana@laptop.local", "bob");
    expect(latest).toBeNull();
    expect(document.body.style.cursor).toBe("");
    // listeners are gone
    await pointer(window, "pointermove", 90, 90);
    expect(latest).toBeNull();
  });

  it("resolves the drop target from the release position", async () => {
    await pointer(row(), "pointerdown", 10, 10);
    await pointer(window, "pointermove", 40, 40);
    hit = bob();
    await pointer(window, "pointerup", 60, 60);
    expect(onDrop).toHaveBeenCalledWith("ana@laptop.local", "bob");
  });

  it("is a no-op to drop on the person the identity already belongs to", async () => {
    await pointer(row(), "pointerdown", 10, 10);
    hit = container.querySelector('[data-person-drop="ana"]');
    await pointer(window, "pointermove", 40, 40);
    await pointer(window, "pointerup", 40, 40);
    expect(onDrop).not.toHaveBeenCalled();
    expect(latest).toBeNull();
  });

  it("does nothing when released over no drop target", async () => {
    await pointer(row(), "pointerdown", 10, 10);
    hit = container.querySelector("#plain");
    await pointer(window, "pointermove", 40, 40);
    await pointer(window, "pointerup", 40, 40);
    expect(onDrop).not.toHaveBeenCalled();
  });

  it("cancels on Escape", async () => {
    await pointer(row(), "pointerdown", 10, 10);
    hit = bob();
    await pointer(window, "pointermove", 40, 40);
    await act(async () => {
      window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
    });
    expect(latest).toBeNull();
    await pointer(window, "pointerup", 40, 40);
    expect(onDrop).not.toHaveBeenCalled();
    expect(document.body.style.cursor).toBe("");
  });

  it("cancels on pointercancel", async () => {
    await pointer(row(), "pointerdown", 10, 10);
    hit = bob();
    await pointer(window, "pointermove", 40, 40);
    await pointer(window, "pointercancel", 40, 40);
    await pointer(window, "pointerup", 40, 40);
    expect(latest).toBeNull();
    expect(onDrop).not.toHaveBeenCalled();
  });

  it("ignores a press on a control inside the row", async () => {
    const inner = container.querySelector<HTMLElement>(
      '[data-testid="inner"]',
    )!;
    await pointer(inner, "pointerdown", 10, 10);
    await pointer(window, "pointermove", 60, 60);
    expect(latest).toBeNull();
  });

  it("ignores non-primary buttons", async () => {
    await pointer(row(), "pointerdown", 10, 10, { button: 2 });
    await pointer(window, "pointermove", 60, 60);
    expect(latest).toBeNull();
  });

  it("removes its window listeners when unmounted mid-drag", async () => {
    await pointer(row(), "pointerdown", 10, 10);
    await pointer(window, "pointermove", 40, 40);
    expect(document.body.style.cursor).toBe("grabbing");
    await act(async () => root.unmount());
    expect(document.body.style.cursor).toBe("");
    root = createRoot(container);
    await pointer(window, "pointerup", 40, 40);
    expect(onDrop).not.toHaveBeenCalled();
  });
});
