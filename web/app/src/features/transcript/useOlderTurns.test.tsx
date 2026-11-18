import { act, renderHook, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { Turn } from "@monoptah/protocol";

const request = vi.fn();
let linkStatus = "ready";
const client = { request, store: { getState: () => ({ status: linkStatus }) } };
vi.mock("@/link", () => ({ useLinkConnection: () => client }));

import { mergeOlder, OLDER_PAGE_SIZE, useOlderTurns } from "./useOlderTurns";

const turn = (id: string): Turn => ({ id, items: [] });
const ids = (turns: Turn[]) => turns.map((t) => t.id);

beforeEach(() => {
  request.mockReset();
  linkStatus = "ready";
});

describe("mergeOlder", () => {
  it("puts older pages in front and never repeats an id", () => {
    expect(ids(mergeOlder([turn("a"), turn("b"), turn("b"), turn("c")], [turn("c"), turn("d")]))).toEqual(["a", "b", "c", "d"]);
    const held = [turn("c")];
    expect(mergeOlder([], held)).toBe(held);
    expect(mergeOlder([turn("c")], held)).toBe(held);
  });
});

describe("useOlderTurns (conversation.older)", () => {
  it("asks for the page before the first turn held and prepends it", async () => {
    request.mockResolvedValueOnce({ turns: [turn("t8"), turn("t9")], older: true });
    const held = [turn("t10"), turn("t11")];
    const { result } = renderHook(() => useOlderTurns("c1", held, true));
    expect(result.current.hasOlder).toBe(true);
    await act(async () => {
      await result.current.loadOlder();
    });
    expect(request).toHaveBeenCalledWith("conversation.older", { conversationId: "c1", before: "t10", limit: OLDER_PAGE_SIZE });
    expect(ids(result.current.turns)).toEqual(["t8", "t9", "t10", "t11"]);
    expect(result.current.hasOlder).toBe(true);
    expect(result.current.loading).toBe(false);
  });

  it("pages from the oldest turn it has, and stops when the desktop says there is no more", async () => {
    request.mockResolvedValueOnce({ turns: [turn("t8"), turn("t9")], older: true }).mockResolvedValueOnce({ turns: [turn("t7")], older: false });
    const held = [turn("t10")];
    const { result } = renderHook(() => useOlderTurns("c1", held, true));
    await act(async () => {
      await result.current.loadOlder();
    });
    await act(async () => {
      await result.current.loadOlder();
    });
    expect(request).toHaveBeenLastCalledWith("conversation.older", { conversationId: "c1", before: "t8", limit: OLDER_PAGE_SIZE });
    expect(ids(result.current.turns)).toEqual(["t7", "t8", "t9", "t10"]);
    expect(result.current.hasOlder).toBe(false);
    await act(async () => {
      await result.current.loadOlder();
    });
    expect(request).toHaveBeenCalledTimes(2);
  });

  it("stops on an empty page even when the desktop claims more", async () => {
    request.mockResolvedValueOnce({ turns: [], older: true });
    const { result } = renderHook(() => useOlderTurns("c1", [turn("t10")], true));
    await act(async () => {
      await result.current.loadOlder();
    });
    expect(result.current.hasOlder).toBe(false);
  });

  it("does nothing without an older page, and asks once while one is in flight", async () => {
    const none = renderHook(() => useOlderTurns("c1", [turn("t10")], false));
    await act(async () => {
      await none.result.current.loadOlder();
    });
    expect(request).not.toHaveBeenCalled();

    let finish!: (value: unknown) => void;
    request.mockReturnValueOnce(new Promise((resolve) => (finish = resolve)));
    const held = [turn("t10")];
    const { result } = renderHook(() => useOlderTurns("c1", held, true));
    act(() => {
      void result.current.loadOlder();
      void result.current.loadOlder();
    });
    await waitFor(() => expect(result.current.loading).toBe(true));
    expect(request).toHaveBeenCalledTimes(1);
    await act(async () => finish({ turns: [turn("t9")], older: false }));
    expect(result.current.loading).toBe(false);
  });

  it("reports a failed page and lets the reader try again", async () => {
    request.mockRejectedValueOnce(new Error("timeout")).mockResolvedValueOnce({ turns: [turn("t9")], older: false });
    const held = [turn("t10")];
    const { result } = renderHook(() => useOlderTurns("c1", held, true));
    await act(async () => {
      await result.current.loadOlder();
    });
    expect(result.current.error).toBe("timeout");
    expect(result.current.hasOlder).toBe(true);
    await act(async () => {
      await result.current.loadOlder();
    });
    expect(result.current.error).toBeNull();
    expect(ids(result.current.turns)).toEqual(["t9", "t10"]);
  });

  it("drops the pages of another conversation and ignores a late answer of the old one", async () => {
    let finish!: (value: unknown) => void;
    request.mockReturnValueOnce(new Promise((resolve) => (finish = resolve)));
    const held = [turn("t10")];
    const { result, rerender } = renderHook(({ id }) => useOlderTurns(id, held, true), { initialProps: { id: "c1" } });
    act(() => void result.current.loadOlder());
    rerender({ id: "c2" });
    await act(async () => finish({ turns: [turn("t9")], older: false }));
    expect(ids(result.current.turns)).toEqual(["t10"]);
    expect(result.current.loading).toBe(false);
    expect(result.current.hasOlder).toBe(true);
  });

  it("does not ask while the link is not ready, and asks once it is", async () => {
    linkStatus = "reconnecting";
    request.mockResolvedValue({ turns: [turn("t9")], older: true });
    const held = [turn("t10")];
    const { result, rerender } = renderHook(() => useOlderTurns("c1", held, true));
    await act(async () => {
      expect(await result.current.loadOlder()).toBe(false);
    });
    expect(request).not.toHaveBeenCalled();
    linkStatus = "ready";
    rerender();
    await act(async () => {
      expect(await result.current.loadOlder()).toBe(true);
    });
    expect(request).toHaveBeenCalledTimes(1);
  });
});
