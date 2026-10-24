// @vitest-environment happy-dom
import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { TelegramStatus } from "../model/telegram";
import { useTelegramLinkable } from "./useTelegramLinkable";

const invoke = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke }));

const connected: TelegramStatus = {
  configured: true,
  enabled: false,
  botUsername: "bot",
  chatId: -100,
  delaySecs: 300,
};

let root: Root;
let container: HTMLDivElement;
let latest: boolean;

function Probe({ project }: { project: string | null }) {
  latest = useTelegramLinkable(project);
  return null;
}

async function render(project: string | null) {
  await act(async () => root.render(createElement(Probe, { project })));
}

beforeEach(() => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  invoke.mockReset();
  container = document.createElement("div");
  root = createRoot(container);
});

afterEach(async () => {
  await act(async () => root.unmount());
  vi.unstubAllGlobals();
});

describe("useTelegramLinkable", () => {
  it("is true once the project has a bot and a connected group", async () => {
    invoke.mockResolvedValue(connected);
    await render("/work/a");
    expect(invoke).toHaveBeenCalledWith("telegram_status", {
      project: "/work/a",
    });
    expect(latest).toBe(true);
  });

  it("stays true while forwarding is turned off", async () => {
    invoke.mockResolvedValue({ ...connected, enabled: false });
    await render("/work/a");
    expect(latest).toBe(true);
  });

  it("is false without a group, without a bot, or when the lookup fails", async () => {
    invoke.mockResolvedValue({ ...connected, chatId: undefined });
    await render("/work/a");
    expect(latest).toBe(false);

    invoke.mockResolvedValue({ ...connected, configured: false });
    await render("/work/b");
    expect(latest).toBe(false);

    invoke.mockRejectedValue("boom");
    await render("/work/c");
    expect(latest).toBe(false);
  });

  it("does not ask Telegram without a project", async () => {
    await render(null);
    expect(invoke).not.toHaveBeenCalled();
    expect(latest).toBe(false);
  });

  it("does not report the previous project's answer for a new project", async () => {
    invoke.mockResolvedValueOnce(connected);
    await render("/work/a");
    expect(latest).toBe(true);

    let release: (status: TelegramStatus) => void = () => undefined;
    invoke.mockReturnValueOnce(
      new Promise<TelegramStatus>((resolve) => (release = resolve)),
    );
    await render("/work/b");
    expect(latest).toBe(false);
    await act(async () => release({ ...connected, configured: false }));
    expect(latest).toBe(false);
  });
});
