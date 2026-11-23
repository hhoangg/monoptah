import { afterEach, describe, expect, it, vi } from "vitest";
import { MeResponse } from "@monoptah/protocol";
import { clearRateLimited, useRateLimitState } from "@/lib/rateLimitState";
import { ApiError, apiFetch, endpoint, setApiTransport } from "./api";
import { endpoints } from "./endpoints";

let restore: (() => void) | undefined;
afterEach(() => restore?.());

function useTransport(fn: (url: string, init: RequestInit) => Promise<Response> | Response) {
  const mock = vi.fn(async (url: string, init: RequestInit) => fn(url, init));
  restore = setApiTransport(mock);
  return mock;
}

const ME = {
  user: { id: "u1", handle: "dev-user", displayName: "Dev", role: "user" },
  invitesLeft: 3,
  recoveryOnly: false,
  recoveryCodesLeft: 10,
  recoveryAckAt: 1,
};

describe("apiFetch", () => {
  it("sends a GET without CSRF or content-type headers, same-origin cookies", async () => {
    const transport = useTransport(() => Response.json(ME));
    const me = await apiFetch(endpoints.me);
    expect(me.user.handle).toBe("dev-user");
    const [url, init] = transport.mock.calls[0]!;
    expect(url).toBe("/api/me");
    expect(init.method).toBe("GET");
    expect(init.credentials).toBe("same-origin");
    const headers = new Headers(init.headers);
    expect(headers.get("X-Monoptah-CSRF")).toBeNull();
    expect(init.body).toBeUndefined();
  });

  it("sends CSRF and JSON content-type on a bodiless non-GET", async () => {
    const transport = useTransport(() => Response.json({ ok: true }));
    await apiFetch(endpoints.logout);
    const [, init] = transport.mock.calls[0]!;
    const headers = new Headers(init.headers);
    expect(init.method).toBe("POST");
    expect(headers.get("X-Monoptah-CSRF")).toBe("1");
    expect(headers.get("Content-Type")).toBe("application/json");
    expect(init.body).toBeUndefined();
  });

  it("serializes the body and fills encoded path params", async () => {
    const transport = useTransport(() => Response.json({ passkey: { id: "a/b", name: "N", createdAt: 1, lastUsedAt: null, deviceType: null, backedUp: false, transports: [] } }));
    await apiFetch(endpoints.renamePasskey, { params: { id: "a/b" }, body: { name: "N" } });
    const [url, init] = transport.mock.calls[0]!;
    expect(url).toBe("/api/passkeys/a%2Fb");
    expect(init.body).toBe('{"name":"N"}');
  });

  it("rejects an invalid body before sending", async () => {
    const transport = useTransport(() => Response.json({}));
    await expect(apiFetch(endpoints.patchMe, { body: { displayName: "" } })).rejects.toThrow();
    expect(transport).not.toHaveBeenCalled();
  });

  it("throws when a path parameter is missing", async () => {
    useTransport(() => Response.json({}));
    await expect(apiFetch(endpoints.revokeSession)).rejects.toThrow(/Missing path parameter/);
  });

  it("parses {error, code} and Retry-After into ApiError", async () => {
    useTransport(() => Response.json({ error: "Slow down", code: "rate_limited" }, { status: 429, headers: { "Retry-After": "30" } }));
    const error = await apiFetch(endpoints.me).catch((e: unknown) => e);
    expect(error).toBeInstanceOf(ApiError);
    expect(error).toMatchObject({ status: 429, code: "rate_limited", message: "Slow down", retryAfter: 30 });
  });

  it("notes a 429 on a session GET in the rate-limit store, but not a 429 on a POST", async () => {
    clearRateLimited();
    useTransport(() => Response.json({ error: "Slow down", code: "rate_limited" }, { status: 429, headers: { "Retry-After": "30" } }));
    await apiFetch(endpoints.logout).catch(() => undefined);
    expect(useRateLimitState.getState().until).toBeNull();
    await apiFetch(endpoints.me).catch(() => undefined);
    expect(useRateLimitState.getState().until).not.toBeNull();
    clearRateLimited();
  });

  it("maps a non-JSON error body to a generic ApiError", async () => {
    useTransport(() => new Response("<html>bad gateway</html>", { status: 502 }));
    const error = await apiFetch(endpoints.me).catch((e: unknown) => e);
    expect(error).toMatchObject({ status: 502, code: "internal", retryAfter: null });
  });

  it("maps a transport failure to a network error", async () => {
    useTransport(() => {
      throw new TypeError("Failed to fetch");
    });
    await expect(apiFetch(endpoints.me)).rejects.toMatchObject({ status: 0, code: "network" });
  });

  it("flags a response that does not match the contract", async () => {
    useTransport(() => Response.json({ nope: true }));
    await expect(apiFetch(endpoints.me)).rejects.toMatchObject({ code: "bad_response" });
  });

  it("endpoint() refuses a route that is not in the registry", () => {
    expect(() => endpoint("GET", "/api/nope", MeResponse)).toThrow(/Unknown API route/);
  });

  it("falls back to fetch when no transport is set", async () => {
    restore = setApiTransport(null);
    const fetchSpy = vi.spyOn(globalThis, "fetch").mockResolvedValue(Response.json(ME));
    await apiFetch(endpoints.me);
    expect(fetchSpy).toHaveBeenCalledWith("/api/me", expect.objectContaining({ credentials: "same-origin" }));
    fetchSpy.mockRestore();
  });
});
