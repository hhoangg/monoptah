// ModelControls, the debounced session.configure, the usage actions and the U-07 default-model row, over a
// mocked data layer (no link): the requests are checked on the mutation mocks.
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, render, renderHook, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type { ReactNode } from "react";
import type * as DataModule from "@/data";
import type * as LinkModule from "@/link";
import { beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import type { UsageLimit } from "@monoptah/protocol";
import { SessionConfigureParamsSchema } from "@monoptah/protocol";
import { makeCatalog } from "./fixtures";

const mutate = vi.fn();
const mutateAsync = vi.fn(async () => ({}));
const queryState: { data: unknown; isError: boolean; isPending: boolean; error: null } = { data: undefined, isError: false, isPending: false, error: null };
const queryParams: Array<[string, unknown]> = [];

vi.mock("@/data", async (importOriginal) => ({
  ...(await importOriginal<typeof DataModule>()),
  useLinkQuery: (method: string, params: unknown) => {
    queryParams.push([method, params]);
    return method === "models.list" ? queryState : { data: { accounts: {} }, isError: false, isPending: false, error: null };
  },
  useLinkMutation: (method: string) => ({ mutate: (params: unknown, options?: unknown) => mutate(method, params, options), mutateAsync: (params: unknown) => (mutateAsync as (m: string, p: unknown) => Promise<unknown>)(method, params), isPending: false, error: null }),
}));
vi.mock("@/link", async (importOriginal) => ({
  ...(await importOriginal<typeof LinkModule>()),
  LinkProvider: ({ children }: { children: ReactNode }) => children,
  useLinkDesktopId: () => "dk1",
}));

import { getPersistAdapter, MemoryPersistAdapter, registerPersistAdapter } from "@/data";
import { clearSlots, getSlot } from "@/ui/slots";
import { startingSelection, loadDefaultModels, parseDefaultModels, saveDefaultModels, EMPTY_DEFAULTS } from "./defaultModel";
import { DefaultModelSection } from "./DefaultModelSection";
import { CONFIGURE_DEBOUNCE_MS, useSessionConfigure, useUsageLimitActions } from "./hooks";
import { ModelControls } from "./ModelControls";
import { currentSelection } from "./modelSettings";

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

beforeEach(() => {
  mutate.mockClear();
  mutateAsync.mockClear();
  queryParams.length = 0;
  queryState.data = makeCatalog();
  queryState.isError = false;
  registerPersistAdapter(new MemoryPersistAdapter());
});

const wrap = ({ children }: { children: ReactNode }) => <QueryClientProvider client={new QueryClient()}>{children}</QueryClientProvider>;

describe("ModelControls", () => {
  it("scopes models.list by session or project, and shows model, effort and access", () => {
    render(<ModelControls conversationId="c1" projectId="p1" />, { wrapper: wrap });
    expect(queryParams[0]).toEqual(["models.list", { conversationId: "c1", projectId: "p1" }]);
    expect(screen.getByRole("button", { name: "Model: Opus, Medium" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "Access: Supervised" })).toBeTruthy();
  });

  it("loading and failure show a disabled placeholder", () => {
    queryState.data = undefined;
    const { rerender } = render(<ModelControls />, { wrapper: wrap });
    expect(screen.getByRole("button", { name: "Model" })).toHaveProperty("disabled", true);
    queryState.isError = true;
    rerender(<ModelControls />);
    expect(screen.getByRole("button", { name: "Models unavailable" })).toBeTruthy();
  });

  it("a new chat reports selections to the page and sends nothing", async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    render(<ModelControls projectId="p1" onChange={onChange} />, { wrapper: wrap });
    await user.click(screen.getByRole("button", { name: "Access: Supervised" }));
    await user.click(within(await screen.findByRole("menu")).getAllByRole("menuitemradio")[2]!);
    expect(onChange).toHaveBeenCalledWith(expect.objectContaining({ model: "opus", runtimeMode: "auto" }));
    expect(screen.getByRole("button", { name: "Access: Auto" })).toBeTruthy();
    expect(mutate).not.toHaveBeenCalled();
  });

  it("a running session debounces changes into one session.configure without full-access", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    try {
      const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
      queryState.data = makeCatalog("full-access");
      render(<ModelControls conversationId="c1" />, { wrapper: wrap });
      expect(screen.getByRole("button", { name: "Access: Full access" })).toBeTruthy();
      await user.click(screen.getByRole("button", { name: "Model: Opus, Medium" }));
      await user.click(await screen.findByRole("button", { name: /Sonnet/ }));
      expect(mutate).not.toHaveBeenCalled();
      await act(async () => {
        vi.advanceTimersByTime(CONFIGURE_DEBOUNCE_MS + 10);
      });
      expect(mutate).toHaveBeenCalledTimes(1);
      const [method, params] = mutate.mock.calls[0]!;
      expect(method).toBe("session.configure");
      expect(params).toMatchObject({ conversationId: "c1", harness: "claude", model: "sonnet" });
      expect("runtimeMode" in (params as object)).toBe(false);
      expect(JSON.stringify(params)).not.toContain("full-access");
      expect(SessionConfigureParamsSchema.safeParse(params).success).toBe(true);
    } finally {
      vi.useRealTimers();
    }
  });
});

