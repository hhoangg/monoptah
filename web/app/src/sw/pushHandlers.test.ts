import { readFileSync } from "node:fs";
import path from "node:path";
import { describe, expect, it, vi } from "vitest";
import { NAVIGATE_MESSAGE_TYPE, createPushHandlers, type ClientLike, type PushDeps } from "./pushHandlers";

const ORIGIN = "https://app.example";

function setup(clients: ClientLike[] = [], overrides: Partial<PushDeps> = {}) {
  const shown: Array<{ title: string; options: NotificationOptions }> = [];
  const opened: string[] = [];
  const deps: PushDeps = {
    origin: ORIGIN,
    showNotification: (title, options) => {
      shown.push({ title, options });
      return Promise.resolve();
    },
    listWindowClients: () => Promise.resolve(clients),
    openWindow: (url) => {
      opened.push(url);
      return Promise.resolve(null);
    },
    resubscribe: () => Promise.resolve(true),
    ...overrides,
  };
  return { handlers: createPushHandlers(deps), shown, opened };
}

function pushEvent(payload: unknown, raw?: () => unknown) {
  const waits: Array<Promise<unknown>> = [];
  return { event: { data: { json: raw ?? (() => payload) }, waitUntil: (p: Promise<unknown>) => void waits.push(p) }, settle: () => Promise.all(waits) };
}

function clickEvent(data: unknown) {
  const waits: Array<Promise<unknown>> = [];
  const close = vi.fn();
  return { event: { notification: { data, close }, waitUntil: (p: Promise<unknown>) => void waits.push(p) }, close, settle: () => Promise.all(waits) };
}

function client(url: string) {
  return { url, focus: vi.fn(() => Promise.resolve()), postMessage: vi.fn() };
}

describe("push", () => {
  it("shows a session notification tagged with the session id", async () => {
    const { handlers, shown } = setup();
    const { event, settle } = pushEvent({ v: 1, t: "Studio Mac", b: "api: Needs your input", url: "/d/dk1/s/s9", tag: "s9" });
    handlers.push(event);
    await settle();
    expect(shown).toEqual([{ title: "Studio Mac", options: { body: "api: Needs your input", icon: "/icons/icon-192.png", badge: "/icons/badge-96.png", tag: "s9", data: { url: "/d/dk1/s/s9" } } }]);
  });

  it("shows the session or Mono name as the title and the message preview as the body", async () => {
    const { handlers, shown } = setup();
    const { event, settle } = pushEvent({ v: 1, t: "Bach Tieu Thuan", b: "Fixed the login bug. All tests pass.", url: "/d/dk1/s/s9", tag: "s9" });
    handlers.push(event);
    await settle();
    expect(shown[0]?.title).toBe("Bach Tieu Thuan");
    expect(shown[0]?.options.body).toBe("Fixed the login bug. All tests pass.");
    expect(shown[0]?.options.data).toEqual({ url: "/d/dk1/s/s9" });
  });

  it("passes the Mono chat URL of a Mono message through (the URL rule lives in the Worker)", async () => {
    const { handlers, shown } = setup();
    const { event, settle } = pushEvent({ v: 1, t: "Studio Mac", b: "Atlas: New message", url: "/d/dk1/monos/m7", tag: "s9" });
    handlers.push(event);
    await settle();
    expect(shown[0]?.options.data).toEqual({ url: "/d/dk1/monos/m7" });
  });

  it("shows the test notification of Settings with its tag and the /settings click target", async () => {
    const { handlers, shown } = setup();
    const { event, settle } = pushEvent({ v: 1, t: "Monoptah", b: "Test notification", url: "/settings", tag: "monoptah-test" });
    handlers.push(event);
    await settle();
    expect(shown).toEqual([{ title: "Monoptah", options: { body: "Test notification", icon: "/icons/icon-192.png", badge: "/icons/badge-96.png", tag: "monoptah-test", data: { url: "/settings" } } }]);
  });

  it("keeps the mono:<id> tag of a Mono chat notification", async () => {
    const { handlers, shown } = setup();
    const { event, settle } = pushEvent({ v: 1, t: "Studio Mac", b: "Atlas: New message", url: "/d/dk1/monos/m7", tag: "mono:m7" });
    handlers.push(event);
    await settle();
    expect(shown[0]?.options.tag).toBe("mono:m7");
    expect(shown[0]?.options.data).toEqual({ url: "/d/dk1/monos/m7" });
  });

  it.each([
    ["missing body", { v: 1, t: "a", url: "/d/a/s/b", tag: "b" }],
    ["numeric title", { v: 1, t: 4, b: "b", url: "/d/a/s/b", tag: "b" }],
    ["array", [1, 2]],
    ["string", "plain text"],
  ])("shows a generic notification with a title and a body for %s", async (_name, payload) => {
    const { handlers, shown } = setup();
    const { event, settle } = pushEvent(payload);
    handlers.push(event);
    await settle();
    expect(shown).toHaveLength(1);
    expect(shown[0]?.title).toBe("Monoptah");
    expect(shown[0]?.options.body).toBeTruthy();
  });

  it("passes the session URL of a habit run through (the URL rule lives in the Worker)", async () => {
    const { handlers, shown } = setup();
    const { event, settle } = pushEvent({ v: 1, t: "Studio Mac", b: "Atlas: Finished", url: "/d/dk1/s/s9", tag: "s9" });
    handlers.push(event);
    await settle();
    expect(shown[0]?.options.data).toEqual({ url: "/d/dk1/s/s9" });
  });

  it.each([
    ["not the contract", { hello: "world" }],
    ["wrong version", { v: 2, t: "a", b: "b", url: "/d/a/s/b", tag: "b" }],
    ["no data", null],
  ])("still shows a generic notification for %s", async (_name, payload) => {
    const { handlers, shown } = setup();
    const { event, settle } = pushEvent(payload);
    handlers.push(event);
    await settle();
    expect(shown).toHaveLength(1);
    expect(shown[0]?.title).toBe("Monoptah");
    expect(shown[0]?.options.data).toEqual({ url: "/" });
  });

  it("shows a generic notification when the data is not JSON or absent", async () => {
    const { handlers, shown } = setup();
    const bad = pushEvent(null, () => {
      throw new SyntaxError("x");
    });
    handlers.push(bad.event);
    await bad.settle();
    const waits: Array<Promise<unknown>> = [];
    handlers.push({ data: null, waitUntil: (p) => void waits.push(p) });
    await Promise.all(waits);
    expect(shown.map((item) => item.title)).toEqual(["Monoptah", "Monoptah"]);
  });

  it("does not look at which windows are visible: a focused app still gets the notification", async () => {
    const listWindowClients = vi.fn(() => Promise.resolve([{ ...client(`${ORIGIN}/d/dk1/s/s9`), visibilityState: "visible", focused: true }]));
    const { handlers, shown } = setup([], { listWindowClients });
    const { event, settle } = pushEvent({ v: 1, t: "a", b: "b", url: "/d/dk1/s/s9", tag: "s9" });
    handlers.push(event);
    await settle();
    expect(shown).toHaveLength(1);
    expect(listWindowClients).not.toHaveBeenCalled();
  });
});

