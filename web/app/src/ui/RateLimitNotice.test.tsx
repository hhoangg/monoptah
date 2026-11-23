import { act, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { clearRateLimited, noteRateLimited, useRateLimitState } from "@/lib/rateLimitState";
import { RateLimitNotice } from "./RateLimitNotice";

beforeEach(() => vi.useFakeTimers());
afterEach(() => {
  clearRateLimited();
  vi.useRealTimers();
});

describe("RateLimitNotice", () => {
  it("shows nothing until a call is limited", () => {
    render(<RateLimitNotice />);
    expect(screen.queryByRole("status")).toBeNull();
  });

  it("shows a polite status with the countdown, then clears itself when the window passes", () => {
    render(<RateLimitNotice />);
    act(() => noteRateLimited(3));
    // The live region text is static, so the countdown is not announced on every tick; the seconds are hidden from it.
    const status = screen.getByRole("status");
    expect(status).toHaveTextContent("Too many requests. Retrying shortly.");
    expect(screen.getByTestId("rate-limit-seconds")).toHaveAttribute("aria-hidden", "true");
    expect(screen.getByTestId("rate-limit-seconds")).toHaveTextContent("(3 s)");
    const announced = status.textContent?.replace(screen.getByTestId("rate-limit-seconds").textContent ?? "", "");
    act(() => void vi.advanceTimersByTime(1000));
    expect(screen.getByTestId("rate-limit-seconds")).toHaveTextContent("(2 s)");
    expect(status.textContent?.replace(screen.getByTestId("rate-limit-seconds").textContent ?? "", "")).toBe(announced);
    act(() => void vi.advanceTimersByTime(2100));
    expect(screen.queryByRole("status")).toBeNull();
    expect(useRateLimitState.getState().until).toBeNull();
  });

  it("goes away at once when a call succeeds", () => {
    render(<RateLimitNotice />);
    act(() => noteRateLimited(30));
    expect(screen.getByRole("status")).toBeInTheDocument();
    act(() => clearRateLimited());
    expect(screen.queryByRole("status")).toBeNull();
  });
});
