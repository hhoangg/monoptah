import { QueryClient } from "@tanstack/react-query";
import { afterEach, describe, expect, it, vi } from "vitest";
import { ApiError } from "./api";
import { installSessionExpiryHandler, type ExpiryRouter } from "./sessionExpiry";

function setup(location: { pathname: string; searchStr: string; hash?: string }, isLoading = false) {
  const queryClient = new QueryClient();
  const navigate = vi.fn();
  const router: ExpiryRouter = { state: { isLoading, location: { hash: "", ...location } }, navigate };
  const off = installSessionExpiryHandler(queryClient, router);
  return { queryClient, navigate, off };
}

const unauthorized = () => new ApiError({ status: 401, code: "unauthorized", message: "Sign in first" });
let off: (() => void) | undefined;
afterEach(() => off?.());

async function failWith(queryClient: QueryClient, key: string[], error: Error) {
  await queryClient.fetchQuery({ queryKey: key, queryFn: () => Promise.reject(error), retry: false }).catch(() => undefined);
  await Promise.resolve();
}

describe("session expiry handler", () => {
  it("clears the account cache and sends the user to /login with the page they were on", async () => {
    const ctx = setup({ pathname: "/settings", searchStr: "?tab=1" });
    off = ctx.off;
    ctx.queryClient.setQueryData(["passkeys"], { passkeys: [] });
    ctx.queryClient.setQueryData(["link", "d1"], { keep: true });
    await failWith(ctx.queryClient, ["sessions"], unauthorized());
    expect(ctx.navigate).toHaveBeenCalledWith({ to: "/login", search: { redirect: "/settings?tab=1" }, replace: true });
    expect(ctx.queryClient.getQueryData(["passkeys"])).toBeUndefined();
    // Desktop link data goes too: nothing of the old account stays readable.
    expect(ctx.queryClient.getQueryData(["link", "d1"])).toBeUndefined();
  });

  it("parks #fp before the redirect and also reacts to a 401 from a mutation", async () => {
    sessionStorage.clear();
    const fp = "ab".repeat(32);
    const ctx = setup({ pathname: "/pair/c0de", searchStr: "", hash: `#fp=${fp}` });
    off = ctx.off;
    await ctx.queryClient.getMutationCache().build(ctx.queryClient, { mutationFn: () => Promise.reject(unauthorized()) }).execute(undefined).catch(() => undefined);
    await Promise.resolve();
    expect(ctx.navigate).toHaveBeenCalledWith({ to: "/login", search: { redirect: "/pair/c0de" }, replace: true });
    expect(sessionStorage.getItem("monoptah.fp")).toContain(fp);
  });

  it("ignores other failures, public pages and a navigation the guard is already handling", async () => {
    const other = setup({ pathname: "/settings", searchStr: "" });
    await failWith(other.queryClient, ["sessions"], new ApiError({ status: 500, code: "internal", message: "x" }));
    expect(other.navigate).not.toHaveBeenCalled();
    other.off();
    const publicPage = setup({ pathname: "/login", searchStr: "" });
    await failWith(publicPage.queryClient, ["me"], unauthorized());
    expect(publicPage.navigate).not.toHaveBeenCalled();
    publicPage.off();
    const loading = setup({ pathname: "/settings", searchStr: "" }, true);
    await failWith(loading.queryClient, ["me"], unauthorized());
    expect(loading.navigate).not.toHaveBeenCalled();
    loading.off();
  });
});
