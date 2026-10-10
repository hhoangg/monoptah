// @vitest-environment happy-dom
// Keep this as .ts because the project test glob intentionally excludes .test.tsx.
import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { NewSessionProviderDialog } from "./NewSessionProviderDialog";
import { setProjectProviderHidden } from "../model/projectProviders";
import { saveProviderSurface } from "../../providers/model/providerSurface";
import { HARNESS_TITLE } from "../model/session";

let container: HTMLDivElement;
let root: Root;

beforeEach(() => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  localStorage.clear();
  container = document.createElement("div");
  document.body.append(container);
  root = createRoot(container);
});

afterEach(() => {
  act(() => root.unmount());
  container.remove();
  vi.unstubAllGlobals();
});

function render(
  onPick = vi.fn(),
  defaultHarness: "claude" | "codex" = "claude",
) {
  act(() =>
    root.render(
      createElement(NewSessionProviderDialog, {
        cwd: "/tmp/project",
        defaultHarness,
        onPick,
        onClose: vi.fn(),
      }),
    ),
  );
  return onPick;
}

function options(): HTMLElement[] {
  return [...document.querySelectorAll<HTMLElement>('[role="option"]')];
}

describe("NewSessionProviderDialog", () => {
  it("excludes a provider hidden for the project", () => {
    setProjectProviderHidden("/tmp/project", "codex", true);
    render();
    const text = options().map((o) => o.textContent);
    expect(text.some((t) => t?.includes(HARNESS_TITLE.codex))).toBe(false);
    expect(text.some((t) => t?.includes(HARNESS_TITLE.claude))).toBe(true);
  });

  it("badges each provider with its surface", () => {
    saveProviderSurface("claude", "tui");
    saveProviderSurface("codex", "chat");
    render();
    const row = (title: string) =>
      options().find((o) => o.textContent?.includes(title));
    expect(row(HARNESS_TITLE.claude)?.textContent).toContain("TUI");
    expect(row(HARNESS_TITLE.codex)?.textContent).toContain("Chat");
  });

  it("picks the second provider after ArrowDown then Enter", () => {
    const onPick = render();
    const rows = options();
    expect(rows[0].getAttribute("aria-selected")).toBe("true");
    act(() => {
      window.dispatchEvent(
        new KeyboardEvent("keydown", { key: "ArrowDown", cancelable: true }),
      );
    });
    act(() => {
      window.dispatchEvent(
        new KeyboardEvent("keydown", { key: "Enter", cancelable: true }),
      );
    });
    expect(onPick).toHaveBeenCalledOnce();
    expect(onPick).toHaveBeenCalledWith("codex");
  });

  it("picks a provider when its row is clicked", () => {
    const onPick = render();
    const codex = options().find((o) =>
      o.textContent?.includes(HARNESS_TITLE.codex),
    );
    act(() => codex!.click());
    expect(onPick).toHaveBeenCalledWith("codex");
  });
});
