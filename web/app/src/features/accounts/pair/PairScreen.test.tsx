import { QueryClientProvider } from "@tanstack/react-query";
import { createMemoryHistory, RouterProvider } from "@tanstack/react-router";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const lib = vi.hoisted(() => ({
  startRegistration: vi.fn(),
  startAuthentication: vi.fn(),
  browserSupportsWebAuthn: vi.fn(() => true),
}));
vi.mock("@simplewebauthn/browser", () => lib);

import { createQueryClient } from "@/lib/queryClient";
import { createAppRouter } from "@/router";
import { setApiTransport, type ApiTransport } from "../api";
import { createMockServer } from "../mock/transport";
import {
  createMockState,
  MOCK_PAIR_CODE,
  MOCK_PAIR_CONFIRMATION,
  MOCK_PAIR_EXPIRED_CODE,
  MOCK_PAIR_FINGERPRINT,
  MOCK_PAIR_LONG_NAME_CODE,
  normalizeUserCode,
  type SeedOptions,
} from "../mock/state";
import { PAIR_EXPIRED_MESSAGE, PAIR_QUOTA_MESSAGE } from "./PairScreen";

const CREDENTIAL = { id: "cred", rawId: "cred", type: "public-key", response: {}, clientExtensionResults: {} };
const MATCH = `#fp=${MOCK_PAIR_FINGERPRINT}`;
const OTHER = `#fp=${"ab".repeat(32)}`;

let restore: (() => void) | undefined;
let sent: Array<{ method: string; url: string; body: unknown }> = [];

function install(options: SeedOptions = {}) {
  const state = createMockState({ signedIn: true, ...options });
  const server = createMockServer({ state });
  const transport: ApiTransport = (url, init) => {
    sent.push({ method: init.method ?? "GET", url, body: init.body ? JSON.parse(String(init.body)) : undefined });
    return server.transport(url, init);
  };
  restore = setApiTransport(transport);
  return state;
}

function mount(url: string) {
  const queryClient = createQueryClient();
  const router = createAppRouter({ history: createMemoryHistory({ initialEntries: [url] }), queryClient });
  render(
    <QueryClientProvider client={queryClient}>
      <RouterProvider router={router} />
    </QueryClientProvider>,
  );
  return { queryClient, router };
}

const approveCalls = () => sent.filter((call) => call.method === "POST" && call.url.endsWith("/approve"));
const urlsOf = (calls: typeof sent) => calls.map((call) => `${call.method} ${call.url}`);

beforeEach(() => {
  sent = [];
  window.localStorage.clear();
  window.sessionStorage.clear();
  lib.startAuthentication.mockResolvedValue(CREDENTIAL);
  lib.browserSupportsWebAuthn.mockReturnValue(true);
});
afterEach(() => {
  restore?.();
  vi.clearAllMocks();
});

