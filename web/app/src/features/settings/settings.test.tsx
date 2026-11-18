import { QueryClientProvider } from "@tanstack/react-query";
import { createMemoryHistory, RouterProvider } from "@tanstack/react-router";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const lib = vi.hoisted(() => ({
  startRegistration: vi.fn(),
  startAuthentication: vi.fn(),
  browserSupportsWebAuthn: vi.fn(() => true),
}));
vi.mock("@simplewebauthn/browser", () => lib);

import { setApiTransport, type ApiTransport } from "@/features/accounts/api";
import { ApiError } from "@/features/accounts/api";
import { settingsErrorMessage } from "./copy";
import { createMockServer } from "@/features/accounts/mock/transport";
import { createMockState, type SeedOptions } from "@/features/accounts/mock/state";
import { useAppearance } from "@/lib/appearance";
import { createQueryClient } from "@/lib/queryClient";
import { createAppRouter } from "@/router";
import { clearSlots, registerSlot } from "@/ui/slots";
import { describeUserAgent, formatRelative } from "./format";
import { inviteLink } from "./InvitesSection";
import { detectPlatform } from "./standalone";

const CREDENTIAL = { id: "cred", rawId: "cred", type: "public-key", response: {}, clientExtensionResults: {} };

let restore: (() => void) | undefined;
let calls: Array<{ method: string; url: string; body?: unknown }> = [];

