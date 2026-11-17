import { QueryClient } from "@tanstack/react-query";
import { afterEach, describe, expect, it, vi } from "vitest";
import { apiFetch, setApiTransport } from "./api";
import { endpoints } from "./endpoints";
import { createMockServer } from "./mock/transport";
import { passkeysQuery, revokeSession } from "./queries";
import { installSessionExpiryHandler, type ExpiryRouter } from "./sessionExpiry";

let off: (() => void) | undefined;
let restore: (() => void) | undefined;
afterEach(() => {
  off?.();
  restore?.();
  sessionStorage.clear();
});

function setup(location: { pathname: string; searchStr: string; hash?: string }, options: { isLoading?: boolean; signedIn?: boolean } = {}) {
  restore = setApiTransport(createMockServer({ signedIn: options.signedIn ?? false }).transport);
  const queryClient = new QueryClient();
  const navigate = vi.fn();
  const router: ExpiryRouter = { state: { isLoading: options.isLoading ?? false, location: { hash: "", ...location } }, navigate };
  off = installSessionExpiryHandler(queryClient, router);
  return { queryClient, navigate };
}

const settle = () => new Promise((resolve) => setTimeout(resolve, 0));

describe("session expiry handler", () => {
  it("an action that answers 401 (revoke session) clears the cache and goes to /login with the page", async () => {
    const ctx = setup({ pathname: "/settings", searchStr: "?tab=1" });
    ctx.queryClient.setQueryData(["passkeys"], { passkeys: [] });
    ctx.queryClient.setQueryData(["link", "d1"], { keep: false });
    await expect(revokeSession(ctx.queryClient, "sess-x")).rejects.toMatchObject({ status: 401 });
    await settle();
    expect(ctx.navigate).toHaveBeenCalledTimes(1);
    expect(ctx.navigate).toHaveBeenCalledWith({ to: "/login", search: { redirect: "/settings?tab=1" }, replace: true });
    expect(ctx.queryClient.getQueryData(["passkeys"])).toBeUndefined();
    expect(ctx.queryClient.getQueryData(["link", "d1"])).toBeUndefined();
  });

  it("a query that answers 401 takes the same single path", async () => {
    const ctx = setup({ pathname: "/settings", searchStr: "" });
    await ctx.queryClient.fetchQuery({ ...passkeysQuery, retry: false }).catch(() => undefined);
    await settle();
    expect(ctx.navigate).toHaveBeenCalledTimes(1);
  });

  it("parks #fp before the redirect", async () => {
    const fp = "ab".repeat(32);
    const ctx = setup({ pathname: "/pair/c0de", searchStr: "", hash: `#fp=${fp}` });
    await revokeSession(ctx.queryClient, "x").catch(() => undefined);
    await settle();
    expect(sessionStorage.getItem("monoptah.fp")).toContain(fp);
  });

  it("ignores a 401 on a public auth route (wrong recovery code is a normal failure)", async () => {
    const ctx = setup({ pathname: "/settings", searchStr: "" });
    await expect(apiFetch(endpoints.recover, { body: { handle: "nobody", code: "AAAA" } })).rejects.toMatchObject({ status: 401 });
    await settle();
    expect(ctx.navigate).not.toHaveBeenCalled();
  });

  it("ignores other failures, public pages and a navigation the guard is already handling", async () => {
    const onPublic = setup({ pathname: "/login", searchStr: "" });
    await revokeSession(onPublic.queryClient, "x").catch(() => undefined);
    await settle();
    expect(onPublic.navigate).not.toHaveBeenCalled();
    off?.();
    const loading = setup({ pathname: "/settings", searchStr: "" }, { isLoading: true });
    await revokeSession(loading.queryClient, "x").catch(() => undefined);
    await settle();
    expect(loading.navigate).not.toHaveBeenCalled();
    off?.();
    const signedIn = setup({ pathname: "/settings", searchStr: "" }, { signedIn: true });
    await revokeSession(signedIn.queryClient, "missing").catch(() => undefined);
    await settle();
    expect(signedIn.navigate).not.toHaveBeenCalled();
  });
});