describe("pairing page: details and fingerprint check", () => {
  it("shows the machine, system, confirmation code and the fingerprint in 8 groups of 4", async () => {
    install();
    mount(`/pair/${MOCK_PAIR_CODE}${MATCH}`);
    await screen.findByRole("heading", { level: 1, name: "Approve this desktop?" });
    expect(screen.getByText("Ada's MacBook Pro")).toBeInTheDocument();
    expect(screen.getByText("macOS 15")).toBeInTheDocument();
    expect(screen.getByText(MOCK_PAIR_CONFIRMATION)).toBeInTheDocument();
    expect(screen.getAllByText("0123 4567 89ab cdef")).toHaveLength(2);
    expect(screen.getByText("The fingerprint matches your link")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /Approve/ })).toBeEnabled();
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("blocks a mismatch: a warning, no Approve control at all, only Deny and Cancel", async () => {
    const state = install();
    const user = userEvent.setup();
    mount(`/pair/${MOCK_PAIR_CODE}${OTHER}`);
    await screen.findByRole("heading", { level: 1, name: "Do not approve this desktop" });
    expect(screen.getByRole("alert")).toHaveTextContent("The fingerprint does not match");
    expect(screen.queryByRole("button", { name: /Approve/ })).toBeNull();
    expect(screen.queryByRole("link", { name: /Approve/ })).toBeNull();
    expect(screen.queryByRole("checkbox")).toBeNull();
    expect(screen.queryByRole("radio")).toBeNull();
    expect(screen.getByRole("link", { name: "Cancel" })).toHaveAttribute("href", "/desktops");
    await user.click(screen.getByRole("button", { name: "Deny" }));
    await screen.findByRole("heading", { level: 1, name: "Request denied" });
    expect(approveCalls()).toHaveLength(0);
    expect(state.pairings.get(normalizeUserCode(MOCK_PAIR_CODE))?.status).toBe("denied");
  });

  it("treats a truncated fragment as a mismatch", async () => {
    install();
    mount(`/pair/${MOCK_PAIR_CODE}#fp=${MOCK_PAIR_FINGERPRINT.slice(0, 32)}`);
    await screen.findByRole("heading", { level: 1, name: "Do not approve this desktop" });
    expect(screen.queryByRole("button", { name: /Approve/ })).toBeNull();
  });

  it("warns when the link has no #fp and needs the comparison confirmed before Approve", async () => {
    const state = install();
    const user = userEvent.setup();
    mount(`/pair/${MOCK_PAIR_CODE}`);
    await screen.findByRole("heading", { level: 1, name: "Approve this desktop?" });
    expect(screen.getByRole("alert")).toHaveTextContent("This link has no fingerprint");
    expect(screen.getByRole("alert")).toHaveTextContent("exactly what your desktop shows");
    const approve = screen.getByRole("button", { name: /Approve/ });
    expect(approve).toBeDisabled();
    await user.click(screen.getByLabelText("The code and fingerprint match my desktop screen"));
    expect(approve).toBeEnabled();
    await user.click(approve);
    await screen.findByRole("heading", { level: 1, name: "Desktop approved" });
    expect(state.pairings.get(normalizeUserCode(MOCK_PAIR_CODE))?.status).toBe("approved");
  });

  it("renders a hostile machine name as plain text, without hidden characters, truncated", async () => {
    install();
    mount(`/pair/${MOCK_PAIR_LONG_NAME_CODE}${MATCH}`);
    await screen.findByRole("heading", { level: 1, name: "Approve this desktop?" });
    const name = screen.getByText(/Fake/, { selector: "dd" });
    expect(name.querySelector("b")).toBeNull();
    expect(name.textContent).toContain("<b>Fake</b>");
    expect(name.textContent).not.toContain("\u202E");
    expect([...(name.textContent ?? "")].length).toBeLessThanOrEqual(64);
  });
});

