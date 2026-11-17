import { QueryClient } from "@tanstack/react-query";
import { afterEach, describe, expect, it, vi } from "vitest";
import { ApiError } from "./api";
import { installSessionExpiryHandler, type ExpiryRouter } from "./sessionExpiry";

function setup(location: { pathname: string; searchStr: string }, isLoading = false) {
  const queryClient = new QueryClient();
  const navigate = vi.fn();
  const router: ExpiryRouter = { state: { isLoading, location }, navigate };
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
    expect(ctx.queryClient.getQueryData(["link", "d1"])).toEqual({ keep: true });
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
