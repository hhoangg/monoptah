import { render } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { closeSessionNotifications, useCloseNotificationsOnOpen } from "./notifications";

function installRegistration(tags: Record<string, number>) {
  const closed: string[] = [];
  const getNotifications = vi.fn(({ tag }: { tag: string }) =>
    Promise.resolve(Array.from({ length: tags[tag] ?? 0 }, () => ({ close: () => void closed.push(tag) }))),
  );
  Object.defineProperty(navigator, "serviceWorker", { configurable: true, value: { getRegistration: () => Promise.resolve({ getNotifications }) } });
  return { closed, getNotifications };
}

afterEach(() => {
  Reflect.deleteProperty(navigator, "serviceWorker");
});

describe("closeSessionNotifications", () => {
  it("closes the delivered notifications of one tag only", async () => {
    const { closed, getNotifications } = installRegistration({ s9: 2, s1: 1 });
    expect(await closeSessionNotifications("s9")).toBe(2);
    expect(getNotifications).toHaveBeenCalledWith({ tag: "s9" });
    expect(closed).toEqual(["s9", "s9"]);
  });

  it("is a no-op without a service worker or registration", async () => {
    expect(await closeSessionNotifications("s9")).toBe(0);
    Object.defineProperty(navigator, "serviceWorker", { configurable: true, value: { getRegistration: () => Promise.resolve(undefined) } });
    expect(await closeSessionNotifications("s9")).toBe(0);
  });
});

describe("useCloseNotificationsOnOpen", () => {
  function Probe({ id }: { id: string | undefined }) {
    useCloseNotificationsOnOpen(id);
    return null;
  }

  it("closes on open, on a change of session and when the tab becomes visible", async () => {
    const { getNotifications } = installRegistration({ s9: 1, s2: 1 });
    const view = render(<Probe id="s9" />);
    await vi.waitFor(() => expect(getNotifications).toHaveBeenCalledTimes(1));
    view.rerender(<Probe id="s2" />);
    await vi.waitFor(() => expect(getNotifications).toHaveBeenLastCalledWith({ tag: "s2" }));
    document.dispatchEvent(new Event("visibilitychange"));
    await vi.waitFor(() => expect(getNotifications).toHaveBeenCalledTimes(3));
  });

  it("does nothing without a session id", () => {
    const { getNotifications } = installRegistration({});
    render(<Probe id={undefined} />);
    expect(getNotifications).not.toHaveBeenCalled();
  });
});