describe("pairing page: approve and deny", () => {
  it("runs fresh re-auth, then approves a new desktop with an empty body", async () => {
    const state = install();
    const user = userEvent.setup();
    mount(`/pair/${MOCK_PAIR_CODE}${MATCH}`);
    await screen.findByRole("heading", { level: 1, name: "Approve this desktop?" });
    expect(screen.getByRole("radio", { name: "A new desktop" })).toBeChecked();
    await user.click(screen.getByRole("button", { name: /Approve/ }));
    await screen.findByRole("heading", { level: 1, name: "Desktop approved" });
    expect(screen.getByText(/Go back to Monoptah on your desktop to finish/)).toBeInTheDocument();

    // The first approve is refused as not fresh, the passkey ceremony runs, then the approve is retried.
    const calls = urlsOf(sent.filter((call) => call.url.includes("/pair/") || call.url.includes("/reauth/")));
    expect(calls).toEqual([
      `GET /api/pair/${MOCK_PAIR_CODE}`,
      `POST /api/pair/${MOCK_PAIR_CODE}/approve`,
      "POST /api/auth/reauth/options",
      "POST /api/auth/reauth/verify",
      `POST /api/pair/${MOCK_PAIR_CODE}/approve`,
    ]);
    expect(lib.startAuthentication).toHaveBeenCalledTimes(1);
    expect(approveCalls().map((call) => call.body)).toEqual([{}, {}]);
    expect(state.desktops).toHaveLength(3);
    // The desktops list is refreshed so the new desktop shows up.
    await waitFor(() => expect(sent.filter((call) => call.url === "/api/desktops")).toHaveLength(2));
  });

  it("sends replaces for the chosen desktop and keeps the desktop count", async () => {
    const state = install();
    const user = userEvent.setup();
    mount(`/pair/${MOCK_PAIR_CODE}${MATCH}`);
    await screen.findByRole("heading", { level: 1, name: "Approve this desktop?" });
    await user.click(screen.getByRole("radio", { name: "This replaces Fake Linux box" }));
    expect(screen.getByText(/old key stops working/)).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: /Approve/ }));
    await screen.findByRole("heading", { level: 1, name: "Desktop approved" });
    expect(screen.getByText(/Fake Linux box now uses the new desktop key/)).toBeInTheDocument();
    expect(approveCalls().map((call) => call.body)).toEqual([{ replaces: "desktop-dev-2" }, { replaces: "desktop-dev-2" }]);
    expect(state.desktops).toHaveLength(2);
  });

  it("offers no replace choice for an account without desktops", async () => {
    install({ desktopCount: 0 });
    mount(`/pair/${MOCK_PAIR_CODE}${MATCH}`);
    await screen.findByRole("heading", { level: 1, name: "Approve this desktop?" });
    expect(screen.queryByRole("radio")).toBeNull();
    expect(screen.getByRole("button", { name: /Approve/ })).toBeInTheDocument();
  });

  it("stays on the page with a soft notice when the passkey prompt is closed", async () => {
    const state = install();
    lib.startAuthentication.mockRejectedValue(Object.assign(new Error("x"), { name: "NotAllowedError" }));
    const user = userEvent.setup();
    mount(`/pair/${MOCK_PAIR_CODE}${MATCH}`);
    await screen.findByRole("heading", { level: 1, name: "Approve this desktop?" });
    await user.click(screen.getByRole("button", { name: /Approve/ }));
    expect(await screen.findByText(/The passkey prompt was closed/)).toBeInTheDocument();
    expect(screen.queryByRole("alert")).toBeNull();
    expect(screen.getByRole("button", { name: /Approve/ })).toBeEnabled();
    expect(state.pairings.get(normalizeUserCode(MOCK_PAIR_CODE))?.status).toBe("pending");
  });

  it("explains the desktop limit and keeps the page usable", async () => {
    install({ desktopCount: 5 });
    const user = userEvent.setup();
    mount(`/pair/${MOCK_PAIR_CODE}${MATCH}`);
    await screen.findByRole("heading", { level: 1, name: "Approve this desktop?" });
    await user.click(screen.getByRole("button", { name: /Approve/ }));
    expect(await screen.findByRole("alert")).toHaveTextContent(PAIR_QUOTA_MESSAGE);
    expect(screen.getByRole("radio", { name: "This replaces Fake Raspberry Pi" })).toBeInTheDocument();
  });

  it("denies without a passkey prompt and shows the confirmation", async () => {
    const state = install();
    const user = userEvent.setup();
    mount(`/pair/${MOCK_PAIR_CODE}${MATCH}`);
    await screen.findByRole("heading", { level: 1, name: "Approve this desktop?" });
    await user.click(screen.getByRole("button", { name: "Deny" }));
    await screen.findByRole("heading", { level: 1, name: "Request denied" });
    expect(urlsOf(sent)).toContain(`POST /api/pair/${MOCK_PAIR_CODE}/deny`);
    expect(lib.startAuthentication).not.toHaveBeenCalled();
    expect(state.pairings.get(normalizeUserCode(MOCK_PAIR_CODE))?.status).toBe("denied");
    expect(state.desktops).toHaveLength(2);
  });

  it("shows the expired copy when the request was approved elsewhere meanwhile", async () => {
    const state = install();
    const user = userEvent.setup();
    mount(`/pair/${MOCK_PAIR_CODE}${MATCH}`);
    await screen.findByRole("heading", { level: 1, name: "Approve this desktop?" });
    const stored = state.pairings.get(normalizeUserCode(MOCK_PAIR_CODE));
    if (stored) stored.status = "approved";
    await user.click(screen.getByRole("button", { name: /Approve/ }));
    await screen.findByRole("heading", { level: 1, name: "Code expired" });
  });
});

