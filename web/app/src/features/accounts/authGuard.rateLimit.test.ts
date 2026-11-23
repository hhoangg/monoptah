import { MeResponse } from "@monoptah/protocol";
import { QueryClient } from "@tanstack/react-query";
import type { ParsedLocation } from "@tanstack/react-router";
import { isRedirect } from "@tanstack/react-router";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { restoreAccountSnapshots } from "@/features/offline/accountCache";
import { clearRateLimited } from "@/lib/rateLimitState";
import { setApiTransport, type ApiTransport } from "./api";
import { authGuard } from "./authGuard";
import { createMockServer } from "./mock/transport";

vi.mock("@/features/offline/accountCache", () => ({ restoreAccountSnapshots: vi.fn(async () => undefined) }));

let restore: (() => void) | undefined;
beforeEach(() => {
  sessionStorage.clear();
  vi.mocked(restoreAccountSnapshots).mockResolvedValue(undefined);
});
afterEach(() => {
  restore?.();
  clearRateLimited();
  vi.useRealTimers();
});

const location = (pathname: string) => ({ pathname, searchStr: "", hash: "", href: pathname }) as unknown as ParsedLocation;

function limitedResponse(retryAfter: string) {
  return new Response(JSON.stringify({ error: "Too many requests", code: "rate_limited" }), {
    status: 429,
    headers: { "Content-Type": "application/json", "Retry-After": retryAfter },
  });
}

/** `/api/me` answers `first` once per entry, then the real mock server. */
function install(first: Array<() => Response>, options: { signedIn: boolean }) {
  const mock = createMockServer(options).transport;
  const seen = { me: 0 };
  const transport: ApiTransport = async (url, init) => {
    if (url === "/api/me") {
      seen.me++;
      const next = first.shift();
      if (next) return next();
    }
    return mock(url, init);
  };
  restore = setApiTransport(transport);
  return { seen, mock };
}

describe("authGuard with a 429 on /api/me", () => {
  it("waits for Retry-After and then lets a signed-in user through", async () => {
    vi.useFakeTimers();
    const { seen } = install([() => limitedResponse("2")], { signedIn: true });
    const done = authGuard({ location: location("/settings/desktops"), context: { queryClient: new QueryClient() } });
    await vi.advanceTimersByTimeAsync(500);
    expect(seen.me).toBe(1);
    await vi.advanceTimersByTimeAsync(2500);
    await expect(done).resolves.toBeUndefined();
    expect(seen.me).toBe(2);
  });

  it("uses the saved account at once instead of waiting", async () => {
    const { seen, mock } = install([() => limitedResponse("30")], { signedIn: true });
    const me = MeResponse.parse(await (await mock("/api/me", { method: "GET" })).json());
    vi.mocked(restoreAccountSnapshots).mockResolvedValue(me);
    await expect(authGuard({ location: location("/settings/desktops"), context: { queryClient: new QueryClient() } })).resolves.toBeUndefined();
    expect(seen.me).toBe(1);
  });

  it("still redirects a 401 that follows the 429", async () => {
    vi.useFakeTimers();
    install([() => limitedResponse("1")], { signedIn: false });
    const result = Promise.resolve(authGuard({ location: location("/settings/desktops"), context: { queryClient: new QueryClient() } })).catch((error: unknown) => error);
    await vi.advanceTimersByTimeAsync(3000);
    const error = await result;
    expect(isRedirect(error) && error.options).toMatchObject({ to: "/login" });
  });

  it("throws the 429 after the attempts ran out (the router error component then recovers)", async () => {
    vi.useFakeTimers();
    install(
      Array.from({ length: 10 }, () => () => limitedResponse("1")),
      { signedIn: true },
    );
    const result = Promise.resolve(authGuard({ location: location("/settings/desktops"), context: { queryClient: new QueryClient() } })).catch((error: unknown) => error);
    await vi.advanceTimersByTimeAsync(20_000);
    expect(await result).toMatchObject({ status: 429, code: "rate_limited" });
  });
});
