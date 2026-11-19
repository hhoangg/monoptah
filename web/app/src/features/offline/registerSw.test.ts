import { afterEach, describe, expect, it, vi } from "vitest";

type Listener = (event?: { isUpdate?: boolean }) => void;
const wb = vi.hoisted(() => ({ listeners: new Map<string, Listener>(), skip: vi.fn() }));
vi.mock("workbox-window", () => ({
  Workbox: class {
    addEventListener(name: string, listener: Listener) {
      wb.listeners.set(name, listener);
    }
    messageSkipWaiting = wb.skip;
    register = () => Promise.resolve(undefined);
  },
}));
const toastMock = vi.hoisted(() => vi.fn());
vi.mock("@/ui/sonner", () => ({ toast: toastMock }));
import { setSendInProgressCheck } from "./swUpdate";
import { registerServiceWorker, shouldRegisterServiceWorker } from "./registerSw";

describe("service worker registration gate", () => {
  it("registers in production", () => {
    expect(shouldRegisterServiceWorker({ PROD: true })).toBe(true);
  });

  it("does not register in development or in vitest without the opt-in", () => {
    expect(shouldRegisterServiceWorker({ PROD: false })).toBe(false);
    expect(shouldRegisterServiceWorker(import.meta.env)).toBe(false);
  });

  it("registers in development with VITE_REGISTER_SW=1", () => {
    expect(shouldRegisterServiceWorker({ PROD: false, VITE_REGISTER_SW: "1" })).toBe(true);
    expect(shouldRegisterServiceWorker({ PROD: false, VITE_REGISTER_SW: "0" })).toBe(false);
  });

  it("does nothing where service workers do not exist (jsdom)", () => {
    expect(registerServiceWorker()).toBeUndefined();
  });
});

describe("update prompt wiring", () => {
  type Toast = { action: { onClick: (event: { preventDefault: () => void }) => void } };
  const reload = vi.fn();
  afterEach(() => {
    vi.unstubAllGlobals();
    wb.listeners.clear();
    wb.skip.mockClear();
    toastMock.mockClear();
    reload.mockClear();
    setSendInProgressCheck(() => false);
  });

  function register() {
    vi.stubGlobal("navigator", { ...navigator, serviceWorker: {} });
    vi.stubGlobal("location", { ...window.location, reload });
    expect(registerServiceWorker()).toBeDefined();
  }
  const tap = () => (toastMock.mock.calls.at(-1) as [string, Toast])[1].action.onClick({ preventDefault: () => {} });

  it("skips waiting on the tap and reloads when the worker takes control", () => {
    register();
    wb.listeners.get("waiting")?.();
    tap();
    expect(wb.skip).toHaveBeenCalledTimes(1);
    expect(reload).not.toHaveBeenCalled();
    wb.listeners.get("controlling")?.({ isUpdate: true });
    expect(reload).toHaveBeenCalledTimes(1);
  });

  it("shows nothing and does not reload when the worker takes control on the first install", () => {
    register();
    wb.listeners.get("controlling")?.({ isUpdate: false });
    expect(toastMock).not.toHaveBeenCalled();
    expect(reload).not.toHaveBeenCalled();
  });

  it("does not reload by itself when another tab applied the update, and a tap then only reloads", () => {
    register();
    wb.listeners.get("waiting")?.();
    wb.listeners.get("controlling")?.({ isUpdate: true });
    expect(reload).not.toHaveBeenCalled();
    tap();
    expect(wb.skip).not.toHaveBeenCalled();
    expect(reload).toHaveBeenCalledTimes(1);
  });

  it("holds the reload after control changes while a send is in progress", () => {
    vi.useFakeTimers();
    let sending = false;
    setSendInProgressCheck(() => sending);
    register();
    wb.listeners.get("waiting")?.();
    tap();
    sending = true;
    wb.listeners.get("controlling")?.({ isUpdate: true });
    expect(reload).not.toHaveBeenCalled();
    sending = false;
    vi.advanceTimersByTime(600);
    expect(reload).toHaveBeenCalledTimes(1);
    vi.useRealTimers();
  });
});
