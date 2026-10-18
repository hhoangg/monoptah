// @vitest-environment happy-dom
import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { TicketScoringSettings } from "./TicketScoringSettings";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

type Config = {
  runner: "claude-code" | "api";
  model: string;
  hasApiKey: boolean;
};

let container: HTMLDivElement;
let root: Root;
let stored: Config;

beforeEach(() => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  stored = { runner: "claude-code", model: "", hasApiKey: false };
  container = document.createElement("div");
  document.body.append(container);
  root = createRoot(container);
  vi.mocked(invoke).mockReset();
  vi.mocked(invoke).mockImplementation(async (command, args) => {
    if (command === "contributors_ai_config_get") return stored;
    if (command === "contributors_ai_config_set") {
      const next = args as {
        runner: Config["runner"];
        model: string;
        apiKey: string | null;
      };
      stored = {
        runner: next.runner,
        model: next.model,
        hasApiKey: next.apiKey === null ? stored.hasApiKey : next.apiKey !== "",
      };
      return stored;
    }
    throw new Error(`Unexpected command: ${command}`);
  });
});
afterEach(async () => {
  await act(async () => root.unmount());
  container.remove();
  vi.unstubAllGlobals();
});

async function render() {
  await act(async () => root.render(createElement(TicketScoringSettings)));
}

async function input(label: string, value: string) {
  const field = container.querySelector<HTMLInputElement>(
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
    container
      .querySelector("form")!
      .dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
  });
}

function button(label: string) {
  return [...container.querySelectorAll("button")].find(
    (candidate) => candidate.textContent === label,
  )!;
}

function radio(label: string) {
  return container.querySelector<HTMLInputElement>(
    `input[type="radio"][aria-label="${label}"]`,
  )!;
}

const KEY_FIELD = 'input[aria-label="API key"]';

it("shows the key field only for the API runner", async () => {
  await render();
  expect(container.textContent).toContain(
    "claude-haiku-5-5 costs far less per ticket",
  );
  expect(
    container.querySelector<HTMLInputElement>('input[aria-label="Model"]')!
      .placeholder,
  ).toBe("claude-opus-5-5");
  expect(radio("Claude Code (your subscription)").checked).toBe(true);
  expect(container.querySelector(KEY_FIELD)).toBeNull();

  await act(async () => radio("Anthropic API key").click());
  const key = container.querySelector<HTMLInputElement>(KEY_FIELD)!;
  expect(key.type).toBe("password");

  await act(async () => radio("Claude Code (your subscription)").click());
  expect(container.querySelector(KEY_FIELD)).toBeNull();
});

it("saving keeps the stored key when the field is left empty", async () => {
  stored = { runner: "api", model: "claude-opus-5-5", hasApiKey: true };
  await render();
  expect(container.textContent).toContain("Key saved");
  expect(container.querySelector<HTMLInputElement>(KEY_FIELD)!.value).toBe("");
  await input("Model", " claude-haiku-5-5 ");
  await submit();
  expect(invoke).toHaveBeenCalledWith("contributors_ai_config_set", {
    runner: "api",
    model: "claude-haiku-5-5",
    apiKey: null,
  });
  expect(container.textContent).toContain("Key saved");
});

it("saving a typed key sends it once and never shows it back", async () => {
  await render();
  await act(async () => radio("Anthropic API key").click());
  expect(container.textContent).not.toContain("Key saved");
  await input("API key", " sk-ant-secret ");
  await submit();
  expect(invoke).toHaveBeenCalledWith("contributors_ai_config_set", {
    runner: "api",
    model: "",
    apiKey: "sk-ant-secret",
  });
  expect(container.textContent).toContain("Key saved");
  expect(container.querySelector<HTMLInputElement>(KEY_FIELD)!.value).toBe("");
  expect(container.innerHTML).not.toContain("sk-ant-secret");
});

it("remove key sends an empty string", async () => {
  stored = { runner: "api", model: "claude-opus-5-5", hasApiKey: true };
  await render();
  await act(async () => button("Remove key").click());
  expect(invoke).toHaveBeenCalledWith("contributors_ai_config_set", {
    runner: "api",
    model: "claude-opus-5-5",
    apiKey: "",
  });
  expect(container.textContent).not.toContain("Key saved");
  expect(button("Remove key")).toBeUndefined();
});

it("shows a save failure", async () => {
  await render();
  vi.mocked(invoke).mockRejectedValueOnce(new Error("disk is full"));
  await submit();
  expect(container.querySelector('[role="alert"]')?.textContent).toContain(
    "disk is full",
  );
});