describe("useSessionConfigure", () => {
  it("sends only the latest selection, and flushes on unmount", () => {
    vi.useFakeTimers();
    try {
      const { result, unmount } = renderHook(() => useSessionConfigure("c1"), { wrapper: wrap });
      const base = { harness: "claude", model: "opus", modelSettings: { effort: "low" } };
      act(() => {
        result.current.configure({ ...base, runtimeMode: "auto" });
        result.current.configure({ ...base, modelSettings: { effort: "high" }, runtimeMode: "auto" });
        vi.advanceTimersByTime(CONFIGURE_DEBOUNCE_MS - 1);
      });
      expect(mutate).not.toHaveBeenCalled();
      act(() => {
        vi.advanceTimersByTime(1);
      });
      expect(mutate).toHaveBeenCalledTimes(1);
      expect(mutate.mock.calls[0]![1]).toMatchObject({ modelSettings: { effort: "high" }, runtimeMode: "auto" });
      act(() => result.current.configure({ ...base, runtimeMode: "supervised" }));
      unmount();
      expect(mutate).toHaveBeenCalledTimes(2);
    } finally {
      vi.useRealTimers();
    }
  });
});

describe("useUsageLimitActions", () => {
  const latest: UsageLimit = { token: "t1", resetsAt: null, resumeAtReset: false, canResume: true, canAutoResume: true, resetLabel: "", limitation: "" };

  it("validates against the latest state and sends the session methods", async () => {
    const { result } = renderHook(() => useUsageLimitActions("c1", latest), { wrapper: wrap });
    await result.current.resume("t1");
    await result.current.resumeAtReset("t1", true);
    expect(mutateAsync).toHaveBeenNthCalledWith(1, "session.resumeUsage", { conversationId: "c1", token: "t1" });
    expect(mutateAsync).toHaveBeenNthCalledWith(2, "session.resumeUsageAtReset", { conversationId: "c1", token: "t1", enabled: true });
  });

  it("refuses a stale token, a missing state and a state that cannot resume", async () => {
    const stale = renderHook(() => useUsageLimitActions("c1", { ...latest, token: "t2" }), { wrapper: wrap });
    await expect(stale.result.current.resume("t1")).rejects.toThrow(/changed/);
    const gone = renderHook(() => useUsageLimitActions("c1", null), { wrapper: wrap });
    await expect(gone.result.current.resume("t1")).rejects.toThrow(/no longer paused/);
    const blocked = renderHook(() => useUsageLimitActions("c1", { ...latest, canResume: false }), { wrapper: wrap });
    await expect(blocked.result.current.resume("t1")).rejects.toThrow(/cannot continue/);
    expect(mutateAsync).not.toHaveBeenCalled();
  });
});

