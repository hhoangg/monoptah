// The URL map of the settings shell: /settings and its old hashes, the old desktop URLs, the routes that must not move,
// and the forced recovery-codes gate. jsdom has no matchMedia, which the redirect treats as a wide window.
import { act, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { setApiTransport } from "@/features/accounts/api";
import { createMockState, nowSeconds, type SeedOptions } from "@/features/accounts/mock/state";
import { createMockServer } from "@/features/accounts/mock/transport";
import { mountApp } from "@/features/desktops/testMount";

vi.mock("@/features/accounts/webauthn", () => ({ createPasskey: vi.fn(), getPasskey: vi.fn() }));

let restore: (() => void) | undefined;

function install(options: SeedOptions = {}) {
  const state = createMockState({ signedIn: true, ...options });
  restore = setApiTransport(createMockServer({ state }).transport);
  return state;
}

/** A matchMedia whose every query answers `wide` (the width query is the only one the redirect asks that matters). */
function stubWidth(wide: boolean) {
  const listeners = new Set<() => void>();
  const state = { wide };
  vi.stubGlobal("matchMedia", (query: string) => ({
    matches: state.wide && query.includes("min-width"),
    media: query,
    addEventListener: (_type: string, listener: () => void) => listeners.add(listener),
    removeEventListener: (_type: string, listener: () => void) => listeners.delete(listener),
    addListener: () => undefined,
    removeListener: () => undefined,
    dispatchEvent: () => false,
  }));
  return {
    resize(next: boolean) {
      state.wide = next;
      for (const listener of listeners) listener();
    },
  };
}

afterEach(() => {
  restore?.();
  vi.unstubAllGlobals();
});

async function settled(router: ReturnType<typeof mountApp>["router"]) {
  await waitFor(() => expect(router.state.status).toBe("idle"));
}

describe("/settings", () => {
  it("redirects to /settings/profile when there is no matchMedia", async () => {
    install();
    const { router } = mountApp("/settings");
    await screen.findByRole("heading", { level: 1, name: "Profile" });
    expect(router.state.location.pathname).toBe("/settings/profile");
  });

  it("redirects to /settings/profile in a wide window", async () => {
    install();
    stubWidth(true);
    const { router } = mountApp("/settings");
    await screen.findByRole("heading", { level: 1, name: "Profile" });
    expect(router.state.location.pathname).toBe("/settings/profile");
  });

  it("stays a list page in a narrow window: the Settings header, the nav and the quick-find box", async () => {
    install();
    stubWidth(false);
    const { router } = mountApp("/settings");
    await screen.findByRole("heading", { level: 1, name: "Settings" });
    expect(router.state.location.pathname).toBe("/settings");
    const nav = screen.getByRole("navigation", { name: "Settings" });
    for (const name of ["Profile", "Security", "Invites", "Appearance", "Notifications", "Install app", "All desktops"]) {
      expect(within(nav).getByRole("link", { name })).toBeInTheDocument();
    }
    expect(within(nav).getByRole("searchbox", { name: "Find a setting" })).toBeInTheDocument();
    // Reading order: the "Settings" h1 comes before the nav in the DOM (the wide layout moves the nav left with CSS only).
    const h1 = screen.getByRole("heading", { level: 1, name: "Settings" });
    expect(h1.compareDocumentPosition(nav) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(nav.closest("aside")?.className).toContain("lg:order-first");
    // Below lg the list sits in a card of rows with a chevron each; from lg the nav is the plain sidebar.
    const account = within(nav).getByRole("list", { name: /^Account/ });
    expect(account.className).toContain("max-lg:bg-surface");
    const profile = within(account).getByRole("link", { name: "Profile" });
    expect(profile.querySelector('[data-icon="account"]')).not.toBeNull();
    expect(profile.querySelector('[data-icon="chevronRight"]')?.getAttribute("class")).toContain("lg:hidden");
  });

  it("puts the nav before the page in the DOM on a sub page, so Tab follows the visual order from 1024px", async () => {
    install();
    stubWidth(true);
    mountApp("/settings/profile");
    const h1 = await screen.findByRole("heading", { level: 1, name: "Profile" });
    const nav = screen.getByRole("navigation", { name: "Settings" });
    expect(nav.compareDocumentPosition(h1) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  });

  it("goes to /settings/profile when a narrow window grows to 1024px while on the list", async () => {
    install();
    const media = stubWidth(false);
    const { router } = mountApp("/settings");
    await screen.findByRole("heading", { level: 1, name: "Settings" });
    act(() => media.resize(true));
    await waitFor(() => expect(router.state.location.pathname).toBe("/settings/profile"));
    await screen.findByRole("heading", { level: 1, name: "Profile" });
  });

  const HASHES = [
    ["settings-account", "/settings/profile", "Profile"],
    ["settings-alerts", "/settings/notifications", "Notifications"],
    ["settings-look", "/settings/appearance", "Appearance"],
    ["settings-app", "/settings/install", "Install app"],
  ] as const;
  it.each(HASHES.flatMap(([hash, path, title]) => [[hash, path, title, "wide"] as const, [hash, path, title, "narrow"] as const]))(
    "maps the old #%s link to %s (%s window)",
    async (hash, path, title, width) => {
      install();
      if (width === "narrow") stubWidth(false);
      const { router } = mountApp(`/settings#${hash}`);
      await screen.findByRole("heading", { level: 1, name: title });
      expect(router.state.location.pathname).toBe(path);
    },
  );
});

describe("old desktop URLs", () => {
  it("/desktops redirects to /settings/desktops", async () => {
    install();
    const { router } = mountApp("/desktops");
    await screen.findByRole("heading", { level: 1, name: "All desktops" });
    expect(router.state.location.pathname).toBe("/settings/desktops");
  });

  it("/desktops/dk1 redirects to /settings/desktops/dk1", async () => {
    install();
    const { router } = mountApp("/desktops/dk1");
    await screen.findByRole("heading", { level: 1 });
    expect(router.state.location.pathname).toBe("/settings/desktops/dk1");
  });

  it("/d/dk1/settings redirects to /settings/desktops/dk1", async () => {
    install();
    const { router } = mountApp("/d/dk1/settings");
    await screen.findByRole("heading", { level: 1 });
    expect(router.state.location.pathname).toBe("/settings/desktops/dk1");
  });

  it("does not redirect /desktops/dk1/verify", async () => {
    install();
    const { router } = mountApp("/desktops/dk1/verify#fp=ab12");
    await screen.findByRole("heading", { level: 1, name: "Verify desktop" });
    expect(router.state.location.pathname).toBe("/desktops/dk1/verify");
  });

  it("does not redirect /pair/CODE", async () => {
    install();
    const { router } = mountApp("/pair/CODE");
    await settled(router);
    expect(router.state.location.pathname).toBe("/pair/CODE");
  });

  it("sends / to /settings/desktops when there is no last place", async () => {
    install();
    const { router } = mountApp("/");
    await screen.findByRole("heading", { level: 1, name: "All desktops" });
    expect(router.state.location.pathname).toBe("/settings/desktops");
  });
});

describe("old /admin URLs", () => {
  /** An admin account with a fresh session, so the admin pages read without asking for a passkey. */
  function installAdmin() {
    const state = install({ role: "admin" });
    state.reauthAt = nowSeconds();
  }

  it.each([
    ["/admin", "/settings/admin", "", "Admin"],
    ["/admin/", "/settings/admin", "", "Admin"],
    ["/admin/users", "/settings/admin/users", "", "Users"],
    ["/admin/users?user=usr-ada", "/settings/admin/users", "?user=usr-ada", "Users"],
    ["/admin/invites", "/settings/admin/invites", "", "Invite tree"],
    ["/admin/whatever", "/settings/admin", "", "Admin"],
  ])("%s ends on %s inside the settings shell", async (url, path, search, title) => {
    installAdmin();
    const { router } = mountApp(url);
    await screen.findByRole("heading", { level: 1, name: title });
    expect(router.state.location.pathname).toBe(path);
    expect(router.state.location.searchStr).toBe(search);
    if (search) expect(router.state.location.search).toEqual({ user: "usr-ada" });
    expect(screen.queryByTestId("app-header")).toBeNull();
    expect(screen.getAllByRole("button", { name: "Leave settings" }).length).toBeGreaterThanOrEqual(1);
  });
});

describe("forced recovery-codes gate", () => {
  it("shows the regenerate screen at /settings/profile instead of the shell", async () => {
    install({ recoveryAckAt: null });
    const { router } = mountApp("/settings/profile");
    await screen.findByRole("heading", { level: 1, name: "Get new recovery codes" });
    expect(router.state.location.pathname).toBe("/settings/profile");
    expect(screen.queryByRole("navigation", { name: "Settings" })).toBeNull();
  });

  it.each(["/settings", "/settings/desktops", "/settings/security", "/desktops", "/d/dk1"])("sends %s to /settings/profile while the codes are unconfirmed", async (url) => {
    install({ recoveryAckAt: null });
    const { router } = mountApp(url);
    await screen.findByRole("heading", { level: 1, name: "Get new recovery codes" });
    expect(router.state.location.pathname).toBe("/settings/profile");
  });
});
