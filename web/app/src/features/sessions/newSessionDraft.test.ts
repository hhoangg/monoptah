import { QueryClient } from "@tanstack/react-query";
import { act, renderHook } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { clearAccountCache, signOut } from "@/features/accounts/queries";
import { getLinkStore } from "@/link";
import { clearLinkStores } from "@/link/store";
import type { ModelSelection } from "@/features/models/modelSettings";
import { clearNewSessionDraft, forgetNewSessionDrafts, useForgetDraftsWhenRevoked, readNewSessionDraft, resetNewSessionDrafts, writeNewSessionDraft } from "./newSessionDraft";

afterEach(() => {
  resetNewSessionDrafts();
  clearLinkStores();
});

describe("new session draft store", () => {
  it("keeps text and the model pick per desktop and project", () => {
    const pick = { harness: "h", model: "m" } as unknown as ModelSelection;
    writeNewSessionDraft("d1", "p1", { text: "hello" });
    writeNewSessionDraft("d1", "p1", { picked: pick });
    expect(readNewSessionDraft("d1", "p1")).toEqual({ text: "hello", picked: pick });
    expect(readNewSessionDraft("d1", "p2")).toEqual({ text: "", picked: null });
    expect(readNewSessionDraft("d2", "p1")).toEqual({ text: "", picked: null });
  });

  it("forgets an emptied draft and a cleared one", () => {
    writeNewSessionDraft("d1", "p1", { text: "x" });
    writeNewSessionDraft("d1", "p1", { text: "" });
    expect(readNewSessionDraft("d1", "p1")).toEqual({ text: "", picked: null });
    writeNewSessionDraft("d1", "p1", { text: "y" });
    clearNewSessionDraft("d1", "p1");
    expect(readNewSessionDraft("d1", "p1").text).toBe("");
  });

  it("is dropped by clearAccountCache (sign-in, register, recover, 401)", () => {
    writeNewSessionDraft("d1", "p1", { text: "private" });
    clearAccountCache(new QueryClient());
    expect(readNewSessionDraft("d1", "p1").text).toBe("");
  });

  it("is dropped by signOut, even when the logout call fails", async () => {
    writeNewSessionDraft("d1", "p1", { text: "private" });
    await signOut(new QueryClient()).catch(() => undefined);
    expect(readNewSessionDraft("d1", "p1").text).toBe("");
  });

  it("drops only one desktop's drafts when it is forgotten", () => {
    writeNewSessionDraft("d1", "p1", { text: "a" });
    writeNewSessionDraft("d1", "p2", { text: "b" });
    writeNewSessionDraft("d2", "p1", { text: "c" });
    forgetNewSessionDrafts("d1");
    expect(readNewSessionDraft("d1", "p1").text).toBe("");
    expect(readNewSessionDraft("d1", "p2").text).toBe("");
    expect(readNewSessionDraft("d2", "p1").text).toBe("c");
  });

  it("is dropped when the desktop's link is revoked", () => {
    writeNewSessionDraft("d1", "p1", { text: "a" });
    writeNewSessionDraft("d2", "p1", { text: "c" });
    renderHook(() => useForgetDraftsWhenRevoked("d1"));
    expect(readNewSessionDraft("d1", "p1").text).toBe("a");
    act(() => getLinkStore("d1").setState({ revoked: true }));
    expect(readNewSessionDraft("d1", "p1").text).toBe("");
    expect(readNewSessionDraft("d2", "p1").text).toBe("c");
  });
});
