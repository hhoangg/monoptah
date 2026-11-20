import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { CodeDialog } from "./CodeDialog";
import { CODE_RESTART_NOTICE } from "./copy";
import type { TerminalSnapshot } from "./controller";

const base: TerminalSnapshot = { phase: "code", challenge: { challengeId: "c1", expiresAt: Date.now() + 60_000, codeLength: 6, attempts: 3, attemptsLeft: 3 }, codeExpired: false, notice: null, error: null, session: null, ended: null, inputNotice: null };

function mount(snapshot: Partial<TerminalSnapshot> = {}) {
  const handlers = { onSubmit: vi.fn(), onNewCode: vi.fn(), onCancel: vi.fn() };
  const view = render(<CodeDialog snapshot={{ ...base, ...snapshot }} where="Example Alpha/src" {...handlers} />);
  return { ...view, ...handlers };
}

beforeEach(() => {
  Element.prototype.hasPointerCapture ??= () => false;
  Element.prototype.scrollIntoView ??= () => {};
});
afterEach(() => vi.useRealTimers());

describe("CodeDialog", () => {
  it("asks for the code shown on the desktop and says where the shell opens", () => {
    mount();
    expect(screen.getByRole("dialog", { name: "Enter the code shown on your desktop" })).toBeInTheDocument();
    expect(screen.getByText(/open a shell in Example Alpha\/src/)).toBeInTheDocument();
  });

  it("upper-cases what is typed, strips spaces, caps it at the code length and enables Open only when complete", async () => {
    const { onSubmit } = mount();
    const input = screen.getByTestId("terminal-code-input");
    const submit = screen.getByRole("button", { name: "Open terminal" });
    expect(submit).toBeDisabled();
    await userEvent.type(input, "ab 23c");
    expect(input).toHaveValue("AB23C");
    expect(submit).toBeDisabled();
    await userEvent.type(input, "dxyz");
    expect(input).toHaveValue("AB23CD");
    expect(submit).toBeEnabled();
    await userEvent.click(submit);
    expect(onSubmit).toHaveBeenCalledWith("AB23CD");
  });

  it("submits on Enter", async () => {
    const { onSubmit } = mount();
    await userEvent.type(screen.getByTestId("terminal-code-input"), "AB23CD{Enter}");
    expect(onSubmit).toHaveBeenCalledWith("AB23CD");
  });

  it("shows the tries left after a wrong code as an alert, and the countdown", () => {
    mount({ challenge: { ...base.challenge!, attemptsLeft: 2 }, notice: "That code is not right. 2 tries left." });
    expect(screen.getByRole("alert")).toHaveTextContent("That code is not right. 2 tries left.");
    expect(screen.getByRole("timer")).toHaveTextContent(/Expires in [01]:\d\d\./);
  });

  it("a restart notice is information, not an error", () => {
    mount({ notice: CODE_RESTART_NOTICE });
    expect(screen.queryByRole("alert")).toBeNull();
    expect(screen.getByRole("status")).toHaveTextContent("new code");
  });

  it("an expired code offers a new one instead of Open", async () => {
    const { onNewCode, onSubmit } = mount({ codeExpired: true });
    expect(screen.queryByRole("button", { name: "Open terminal" })).toBeNull();
    expect(screen.getByTestId("terminal-code-input")).toBeDisabled();
    await userEvent.click(screen.getByRole("button", { name: "Get a new code" }));
    expect(onNewCode).toHaveBeenCalled();
    expect(onSubmit).not.toHaveBeenCalled();
  });

  it("counts the time down and turns expired by itself", async () => {
    vi.useFakeTimers({ now: 1_000_000, shouldAdvanceTime: true });
    mount({ challenge: { ...base.challenge!, expiresAt: 1_000_000 + 2_000 } });
    expect(screen.getByRole("timer")).toHaveTextContent("Expires in 0:02");
    await vi.advanceTimersByTimeAsync(2_500);
    expect(await screen.findByRole("button", { name: "Get a new code" })).toBeInTheDocument();
  });

  it("while the code is being checked nothing can be changed or cancelled", () => {
    mount({ phase: "verifying" });
    expect(screen.getByTestId("terminal-code-input")).toBeDisabled();
    expect(screen.getByRole("button", { name: "Cancel" })).toBeDisabled();
    expect(screen.queryByRole("button", { name: "Close" })).toBeNull();
  });

  it("while the desktop is asked for a code it only waits, and Cancel backs out", async () => {
    const { onCancel } = mount({ phase: "starting", challenge: null });
    expect(screen.getByText(/Asking your desktop for a code/)).toBeInTheDocument();
    expect(screen.queryByTestId("terminal-code-input")).toBeNull();
    await userEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(onCancel).toHaveBeenCalled();
  });

  it("is closed outside the code steps", () => {
    mount({ phase: "running", challenge: null });
    expect(screen.queryByRole("dialog")).toBeNull();
  });
});