describe("notificationclick", () => {
  it.each([
    ["absolute", "https://evil.example/d/x"],
    ["protocol relative", "//evil.example"],
    ["backslash", "/\\evil.example"],
    ["javascript", "javascript:alert(1)"],
    ["other path", "/login"],
  ])("opens the root for a hostile URL (%s)", async (_name, url) => {
    const { handlers, opened } = setup();
    const click = clickEvent({ url });
    handlers.notificationclick(click.event);
    await click.settle();
    expect(click.close).toHaveBeenCalledOnce();
    expect(opened).toEqual(["/"]);
  });

  it("opens the root when the notification has no data", async () => {
    const { handlers, opened } = setup();
    const click = clickEvent(undefined);
    handlers.notificationclick(click.event);
    await click.settle();
    expect(opened).toEqual(["/"]);
  });

  it("opens a window when none of this origin is open", async () => {
    const other = client("https://other.example/");
    const { handlers, opened } = setup([other]);
    const click = clickEvent({ url: "/d/dk1/monos/m7" });
    handlers.notificationclick(click.event);
    await click.settle();
    expect(opened).toEqual(["/d/dk1/monos/m7"]);
    expect(other.postMessage).not.toHaveBeenCalled();
  });

  it("focuses an existing window and asks it to navigate", async () => {
    const existing = client(`${ORIGIN}/desktops`);
    const { handlers, opened } = setup([client("https://other.example/"), existing]);
    const click = clickEvent({ url: "/d/dk1/s/s9" });
    handlers.notificationclick(click.event);
    await click.settle();
    expect(opened).toEqual([]);
    expect(existing.postMessage).toHaveBeenCalledWith({ type: NAVIGATE_MESSAGE_TYPE, url: "/d/dk1/s/s9", at: expect.any(Number) });
    expect(existing.focus).toHaveBeenCalledOnce();
  });
});

describe("notificationclick targets", () => {
  it.each(["/d/x/s/y", "/d/x/monos/m", "/settings", "/settings/notifications"])("opens a new window at %s when none is open", async (url) => {
    const { handlers, opened } = setup();
    const click = clickEvent({ url });
    handlers.notificationclick(click.event);
    await click.settle();
    expect(opened).toEqual([url]);
  });

  it.each(["/d/x/s/y", "/d/x/monos/m", "/settings", "/settings/notifications"])("focuses an open window and navigates it to %s", async (url) => {
    const existing = client(`${ORIGIN}/`);
    const { handlers, opened } = setup([existing]);
    const click = clickEvent({ url });
    handlers.notificationclick(click.event);
    await click.settle();
    expect(opened).toEqual([]);
    expect(existing.postMessage).toHaveBeenCalledWith({ type: NAVIGATE_MESSAGE_TYPE, url, at: expect.any(Number) });
    expect(existing.focus).toHaveBeenCalledOnce();
  });
});

