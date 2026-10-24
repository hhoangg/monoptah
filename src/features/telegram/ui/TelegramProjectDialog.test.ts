// @vitest-environment happy-dom

import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { openUrl } from "@tauri-apps/plugin-opener";
import { copyText } from "../../../platform/tauri/clipboard";
import type { TelegramStatus } from "../model/telegram";
import { invalidateTelegramStatus } from "../model/telegramStatusCache";
import { TelegramProjectDialog } from "./TelegramProjectDialog";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: vi.fn() }));
vi.mock("../../../platform/tauri/clipboard", () => ({
  copyText: vi.fn(async () => undefined),
}));

vi.mock("../model/telegramStatusCache", () => ({
  invalidateTelegramStatus: vi.fn(),
  cachedTelegramStatus: vi.fn(),
}));

const PROJECT = "/work/alpha";
const SECRET = "123456:ABC-secret-token";

const NOT_CONFIGURED: TelegramStatus = {
  configured: false,
  enabled: false,
  botUsername: "",
  delaySecs: 300,
};
const CONFIGURED: TelegramStatus = {
  configured: true,
  enabled: false,
  botUsername: "alpha_bot",
  delaySecs: 300,
};
const CONNECTED: TelegramStatus = {
  ...CONFIGURED,
  enabled: true,
  chatId: -100123,
  chatTitle: "Alpha team",
  isForum: true,
  canManageTopics: true,
};

let root: Root;
let container: HTMLDivElement;
let status: TelegramStatus;
/** What the bot token command leaves behind; defaults to a bot without a group. */
let afterToken: TelegramStatus | null;
let setDelayError: string | null;

function mockInvoke() {
  vi.mocked(invoke).mockImplementation(async (command, args) => {
    if (command === "telegram_status") return status;
    if (command === "telegram_set_token") {
      if ((args as { token: string }).token === "bad")
        throw "Bot token was rejected";
      status = afterToken ?? CONFIGURED;
      return status;
    }
    if (command === "telegram_connect_link")
      return {
        code: "CODE1",
        url: "https://t.me/alpha_bot?startgroup=CODE1&admin=manage_topics",
        fallback: "/connect@alpha_bot CODE1",
      };
    if (command === "telegram_set_enabled") {
      status = { ...status, enabled: (args as { enabled: boolean }).enabled };
      return status;
    }
    if (command === "telegram_set_delay") {
      if (setDelayError) throw setDelayError;
      status = {
        ...status,
        delaySecs: (args as { delaySecs: number }).delaySecs,
      };
      return status;
    }
    if (command === "telegram_disconnect") {
      status = NOT_CONFIGURED;
      return undefined;
    }
    if (command === "telegram_remove_webhook") {
      status = { ...status, problem: undefined };
      return undefined;
    }
    throw new Error(`Unexpected command: ${String(command)}`);
  });
}

async function renderDialog(onClose = vi.fn()) {
  await act(async () =>
    root.render(
      createElement(TelegramProjectDialog, {
        project: PROJECT,
        name: "Alpha",
        onClose,
      }),
    ),
  );
}

function dialog() {
  return document.querySelector<HTMLElement>('[role="dialog"]')!;
}

function button(label: string) {
  return [...dialog().querySelectorAll("button")].find((candidate) =>
    candidate.textContent?.includes(label),
  );
}

async function type(label: string, value: string) {
  const field = dialog().querySelector<HTMLInputElement>(
    `input[aria-label="${label}"]`,
  )!;
  await act(async () => {
    Object.getOwnPropertyDescriptor(
      HTMLInputElement.prototype,
      "value",
    )!.set!.call(field, value);
    field.dispatchEvent(new Event("input", { bubbles: true }));
  });
}

async function submit() {
  await act(async () => {
    dialog()
      .querySelector("form")!
      .dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
  });
}

beforeEach(() => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  status = NOT_CONFIGURED;
  afterToken = null;
  setDelayError = null;
  vi.mocked(invoke).mockReset();
  vi.mocked(openUrl).mockReset();
  vi.mocked(openUrl).mockResolvedValue(undefined);
  vi.mocked(copyText).mockClear();
  vi.mocked(invalidateTelegramStatus).mockClear();
  mockInvoke();
  container = document.createElement("div");
  document.body.append(container);
  root = createRoot(container);
});

