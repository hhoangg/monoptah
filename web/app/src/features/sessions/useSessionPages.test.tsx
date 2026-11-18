import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, renderHook, waitFor } from "@testing-library/react";
import type { ReactNode } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";

const request = vi.fn();
vi.mock("@/link", async (importOriginal) => ({
  ...(await importOriginal<Record<string, unknown>>()),
  useLinkConnection: () => ({ request }),
  useLinkDesktopId: () => "d1",
}));

import { useSessionPages } from "./useSessionPages";

const card = (id: string) => ({ id, title: id, model: "m", branch: "b", projectId: "p1" });

function wrapper() {
  const client = new QueryClient();
  return ({ children }: { children: ReactNode }) => <QueryClientProvider client={client}>{children}</QueryClientProvider>;
}

beforeEach(() => {
  request.mockReset();
});

describe("useSessionPages next-page failures", () => {
  it("keeps loadMore stable and stops asking after a failed page until retryMore", async () => {
    request.mockResolvedValueOnce({ sessions: [card("a")], hasMore: true });
    const { result } = renderHook(() => useSessionPages("p1", ""), { wrapper: wrapper() });
    await waitFor(() => expect(result.current.paged).not.toBeNull());
    const loadMore = result.current.loadMore;

    request.mockImplementation(() => Promise.reject(new Error("offline")));
    // What the list does: ask again whenever the hook renders and the list is near its end.
    for (let round = 0; round < 20; round += 1) {
      await act(async () => {
        result.current.loadMore();
        await new Promise((resolve) => setTimeout(resolve, 5));
      });
    }
    expect(request).toHaveBeenCalledTimes(2);
    expect(result.current.nextError).toBe(true);
    expect(result.current.loadMore).toBe(loadMore);

    request.mockResolvedValueOnce({ sessions: [card("b")], hasMore: false });
    await act(async () => {
      result.current.retryMore();
    });
    await waitFor(() => expect(result.current.paged?.map((s) => s.id)).toEqual(["a", "b"]));
    expect(request).toHaveBeenCalledTimes(3);
    expect(result.current.nextError).toBe(false);
  });
});
