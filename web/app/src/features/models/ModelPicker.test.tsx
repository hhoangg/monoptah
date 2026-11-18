import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import { SendableRuntimeModeSchema } from "@monoptah/protocol";
import { makeCatalog } from "./fixtures";
import { ModelOptions } from "./ModelOptions";
import { EffortSlider } from "./EffortSlider";
import { ModelPicker } from "./ModelPicker";
import { currentSelection, type ModelSelection } from "./modelSettings";
import type { AccountUsage } from "./hooks";

beforeAll(() => {
  Element.prototype.hasPointerCapture ??= () => false;
  Element.prototype.setPointerCapture ??= () => {};
  Element.prototype.releasePointerCapture ??= () => {};
  Element.prototype.scrollIntoView ??= () => {};
  globalThis.ResizeObserver ??= class {
    observe() {}
    unobserve() {}
    disconnect() {}
  };
});

afterEach(() => vi.unstubAllGlobals());

const NO_FULL = (calls: unknown[][]) => expect(JSON.stringify(calls)).not.toContain("full-access");

describe("ModelPicker (dialog layout)", () => {
  const catalog = makeCatalog();

  it("lists providers and models, checks the current one and reports a pick without full-access", async () => {
    const user = userEvent.setup();
    const onChange = vi.fn<(next: ModelSelection) => void>();
    render(<ModelPicker open onOpenChange={() => {}} catalog={catalog} value={currentSelection(catalog)} onChange={onChange} defaultModelId="sonnet" />);
    const dialog = await screen.findByRole("dialog", { name: "Model" });
    expect(within(dialog).getByRole("button", { name: /Opus/, pressed: true })).toBeTruthy();
    expect(within(dialog).getByText("Default")).toBeTruthy();
    await user.click(within(dialog).getByRole("button", { name: /Sonnet/ }));
    expect(onChange).toHaveBeenCalledTimes(1);
    expect(onChange.mock.calls[0]![0]).toMatchObject({ harness: "claude", model: "sonnet", modelSettings: { effort: "medium" }, runtimeMode: "supervised" });
    NO_FULL(onChange.mock.calls);
  });

  it("another provider opens by tapping its section, and switching drops the account", async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    render(<ModelPicker open onOpenChange={() => {}} catalog={catalog} value={{ ...currentSelection(catalog), providerAccountId: "work" }} onChange={onChange} />);
    await user.click(await screen.findByRole("button", { name: /Codex/ }));
    await user.click(screen.getByRole("button", { name: /GPT-5/ }));
    const next = onChange.mock.calls[0]![0] as ModelSelection;
    expect(next).toMatchObject({ harness: "codex", model: "gpt-5" });
    expect(next.providerAccountId).toBeUndefined();
  });

  it("search filters across providers", async () => {
    const user = userEvent.setup();
    render(<ModelPicker open onOpenChange={() => {}} catalog={catalog} value={currentSelection(catalog)} onChange={() => {}} />);
    await user.type(await screen.findByRole("searchbox", { name: "Search models" }), "gpt");
    expect(screen.getByRole("button", { name: /GPT-5/ })).toBeTruthy();
    expect(screen.queryByRole("button", { name: /Sonnet/ })).toBeNull();
    await user.clear(screen.getByRole("searchbox"));
    await user.type(screen.getByRole("searchbox"), "zzz");
    expect(screen.getByText("No matching models.")).toBeTruthy();
  });

  it("disabled (read only) browses but never emits", async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    render(<ModelPicker open onOpenChange={() => {}} catalog={catalog} value={currentSelection(catalog)} onChange={onChange} disabled />);
    const sonnet = await screen.findByRole("button", { name: /Sonnet/ });
    expect((sonnet as HTMLButtonElement).disabled).toBe(true);
    await user.click(sonnet);
    expect(onChange).not.toHaveBeenCalled();
  });

  it("the options screen opens from the summary and goes back", async () => {
    const user = userEvent.setup();
    render(<ModelPicker open onOpenChange={() => {}} catalog={catalog} value={currentSelection(catalog)} onChange={() => {}} />);
    await user.click(await screen.findByRole("button", { name: /Options for Opus: Medium/ }));
    expect(await screen.findByRole("dialog", { name: "Options for Opus" })).toBeTruthy();
    expect(screen.getByRole("slider", { name: "Effort" })).toBeTruthy();
    await user.click(screen.getByRole("button", { name: "Models" }));
    expect(await screen.findByRole("dialog", { name: "Model" })).toBeTruthy();
  });

  it("shows the account picker with usage meters for the chosen provider and refreshes", async () => {
    const user = userEvent.setup();
    const refresh = vi.fn();
    const usage: Pick<AccountUsage, "accounts" | "refreshing" | "error" | "refresh"> = {
      accounts: {
        default: { status: { tone: "low", label: "Running low" }, updatedAt: 1, stale: false, meters: [{ title: "5 hour", usedPercent: 85, percent: 85, label: "85%", reset: "Resets in 2h" }] },
        work: { status: { tone: "ready", label: "Ready" }, updatedAt: 1, stale: true, meters: [] },
      },
      refreshing: false,
      error: null,
      refresh,
    };
    const onChange = vi.fn();
    render(<ModelPicker open onOpenChange={() => {}} catalog={catalog} value={currentSelection(catalog)} onChange={onChange} usage={usage} />);
    expect(await screen.findByText("Running low")).toBeTruthy();
    const bar = screen.getByRole("progressbar", { name: "5 hour usage" });
    expect(bar.getAttribute("aria-valuenow")).toBe("85");
    await user.click(screen.getByRole("button", { name: "Refresh account usage" }));
    expect(refresh).toHaveBeenCalledTimes(1);
    await user.click(screen.getByRole("button", { name: "Account: Personal" }));
    await user.click(screen.getByRole("radio", { name: "Work" }));
    expect(onChange.mock.calls[0]![0]).toMatchObject({ providerAccountId: "work" });
    NO_FULL(onChange.mock.calls);
  });

  it("a running session sends account switches to onSwitchAccount, not onChange", async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    const onSwitchAccount = vi.fn();
    render(<ModelPicker open onOpenChange={() => {}} catalog={catalog} value={currentSelection(catalog)} onChange={onChange} existingSession onSwitchAccount={onSwitchAccount} />);
    await user.click(await screen.findByRole("button", { name: "Account: Personal" }));
    expect(screen.getByText("Switching accounts opens a new chat.")).toBeTruthy();
    await user.click(screen.getByRole("radio", { name: "Work" }));
    expect(onSwitchAccount).toHaveBeenCalledWith("work", expect.objectContaining({ providerAccountId: "work" }));
    expect(onChange).not.toHaveBeenCalled();
  });
});

