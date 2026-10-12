// @vitest-environment happy-dom
import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import type { Editor } from "@tiptap/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import MailEditor from "./MailEditor";

let container: HTMLDivElement;
let root: Root;
let editor: Editor | null;
const handlers = {
  onChange: vi.fn(),
  onSend: vi.fn(),
  onFiles: vi.fn(),
  onImage: vi.fn(),
};

beforeEach(async () => {
  (
    globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }
  ).IS_REACT_ACT_ENVIRONMENT = true;
  Range.prototype.getClientRects = () => [] as unknown as DOMRectList;
  Range.prototype.getBoundingClientRect = () => new DOMRect();
  for (const fn of Object.values(handlers)) fn.mockReset();
  editor = null;
  container = document.createElement("div");
  document.body.append(container);
  root = createRoot(container);
  await act(async () => {
    root.render(
      createElement(MailEditor, {
        content: null,
        ...handlers,
        onEditor: (e) => {
          editor = e;
        },
      }),
    );
  });
  for (let i = 0; i < 20 && !editor; i++) {
    await act(async () => await new Promise((r) => setTimeout(r, 10)));
  }
});

afterEach(() => {
  act(() => root.unmount());
  container.remove();
});

const pm = () => container.querySelector<HTMLElement>(".ProseMirror")!;

it("sends on Cmd/Ctrl+Enter", async () => {
  await act(async () => {
    for (const mod of [{ metaKey: true }, { ctrlKey: true }]) {
      pm().dispatchEvent(
        new KeyboardEvent("keydown", {
          key: "Enter",
          bubbles: true,
          cancelable: true,
          ...mod,
        }),
      );
    }
  });
  expect(handlers.onSend).toHaveBeenCalled();
});

it("hands a pasted file to the composer instead of putting it in the document", async () => {
  const file = new File(["x"], "shot.png", { type: "image/png" });
  const event = new Event("paste", { bubbles: true, cancelable: true });
  Object.defineProperty(event, "clipboardData", {
    value: { files: [file], getData: () => "" },
  });
  await act(async () => {
    pm().dispatchEvent(event);
  });
  expect(handlers.onFiles).toHaveBeenCalledWith([file]);
  expect(editor!.getText()).toBe("");
  expect(pm().querySelector("img")).toBeNull();
});

it("leaves a plain-text paste to ProseMirror", async () => {
  const event = new Event("paste", { bubbles: true, cancelable: true });
  Object.defineProperty(event, "clipboardData", {
    value: {
      files: [],
      getData: (t: string) => (t === "text/plain" ? "hello" : ""),
    },
  });
  await act(async () => {
    pm().dispatchEvent(event);
  });
  expect(handlers.onFiles).not.toHaveBeenCalled();
});

it("keeps data-mono-att on an image, in the JSON the serializer reads and in the DOM", async () => {
  await act(async () => {
    editor!.commands.insertContent({
      type: "image",
      attrs: { src: "blob:x", "data-mono-att": "a1" },
    });
  });
  const img = editor!.getJSON().content![0];
  expect(img.attrs!["data-mono-att"]).toBe("a1");
  expect(pm().querySelector("img")!.getAttribute("data-mono-att")).toBe("a1");
});

it("reports edits and offers the toolbar's formats", async () => {
  await act(async () => {
    editor!.commands.insertContent("hi");
  });
  expect(handlers.onChange).toHaveBeenCalled();
  const bold = container.querySelector<HTMLButtonElement>(
    'button[aria-label="Bold"]',
  )!;
  await act(async () => {
    editor!.commands.selectAll();
    bold.dispatchEvent(new MouseEvent("click", { bubbles: true }));
  });
  expect(pm().querySelector("strong")?.textContent).toBe("hi");
  expect(bold.getAttribute("aria-pressed")).toBe("true");
});
