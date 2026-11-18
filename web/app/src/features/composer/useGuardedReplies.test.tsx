import { act, renderHook, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { clearHooks, registerSendGuard } from "@/ui/hooks";
import { useGuardedReplies } from "./useSendGuard";

afterEach(() => clearHooks());

function setup(sent = true, error?: string) {
  const send = vi.fn(() => Promise.resolve({ sent, ...(error ? { error } : {}) }));
  const retry = vi.fn(() => Promise.resolve({ sent: true }));
  const view = renderHook(() => useGuardedReplies({ desktopId: "d1", sessionId: "s1", target: { kind: "remote" }, send, retry }));
  return { send, retry, view };
}

describe("useGuardedReplies", () => {
  it("onChoose sends when the guard allows, with the session context", async () => {
    const guard = vi.fn(() => Promise.resolve({ kind: "allow" as const }));
    registerSendGuard(guard);
    const { send, view } = setup();
    await act(() => view.result.current.onChoose("yes"));
    expect(guard).toHaveBeenCalledWith({ desktopId: "d1", sessionId: "s1", target: { kind: "remote" } });
    expect(send).toHaveBeenCalledWith("yes");
  });

  it("onChoose rejects with the guard reason and does not send", async () => {
    registerSendGuard(() => Promise.resolve({ kind: "refuse" as const, reason: "Not now" }));
    const { send, view } = setup();
    await expect(act(() => view.result.current.onChoose("yes"))).rejects.toThrow("Not now");
    expect(send).not.toHaveBeenCalled();
  });

  it("onChoose rejects with a readable reason when the desktop refuses", async () => {
    const { view } = setup(false, "read_only");
    await expect(act(() => view.result.current.onChoose("yes"))).rejects.toThrow(/only read/);
  });

  it("onRetry does not retry when the guard refuses, and keeps the reason", async () => {
    registerSendGuard(() => Promise.resolve({ kind: "refuse" as const, reason: "Instance stopped" }));
    const { retry, view } = setup();
    act(() => view.result.current.onRetry("pending:1"));
    await waitFor(() => expect(view.result.current.error).toBe("Instance stopped"));
    expect(retry).not.toHaveBeenCalled();
  });

  it("clearError drops a refusal that a later successful send has made stale", async () => {
    registerSendGuard(() => Promise.resolve({ kind: "refuse" as const, reason: "Instance stopped" }));
    const { view } = setup();
    act(() => view.result.current.onRetry("pending:1"));
    await waitFor(() => expect(view.result.current.error).toBe("Instance stopped"));
    act(() => view.result.current.clearError());
    expect(view.result.current.error).toBeNull();
  });

  it("onRetry retries when the guard allows", async () => {
    const { retry, view } = setup();
    act(() => view.result.current.onRetry("pending:1"));
    await waitFor(() => expect(retry).toHaveBeenCalledWith("pending:1"));
  });
});
