import { QueryClient } from "@tanstack/react-query";
import type { ParsedLocation } from "@tanstack/react-router";
import { isRedirect } from "@tanstack/react-router";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { setApiTransport } from "./api";
import { authGuard } from "./authGuard";
import { createMockServer, type MockTransportOptions } from "./mock/transport";

const FP = "cd".repeat(32);
let restore: (() => void) | undefined;
afterEach(() => restore?.());
beforeEach(() => sessionStorage.clear());

function location(pathname: string, init: { searchStr?: string; hash?: string } = {}): ParsedLocation {
  return { pathname, searchStr: init.searchStr ?? "", hash: init.hash ?? "", href: pathname } as unknown as ParsedLocation;
}

async function run(path: string, options: MockTransportOptions, init?: { searchStr?: string; hash?: string }) {
  restore = setApiTransport(createMockServer(options).transport);
  const queryClient = new QueryClient();
  try {
    await authGuard({ location: location(path, init), context: { queryClient } });
    return null;
  } catch (error) {
    if (isRedirect(error)) return error.options;
    throw error;
  }
}

describe("authGuard", () => {
  it.each(["/login", "/register", "/recover", "/install", "/login/"])("lets the public route %s pass without a session", async (path) => {
    expect(await run(path, { signedIn: false })).toBeNull();
  });

  it("lets a signed-in user through", async () => {
    expect(await run("/desktops", { signedIn: true })).toBeNull();
  });

  it("redirects a 401 to /login with the wanted path", async () => {
    const redirect = await run("/pair/c0de", { signedIn: false }, { searchStr: "?x=1" });
    expect(redirect).toMatchObject({ to: "/login", search: { redirect: "/pair/c0de?x=1" } });
  });

  it("saves #fp before the login redirect and keeps it out of the redirect value", async () => {
    const redirect = await run("/pair/c0de", { signedIn: false }, { hash: `fp=${FP}` });
    expect(JSON.stringify(redirect)).not.toContain(FP);
    expect(sessionStorage.getItem("monoptah.fp")).toContain(FP);
  });

  it("confines a recoveryOnly session to /recover", async () => {
    expect(await run("/settings", { signedIn: true, recoveryOnly: true })).toMatchObject({ to: "/recover" });
    expect(await run("/recover", { signedIn: true, recoveryOnly: true })).toBeNull();
  });

  it("treats a 403 recovery_only from GET /api/me as a recovery session (defensive: the Worker allows /api/me there today)", async () => {
    restore = setApiTransport(async () => new Response(JSON.stringify({ error: "recovery only", code: "recovery_only" }), { status: 403, headers: { "Content-Type": "application/json" } }));
    const queryClient = new QueryClient();
    let error: unknown = null;
    try {
      await authGuard({ location: location("/settings"), context: { queryClient } });
    } catch (caught) {
      error = caught;
    }
    expect(isRedirect(error) && error.options).toMatchObject({ to: "/recover" });
  });

  it("forces the regenerate screen while recoveryAckAt is null", async () => {
    expect(await run("/desktops", { signedIn: true, recoveryAckAt: null })).toMatchObject({ to: "/settings" });
    expect(await run("/d/dk1", { signedIn: true, recoveryAckAt: null })).toMatchObject({ to: "/settings" });
    expect(await run("/settings", { signedIn: true, recoveryAckAt: null })).toBeNull();
  });

  it("does not turn a server failure into a login redirect", async () => {
    restore = setApiTransport(async () => Response.json({ error: "boom", code: "internal" }, { status: 500 }));
    const queryClient = new QueryClient();
    await expect(authGuard({ location: location("/desktops"), context: { queryClient } })).rejects.toMatchObject({ status: 500 });
  });

  it("never lets an open-redirect path become the redirect value", async () => {
    const redirect = await run("//evil.example/x", { signedIn: false });
    expect(redirect).toMatchObject({ to: "/login", search: { redirect: "/" } });
  });
});
