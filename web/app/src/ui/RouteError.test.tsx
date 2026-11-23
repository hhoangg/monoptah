import { render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ApiError } from "@/features/accounts/api";
import { RouteError } from "./RouteError";

const invalidate = vi.fn(async () => undefined);
vi.mock("@tanstack/react-router", () => ({
  useRouter: () => ({ invalidate }),
  ErrorComponent: ({ error }: { error: Error }) => <p>Something went wrong! {error.message}</p>,
}));

beforeEach(() => {
  vi.useFakeTimers();
  invalidate.mockClear();
});
afterEach(() => vi.useRealTimers());

describe("RouteError", () => {
  it("does not show the error screen for a 429 and loads the route again after Retry-After", () => {
    const error = new ApiError({ status: 429, code: "rate_limited", message: "Too many requests", retryAfter: 2 });
    render(<RouteError error={error} reset={() => undefined} info={undefined} />);
    expect(screen.queryByText(/something went wrong/i)).toBeNull();
    vi.advanceTimersByTime(1900);
    expect(invalidate).not.toHaveBeenCalled();
    vi.advanceTimersByTime(700);
    expect(invalidate).toHaveBeenCalledTimes(1);
  });

  it("keeps the router's own error screen for any other error", () => {
    render(<RouteError error={new Error("boom")} reset={() => undefined} info={undefined} />);
    expect(screen.getByText(/something went wrong/i)).toBeInTheDocument();
    vi.advanceTimersByTime(120_000);
    expect(invalidate).not.toHaveBeenCalled();
  });
});
