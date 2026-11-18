import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { IDBFactory } from "fake-indexeddb";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { setApiTransport } from "@/features/accounts/api";
import { createMockServer } from "@/features/accounts/mock/transport";
import { postSignInTarget } from "@/features/accounts/redirect";
import { base64UrlEncode } from "@/link/base64url";
import { fingerprint, displayFingerprint } from "@/link/fingerprint";
import { checkPin, getPin } from "@/link/pins";
import type * as LinkModule from "@/link";

const h = vi.hoisted(() => ({ reconnect: vi.fn() }));
vi.mock("@/link", async (importOriginal) => ({ ...(await importOriginal<typeof LinkModule>()), getLinkConnection: () => ({ reconnect: h.reconnect }) }));

import { createFakeServer, desktop, type FakeServer } from "./testServer";
import { mountApp } from "./testMount";

const OLD_KEY = new Uint8Array(32).fill(1);
const NEW_KEY = new Uint8Array(32).fill(2);
const OTHER_KEY = new Uint8Array(32).fill(3);
const NEW_FP = fingerprint(NEW_KEY);
let server: FakeServer;
let restore: (() => void) | undefined;
let keys: Record<string, string> = {};

function install(serverKey: Uint8Array) {
  keys = { d1: base64UrlEncode(serverKey) };
  server = createFakeServer({ desktops: [desktop({ id: "d1", name: "Studio Mac" })], publicKeys: keys });
  restore = setApiTransport(server.transport);
}

async function trust() {
  await userEvent.click(await screen.findByRole("checkbox", { name: /matches Settings > Web app on my desktop/ }));
  await userEvent.click(screen.getByRole("button", { name: "Trust this key" }));
}

async function pinnedFingerprint(): Promise<string | undefined> {
  return (await getPin("d1"))?.fingerprint;
}

beforeEach(async () => {
  globalThis.indexedDB = new IDBFactory();
  sessionStorage.clear();
  await checkPin("d1", OLD_KEY);
});
afterEach(() => {
  restore?.();
  vi.restoreAllMocks();
  vi.clearAllMocks();
});

