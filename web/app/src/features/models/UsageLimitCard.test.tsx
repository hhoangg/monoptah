import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type { UsageLimit } from "@monoptah/protocol";
import { UsageLimitCard } from "./UsageLimitCard";

const NOW = Date.UTC(2026, 5, 1, 12, 0, 0);
const state: UsageLimit = { token: "t1", resetsAt: NOW + 3 * 3_600_000, resumeAtReset: false, canResume: true, canAutoResume: true, resetLabel: "Resets at 3 PM.", limitation: "Only this session waits." };

describe("UsageLimitCard", () => {
  it("shows the desktop's wording and resumes", async () => {
    const user = userEvent.setup();
    const onResume = vi.fn(async () => {});
    render(<UsageLimitCard state={state} now={NOW} onResume={onResume} onResumeAtReset={async () => {}} />);
    expect(screen.getByText("Usage limit reached")).toBeTruthy();
    expect(screen.getByText(/Your conversation is saved\. Resets at 3 PM\./)).toBeTruthy();
    expect(screen.getByText("Only this session waits.")).toBeTruthy();
    expect(screen.getByText(/in 3h/)).toBeTruthy();
    await user.click(screen.getByRole("button", { name: "Resume" }));
    expect(onResume).toHaveBeenCalledTimes(1);
  });

  it("the switch arms and cancels the automatic resume", async () => {
    const user = userEvent.setup();
    const onResumeAtReset = vi.fn(async () => {});
    const { rerender } = render(<UsageLimitCard state={state} now={NOW} onResume={async () => {}} onResumeAtReset={onResumeAtReset} />);
    await user.click(screen.getByRole("switch", { name: "Resume at reset" }));
    expect(onResumeAtReset).toHaveBeenLastCalledWith(true);
    rerender(<UsageLimitCard state={{ ...state, resumeAtReset: true }} now={NOW} onResume={async () => {}} onResumeAtReset={onResumeAtReset} />);
    const armed = screen.getByRole("switch", { name: "Cancel automatic resume" });
    expect(armed.getAttribute("aria-checked")).toBe("true");
    await user.click(armed);
    expect(onResumeAtReset).toHaveBeenLastCalledWith(false);
  });

  it("shows the failure inline and lets the user try again", async () => {
    const user = userEvent.setup();
    const onResume = vi.fn().mockRejectedValueOnce(new Error("This session changed.")).mockResolvedValue(undefined);
    render(<UsageLimitCard state={state} now={NOW} onResume={onResume} onResumeAtReset={async () => {}} />);
    await user.click(screen.getByRole("button", { name: "Resume" }));
    expect((await screen.findByRole("alert")).textContent).toBe("This session changed. Try again using the controls below.");
    await user.click(screen.getByRole("button", { name: "Resume" }));
    await waitFor(() => expect(screen.queryByRole("alert")).toBeNull());
  });

  it("ignores a second press while one is running", async () => {
    const user = userEvent.setup();
    let release: () => void = () => {};
    const onResume = vi.fn(() => new Promise<void>((resolve) => (release = resolve)));
    render(<UsageLimitCard state={state} now={NOW} onResume={onResume} onResumeAtReset={async () => {}} />);
    await user.click(screen.getByRole("button", { name: "Resume" }));
    expect(screen.getByRole("button", { name: "Updating..." })).toHaveProperty("disabled", true);
    release();
    await waitFor(() => expect(screen.getByRole("button", { name: "Resume" })).toHaveProperty("disabled", false));
    expect(onResume).toHaveBeenCalledTimes(1);
  });

  it("offline and read-only disable both controls and say why; the desktop's flags gate them", () => {
    const { rerender } = render(<UsageLimitCard state={state} connected={false} onResume={async () => {}} onResumeAtReset={async () => {}} />);
    expect(screen.getByText("Reconnect to desktop to change continuation settings.")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Resume" })).toHaveProperty("disabled", true);
    expect(screen.getByRole("switch")).toHaveProperty("disabled", true);
    rerender(<UsageLimitCard state={state} readOnly onResume={async () => {}} onResumeAtReset={async () => {}} />);
    expect(screen.getByText(/read only/)).toBeTruthy();
    expect(screen.getByRole("button", { name: "Resume" })).toHaveProperty("disabled", true);
    rerender(<UsageLimitCard state={{ ...state, canResume: false, canAutoResume: false }} onResume={async () => {}} onResumeAtReset={async () => {}} />);
    expect(screen.getByRole("button", { name: "Resume" })).toHaveProperty("disabled", true);
    expect(screen.getByRole("switch")).toHaveProperty("disabled", true);
  });
});
