import { act, render, renderHook, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
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
    expect(send).toHaveBeenCalledWith("yes", undefined, {});
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
    await waitFor(() => expect(retry).toHaveBeenCalledWith("pending:1", {}));
  });
});

describe("useGuardedReplies wake consent (question answers and Retry)", () => {
  type Replies = ReturnType<typeof useGuardedReplies>;
  const answers = (...results: Array<{ sent: boolean; error?: string }>) => {
    const queue = [...results];
    return vi.fn((..._args: unknown[]) => Promise.resolve(queue.shift() ?? { sent: true }));
  };
  function mountReplies(send: ReturnType<typeof answers>, retry: ReturnType<typeof answers>) {
    const holder: { current: Replies | null } = { current: null };
    function Harness() {
      const replies = useGuardedReplies({ desktopId: "d1", sessionId: "s1", target: { kind: "sandbox" }, send: send as never, retry: retry as never });
      holder.current = replies;
      return replies.dialog;
    }
    render(<Harness />);
    return holder as { current: Replies };
  }
  const wakeGuard = () => registerSendGuard(() => Promise.resolve({ kind: "confirm" as const, notice: "Wake cost", wake: true as const }));
  const allowGuard = () => registerSendGuard(() => Promise.resolve({ kind: "allow" as const }));
  const press = async (name: string | RegExp) => {
    const dialog = await screen.findByRole("dialog");
    await userEvent.click(within(dialog).getByRole("button", { name }));
  };

  it("onChoose sends wake: true once after the guard's wake notice is confirmed, and nothing before", async () => {
    wakeGuard();
    const send = answers();
    const replies = mountReplies(send, answers());
    let done!: Promise<void>;
    act(() => {
      done = replies.current.onChoose("yes");
    });
    await screen.findByRole("dialog");
    expect(send).not.toHaveBeenCalled();
    await press("Send");
    await act(() => done);
    expect(send).toHaveBeenCalledTimes(1);
    expect(send).toHaveBeenCalledWith("yes", undefined, { wake: true });
  });

  it("onChoose answered wake_required asks, then resends with wake: true once", async () => {
    allowGuard();
    const send = answers({ sent: false, error: "wake_required" });
    const replies = mountReplies(send, answers());
    let done!: Promise<void>;
    act(() => {
      done = replies.current.onChoose("yes");
    });
    await screen.findByRole("dialog");
    expect(send).toHaveBeenCalledTimes(1);
    expect(send).toHaveBeenLastCalledWith("yes", undefined, {});
    await press("Send");
    await act(() => done);
    expect(send).toHaveBeenCalledTimes(2);
    expect(send).toHaveBeenLastCalledWith("yes", undefined, { wake: true });
  });

  it("cancelling the wake notice of an answer resends nothing", async () => {
    allowGuard();
    const send = answers({ sent: false, error: "wake_required" });
    const replies = mountReplies(send, answers());
    let cancelled!: Promise<void>;
    act(() => {
      cancelled = replies.current.onChoose("no");
    });
    cancelled.catch(() => undefined);
    await press(/Cancel/);
    await expect(cancelled).rejects.toThrow("Send cancelled");
    expect(send).toHaveBeenCalledTimes(1);
  });

  it("onRetry asks first for a stopped instance and retries with wake: true once", async () => {
    wakeGuard();
    const retry = answers();
    const replies = mountReplies(answers(), retry);
    act(() => replies.current.onRetry("pending:1"));
    await screen.findByRole("dialog");
    expect(retry).not.toHaveBeenCalled();
    await press("Send");
    await waitFor(() => expect(retry).toHaveBeenCalledTimes(1));
    expect(retry).toHaveBeenCalledWith("pending:1", { wake: true });
  });

  it("onRetry answered wake_required asks and retries again with wake: true once", async () => {
    allowGuard();
    const retry = answers({ sent: false, error: "wake_required" });
    const replies = mountReplies(answers(), retry);
    act(() => replies.current.onRetry("pending:1"));
    await screen.findByRole("dialog");
    expect(retry).toHaveBeenCalledTimes(1);
    expect(retry).toHaveBeenLastCalledWith("pending:1", {});
    await press("Send");
    await waitFor(() => expect(retry).toHaveBeenCalledTimes(2));
    expect(retry).toHaveBeenLastCalledWith("pending:1", { wake: true });
  });
});