describe("/desktops/:id/verify", () => {
  it("does not re-pin on load: it shows the fingerprint and waits for an explicit confirmation", async () => {
    install(NEW_KEY);
    mountApp(`/desktops/d1/verify#fp=${NEW_FP}`);
    await screen.findByText("Check the fingerprint");
    expect(screen.getByText(displayFingerprint(NEW_FP))).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Trust this key" })).toBeDisabled();
    expect(await pinnedFingerprint()).toBe(fingerprint(OLD_KEY));
    expect(h.reconnect).not.toHaveBeenCalled();
    // Ticking the comparison alone changes nothing.
    await userEvent.click(screen.getByRole("checkbox"));
    expect(await pinnedFingerprint()).toBe(fingerprint(OLD_KEY));
    expect(h.reconnect).not.toHaveBeenCalled();
  });

  it("re-pins on a full fingerprint match only after the click, reconnects and offers Open desktop", async () => {
    install(NEW_KEY);
    mountApp(`/desktops/d1/verify#fp=${NEW_FP}`);
    await screen.findByRole("heading", { level: 1, name: "Verify desktop" });
    await screen.findByText("Check the fingerprint");
    expect(await pinnedFingerprint()).toBe(fingerprint(OLD_KEY));
    await trust();
    expect(await screen.findByText("Desktop verified")).toBeInTheDocument();
    expect(screen.getByText(displayFingerprint(NEW_FP))).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Open desktop" })).toHaveAttribute("href", "/d/d1");
    expect(await pinnedFingerprint()).toBe(NEW_FP);
    expect(h.reconnect).toHaveBeenCalledTimes(1);
  });

  it("accepts the fingerprint in upper case", async () => {
    install(NEW_KEY);
    mountApp(`/desktops/d1/verify#fp=${NEW_FP.toUpperCase()}`);
    await trust();
    expect(await screen.findByText("Desktop verified")).toBeInTheDocument();
    expect(await pinnedFingerprint()).toBe(NEW_FP);
  });

  it("blocks on a mismatch, keeps the old pin and does not reconnect", async () => {
    install(OTHER_KEY);
    mountApp(`/desktops/d1/verify#fp=${NEW_FP}`);
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("This link does not match the desktop's key. Do not continue.");
    expect(screen.queryByRole("link", { name: "Open desktop" })).toBeNull();
    expect(screen.queryByRole("button", { name: /trust/i })).toBeNull();
    expect(screen.queryByRole("checkbox")).toBeNull();
    expect(await pinnedFingerprint()).toBe(fingerprint(OLD_KEY));
    expect(h.reconnect).not.toHaveBeenCalled();
  });

  it("cannot be accepted silently when the link fingerprint differs from the served key", async () => {
    install(OTHER_KEY);
    mountApp(`/desktops/d1/verify#fp=${NEW_FP}`);
    await screen.findByRole("alert");
    expect(screen.queryByRole("checkbox")).toBeNull();
    expect(screen.queryByRole("button", { name: "Trust this key" })).toBeNull();
    expect(await pinnedFingerprint()).toBe(fingerprint(OLD_KEY));
  });

  it("refuses at the click when the served key changed after the fingerprint was shown", async () => {
    install(NEW_KEY);
    mountApp(`/desktops/d1/verify#fp=${NEW_FP}`);
    await screen.findByText("Check the fingerprint");
    keys.d1 = base64UrlEncode(OTHER_KEY);
    await trust();
    expect(await screen.findByRole("alert")).toHaveTextContent("Do not continue");
    expect(await pinnedFingerprint()).toBe(fingerprint(OLD_KEY));
    expect(h.reconnect).not.toHaveBeenCalled();
  });

  it("explains an incomplete fragment and keeps the pin", async () => {
    install(NEW_KEY);
    mountApp("/desktops/d1/verify#fp=ab12");
    expect(await screen.findByText("This link is incomplete")).toBeInTheDocument();
    expect(screen.getByText(/Verify on another device/)).toBeInTheDocument();
    expect(await pinnedFingerprint()).toBe(fingerprint(OLD_KEY));
    expect(h.reconnect).not.toHaveBeenCalled();
  });

  it("explains a missing fragment without asking the server for the key", async () => {
    install(NEW_KEY);
    mountApp("/desktops/d1/verify");
    expect(await screen.findByText("This link is incomplete")).toBeInTheDocument();
    expect(screen.getByRole("heading", { level: 1, name: "Verify desktop" })).toBeInTheDocument();
    expect(server.calls.some((call) => call.path === "/api/desktops/d1")).toBe(false);
    expect(await pinnedFingerprint()).toBe(fingerprint(OLD_KEY));
  });

  it("removes the fragment from the address bar after reading it", async () => {
    install(NEW_KEY);
    const replace = vi.spyOn(window.history, "replaceState");
    mountApp(`/desktops/d1/verify#fp=${NEW_FP}`);
    await screen.findByText("Check the fingerprint");
    expect(replace).toHaveBeenCalled();
    for (const call of replace.mock.calls) expect(String(call[2])).not.toContain("#");
  });

  it("shows a retry when the key cannot be loaded, then verifies", async () => {
    install(NEW_KEY);
    const inner = server.transport;
    let failing = true;
    restore?.();
    restore = setApiTransport(async (url, init) => {
      if (failing && url === "/api/desktops/d1") return new Response(JSON.stringify({ error: "x", code: "internal" }), { status: 500 });
      return inner(url, init);
    });
    mountApp(`/desktops/d1/verify#fp=${NEW_FP}`);
    await screen.findByText(/could not be loaded/);
    failing = false;
    await userEvent.click(screen.getByRole("button", { name: "Try again" }));
    await trust();
    expect(await screen.findByText("Desktop verified")).toBeInTheDocument();
    expect(await pinnedFingerprint()).toBe(NEW_FP);
  });

  it("survives a sign-in: the guard parks the fragment, the sign-in target and the page restore it", async () => {
    restore = setApiTransport(createMockServer({ signedIn: false }).transport);
    const { router } = mountApp(`/desktops/d1/verify#fp=${NEW_FP}`);
    await waitFor(() => expect(router.state.location.pathname).toBe("/login"));
    expect(router.state.location.search).toMatchObject({ redirect: "/desktops/d1/verify" });
    expect(sessionStorage.getItem("monoptah.fp")).toContain(NEW_FP);
    expect(postSignInTarget("/desktops/d1/verify")).toBe(`/desktops/d1/verify#fp=${NEW_FP}`);
  });

  it("reads a fragment that was parked across the sign-in when the address carries none", async () => {
    install(NEW_KEY);
    sessionStorage.setItem("monoptah.fp", JSON.stringify({ path: "/desktops/d1/verify", fp: NEW_FP }));
    mountApp("/desktops/d1/verify");
    await trust();
    expect(await screen.findByText("Desktop verified")).toBeInTheDocument();
    expect(sessionStorage.getItem("monoptah.fp")).toBeNull();
  });

  it("clears a leftover parked value when the address already carries the fragment", async () => {
    install(NEW_KEY);
    sessionStorage.setItem("monoptah.fp", JSON.stringify({ path: "/desktops/d1/verify", fp: fingerprint(OTHER_KEY) }));
    mountApp(`/desktops/d1/verify#fp=${NEW_FP}`);
    await trust();
    expect(await screen.findByText("Desktop verified")).toBeInTheDocument();
    // The address wins, and the parked value of an older link is gone.
    expect(await pinnedFingerprint()).toBe(NEW_FP);
    expect(sessionStorage.getItem("monoptah.fp")).toBeNull();
  });

  it("reads the parked value in an effect: under strict mode (two effect runs) it still verifies once", async () => {
    install(NEW_KEY);
    sessionStorage.setItem("monoptah.fp", JSON.stringify({ path: "/desktops/d1/verify", fp: NEW_FP }));
    mountApp("/desktops/d1/verify", { strict: true });
    await screen.findByText("Check the fingerprint");
    expect(server.calls.filter((call) => call.path === "/api/desktops/d1")).toHaveLength(1);
    await trust();
    expect(await screen.findByText("Desktop verified")).toBeInTheDocument();
    // One more fetch: the click re-reads the key.
    expect(server.calls.filter((call) => call.path === "/api/desktops/d1")).toHaveLength(2);
    expect(sessionStorage.getItem("monoptah.fp")).toBeNull();
  });
});
