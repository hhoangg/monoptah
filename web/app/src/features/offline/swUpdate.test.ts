import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const toastMock = vi.hoisted(() => vi.fn());
vi.mock("@/ui/sonner", () => ({ toast: toastMock }));

import { applyWhenNoSend, isSendInProgress, setSendInProgressCheck, showUpdateToast, SEND_POLL_MS, UPDATE_TOAST_ID, UPDATE_TOAST_TITLE, UPDATE_WAITING_COPY } from "./swUpdate";

type Options = { id: string; duration: number; action?: { label: string; onClick: (event: { preventDefault: () => void }) => void }; description?: string };

let restore: () => void;
beforeEach(() => {
  vi.useFakeTimers();
  toastMock.mockClear();
  const previous = setSendInProgressCheck(() => false);
  restore = () => setSendInProgressCheck(previous);
});
afterEach(() => {
  restore();
  vi.useRealTimers();
});

describe("send-in-progress registry", () => {
  it("defaults to no send in progress", () => {
    expect(isSendInProgress()).toBe(false);
  });

  it("follows the registered check and treats a throwing check as no send", () => {
    setSendInProgressCheck(() => true);
    expect(isSendInProgress()).toBe(true);
    setSendInProgressCheck(() => {
      throw new Error("boom");
    });
    expect(isSendInProgress()).toBe(false);
  });
});

describe("update toast", () => {
  function shown(): Options {
    const call = toastMock.mock.calls[0] as [string, Options];
    expect(call[0]).toBe(UPDATE_TOAST_TITLE);
    return call[1];
  }

  it("shows a persistent toast with a Reload action and reloads at once when no send runs", () => {
    const apply = vi.fn();
    showUpdateToast(apply);
    const options = shown();
    expect(UPDATE_TOAST_TITLE).toBe("Update available");
    expect(options.description).toBe("A new version of Monoptah is ready.");
    expect(options).toMatchObject({ id: UPDATE_TOAST_ID, duration: Infinity });
    expect(options.action?.label).toBe("Reload");
    expect(apply).not.toHaveBeenCalled();
    const preventDefault = vi.fn();
    options.action?.onClick({ preventDefault });
    expect(apply).toHaveBeenCalledTimes(1);
    expect(preventDefault).not.toHaveBeenCalled();
  });

  it("never reloads while a send is in progress and reloads once it ends", () => {
    let sending = true;
    setSendInProgressCheck(() => sending);
    const apply = vi.fn();
    showUpdateToast(apply);
    const preventDefault = vi.fn();
    shown().action?.onClick({ preventDefault });
    expect(preventDefault).toHaveBeenCalled();
    expect(apply).not.toHaveBeenCalled();
    // The toast stays and says why.
    expect(toastMock).toHaveBeenLastCalledWith(UPDATE_TOAST_TITLE, expect.objectContaining({ id: UPDATE_TOAST_ID, description: UPDATE_WAITING_COPY }));
    vi.advanceTimersByTime(SEND_POLL_MS * 10);
    expect(apply).not.toHaveBeenCalled();
    sending = false;
    vi.advanceTimersByTime(SEND_POLL_MS);
    expect(apply).toHaveBeenCalledTimes(1);
    vi.advanceTimersByTime(SEND_POLL_MS * 10);
    expect(apply).toHaveBeenCalledTimes(1);
  });

  it("applyWhenNoSend can be cancelled", () => {
    setSendInProgressCheck(() => true);
    const apply = vi.fn();
    const cancel = applyWhenNoSend(apply);
    cancel();
    setSendInProgressCheck(() => false);
    vi.advanceTimersByTime(SEND_POLL_MS * 4);
    expect(apply).not.toHaveBeenCalled();
  });
});
