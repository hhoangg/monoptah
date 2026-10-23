// @vitest-environment happy-dom
import { act, createElement, type ComponentProps } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import type { TelegramStatus } from "../../features/telegram/model/telegram";
import {
  historyWithLiveSessions,
  summaryFromSession,
} from "../../features/sessions/data/sessionHistory";
import { newSession } from "../../features/sessions/model/session";
import { Sidebar } from "./Sidebar";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
  convertFileSrc: (path: string) => path,
}));
vi.mock("../../features/source-control/hooks/useProjectDiffStats", () => ({
  useProjectDiffStats: vi.fn(() => null),
}));
vi.mock("../../features/source-control/hooks/useGitFileStatuses", () => ({
  useGitFileStatuses: () => ({ files: new Map(), dirs: new Map() }),
}));
vi.mock("./SidebarUpdate", () => ({ SidebarUpdateFooter: () => null }));
vi.mock("../../features/files/ui/FileTree", () => ({
  FileTree: () => null,
}));
vi.mock("../../platform/tauri/clipboard", () => ({
  copyText: vi.fn(async () => undefined),
}));

const CONNECTED: TelegramStatus = {
  configured: true,
  enabled: true,
  botUsername: "alpha_bot",
  chatId: -100123,
};

let container: HTMLDivElement;
let root: Root;
let props: ComponentProps<typeof Sidebar>;
let status: TelegramStatus;

function session(id: string, cwd: string) {
  return {
    id,
    cwd,
    harness: "claude" as const,
    model: "",
    runtimeMode: "supervised" as const,
    title: `Session ${id}`,
    createdAt: Date.now(),
    updatedAt: Date.now(),
  };
}

function menuItem(label: string) {
  return [
    ...document.querySelectorAll<HTMLButtonElement>('[role="menuitem"]'),
  ].find((item) => item.textContent?.startsWith(label));
}

async function renderSidebar() {
  await act(async () => root.render(createElement(Sidebar, props)));
}

async function openMenu(id = "session-1") {
  await act(async () => {
    container
      .querySelector(`[data-session-card="${id}"]`)!
      .dispatchEvent(
        new MouseEvent("contextmenu", { bubbles: true, cancelable: true }),
      );
  });
}

beforeEach(() => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  localStorage.clear();
  status = CONNECTED;
  vi.mocked(invoke).mockReset();
  vi.mocked(invoke).mockImplementation(async (command) => {
    if (command === "telegram_status") return status;
    if (command === "telegram_link_code")
      return { code: "CODE1", command: "/link@alpha_bot CODE1" };
    if (command === "telegram_session_topic") return null;
    if (command === "list_external_editors") return [];
    return null;
  });
  props = {
    cwd: "/workspace/project",
    open: true,
    sessions: [session("session-1", "/workspace/project")],
    busySessionIds: new Set(),
    approvalSessionIds: new Set(),
    activeSessionId: "session-1",
    status: "idle",
    pending: false,
    tab: "sessions",
    filesSearchOpen: false,
    onSelectSession: vi.fn(),
    onRenameSession: vi.fn(),
    onOpenFile: vi.fn(),
    onTabChange: vi.fn(),
    onFilesSearchOpenChange: vi.fn(),
  };
  container = document.createElement("div");
  document.body.append(container);
  root = createRoot(container);
});

afterEach(() => {
  act(() => root.unmount());
  container.remove();
  localStorage.clear();
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

describe("sidebar Telegram topic link", () => {
  it("offers the link when the project has a connected group and opens the dialog", async () => {
    await renderSidebar();
    await openMenu();

    expect(invoke).toHaveBeenCalledWith("telegram_status", {
      project: "/workspace/project",
    });
    const item = menuItem("Link Telegram topic…");
    expect(item).toBeDefined();

    await act(async () => item!.click());
    expect(invoke).toHaveBeenCalledWith("telegram_link_code", {
      project: "/workspace/project",
      sessionId: "session-1",
    });
    expect(document.querySelector('[role="dialog"]')?.textContent).toContain(
      "/link@alpha_bot CODE1",
    );
  });

  it("hides the link when Telegram is not set up for the project", async () => {
    status = { configured: false, enabled: false, botUsername: "" };
    await renderSidebar();
    await openMenu();

    expect(menuItem("Rename")).toBeDefined();
    expect(menuItem("Link Telegram topic…")).toBeUndefined();
  });

  it("hides the link when no group is connected yet", async () => {
    status = { ...CONNECTED, chatId: undefined };
    await renderSidebar();
    await openMenu();

    expect(menuItem("Link Telegram topic…")).toBeUndefined();
  });

  it("hides the link for a saved terminal session, built the way the app builds its rows", async () => {
    const live = newSession("claude", "/workspace/project");
    live.id = "session-1";
    live.surface = "tui";
    live.providerSessionId = "provider-1";
    const saved = session("session-1", "/workspace/project");
    // The backend's saved row has no surface; the live session supplies it.
    const rows = historyWithLiveSessions([saved], [live], "/workspace/project");
    props = { ...props, sessions: rows };
    await renderSidebar();
    await openMenu();

    expect(menuItem("Rename")).toBeDefined();
    expect(menuItem("Link Telegram topic…")).toBeUndefined();
    expect(invoke).not.toHaveBeenCalledWith(
      "telegram_status",
      expect.anything(),
    );
  });

  it("hides the link for a live terminal session that is not saved yet", async () => {
    const live = newSession("claude", "/workspace/project");
    live.id = "session-1";
    live.surface = "tui";
    live.providerSessionId = "provider-1";
    props = { ...props, sessions: [summaryFromSession(live)] };
    await renderSidebar();
    await openMenu();

    expect(menuItem("Rename")).toBeDefined();
    expect(menuItem("Link Telegram topic…")).toBeUndefined();
  });
});