describe("default model (U-07)", () => {
  it("parses a damaged or old record safely, dropping full-access", () => {
    expect(parseDefaultModels(null)).toEqual(EMPTY_DEFAULTS);
    expect(parseDefaultModels("x")).toEqual(EMPTY_DEFAULTS);
    const parsed = parseDefaultModels({ harness: "claude", runtimeMode: "full-access", models: { claude: { model: "opus", modelSettings: { effort: "max", n: 3 } }, bad: 1 } });
    expect(parsed.runtimeMode).toBeNull();
    expect(parsed.models).toEqual({ claude: { harness: "claude", model: "opus", modelSettings: { effort: "max" }, name: "opus" } });
  });

  it("saves per desktop under defaultModel:<desktopId> and never stores full-access", async () => {
    await saveDefaultModels("dk1", { ...EMPTY_DEFAULTS, harness: "claude", runtimeMode: "full-access" as never });
    expect(await getPersistAdapter().get("defaultModel:dk1")).toMatchObject({ runtimeMode: null });
    expect((await loadDefaultModels("dk2")).harness).toBeNull();
    await getPersistAdapter().clear("dk1");
    expect((await loadDefaultModels("dk1")).harness).toBeNull();
  });

  it("startingSelection uses the saved model when listed, else the desktop's, and the saved access", () => {
    const catalog = makeCatalog("full-access");
    const saved = { harness: "codex", models: { codex: { harness: "codex", model: "gpt-5", modelSettings: { reasoningEffort: "high" }, name: "GPT-5" } }, runtimeMode: null };
    expect(startingSelection(catalog, saved)).toMatchObject({ harness: "codex", model: "gpt-5", modelSettings: { reasoningEffort: "high" }, runtimeMode: "full-access" });
    expect(startingSelection(catalog, { ...saved, models: { codex: { ...saved.models.codex, model: "gone" } } })).toEqual(currentSelection(catalog));
    expect(startingSelection(catalog, { ...saved, runtimeMode: "auto" }).runtimeMode).toBe("auto");
  });

  it("registers in the desktop-settings-sections slot", async () => {
    clearSlots();
    await import("./register");
    expect(getSlot("desktop-settings-sections")).toBe(DefaultModelSection);
    clearSlots();
  });

  it("the section saves the picked model and access for the desktop and can reset", async () => {
    const user = userEvent.setup();
    render(<DefaultModelSection desktopId="dk1" />, { wrapper: wrap });
    expect(await screen.findByText("Opus")).toBeTruthy();
    await user.click(screen.getByRole("button", { name: /Default model/ }));
    const dialog = await screen.findByRole("dialog", { name: "Model" });
    await user.click(within(dialog).getByRole("button", { name: /Codex/ }));
    await user.click(within(dialog).getByRole("button", { name: /GPT-5/ }));
    await waitFor(async () => expect(await loadDefaultModels("dk1")).toMatchObject({ harness: "codex", models: { codex: { model: "gpt-5" } } }));
    expect(within(dialog).queryByRole("button", { name: /^Access/ })).toBeNull();
    await user.keyboard("{Escape}");
    await user.click(screen.getByRole("button", { name: "Access: Supervised" }));
    await user.click(within(await screen.findByRole("menu")).getAllByRole("menuitemradio")[1]!);
    await waitFor(async () => expect((await loadDefaultModels("dk1")).runtimeMode).toBe("auto-accept-edits"));
    await user.click(await screen.findByRole("button", { name: "Use the desktop defaults" }));
    await waitFor(async () => expect(await loadDefaultModels("dk1")).toEqual(EMPTY_DEFAULTS));
  });

  it("without a link the row says it needs the desktop", () => {
    queryState.data = undefined;
    queryState.isError = true;
    render(<DefaultModelSection desktopId="dk1" />, { wrapper: wrap });
    expect(screen.getByText("Needs a connection to the desktop")).toBeTruthy();
  });
});
