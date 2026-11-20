import { act, renderHook } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { useConfirmedMissing } from "./useConfirmedMissing";

function setup(initial: { isFetching?: boolean; dataUpdatedAt?: number; loaded?: boolean; missing?: boolean }) {
  const refetch = vi.fn(() => Promise.resolve());
  const props = { isFetching: false, dataUpdatedAt: 1000, loaded: true, missing: true, ...initial };
  const hook = renderHook((p: typeof props) => useConfirmedMissing({ isFetching: p.isFetching, dataUpdatedAt: p.dataUpdatedAt, refetch }, p.loaded, p.missing, () => 5000), { initialProps: props });
  return { hook, refetch, props };
}

describe("useConfirmedMissing", () => {
  it("does not flag a stale catalog (fetched before the mount) and asks for a refetch once", () => {
    const { hook, refetch, props } = setup({});
    expect(hook.result.current).toBe(false);
    expect(refetch).toHaveBeenCalledOnce();
    hook.rerender({ ...props });
    expect(refetch).toHaveBeenCalledOnce();
  });

  it("flags it only after a fetch that finished after the mount, and not while fetching", () => {
    const { hook, props } = setup({});
    hook.rerender({ ...props, isFetching: true, dataUpdatedAt: 6000 });
    expect(hook.result.current).toBe(false);
    act(() => hook.rerender({ ...props, isFetching: false, dataUpdatedAt: 6000 }));
    expect(hook.result.current).toBe(true);
  });

  it("never flags an item that is in the catalog or a catalog that has not loaded", () => {
    expect(setup({ missing: false, dataUpdatedAt: 6000 }).hook.result.current).toBe(false);
    const unloaded = setup({ loaded: false, dataUpdatedAt: 6000 });
    expect(unloaded.hook.result.current).toBe(false);
    expect(unloaded.refetch).not.toHaveBeenCalled();
  });
});