function install(options: SeedOptions = {}) {
  const state = createMockState({ signedIn: true, ...options });
  const server = createMockServer({ state });
  const transport: ApiTransport = (url, init) => {
    calls.push({ method: init.method ?? "GET", url, body: init.body ? JSON.parse(String(init.body)) : undefined });
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

function section(title: string): HTMLElement {
  const heading = screen.getByRole("heading", { level: 2, name: title });
  return heading.closest("section") as HTMLElement;
}

beforeEach(() => {
  calls = [];
  window.localStorage.clear();
  window.sessionStorage.clear();
  lib.startRegistration.mockResolvedValue(CREDENTIAL);
  lib.startAuthentication.mockResolvedValue(CREDENTIAL);
  lib.browserSupportsWebAuthn.mockReturnValue(true);
});
afterEach(() => {
  restore?.();
  clearSlots();
  vi.clearAllMocks();
  vi.unstubAllGlobals();
  document.documentElement.removeAttribute("data-scheme");
  window.history.replaceState(null, "", "/");
});

describe("/settings", () => {
  it("renders one h1 and the account groups, and hosts the settings-sections slot", async () => {
    registerSlot("settings-sections", () => <p>Slot content</p>);
    install();
    mount("/settings");
    await screen.findByRole("heading", { level: 1, name: "Settings" });
    expect(screen.getAllByRole("heading", { level: 1 })).toHaveLength(1);
    for (const title of ["Account", "Passkeys", "Recovery codes", "Signed-in browsers", "Invites", "Appearance"]) {
      expect(screen.getByRole("heading", { level: 2, name: title })).toBeInTheDocument();
    }
    expect(screen.getByText("Slot content")).toBeInTheDocument();
    expect(screen.getByText("@dev-user")).toBeInTheDocument();
  });

  it("keeps the forced regenerate screen while the codes are unconfirmed", async () => {
    install({ recoveryAckAt: null });
    mount("/settings");
    await screen.findByRole("heading", { level: 1, name: "Get new recovery codes" });
    expect(screen.queryByRole("heading", { level: 2, name: "Passkeys" })).toBeNull();
  });
});

describe("passkeys", () => {
  it("disables remove for the last passkey and explains why", async () => {
    install({ passkeyCount: 1 });
    mount("/settings");
    const passkeys = await waitFor(() => {
      const el = section("Passkeys");
      expect(within(el).getAllByRole("button", { name: /^Remove passkey/ })).toHaveLength(1);
      return el;
    });
    expect(within(passkeys).getByRole("button", { name: /^Remove passkey/ })).toBeDisabled();
    expect(within(passkeys).getByText(/only one cannot be removed/)).toBeInTheDocument();
  });

  it("enables remove with two passkeys and removes one after a fresh retry", async () => {
    install({ passkeyCount: 2 });
    const user = userEvent.setup();
    mount("/settings");
    const passkeys = await waitFor(() => {
      const el = section("Passkeys");
      expect(within(el).getAllByRole("button", { name: /^Remove passkey/ })).toHaveLength(2);
      return el;
    });
    const [first] = within(passkeys).getAllByRole("button", { name: /^Remove passkey/ });
    expect(first).toBeEnabled();
    await user.click(first!);
    await user.click(await screen.findByRole("button", { name: "Remove passkey" }));
    await waitFor(() => expect(within(section("Passkeys")).getAllByRole("button", { name: /^Remove passkey/ })).toHaveLength(1));
    expect(calls.filter((c) => c.method === "DELETE" && c.url.startsWith("/api/passkeys/"))).toHaveLength(2); // fresh_required, then the retry
    expect(calls.some((c) => c.url === "/api/auth/reauth/verify")).toBe(true);
  });

  it("renames after a fresh re-auth retry", async () => {
    install({ passkeyCount: 2 });
    const user = userEvent.setup();
    mount("/settings");
    const rename = await screen.findAllByRole("button", { name: /^Rename passkey/ });
    await user.click(rename[0]!);
    const field = await screen.findByLabelText("Passkey name");
    await user.clear(field);
    await user.type(field, "Work laptop");
    await user.click(screen.getByRole("button", { name: "Save" }));
    await screen.findByText("Work laptop");
    const patches = calls.filter((c) => c.method === "PATCH" && c.url.startsWith("/api/passkeys/"));
    expect(patches).toHaveLength(2);
    const order = calls.map((c) => `${c.method} ${c.url}`);
    expect(order.indexOf("POST /api/auth/reauth/options")).toBeGreaterThan(order.indexOf(`PATCH ${patches[0]!.url}`));
    expect(lib.startAuthentication).toHaveBeenCalledTimes(1);
  });

  it("shows a soft notice and changes nothing when the re-auth prompt is closed", async () => {
    install({ passkeyCount: 2 });
    lib.startAuthentication.mockRejectedValue(Object.assign(new Error("closed"), { name: "NotAllowedError" }));
    const user = userEvent.setup();
    mount("/settings");
    const rename = await screen.findAllByRole("button", { name: /^Rename passkey/ });
    const original = rename[0]!.getAttribute("aria-label")!.replace("Rename passkey ", "");
    await user.click(rename[0]!);
    const field = await screen.findByLabelText("Passkey name");
    await user.clear(field);
    await user.type(field, "Nope");
    await user.click(screen.getByRole("button", { name: "Save" }));
    const dialog = await screen.findByRole("dialog");
    expect(await within(dialog).findByText(/prompt was closed/)).toBeInTheDocument();
    expect(within(dialog).queryByRole("alert")).toBeNull();
    expect(calls.filter((c) => c.method === "PATCH")).toHaveLength(1);
    expect(original.length).toBeGreaterThan(0);
  });
});

describe("recovery codes", () => {
  it("regenerates after confirmation, shows the codes once and keeps them out of caches and storage", async () => {
    install();
    const user = userEvent.setup();
    const { queryClient } = mount("/settings");
    await user.click(await screen.findByRole("button", { name: "Generate new codes" }));
    await user.click(within(await screen.findByRole("dialog")).getByRole("button", { name: "Generate new codes" }));
    const list = await screen.findByRole("list", { name: "Recovery codes" });
    const codes = within(list).getAllByRole("listitem").map((li) => li.textContent?.replace(/^\d+/, "") ?? "");
    expect(codes).toHaveLength(10);
    const cached = JSON.stringify([queryClient.getQueryCache().getAll().map((q) => q.state.data), queryClient.getMutationCache().getAll().map((m) => m.state.data)]);
    const stored = JSON.stringify([...Object.entries(localStorage), ...Object.entries(sessionStorage)]);
    for (const code of codes) {
      expect(cached).not.toContain(code);
      expect(stored).not.toContain(code);
    }
    expect(screen.getByRole("button", { name: "Done" })).toBeDisabled();
    await user.click(screen.getByLabelText("I saved these codes"));
    await user.click(screen.getByRole("button", { name: "Done" }));
    expect(await screen.findByRole("button", { name: "Generate new codes" })).toBeInTheDocument();
    expect(calls.filter((c) => c.url === "/api/recovery-codes/ack").map((c) => c.body)).toEqual([{ batch: 2 }]);
    expect(screen.queryByRole("list", { name: "Recovery codes" })).toBeNull();
  });
});

describe("recovery codes and a me refetch", () => {
  it("keeps the new codes on screen when me is refetched with a cleared ack, until the user confirms", async () => {
    install();
    const user = userEvent.setup();
    const { queryClient } = mount("/settings");
    await user.click(await screen.findByRole("button", { name: "Generate new codes" }));
    await user.click(within(await screen.findByRole("dialog")).getByRole("button", { name: "Generate new codes" }));
    await screen.findByRole("list", { name: "Recovery codes" });

    // The Worker cleared the ack: a reconnect or a stale refetch of `me` now says recoveryAckAt null.
    await queryClient.refetchQueries({ queryKey: ["me"] });
    expect(queryClient.getQueryData<{ recoveryAckAt: number | null }>(["me"])?.recoveryAckAt).toBeNull();
    expect(screen.getByRole("list", { name: "Recovery codes" })).toBeInTheDocument();
    expect(screen.queryByRole("heading", { level: 1, name: "Get new recovery codes" })).toBeNull();

    await user.click(screen.getByLabelText("I saved these codes"));
    await user.click(screen.getByRole("button", { name: "Done" }));
    expect(await screen.findByRole("button", { name: "Generate new codes" })).toBeInTheDocument();
    expect(queryClient.getQueryData<{ recoveryAckAt: number | null }>(["me"])?.recoveryAckAt).not.toBeNull();
  });
});

describe("recovery codes replaced in another tab", () => {
  it("an ack 409 shows its own message and drops the dead codes", async () => {
    const state = install();
    const user = userEvent.setup();
    mount("/settings");
    await user.click(await screen.findByRole("button", { name: "Generate new codes" }));
    await user.click(within(await screen.findByRole("dialog")).getByRole("button", { name: "Generate new codes" }));
    await screen.findByRole("list", { name: "Recovery codes" });
    state.recoveryBatch += 1;
    await user.click(screen.getByLabelText("I saved these codes"));
    await user.click(screen.getByRole("button", { name: "Done" }));
    expect(await screen.findByText("These codes were replaced in another tab or window. Generate a new set.")).toBeInTheDocument();
    expect(screen.queryByRole("list", { name: "Recovery codes" })).toBeNull();
    expect(screen.getByRole("button", { name: "Generate new codes" })).toBeInTheDocument();
  });
});

describe("signed-in browsers", () => {
  it("marks the current browser and gives it no sign-out button", async () => {
    install();
    mount("/settings");
    const sessions = await waitFor(() => {
      const el = section("Signed-in browsers");
      expect(within(el).getAllByRole("listitem").length).toBeGreaterThan(1);
      return el;
    });
    expect(within(sessions).getAllByText("This browser")).toHaveLength(1);
    expect(within(sessions).getAllByRole("button", { name: /^Sign out [^e]/ })).toHaveLength(2);
  });

  it("signs out one browser through DELETE /api/sessions/:id", async () => {
    install();
    const user = userEvent.setup();
    mount("/settings");
    const buttons = await screen.findAllByRole("button", { name: /Fake Phone Browser|Unknown browser/ });
    await user.click(buttons[0]!);
    await waitFor(() => expect(calls.filter((c) => c.method === "DELETE" && c.url.startsWith("/api/sessions/"))).toHaveLength(1));
    expect(calls.some((c) => c.url === "/api/sessions/sess-current")).toBe(false);
  });

  it("signs out everywhere else by revoking every other session and never the current one", async () => {
    install();
    const user = userEvent.setup();
    mount("/settings");
    await screen.findAllByText("This browser");
    await user.click(screen.getByRole("button", { name: "Sign out everywhere else" }));
    await user.click(within(await screen.findByRole("dialog")).getByRole("button", { name: "Sign out everywhere else" }));
    await waitFor(() => expect(screen.getByText("This is the only browser signed in.")).toBeInTheDocument());
    const revoked = calls.filter((c) => c.method === "DELETE" && c.url.startsWith("/api/sessions/")).map((c) => c.url);
    expect(revoked.sort()).toEqual(["/api/sessions/sess-other-1", "/api/sessions/sess-other-2"]);
    expect(screen.getByRole("button", { name: "Sign out everywhere else" })).toBeDisabled();
  });

  it("signs out through POST /api/auth/logout and goes to /login", async () => {
    install();
    const user = userEvent.setup();
    const { router } = mount("/settings");
    await user.click(await screen.findByRole("button", { name: "Sign out" }));
    await waitFor(() => expect(router.state.location.pathname).toBe("/login"));
    expect(calls.some((c) => c.method === "POST" && c.url === "/api/auth/logout")).toBe(true);
  });
});

describe("invites", () => {
  it("creates an invite, shows the link once, copies it and never stores the code", async () => {
    install();
    const user = userEvent.setup();
    const write = vi.spyOn(navigator.clipboard, "writeText");
    const { queryClient } = mount("/settings");
    await screen.findByText("3 of 3 available");
    await user.click(screen.getByRole("button", { name: "Create invite" }));
    const box = await screen.findByTestId("new-invite");
    const link = within(box).getByText(/\/register#invite=/).textContent ?? "";
    expect(link.startsWith(`${window.location.origin}/register#invite=`)).toBe(true);
    const code = link.split("#invite=")[1]!;
    expect(await screen.findByText("2 of 3 available")).toBeInTheDocument();

    await user.click(within(box).getByRole("button", { name: "Copy link" }));
    expect(write).toHaveBeenCalledWith(link);

    const stored = JSON.stringify([...Object.entries(localStorage), ...Object.entries(sessionStorage)]);
    const cached = JSON.stringify([queryClient.getQueryCache().getAll().map((q) => q.state.data), queryClient.getMutationCache().getAll().map((m) => m.state.data)]);
    expect(stored).not.toContain(code);
    expect(cached).not.toContain(code);

    await user.click(within(box).getByRole("button", { name: "Done" }));
    expect(screen.queryByTestId("new-invite")).toBeNull();
    expect(screen.getByText("Unused invite")).toBeInTheDocument();
  });

  it("revokes an unused invite and restores the quota", async () => {
    install();
    const user = userEvent.setup();
    mount("/settings");
    await screen.findByText("3 of 3 available");
    await user.click(screen.getByRole("button", { name: "Create invite" }));
    await screen.findByText("2 of 3 available");
    await user.click(await screen.findByRole("button", { name: /^Revoke the invite/ }));
    await screen.findByText("Revoked invite");
    expect(calls.some((c) => c.method === "DELETE" && c.url.startsWith("/api/invites/"))).toBe(true);
    expect(screen.queryByRole("button", { name: /^Revoke the invite/ })).toBeNull();
  });

  it("explains a revoke conflict as used or revoked, and keeps other conflicts generic", async () => {
    const state = install();
    const user = userEvent.setup();
    mount("/settings");
    await screen.findByText("3 of 3 available");
    await user.click(screen.getByRole("button", { name: "Create invite" }));
    await screen.findByText("2 of 3 available");
    // Someone used the invite in the meantime.
    for (const invite of state.invites) invite.usedAt = 1;
    await user.click(await screen.findByRole("button", { name: /^Revoke the invite/ }));
    expect(await screen.findByText("This invite was already used or revoked.")).toBeInTheDocument();
    expect(settingsErrorMessage(new ApiError({ status: 409, code: "conflict", message: "x" }))).toBe("Something went wrong. Try again.");
  });

  it("disables create when no invites are left", async () => {
    install();
    const user = userEvent.setup();
    mount("/settings");
    await screen.findByText("3 of 3 available");
    for (let i = 0; i < 3; i += 1) {
      await user.click(screen.getByRole("button", { name: "Create invite" }));
      await screen.findByText(`${2 - i} of 3 available`);
    }
    expect(screen.getByRole("button", { name: "Create invite" })).toBeDisabled();
  });

  it("shows the paused state when the server answers invites_paused", async () => {
    install({ invitesPaused: true });
    const user = userEvent.setup();
    mount("/settings");
    await screen.findByText("3 of 3 available");
    await user.click(screen.getByRole("button", { name: "Create invite" }));
    expect(await screen.findByText("Invites are paused")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Create invite" })).toBeDisabled();
    expect(screen.queryByTestId("new-invite")).toBeNull();
  });

  it("builds the link from the origin with the code in the fragment", () => {
    expect(inviteLink("ABC-123", "https://zptah.app")).toBe("https://zptah.app/register#invite=ABC-123");
  });
});

describe("appearance", () => {
  it("switches the scheme and persists it under the monoptah. key", async () => {
    install();
    const user = userEvent.setup();
    mount("/settings");
    const group = await screen.findByRole("radiogroup", { name: "Colour scheme" });
    expect(within(group).getByRole("radio", { name: "System" })).toBeChecked();
    await user.click(within(group).getByRole("radio", { name: "Dark" }));
    expect(window.localStorage.getItem("monoptah.appearance")).toBe("dark");
    expect(document.documentElement.getAttribute("data-scheme")).toBe("dark");
    expect(within(group).getByRole("radio", { name: "Dark" })).toBeChecked();
    await user.click(within(group).getByRole("radio", { name: "Light" }));
    expect(window.localStorage.getItem("monoptah.appearance")).toBe("light");
    expect(document.documentElement.getAttribute("data-scheme")).toBe("light");
    // Arrow keys move the choice.
    await user.keyboard("{ArrowRight}");
    expect(window.localStorage.getItem("monoptah.appearance")).toBe("dark");
    expect(useAppearance.getState().appearance).toBe("dark");
    expect(Object.keys(localStorage).every((key) => key.startsWith("monoptah."))).toBe(true);
    useAppearance.getState().setAppearance("system");
  });
});

describe("/install", () => {
  function stubMedia(standalone: boolean) {
    vi.stubGlobal("matchMedia", (query: string) => ({
      matches: standalone && query.includes("display-mode: standalone"),
      media: query,
      addEventListener: () => undefined,
      removeEventListener: () => undefined,
    }));
  }

  it("is public and shows the three platform guides in the browser", async () => {
    install({ signedIn: false });
    stubMedia(false);
    mount("/install");
    await screen.findByRole("heading", { level: 1, name: "Install Monoptah" });
    expect(screen.getAllByRole("heading", { level: 1 })).toHaveLength(1);
    expect(screen.getByText(/Add to Home Screen/)).toBeInTheDocument();
    expect(screen.getByText(/Install app/)).toBeInTheDocument();
    expect(screen.getByText(/install icon/)).toBeInTheDocument();
    expect(screen.queryByText("Installed")).toBeNull();
    expect(calls.some((c) => c.url === "/api/me")).toBe(false);
  });

  it("shows the Installed state in standalone display mode", async () => {
    install({ signedIn: false });
    stubMedia(true);
    mount("/install");
    expect(await screen.findByText("Installed")).toBeInTheDocument();
    expect(screen.queryByText(/Scroll down and choose/)).toBeNull();
  });

  it("detects iOS navigator.standalone", async () => {
    install({ signedIn: false });
    stubMedia(false);
    Object.defineProperty(navigator, "standalone", { value: true, configurable: true });
    try {
      mount("/install");
      expect(await screen.findByText("Installed")).toBeInTheDocument();
    } finally {
      Reflect.deleteProperty(navigator, "standalone");
    }
  });

  it("offers no notification button here (P-05)", async () => {
    install({ signedIn: false });
    stubMedia(false);
    mount("/install");
    await screen.findByRole("heading", { level: 1, name: "Install Monoptah" });
    expect(screen.queryByRole("button")).toBeNull();
  });

  it("detects the platform from the user agent", () => {
    expect(detectPlatform("Mozilla/5.0 (iPhone; CPU iPhone OS 17_0 like Mac OS X)", 5)).toBe("ios");
    expect(detectPlatform("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)", 5)).toBe("ios");
    expect(detectPlatform("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)", 0)).toBe("desktop");
    expect(detectPlatform("Mozilla/5.0 (Linux; Android 14)", 5)).toBe("android");
    expect(detectPlatform("Mozilla/5.0 (X11; Linux x86_64)", 0)).toBe("desktop");
  });
});

describe("format helpers", () => {
  it("describes user agents and relative times", () => {
    expect(describeUserAgent("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 Version/17.0 Safari/605.1.15")).toBe("Safari on macOS");
    expect(describeUserAgent("Mozilla/5.0 (Windows NT 10.0) Chrome/120.0 Safari/537.36 Edg/120.0")).toBe("Edge on Windows");
    expect(describeUserAgent(null)).toBe("Unknown browser");
    expect(formatRelative(1000, 1030)).toBe("just now");
    expect(formatRelative(1000, 1000 + 7200)).toBe("2 hours ago");
    expect(formatRelative(1000, 1000 + 86400)).toBe("1 day ago");
  });
});
