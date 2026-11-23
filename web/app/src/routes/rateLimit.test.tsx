// Consecutive 429s on /api/me during the root auth guard must not reach the router's error screen. With a few 429s the
// guard itself waits for Retry-After; with more than it retries, `loadMe` gives up, the router error component (RouteError)
// takes over and calls `router.invalidate` after Retry-After. Either way the "Too many requests" notice shows meanwhile
// and the page renders at the end. The DOM is watched the whole time, so a flash of the error screen cannot slip through.
import { render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { setApiTransport, type ApiTransport } from "@/features/accounts/api";
import { createMockState } from "@/features/accounts/mock/state";
import { createMockServer } from "@/features/accounts/mock/transport";
import { mountApp } from "@/features/desktops/testMount";
import { MAX_RATE_LIMIT_RETRIES } from "@/lib/rateLimit";
import { clearRateLimited } from "@/lib/rateLimitState";
import { RateLimitNotice } from "@/ui/RateLimitNotice";

vi.mock("@/features/accounts/webauthn", () => ({ createPasskey: vi.fn(), getPasskey: vi.fn() }));
vi.mock("@/features/offline/accountCache", () => ({ restoreAccountSnapshots: vi.fn(async () => undefined) }));

let restore: (() => void) | undefined;
let observer: MutationObserver | undefined;
let sawErrorScreen = false;

/** Records whether "Something went wrong" was ever in the DOM, even for one frame. */
function watchForErrorScreen() {
  sawErrorScreen = false;
  const wrong = /something went wrong/i;
  // Records carry the nodes that were added even if they are gone again by the time this runs.
  const check = (records: MutationRecord[]) => {
    for (const record of records) {
      if (wrong.test(record.target.textContent ?? "") || [...record.addedNodes].some((node) => wrong.test(node.textContent ?? ""))) sawErrorScreen = true;
    }
  };
  observer = new MutationObserver(check);
  observer.observe(document.body, { subtree: true, childList: true, characterData: true });
}

/** Answers the first `limited` calls of /api/me with 429 (Retry-After 1) and everything else like the mock server. */
function limitMe(limited: number) {
  const mock = createMockServer({ state: createMockState({ signedIn: true }) }).transport;
  const counter = { calls: 0 };
  const transport: ApiTransport = async (url, init) => {
    if (url === "/api/me" && counter.calls++ < limited) {
      return new Response(JSON.stringify({ error: "Too many requests", code: "rate_limited" }), {
        status: 429,
        headers: { "Content-Type": "application/json", "Retry-After": "1" },
      });
    }
    return mock(url, init);
  };
  restore = setApiTransport(transport);
  return counter;
}

beforeEach(() => {
  // No jitter, so every wait is exactly Retry-After.
  vi.spyOn(Math, "random").mockReturnValue(0);
});
afterEach(() => {
  observer?.disconnect();
  restore?.();
  clearRateLimited();
  vi.restoreAllMocks();
});

describe("consecutive 429s on /api/me", () => {
  it("2 in a row: the guard waits, the notice shows, the error screen never does, the page renders", async () => {
    const counter = limitMe(2);
    watchForErrorScreen();

    render(<RateLimitNotice />);
    const { router } = mountApp("/settings/profile");
    const invalidate = vi.spyOn(router, "invalidate");

    const notice = await screen.findByTestId("rate-limit-notice");
    expect(within(notice).getByRole("status")).toHaveTextContent("Too many requests. Retrying shortly.");
    expect(screen.queryByText(/something went wrong/i)).toBeNull();

    await screen.findByRole("heading", { level: 1, name: "Profile" }, { timeout: 8000 });
    await waitFor(() => expect(screen.queryByTestId("rate-limit-notice")).toBeNull());
    expect(counter.calls).toBe(3);
    // The guard's own wait was enough: the router error component was not needed.
    expect(invalidate).not.toHaveBeenCalled();
    expect(sawErrorScreen).toBe(false);
  }, 15_000);

  it("more than the guard retries: RouteError takes over, invalidates after Retry-After and the page renders", async () => {
    const limited = MAX_RATE_LIMIT_RETRIES + 1;
    const counter = limitMe(limited);
    watchForErrorScreen();

    render(<RateLimitNotice />);
    const { router } = mountApp("/settings/profile");
    const invalidate = vi.spyOn(router, "invalidate");

    expect(await screen.findByTestId("rate-limit-notice")).toBeInTheDocument();
    expect(screen.queryByText(/something went wrong/i)).toBeNull();

    // The guard gave up (limited calls spent), RouteError waited and asked the router to load the route again.
    await waitFor(() => expect(invalidate).toHaveBeenCalled(), { timeout: 8000 });
    expect(screen.queryByText(/something went wrong/i)).toBeNull();

    await screen.findByRole("heading", { level: 1, name: "Profile" }, { timeout: 8000 });
    await waitFor(() => expect(screen.queryByTestId("rate-limit-notice")).toBeNull());
    expect(counter.calls).toBeGreaterThan(limited);
    expect(sawErrorScreen).toBe(false);
  }, 20_000);
});
