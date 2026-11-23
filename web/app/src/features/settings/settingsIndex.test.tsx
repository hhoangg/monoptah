import { cleanup, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { setApiTransport } from "@/features/accounts/api";
import { createMockState, type SeedOptions } from "@/features/accounts/mock/state";
import { createMockServer } from "@/features/accounts/mock/transport";
import { mountApp } from "@/features/desktops/testMount";
import { WsPSettingsSections } from "@/features/themes/SettingsSections";
import { clearSlots, registerSlot } from "@/ui/slots";
import { BROWSER_ONLY_LABEL, SETTINGS_GROUP_LABELS, SETTINGS_INDEX, anchorsOf, searchSettings, type SettingsRoute } from "./settingsIndex";

vi.mock("@/features/accounts/webauthn", () => ({ createPasskey: vi.fn(), getPasskey: vi.fn(), passkeysSupported: vi.fn(() => true) }));

let restore: (() => void) | undefined;

function install(options: SeedOptions = {}) {
  const state = createMockState({ signedIn: true, ...options });
  restore = setApiTransport(createMockServer({ state }).transport);
  return state;
}

/** Every width query matches (the Layout picker only renders from 640px). */
function stubWide() {
  vi.stubGlobal("matchMedia", (query: string) => ({
    matches: query.includes("min-width"),
    media: query,
    addEventListener: () => undefined,
    removeEventListener: () => undefined,
    addListener: () => undefined,
    removeListener: () => undefined,
    dispatchEvent: () => false,
  }));
}

/** No width query matches: a narrow window. */
function stubNarrow() {
  vi.stubGlobal("matchMedia", (query: string) => ({
    matches: false,
    media: query,
    addEventListener: () => undefined,
    removeEventListener: () => undefined,
    addListener: () => undefined,
    removeListener: () => undefined,
    dispatchEvent: () => false,
  }));
}

beforeEach(() => {
  registerSlot("settings-sections", WsPSettingsSections);
});
afterEach(() => {
  restore?.();
  clearSlots();
  vi.unstubAllGlobals();
  Reflect.deleteProperty(Element.prototype, "scrollIntoView");
});

describe("searchSettings", () => {
  it("returns nothing for an empty or blank query", () => {
    expect(searchSettings("")).toEqual([]);
    expect(searchSettings("   ")).toEqual([]);
  });

  it("finds an entry by its label and returns the matched positions", () => {
    const [best] = searchSettings("passkeys");
    expect(best?.entry.id).toBe("security.passkeys");
    expect(best?.positions.length).toBeGreaterThan(0);
  });

  it("finds an entry by a keyword, with no positions", () => {
    const hit = searchSettings("webauthn").find((candidate) => candidate.entry.id === "security.passkeys");
    expect(hit).toBeDefined();
    expect(hit?.positions).toEqual([]);
  });

  it("ranks label matches before keyword-only matches", () => {
    const hits = searchSettings("notif");
    expect(hits.length).toBeGreaterThan(2);
    const firstKeywordOnly = hits.findIndex((hit) => hit.positions.length === 0);
    if (firstKeywordOnly !== -1) expect(hits.slice(firstKeywordOnly).every((hit) => hit.positions.length === 0)).toBe(true);
    expect(hits[0]?.positions.length).toBeGreaterThan(0);
  });

  it("hides admin-only entries from non-admins", () => {
    expect(searchSettings("admin").some((hit) => hit.entry.id === "admin")).toBe(false);
    expect(searchSettings("admin", { admin: true }).some((hit) => hit.entry.id === "admin")).toBe(true);
  });

  it("lists an entry with a minWidth only when the viewport is at least that wide, and when matchMedia is missing", () => {
    const layout = () => searchSettings("layout").some((hit) => hit.entry.id === "appearance.layout");
    expect(SETTINGS_INDEX.find((entry) => entry.id === "appearance.layout")?.minWidth).toBe(640);
    // jsdom has no matchMedia: every entry counts as present.
    expect(layout()).toBe(true);
    stubNarrow();
    expect(layout()).toBe(false);
    vi.unstubAllGlobals();
    stubWide();
    expect(layout()).toBe(true);
  });

  it("has unique ids", () => {
    const ids = SETTINGS_INDEX.map((entry) => entry.id);
    expect(new Set(ids).size).toBe(ids.length);
  });
});

describe("anchors of the pages", () => {
  const PAGES: SettingsRoute[] = ["/settings/profile", "/settings/security", "/settings/invites", "/settings/appearance", "/settings/notifications", "/settings/install", "/settings/desktops"];

  it.each(PAGES)("%s renders every anchor of the index as an element id", async (route) => {
    install();
    stubWide();
    mountApp(route);
    await screen.findByRole("heading", { level: 1 });
    const anchors = anchorsOf(route);
    expect(anchors.length).toBeGreaterThan(0);
    await waitFor(() => {
      for (const anchor of anchors) expect(document.getElementById(anchor), `#${anchor} on ${route}`).not.toBeNull();
    });
  });

  it("indexes every non-per-desktop anchor on a page of the list above", () => {
    const routes = new Set<string>(PAGES);
    for (const entry of SETTINGS_INDEX) {
      if (entry.perDesktop || entry.route === "/admin") continue;
      expect(routes.has(entry.route), entry.id).toBe(true);
    }
  });
});

describe("settings nav", () => {
  it("lists the groups, the This browser badge and the account and browser pages", async () => {
    install();
    mountApp("/settings/profile");
    const nav = await screen.findByRole("navigation", { name: "Settings" });
    for (const label of [SETTINGS_GROUP_LABELS.account, SETTINGS_GROUP_LABELS.browser, SETTINGS_GROUP_LABELS.desktops]) {
      // The group labels are not headings: each one labels its list.
      expect(within(nav).getByRole("list", { name: new RegExp(`^${label}`) })).toBeInTheDocument();
    }
    expect(within(nav).getAllByText(BROWSER_ONLY_LABEL)).toHaveLength(1);
    const hrefs: Record<string, string> = {
      Profile: "/settings/profile",
      Security: "/settings/security",
      Invites: "/settings/invites",
      Appearance: "/settings/appearance",
      Notifications: "/settings/notifications",
      "Install app": "/settings/install",
      "All desktops": "/settings/desktops",
    };
    for (const [name, href] of Object.entries(hrefs)) expect(within(nav).getByRole("link", { name })).toHaveAttribute("href", href);
  });

  it("lists one item per desktop with its presence in the accessible name", async () => {
    install();
    mountApp("/settings/profile");
    const nav = await screen.findByRole("navigation", { name: "Settings" });
    const online = await within(nav).findByRole("link", { name: /Fake Studio Mac.*online/ });
    expect(online).toHaveAttribute("href", "/settings/desktops/desktop-dev-1");
    expect(online.textContent).not.toMatch(/offline/);
    const offline = within(nav).getByRole("link", { name: /Fake Linux box.*offline/ });
    expect(offline).toHaveAttribute("href", "/settings/desktops/desktop-dev-2");
  });

  it("shows Admin to admins only", async () => {
    install({ role: "admin" });
    mountApp("/settings/profile");
    const nav = await screen.findByRole("navigation", { name: "Settings" });
    expect(await within(nav).findByRole("link", { name: "Admin" })).toHaveAttribute("href", "/admin");
    expect(within(nav).getByRole("list", { name: SETTINGS_GROUP_LABELS.admin })).toBeInTheDocument();
  });

  it("shows no Admin item to a member", async () => {
    install({ role: "user" });
    mountApp("/settings/profile");
    const nav = await screen.findByRole("navigation", { name: "Settings" });
    await within(nav).findByRole("link", { name: /Fake Studio Mac/ });
    expect(within(nav).queryByRole("link", { name: "Admin" })).toBeNull();
  });

  it("marks the active page with aria-current", async () => {
    install();
    mountApp("/settings/security");
    const nav = await screen.findByRole("navigation", { name: "Settings" });
    expect(within(nav).getByRole("link", { name: "Security" })).toHaveAttribute("aria-current", "page");
    expect(within(nav).getByRole("link", { name: "Profile" })).not.toHaveAttribute("aria-current");
  });

  it("marks only the desktop item, not All desktops, on a desktop page", async () => {
    install();
    mountApp("/settings/desktops/desktop-dev-1");
    const nav = await screen.findByRole("navigation", { name: "Settings" });
    expect(await within(nav).findByRole("link", { name: /Fake Studio Mac/ })).toHaveAttribute("aria-current", "page");
    expect(within(nav).getByRole("link", { name: "All desktops" })).not.toHaveAttribute("aria-current");
  });

  it("scrolls to the anchor of a #hash link once the card is on screen", async () => {
    install();
    const scroll = vi.fn();
    Object.defineProperty(Element.prototype, "scrollIntoView", { value: scroll, configurable: true, writable: true });
    mountApp("/settings/security#passkeys");
    await screen.findByRole("heading", { level: 1, name: "Security" });
    await waitFor(() => expect(scroll.mock.contexts.some((context) => (context as Element).id === "passkeys")).toBe(true));
  });
});

describe("quick-find", () => {
  async function box() {
    const nav = await screen.findByRole("navigation", { name: "Settings" });
    return { nav, input: within(nav).getByRole("searchbox", { name: "Find a setting" }) };
  }

  it("has a search box with the placeholder", async () => {
    install();
    mountApp("/settings/profile");
    const { input } = await box();
    expect(input).toHaveAttribute("type", "search");
    expect(input).toHaveAttribute("placeholder", "Find a setting");
  });

  it("replaces the groups with the matching settings, linking to route#anchor", async () => {
    install();
    const user = userEvent.setup();
    mountApp("/settings/profile");
    const { nav, input } = await box();
    await user.type(input, "passkeys");
    const link = await within(nav).findByRole("link", { name: /^Passkeys/ });
    expect(link).toHaveAttribute("href", "/settings/security#passkeys");
    expect(within(nav).queryByRole("list", { name: new RegExp(`^${SETTINGS_GROUP_LABELS.account}`) })).toBeNull();
    expect(within(nav).queryByRole("heading")).toBeNull();
    expect(within(nav).queryByRole("link", { name: "Profile" })).toBeNull();
  });

  it("offers the Layout setting only from 640px, where its picker exists", async () => {
    install();
    const user = userEvent.setup();
    stubNarrow();
    mountApp("/settings/profile");
    const { nav, input } = await box();
    await user.type(input, "layout");
    expect(within(nav).queryByRole("link", { name: /^Layout/ })).toBeNull();
    cleanup();
    vi.unstubAllGlobals();
    stubWide();
    mountApp("/settings/profile");
    const wide = await box();
    await user.type(wide.input, "layout");
    expect(await within(wide.nav).findByRole("link", { name: /^Layout/ })).toHaveAttribute("href", "/settings/appearance#layout");
  });

  it("scrolls again when a result is picked whose anchor is already in the URL", async () => {
    install();
    const user = userEvent.setup();
    const scroll = vi.fn();
    Object.defineProperty(Element.prototype, "scrollIntoView", { value: scroll, configurable: true, writable: true });
    // Only the shell's own scrolls count: it passes an options object, while the router's hash scrolling (scrollRestoration)
    // calls scrollIntoView(true) on its own and would otherwise satisfy the assertion by itself.
    const passkeyScrolls = () => scroll.mock.contexts.filter((context, index) => (context as Element).id === "passkeys" && typeof scroll.mock.calls[index]?.[0] === "object").length;
    mountApp("/settings/security");
    const { nav, input } = await box();
    await user.type(input, "passkeys");
    await user.click(await within(nav).findByRole("link", { name: /^Passkeys/ }));
    await waitFor(() => expect(passkeyScrolls()).toBeGreaterThan(0));
    // The router's own hash scrolling (scrollRestoration) may add a call of its own: let both settle, then count from here.
    await new Promise((resolve) => setTimeout(resolve, 100));
    // The URL is now /settings/security#passkeys: the same pick is a no-op for the router but must scroll again.
    const before = passkeyScrolls();
    await user.type(input, "passkeys");
    await user.click(await within(nav).findByRole("link", { name: /^Passkeys/ }));
    await waitFor(() => expect(passkeyScrolls()).toBeGreaterThan(before));
  });

  it("matches by keyword and shows the page name", async () => {
    install();
    const user = userEvent.setup();
    mountApp("/settings/profile");
    const { nav, input } = await box();
    await user.type(input, "dark mode");
    const link = await within(nav).findByRole("link", { name: /^Colour scheme/ });
    expect(link).toHaveAttribute("href", "/settings/appearance#colour-scheme");
    expect(link).toHaveTextContent("Appearance");
  });

  it("highlights the matched characters of the label", async () => {
    install();
    const user = userEvent.setup();
    mountApp("/settings/profile");
    const { nav, input } = await box();
    await user.type(input, "inv");
    const link = await within(nav).findByRole("link", { name: /^Invites/ });
    expect(link.querySelectorAll("[data-match]").length).toBeGreaterThan(0);
  });

  it("expands a per-desktop setting once per desktop, with the desktop name as detail", async () => {
    install();
    const user = userEvent.setup();
    mountApp("/settings/profile");
    const { nav, input } = await box();
    await within(nav).findByRole("link", { name: /Fake Studio Mac/ });
    await user.type(input, "desktop name");
    await waitFor(() => expect(within(nav).getAllByRole("link").filter((link) => link.getAttribute("href")?.endsWith("#desktop-name"))).toHaveLength(2));
    const links = within(nav)
      .getAllByRole("link")
      .filter((link) => link.getAttribute("href")?.endsWith("#desktop-name"));
    expect(links.map((link) => link.getAttribute("href")).sort()).toEqual(["/settings/desktops/desktop-dev-1#desktop-name", "/settings/desktops/desktop-dev-2#desktop-name"]);
    expect(links[0]).toHaveTextContent(/Fake (Studio Mac|Linux box)/);
  });

  it("lists the Admin entry for admins only", async () => {
    install({ role: "admin" });
    const user = userEvent.setup();
    mountApp("/settings/profile");
    const { nav, input } = await box();
    await user.type(input, "administration");
    expect(await within(nav).findByRole("link", { name: /^Admin/ })).toHaveAttribute("href", "/admin");
  });

  it("does not list the Admin entry to a member", async () => {
    install({ role: "user" });
    const user = userEvent.setup();
    mountApp("/settings/profile");
    const { nav, input } = await box();
    await user.type(input, "administration");
    expect(await within(nav).findByText("No settings match.")).toBeInTheDocument();
  });

  it("says when nothing matches", async () => {
    install();
    const user = userEvent.setup();
    mountApp("/settings/profile");
    const { nav, input } = await box();
    await user.type(input, "zzzzqqq");
    expect(await within(nav).findByText("No settings match.")).toBeInTheDocument();
  });

  it("clears the query with Escape and brings the groups back", async () => {
    install();
    const user = userEvent.setup();
    mountApp("/settings/profile");
    const { nav, input } = await box();
    await user.type(input, "passkeys");
    await within(nav).findByRole("link", { name: /^Passkeys/ });
    await user.keyboard("{Escape}");
    expect(input).toHaveValue("");
    expect(within(nav).getByRole("link", { name: "Profile" })).toBeInTheDocument();
  });

  it("clears the query and navigates when a result is picked", async () => {
    install();
    const user = userEvent.setup();
    const { router } = mountApp("/settings/profile");
    const { nav, input } = await box();
    await user.type(input, "passkeys");
    await user.click(await within(nav).findByRole("link", { name: /^Passkeys/ }));
    await waitFor(() => expect(router.state.location.pathname).toBe("/settings/security"));
    expect(router.state.location.hash).toBe("passkeys");
    expect(input).toHaveValue("");
  });
});