describe("ModelPicker (sheet layout)", () => {
  it("renders a vaul sheet under 640px", async () => {
    vi.stubGlobal("matchMedia", (query: string) => ({ matches: false, media: query, addEventListener() {}, removeEventListener() {}, addListener() {}, removeListener() {}, dispatchEvent: () => false, onchange: null }));
    const catalog = makeCatalog();
    render(<ModelPicker open onOpenChange={() => {}} catalog={catalog} value={currentSelection(catalog)} onChange={() => {}} />);
    const sheet = await screen.findByRole("dialog", { name: "Model" });
    expect(sheet.getAttribute("data-vaul-drawer")).not.toBeNull();
  });
});

describe("ModelOptions and EffortSlider", () => {
  const catalog = makeCatalog();
  const opus = catalog.models[0]!;

  it("the slider emits the option at the new step and tints hot efforts", async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    const options = opus.settings[0]!.options;
    const { rerender } = render(<EffortSlider label="Effort" options={options} value="medium" onChange={onChange} />);
    const slider = screen.getByRole("slider", { name: "Effort" });
    expect(slider.getAttribute("aria-valuetext")).toBe("Medium");
    slider.focus();
    await user.keyboard("{ArrowRight}");
    expect(onChange).toHaveBeenCalledWith("high");
    expect(document.querySelector("[data-hot]")).toBeNull();
    rerender(<EffortSlider label="Effort" options={options} value="max" onChange={onChange} />);
    expect(document.querySelector("[data-hot]")?.textContent).toBe("Max");
    expect(document.querySelectorAll("[data-step]").length).toBe(options.length);
  });

  it("effort, fast mode and access changes are emitted as sendable selections", async () => {
    const user = userEvent.setup();
    const onChange = vi.fn<(next: ModelSelection) => void>();
    render(<ModelOptions model={opus} value={currentSelection(catalog)} onChange={onChange} />);
    screen.getByRole("slider", { name: "Effort" }).focus();
    await user.keyboard("{ArrowRight}");
    expect(onChange.mock.lastCall![0].modelSettings).toMatchObject({ effort: "high", fast: "false" });
    await user.click(screen.getByRole("switch"));
    expect(onChange.mock.lastCall![0].modelSettings).toMatchObject({ fast: "true" });
    await user.click(screen.getByRole("button", { name: "Access: Supervised" }));
    const items = within(await screen.findByRole("menu")).getAllByRole("menuitemradio");
    expect(items).toHaveLength(SendableRuntimeModeSchema.options.length);
    await user.click(items[2]!);
    expect(onChange.mock.lastCall![0].runtimeMode).toBe("auto");
    NO_FULL(onChange.mock.calls);
  });

  it("a session in full access shows it read only and emits no access on any change", async () => {
    const user = userEvent.setup();
    const onChange = vi.fn<(next: ModelSelection) => void>();
    const full = makeCatalog("full-access");
    render(<ModelOptions model={opus} value={currentSelection(full)} onChange={onChange} />);
    expect(screen.getByText("Full access (set on desktop)")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Access: Full access" })).toBeTruthy();
    screen.getByRole("slider", { name: "Effort" }).focus();
    await user.keyboard("{ArrowRight}");
    await user.click(screen.getByRole("switch"));
    expect(onChange).toHaveBeenCalledTimes(2);
    for (const [next] of onChange.mock.calls) expect("runtimeMode" in next).toBe(false);
    // The menu offers no way to choose full access either.
    await user.click(screen.getByRole("button", { name: "Access: Full access" }));
    const items = within(await screen.findByRole("menu")).getAllByRole("menuitemradio");
    expect(items.map((item) => item.textContent).join(" ")).not.toMatch(/Full access/);
  });

  it("disabled blocks every control, and fx shows no access row", () => {
    const onChange = vi.fn();
    const { unmount } = render(<ModelOptions model={opus} value={currentSelection(catalog)} onChange={onChange} disabled />);
    expect(screen.getByRole("switch").hasAttribute("disabled")).toBe(true);
    expect(screen.getByRole("button", { name: "Access: Supervised" }).hasAttribute("disabled")).toBe(true);
    unmount();
    render(<ModelOptions model={undefined} value={{ ...currentSelection(catalog), harness: "fx" }} onChange={onChange} />);
    expect(screen.getByText("This model has no options.")).toBeTruthy();
  });

  it("other select settings render as radio choices", async () => {
    const user = userEvent.setup();
    const onChange = vi.fn<(next: ModelSelection) => void>();
    const model = { ...opus, settings: [{ id: "context", label: "Context", kind: "select" as const, value: "std", options: [{ value: "std", label: "Standard" }, { value: "big", label: "1M" }] }] };
    render(<ModelOptions model={model} value={{ ...currentSelection(catalog), modelSettings: {} }} onChange={onChange} showAccess={false} />);
    const group = screen.getByRole("radiogroup", { name: "Context" });
    expect(within(group).getByRole("radio", { name: "Standard" }).getAttribute("aria-checked")).toBe("true");
    await user.click(within(group).getByRole("radio", { name: "1M" }));
    expect(onChange.mock.lastCall![0].modelSettings).toEqual({ context: "big" });
  });
});
