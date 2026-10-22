import { afterEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({ invoke: vi.fn() }));

vi.mock("@tauri-apps/api/core", () => ({ invoke: mocks.invoke }));

async function loadStore() {
  vi.resetModules();
  return import("./sessionStore");
}

afterEach(() => {
  mocks.invoke.mockReset();
});

describe("deleteSession", () => {
  it("sends exactly the same arguments as before when no Mono reset is given", async () => {
    mocks.invoke.mockResolvedValue([]);
    const { deleteSession } = await loadStore();
    await deleteSession("s1", ["/img.png"]);
    expect(mocks.invoke).toHaveBeenLastCalledWith("session_delete", {
      sessionId: "s1",
      imagePaths: ["/img.png"],
    });
    await deleteSession("s2", [], { deleteProviderFiles: true });
    expect(mocks.invoke).toHaveBeenLastCalledWith("session_delete", {
      sessionId: "s2",
      imagePaths: [],
      deleteProviderFiles: true,
    });
  });

  it("passes the Mono reset sessions to the backend", async () => {
    mocks.invoke.mockResolvedValue([]);
    const { deleteSession } = await loadStore();
    const sessions = [
      {
        harness: "claude" as const,
        id: "11111111-1111-4111-8111-111111111111",
      },
    ];
    await deleteSession("s1", [], { monoReset: { sessions } });
    expect(mocks.invoke).toHaveBeenLastCalledWith("session_delete", {
      sessionId: "s1",
      imagePaths: [],
      monoReset: { sessions },
    });
  });

  it("returns the problems the backend reports", async () => {
    mocks.invoke.mockResolvedValue(["Could not delete one file"]);
    const { deleteSession } = await loadStore();
    expect(
      await deleteSession("s1", [], { monoReset: { sessions: [] } }),
    ).toEqual(["Could not delete one file"]);
  });

  it("returns no problems when the backend reports none or nothing", async () => {
    const { deleteSession } = await loadStore();
    mocks.invoke.mockResolvedValue([]);
    expect(await deleteSession("s1")).toEqual([]);
    mocks.invoke.mockResolvedValue(undefined);
    expect(await deleteSession("s2")).toEqual([]);
  });

  it("still rejects when the backend fails", async () => {
    mocks.invoke.mockRejectedValue(new Error("Storage unavailable"));
    const { deleteSession } = await loadStore();
    await expect(deleteSession("s1")).rejects.toThrow("Storage unavailable");
  });
});