describe("pairing page: codes that cannot be used", () => {
  it.each([
    ["an expired code", MOCK_PAIR_EXPIRED_CODE],
    ["an unknown code", "QQQQ-2222"],
    ["a malformed code", "c0de"],
  ])("shows the F-03 message for %s", async (_label, code) => {
    install();
    mount(`/pair/${code}${MATCH}`);
    await screen.findByRole("heading", { level: 1, name: "Code expired" });
    expect(screen.getByText(PAIR_EXPIRED_MESSAGE)).toBeInTheDocument();
    expect(PAIR_EXPIRED_MESSAGE).toBe("This code expired. Choose Pair on your desktop to make a new one.");
    expect(screen.queryByRole("button", { name: /Approve/ })).toBeNull();
  });

  it("shows a used code as expired", async () => {
    const state = install();
    const stored = state.pairings.get(normalizeUserCode(MOCK_PAIR_CODE));
    if (stored) stored.status = "approved";
    mount(`/pair/${MOCK_PAIR_CODE}${MATCH}`);
    await screen.findByRole("heading", { level: 1, name: "Code expired" });
  });

  it("offers a retry on a network failure instead of the expired copy", async () => {
    install();
    restore?.();
    restore = setApiTransport((url) => (url.startsWith("/api/pair/") ? Promise.reject(new TypeError("offline")) : createMockServer({ signedIn: true }).transport(url, {})));
    mount(`/pair/${MOCK_PAIR_CODE}${MATCH}`);
    expect(await screen.findByRole("alert")).toHaveTextContent("Cannot reach Monoptah");
    expect(screen.getByRole("button", { name: "Try again" })).toBeInTheDocument();
    expect(screen.queryByText(PAIR_EXPIRED_MESSAGE)).toBeNull();
  });
});

describe("pairing page: signed-out round trip", () => {
  it("keeps the code and #fp through /login and lands on the matching approval page", async () => {
    install({ signedIn: false });
    const user = userEvent.setup();
    const { router } = mount(`/pair/${MOCK_PAIR_CODE}${MATCH}`);
    await screen.findByRole("heading", { level: 1, name: "Sign in to Monoptah" });
    expect(router.state.location.pathname).toBe("/login");
    expect(router.state.location.search).toMatchObject({ redirect: `/pair/${MOCK_PAIR_CODE}` });
    // The fragment is not in the redirect query; it is parked in sessionStorage.
    expect(router.state.location.searchStr).not.toContain(MOCK_PAIR_FINGERPRINT);
    expect(window.sessionStorage.getItem("monoptah.fp")).toContain(MOCK_PAIR_FINGERPRINT);

    lib.startAuthentication.mockResolvedValue(CREDENTIAL);
    await user.click(screen.getByRole("button", { name: /Sign in with passkey/ }));
    await screen.findByRole("heading", { level: 1, name: "Approve this desktop?" });
    expect(router.state.location.pathname).toBe(`/pair/${MOCK_PAIR_CODE}`);
    // The restored fragment is compared: a match, not the missing-fingerprint warning.
    await screen.findByText("The fingerprint matches your link");
    expect(screen.queryByText("This link has no fingerprint")).toBeNull();
    await waitFor(() => expect(window.sessionStorage.getItem("monoptah.fp")).toBeNull());
  });

  it("keeps a mismatching #fp through sign-in and still blocks", async () => {
    install({ signedIn: false });
    const user = userEvent.setup();
    mount(`/pair/${MOCK_PAIR_CODE}${OTHER}`);
    await screen.findByRole("heading", { level: 1, name: "Sign in to Monoptah" });
    await user.click(screen.getByRole("button", { name: /Sign in with passkey/ }));
    await screen.findByRole("heading", { level: 1, name: "Do not approve this desktop" });
    expect(screen.queryByRole("button", { name: /Approve/ })).toBeNull();
  });
});