afterEach(async () => {
  await act(async () => root.unmount());
  container.remove();
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

describe("telegram project dialog", () => {
  it("saves token and shows bot username", async () => {
    await renderDialog();
    expect(dialog().textContent).toContain(
      "Questions that are still unanswered after the chosen time are sent to this group. Any member can answer.",
    );
    await type("Bot token", ` ${SECRET} `);
    await submit();

    expect(invoke).toHaveBeenCalledWith("telegram_set_token", {
      project: PROJECT,
      token: SECRET,
    });
    expect(dialog().textContent).toContain("@alpha_bot");
  });

  it("shows an inline error when the token is rejected", async () => {
    await renderDialog();
    await type("Bot token", "bad");
    await submit();

    expect(dialog().querySelector('[role="alert"]')?.textContent).toBe(
      "Bot token was rejected",
    );
    expect(dialog().querySelector('input[type="password"]')).not.toBeNull();
  });

  it("shows connect link and fallback command", async () => {
    status = CONFIGURED;
    await renderDialog();
    expect(button("Connect a group")).toBeDefined();

    await act(async () => button("Connect a group")!.click());
    expect(invoke).toHaveBeenCalledWith("telegram_connect_link", {
      project: PROJECT,
    });
    expect(dialog().textContent).toContain("/connect@alpha_bot CODE1");

    await act(async () => button("Open Telegram")!.click());
    expect(openUrl).toHaveBeenCalledWith(
      "https://t.me/alpha_bot?startgroup=CODE1&admin=manage_topics",
    );
    await act(async () => button("Copy")!.click());
    expect(copyText).toHaveBeenCalledWith("/connect@alpha_bot CODE1");
  });

  it("polls status and shows the connected group", async () => {
    vi.useFakeTimers();
    status = CONFIGURED;
    await renderDialog();
    expect(dialog().textContent).not.toContain("Connected to");

    status = CONNECTED;
    await act(async () => {
      await vi.advanceTimersByTimeAsync(3000);
    });
    expect(dialog().textContent).toContain("Connected to Alpha team");
    expect(
      dialog().querySelector('[role="switch"]')?.getAttribute("aria-checked"),
    ).toBe("true");
  });

  it("stops polling when closed", async () => {
    vi.useFakeTimers();
    status = CONFIGURED;
    await renderDialog();
    await act(async () => root.unmount());
    vi.mocked(invoke).mockClear();
    await vi.advanceTimersByTimeAsync(10_000);
    expect(invoke).not.toHaveBeenCalled();
    root = createRoot(container);
  });

  it("turns forwarding on and off and disconnects", async () => {
    status = CONNECTED;
    await renderDialog();
    await act(async () =>
      dialog().querySelector<HTMLElement>('[role="switch"]')!.click(),
    );
    expect(invoke).toHaveBeenCalledWith("telegram_set_enabled", {
      project: PROJECT,
      enabled: false,
    });

    await act(async () => button("Disconnect")!.click());
    await act(async () => button("Disconnect")!.click());
    expect(invoke).toHaveBeenCalledWith("telegram_disconnect", {
      project: PROJECT,
    });
    expect(dialog().querySelector('input[type="password"]')).not.toBeNull();
  });

  it("labels the switch and says forwarding is off after connecting", async () => {
    status = { ...CONNECTED, enabled: false };
    await renderDialog();

    expect(dialog().textContent).toContain("Forward questions");
    expect(dialog().textContent).toContain(
      "Forwarding is off. Turn it on to send questions to this group.",
    );
    const toggle = dialog().querySelector<HTMLElement>('[role="switch"]')!;
    expect(toggle.getAttribute("aria-label")).toBe("Forward questions");
    expect(toggle.getAttribute("aria-checked")).toBe("false");

    await act(async () => toggle.click());
    expect(invoke).toHaveBeenCalledWith("telegram_set_enabled", {
      project: PROJECT,
      enabled: true,
    });
    expect(dialog().textContent).not.toContain("Forwarding is off");
  });

  it("does not show the off hint while forwarding is on", async () => {
    status = CONNECTED;
    await renderDialog();

    expect(dialog().textContent).toContain("Forward questions");
    expect(dialog().textContent).not.toContain("Forwarding is off");
  });

  function delayTrigger() {
    return dialog().querySelector<HTMLButtonElement>(
      'button[aria-label^="Send after"]',
    );
  }

  async function chooseDelay(label: string) {
    await act(async () => delayTrigger()!.click());
    const option = [
      ...document.querySelectorAll<HTMLButtonElement>('[role="option"]'),
    ].find((candidate) => candidate.textContent?.trim() === label);
    expect(option, label).toBeDefined();
    await act(async () => option!.click());
  }

  it("shows the current delay in a Send after row", async () => {
    status = CONNECTED;
    await renderDialog();

    expect(dialog().textContent).toContain("Send after");
    expect(delayTrigger()?.getAttribute("aria-label")).toBe(
      "Send after: 5 minutes",
    );
    await act(async () => delayTrigger()!.click());
    const labels = [...document.querySelectorAll('[role="option"]')].map(
      (option) => option.textContent?.trim(),
    );
    expect(labels).toEqual([
      "Immediately",
      "30 seconds",
      "1 minute",
      "2 minutes",
      "5 minutes",
      "10 minutes",
      "15 minutes",
      "30 minutes",
      "1 hour",
    ]);
  });

  it("saves a chosen delay, shows it and drops the cached status", async () => {
    status = CONNECTED;
    await renderDialog();
    await chooseDelay("30 seconds");

    expect(invoke).toHaveBeenCalledWith("telegram_set_delay", {
      project: PROJECT,
      delaySecs: 30,
    });
    expect(invalidateTelegramStatus).toHaveBeenCalledWith(PROJECT);
    expect(delayTrigger()?.getAttribute("aria-label")).toBe(
      "Send after: 30 seconds",
    );
  });

  it("says questions are sent right away for Immediately and saves zero", async () => {
    status = CONNECTED;
    await renderDialog();
    await chooseDelay("Immediately");

    expect(invoke).toHaveBeenCalledWith("telegram_set_delay", {
      project: PROJECT,
      delaySecs: 0,
    });
    expect(delayTrigger()?.getAttribute("aria-label")).toBe(
      "Send after: Immediately",
    );
    expect(dialog().textContent).toContain(
      "Questions are sent to this group right away. Any member can answer.",
    );
    expect(dialog().textContent).not.toContain("still unanswered");
  });

  it("shows an immediate delay that was saved earlier", async () => {
    status = { ...CONNECTED, delaySecs: 0 };
    await renderDialog();

    expect(delayTrigger()?.getAttribute("aria-label")).toBe(
      "Send after: Immediately",
    );
  });

  it("shows a stored value that is not a preset as a custom option", async () => {
    status = { ...CONNECTED, delaySecs: 90 };
    await renderDialog();

    expect(delayTrigger()?.getAttribute("aria-label")).toBe(
      "Send after: 90 seconds (custom)",
    );
    await act(async () => delayTrigger()!.click());
    const labels = [...document.querySelectorAll('[role="option"]')].map(
      (option) => option.textContent?.trim(),
    );
    expect(labels).toHaveLength(10);
    expect(labels.indexOf("90 seconds (custom)")).toBe(
      labels.indexOf("1 minute") + 1,
    );
  });

  it("keeps the shown delay and reports the error when saving fails", async () => {
    status = CONNECTED;
    setDelayError = "Save a bot token first";
    await renderDialog();
    await chooseDelay("10 minutes");

    expect(dialog().querySelector('[role="alert"]')?.textContent).toBe(
      "Save a bot token first",
    );
    expect(delayTrigger()?.getAttribute("aria-label")).toBe(
      "Send after: 5 minutes",
    );
    expect(invalidateTelegramStatus).toHaveBeenCalledWith(PROJECT);
  });

  it("offers no delay before a group is connected", async () => {
    status = CONFIGURED;
    await renderDialog();

    expect(delayTrigger()).toBeNull();
    expect(dialog().textContent).not.toContain("Send after");
  });

  it("never renders Connected to with an empty chat title", async () => {
    status = { ...CONNECTED, chatTitle: "" };
    await renderDialog();

    expect(dialog().textContent).toContain("Connected to this group");
    expect(dialog().textContent).not.toMatch(/Connected to\s*$/m);
  });

  it("falls back for a blank chat title", async () => {
    status = { ...CONNECTED, chatTitle: "   " };
    await renderDialog();

    expect(dialog().textContent).toContain("Connected to this group");
  });

  it("warns that any group member can answer once a group is connected", async () => {
    status = CONNECTED;
    await renderDialog();

    expect(dialog().textContent).toContain(
      "Any member of the group can answer the agent, and a text reply becomes its answer. Keep the group private.",
    );
  });

  it("shows no group warning before a group is connected", async () => {
    status = CONFIGURED;
    await renderDialog();

    expect(dialog().textContent).not.toContain("Keep the group private");
  });

  it("shows admin note when bot cannot manage topics", async () => {
    status = { ...CONNECTED, canManageTopics: false };
    await renderDialog();

    expect(dialog().textContent).toContain(
      "Make the bot an admin with Manage topics so it can create a topic per session.",
    );
  });

  it("omits the admin note when the bot can manage topics", async () => {
    status = CONNECTED;
    await renderDialog();

    expect(dialog().textContent).toContain("Topics");
    expect(dialog().textContent).not.toContain("Make the bot an admin");
  });

  it("shows remove webhook action for webhook problem", async () => {
    status = { ...CONNECTED, problem: "This bot has a webhook" };
    await renderDialog();
    expect(dialog().textContent).toContain("This bot has a webhook");

    await act(async () => button("Remove webhook")!.click());
    expect(invoke).toHaveBeenCalledWith("telegram_remove_webhook", {
      project: PROJECT,
    });
    expect(button("Remove webhook")).toBeUndefined();
  });

  it("does not offer remove webhook for other problems", async () => {
    status = { ...CONNECTED, problem: "Another app is reading this bot" };
    await renderDialog();

    expect(dialog().textContent).toContain("Another app is reading this bot");
    expect(button("Remove webhook")).toBeUndefined();
  });

  it("token is never rendered after save", async () => {
    await renderDialog();
    await type("Bot token", SECRET);
    await submit();

    expect(dialog().querySelector('input[type="password"]')).toBeNull();
    expect(dialog().textContent).not.toContain(SECRET);
    expect(dialog().innerHTML).not.toContain(SECRET);
    for (const input of dialog().querySelectorAll("input")) {
      expect(input.value).not.toContain(SECRET);
    }
  });

  describe("status cache invalidation", () => {
    it("invalidates after a token is saved", async () => {
      await renderDialog();
      vi.mocked(invalidateTelegramStatus).mockClear();
      await type("Bot token", SECRET);
      await submit();
      expect(invalidateTelegramStatus).toHaveBeenCalledWith(PROJECT);
    });

    it("invalidates after the enable toggle", async () => {
      status = CONNECTED;
      await renderDialog();
      vi.mocked(invalidateTelegramStatus).mockClear();
      await act(async () =>
        dialog().querySelector<HTMLElement>('[role="switch"]')!.click(),
      );
      expect(invalidateTelegramStatus).toHaveBeenCalledWith(PROJECT);
    });

    it("invalidates after disconnecting", async () => {
      status = CONNECTED;
      await renderDialog();
      vi.mocked(invalidateTelegramStatus).mockClear();
      await act(async () => button("Disconnect")!.click());
      await act(async () => button("Disconnect")!.click());
      expect(invalidateTelegramStatus).toHaveBeenCalledWith(PROJECT);
    });

    it("invalidates when the poll sees the group connect", async () => {
      vi.useFakeTimers();
      status = CONFIGURED;
      await renderDialog();
      expect(invalidateTelegramStatus).not.toHaveBeenCalled();
      status = CONNECTED;
      await act(async () => {
        await vi.advanceTimersByTimeAsync(3000);
      });
      expect(invalidateTelegramStatus).toHaveBeenCalledWith(PROJECT);
    });
  });

  describe("changing the token", () => {
    const calls = (command: string) =>
      vi.mocked(invoke).mock.calls.filter(([name]) => name === command);

    it("offers Change token without showing a field or the old token", async () => {
      status = CONNECTED;
      await renderDialog();

      expect(button("Change token")).toBeDefined();
      expect(dialog().querySelector('input[type="password"]')).toBeNull();

      await act(async () => button("Change token")!.click());
      const field = dialog().querySelector<HTMLInputElement>(
        'input[type="password"]',
      )!;
      expect(field.value).toBe("");
      expect(dialog().textContent).toContain(
        "A token for the same bot keeps the group and topic links.",
      );
      expect(dialog().textContent).toContain(
        "A different bot clears the group and topic links, turns Forward questions off, and questions already sent stop working.",
      );
    });

    it("keeps the group when the same bot is saved", async () => {
      status = CONNECTED;
      afterToken = CONNECTED;
      await renderDialog();
      await act(async () => button("Change token")!.click());
      await type("Bot token", ` ${SECRET} `);
      await submit();

      expect(calls("telegram_set_token")).toEqual([
        ["telegram_set_token", { project: PROJECT, token: SECRET }],
      ]);
      expect(dialog().querySelector('input[type="password"]')).toBeNull();
      expect(dialog().textContent).toContain("Connected to Alpha team");
      expect(dialog().innerHTML).not.toContain(SECRET);
      expect(button("Change token")).toBeDefined();
    });

    it("shows the new bot without a group when another bot is saved", async () => {
      status = CONNECTED;
      afterToken = { ...CONFIGURED, botUsername: "beta_bot" };
      await renderDialog();
      await act(async () => button("Change token")!.click());
      await type("Bot token", SECRET);
      await submit();

      expect(dialog().textContent).toContain("@beta_bot");
      expect(dialog().textContent).not.toContain("Connected to");
      expect(button("Connect a group")).toBeDefined();
    });

    it("recovers from a rejected token and keeps the connection", async () => {
      status = CONNECTED;
      await renderDialog();
      await act(async () => button("Change token")!.click());
      await type("Bot token", "bad");
      await submit();

      expect(dialog().querySelector('[role="alert"]')?.textContent).toBe(
        "Bot token was rejected",
      );
      expect(dialog().textContent).toContain("Connected to Alpha team");
      expect(dialog().querySelector('input[type="password"]')).not.toBeNull();

      afterToken = CONNECTED;
      await type("Bot token", SECRET);
      await submit();
      expect(dialog().querySelector('[role="alert"]')).toBeNull();
      expect(dialog().querySelector('input[type="password"]')).toBeNull();
    });

    it("cancel closes the field and forgets what was typed", async () => {
      status = CONNECTED;
      await renderDialog();
      await act(async () => button("Change token")!.click());
      await type("Bot token", SECRET);
      await act(async () => button("Cancel")!.click());

      expect(dialog().querySelector('input[type="password"]')).toBeNull();
      expect(calls("telegram_set_token")).toHaveLength(0);
      await act(async () => button("Change token")!.click());
      expect(
        dialog().querySelector<HTMLInputElement>('input[type="password"]')!
          .value,
      ).toBe("");
    });
  });

  describe("disconnect confirmation", () => {
    const disconnects = () =>
      vi
        .mocked(invoke)
        .mock.calls.filter(([name]) => name === "telegram_disconnect");

    it("asks first and says topic links will be lost", async () => {
      status = CONNECTED;
      await renderDialog();
      await act(async () => button("Disconnect")!.click());

      expect(disconnects()).toHaveLength(0);
      expect(dialog().textContent).toContain(
        "Topic links will be lost and questions already sent stop working.",
      );
      expect(button("Cancel")).toBeDefined();
    });

    it("cancel keeps the connection", async () => {
      status = CONNECTED;
      await renderDialog();
      await act(async () => button("Disconnect")!.click());
      await act(async () => button("Cancel")!.click());

      expect(disconnects()).toHaveLength(0);
      expect(dialog().textContent).not.toContain("Topic links will be lost");
      expect(dialog().textContent).toContain("Connected to Alpha team");
    });

    it("disconnects after confirming", async () => {
      status = CONNECTED;
      await renderDialog();
      await act(async () => button("Disconnect")!.click());
      await act(async () => button("Disconnect")!.click());

      expect(disconnects()).toHaveLength(1);
      expect(dialog().querySelector('input[type="password"]')).not.toBeNull();
    });
  });
});
