import { createStore } from "zustand/vanilla";
import { describe, expect, it, vi } from "vitest";
import { LinkRequestError, type LinkState } from "@/link";
import { initialLinkState } from "@/link/store";
import { DesktopLinkError } from "./desktopLink";
import { createRealDesktopLink, toDesktopLinkStatus, type SettingsLinkConnection } from "./realLink";

function fakeConnection(initial: Partial<LinkState> = {}) {
  const store = createStore<LinkState>(() => ({ ...initialLinkState(), ...initial }));
  const request = vi.fn(async () => ({ ok: true }));
  const connection = {
    store,
    getState: () => store.getState(),
    request,
    reconnect: vi.fn(),
    retain: vi.fn(),
    release: vi.fn(),
  } as unknown as SettingsLinkConnection;
  return { store, request, connection };
}

describe("toDesktopLinkStatus", () => {
  it("maps link states to the settings statuses", () => {
    const base = { blocked: null, presence: null } as const;
    expect(toDesktopLinkStatus({ ...base, status: "ready" })).toBe("connected");
    expect(toDesktopLinkStatus({ ...base, status: "ready", presence: "offline" })).toBe("offline");
    expect(toDesktopLinkStatus({ ...base, status: "handshaking" })).toBe("connecting");
    expect(toDesktopLinkStatus({ ...base, status: "pending" })).toBe("connecting");
    expect(toDesktopLinkStatus({ ...base, status: "reconnecting" })).toBe("reconnecting");
    expect(toDesktopLinkStatus({ ...base, status: "closed" })).toBe("offline");
    expect(toDesktopLinkStatus({ status: "closed", blocked: "revoked", presence: null })).toBe("failed");
  });
});

describe("createRealDesktopLink", () => {
  it("calls the connection when ready", async () => {
    const { connection, request } = fakeConnection({ status: "ready" });
    await expect(createRealDesktopLink(connection).call("workspace.list")).resolves.toEqual({ ok: true });
    expect(request).toHaveBeenCalledWith("workspace.list", undefined);
  });

  it("waits for the handshake before a call", async () => {
    const { connection, store, request } = fakeConnection({ status: "handshaking" });
    const pending = createRealDesktopLink(connection).call("workspace.list");
    expect(request).not.toHaveBeenCalled();
    store.setState({ status: "ready" });
    await pending;
    expect(request).toHaveBeenCalledTimes(1);
  });

  it("rejects with desktop_offline when the link is closed or the wait runs out", async () => {
    const closed = fakeConnection({ status: "closed" });
    await expect(createRealDesktopLink(closed.connection).call("workspace.list")).rejects.toMatchObject({ code: "desktop_offline" });
    const slow = fakeConnection({ status: "connecting" });
    await expect(createRealDesktopLink(slow.connection, 5).call("workspace.list")).rejects.toBeInstanceOf(DesktopLinkError);
  });

  it("keeps the desktop error code of a refused request", async () => {
    const { connection, request } = fakeConnection({ status: "ready" });
    request.mockRejectedValueOnce(new LinkRequestError("not allowed", "forbidden"));
    await expect(createRealDesktopLink(connection).call("workspace.list")).rejects.toMatchObject({ code: "forbidden" });
  });

  it("reconnect restarts the handshake and resolves when ready", async () => {
    const { connection, store } = fakeConnection({ status: "closed", blocked: "revoked" });
    const link = createRealDesktopLink(connection);
    vi.mocked(connection.reconnect).mockImplementation(() => {
      store.setState({ blocked: null, status: "connecting" });
      queueMicrotask(() => store.setState({ status: "ready" }));
    });
    await link.reconnect();
    expect(connection.reconnect).toHaveBeenCalledTimes(1);
  });

  it("holds the connection while a listener is subscribed", () => {
    const { connection, store } = fakeConnection({ status: "ready" });
    const listener = vi.fn();
    const unsubscribe = createRealDesktopLink(connection).subscribe(listener);
    expect(connection.retain).toHaveBeenCalledTimes(1);
    store.setState({ status: "reconnecting" });
    expect(listener).toHaveBeenCalled();
    unsubscribe();
    expect(connection.release).toHaveBeenCalledTimes(1);
  });
});