describe("pushsubscriptionchange", () => {
  it("re-subscribes, and a failure does not break the event", async () => {
    const resubscribe = vi.fn(() => Promise.resolve(true));
    const { handlers } = setup([], { resubscribe });
    const waits: Array<Promise<unknown>> = [];
    handlers.pushsubscriptionchange({ waitUntil: (p) => void waits.push(p) });
    await Promise.all(waits);
    expect(resubscribe).toHaveBeenCalledOnce();

    const failing = setup([], { resubscribe: () => Promise.reject(new Error("offline")) });
    const failWaits: Array<Promise<unknown>> = [];
    failing.handlers.pushsubscriptionchange({ waitUntil: (p) => void failWaits.push(p) });
    await expect(Promise.all(failWaits)).resolves.toBeDefined();
  });
});

describe("sw.ts wiring", () => {
  const source = readFileSync(path.resolve(import.meta.dirname, "../sw.ts"), "utf8");

  it("registers the three push listeners", () => {
    for (const type of ["push", "notificationclick", "pushsubscriptionchange"]) {
      expect(source).toContain(`addEventListener("${type}"`);
    }
  });

  it("never suppresses a notification by window state", () => {
    const handlers = readFileSync(path.resolve(import.meta.dirname, "pushHandlers.ts"), "utf8");
    expect(`${source}\n${handlers}`).not.toMatch(/visibilityState|\.focused/);
  });
});

describe("notificationclick pending target", () => {
  it.each([
    ["a Mono message", "/d/dk2/monos/m7"],
    ["a session event", "/d/dk1/s/s9"],
  ])("saves %s for the page and still opens a window when none is open", async (_name, url) => {
    const saved: Array<[string, number]> = [];
    const { handlers, opened } = setup([], { savePending: (u, at) => Promise.resolve(void saved.push([u, at])), now: () => 1234 });
    const click = clickEvent({ url });
    handlers.notificationclick(click.event);
    await click.settle();
    expect(saved).toEqual([[url, 1234]]);
    expect(opened).toEqual([url]);
  });

  it("saves the target and posts it to an open window (a different desktop than the one shown)", async () => {
    const saved: string[] = [];
    const open = client(`${ORIGIN}/d/dk1/s/s1`);
    const { handlers } = setup([open], { savePending: (u) => Promise.resolve(void saved.push(u)) });
    const click = clickEvent({ url: "/d/dk2/monos/m7" });
    handlers.notificationclick(click.event);
    await click.settle();
    expect(saved).toEqual(["/d/dk2/monos/m7"]);
    expect(open.postMessage).toHaveBeenCalledWith({ type: NAVIGATE_MESSAGE_TYPE, url: "/d/dk2/monos/m7", at: expect.any(Number) });
    expect(open.focus).toHaveBeenCalledOnce();
  });

  it("gives the saved target and the message the same tap id", async () => {
    const saved: number[] = [];
    const open = client(`${ORIGIN}/`);
    const { handlers } = setup([open], { savePending: (_u, at) => Promise.resolve(void saved.push(at)), now: () => 777 });
    const click = clickEvent({ url: "/d/dk1/s/s9" });
    handlers.notificationclick(click.event);
    await click.settle();
    expect(saved).toEqual([777]);
    expect(open.postMessage).toHaveBeenCalledWith({ type: NAVIGATE_MESSAGE_TYPE, url: "/d/dk1/s/s9", at: 777 });
  });

  it("saves nothing for the root fallback", async () => {
    const savePending = vi.fn(() => Promise.resolve());
    const { handlers } = setup([], { savePending });
    const click = clickEvent(undefined);
    handlers.notificationclick(click.event);
    await click.settle();
    expect(savePending).not.toHaveBeenCalled();
  });

  it("survives a failing store, a rejecting openWindow and a failing focus", async () => {
    const { handlers } = setup([], { savePending: () => Promise.reject(new Error("quota")), openWindow: () => Promise.reject(new Error("ios")) });
    const click = clickEvent({ url: "/d/dk1/s/s9" });
    handlers.notificationclick(click.event);
    await expect(click.settle()).resolves.toBeDefined();
    const open = { ...client(`${ORIGIN}/`), focus: vi.fn(() => Promise.reject(new Error("no"))) };
    const second = setup([open]);
    const again = clickEvent({ url: "/d/dk1/s/s9" });
    second.handlers.notificationclick(again.event);
    await expect(again.settle()).resolves.toBeDefined();
  });
});
