// Static index of every settings item: the quick-find box of the settings nav and the Settings group of the Mod+K
// palette both search it. `anchor` is the DOM id of the item on its page, so a link to `route#anchor` scrolls to it;
// every page that renders an indexed item must give that element the same id (see settingsIndex.test.ts).
import { fuzzyMatch } from "@/lib/fuzzy";

export type SettingsGroupId = "account" | "browser" | "desktops" | "admin";

/** The pages of the settings shell (and /admin, which keeps its own screens). */
export type SettingsRoute =
  | "/settings/profile"
  | "/settings/security"
  | "/settings/invites"
  | "/settings/appearance"
  | "/settings/notifications"
  | "/settings/install"
  | "/settings/desktops"
  | "/settings/desktops/$id"
  | "/admin";

export interface SettingsIndexEntry {
  /** Unique and stable (palette option ids, React keys). */
  id: string;
  label: string;
  /** Other words people may type for it. Matched after the label; never highlighted. */
  keywords: readonly string[];
  group: SettingsGroupId;
  /** The page the entry lives on, shown beside the label in the results. */
  page: string;
  route: SettingsRoute;
  /** DOM id of the item on its page. Absent for an entry that is the page itself. */
  anchor?: string;
  /** The route needs a desktop id (`$id`): the nav lists it once per desktop, the palette for the open desktop. */
  perDesktop?: boolean;
  /** Shown only to admins. */
  adminOnly?: boolean;
  /** The item exists on its page only from this viewport width (px), so below it the entry must not be offered. */
  minWidth?: number;
}

/**
 * Whether the item of `entry` exists at the current viewport width (see `minWidth`). Without `matchMedia` (jsdom) every
 * entry counts as present.
 */
export function entryFitsViewport(entry: SettingsIndexEntry): boolean {
  if (entry.minWidth === undefined || typeof window === "undefined" || typeof window.matchMedia !== "function") return true;
  return window.matchMedia(`(min-width: ${entry.minWidth}px)`).matches;
}

export const SETTINGS_GROUP_LABELS: Record<SettingsGroupId, string> = {
  account: "Account",
  browser: "This browser",
  desktops: "Desktops",
  admin: "Admin",
};

/** The badge of the "This browser" group and its pages: these choices are stored in this browser only. */
export const BROWSER_ONLY_LABEL = "Only on this browser";

export const SETTINGS_INDEX: readonly SettingsIndexEntry[] = [
  // Account > Profile
  { id: "profile", label: "Profile", keywords: ["account", "me"], group: "account", page: "Profile", route: "/settings/profile" },
  { id: "profile.display-name", label: "Display name", keywords: ["name", "rename", "account"], group: "account", page: "Profile", route: "/settings/profile", anchor: "display-name" },
  { id: "profile.handle", label: "Handle", keywords: ["username", "user name", "@"], group: "account", page: "Profile", route: "/settings/profile", anchor: "handle" },
  { id: "profile.sign-out", label: "Sign out", keywords: ["log out", "logout", "sign off"], group: "account", page: "Profile", route: "/settings/profile", anchor: "sign-out" },
  // Account > Security
  { id: "security", label: "Security", keywords: ["safety", "login"], group: "account", page: "Security", route: "/settings/security" },
  { id: "security.passkeys", label: "Passkeys", keywords: ["passkey", "webauthn", "security key", "touch id", "face id", "fingerprint"], group: "account", page: "Security", route: "/settings/security", anchor: "passkeys" },
  { id: "security.recovery-codes", label: "Recovery codes", keywords: ["backup codes", "regenerate", "lost passkey"], group: "account", page: "Security", route: "/settings/security", anchor: "recovery-codes" },
  { id: "security.browsers", label: "Signed-in browsers", keywords: ["sessions", "sign out other", "log out other", "browsers"], group: "account", page: "Security", route: "/settings/security", anchor: "signed-in-browsers" },
  // Account > Invites
  { id: "invites", label: "Invites", keywords: ["invite", "invite link", "people", "friends"], group: "account", page: "Invites", route: "/settings/invites", anchor: "invites" },
  // This browser > Appearance
  { id: "appearance", label: "Appearance", keywords: ["look", "feel"], group: "browser", page: "Appearance", route: "/settings/appearance" },
  { id: "appearance.colour-scheme", label: "Colour scheme", keywords: ["color scheme", "dark mode", "light mode", "system"], group: "browser", page: "Appearance", route: "/settings/appearance", anchor: "colour-scheme" },
  { id: "appearance.layout", label: "Layout", keywords: ["classic", "focused", "session list", "column"], group: "browser", page: "Appearance", route: "/settings/appearance", anchor: "layout", minWidth: 640 },
  { id: "appearance.theme", label: "Theme", keywords: ["accent", "tint", "color", "colour"], group: "browser", page: "Appearance", route: "/settings/appearance", anchor: "theme" },
  { id: "appearance.backgrounds", label: "Backgrounds", keywords: ["wallpaper", "background image", "photo"], group: "browser", page: "Appearance", route: "/settings/appearance", anchor: "backgrounds" },
  { id: "appearance.monos-only", label: "Monos only", keywords: ["mono", "home screen", "hide projects"], group: "browser", page: "Appearance", route: "/settings/appearance", anchor: "monos-only" },
  // This browser > Notifications
  { id: "notifications", label: "Notifications", keywords: ["push", "alerts"], group: "browser", page: "Notifications", route: "/settings/notifications" },
  { id: "notifications.permission", label: "Notification permission", keywords: ["push", "allow notifications", "enable notifications", "turn on"], group: "browser", page: "Notifications", route: "/settings/notifications", anchor: "notification-permission" },
  { id: "notifications.test", label: "Test notification", keywords: ["push", "send test"], group: "browser", page: "Notifications", route: "/settings/notifications", anchor: "test-notification" },
  { id: "notifications.pause", label: "Pause all notifications", keywords: ["pause all", "resume all", "mute", "do not disturb"], group: "browser", page: "Notifications", route: "/settings/notifications", anchor: "pause-notifications" },
  { id: "notifications.per-desktop", label: "Notifications per desktop", keywords: ["categories", "approvals", "questions", "agent finished", "messages from monos"], group: "browser", page: "Notifications", route: "/settings/notifications", anchor: "notifications-per-desktop" },
  // This browser > Install app
  { id: "install", label: "Install app", keywords: ["pwa", "home screen", "add to dock", "install monoptah"], group: "browser", page: "Install app", route: "/settings/install", anchor: "install" },
  // Desktops > All desktops
  { id: "desktops", label: "All desktops", keywords: ["desktops", "machines", "computers", "desktop list"], group: "desktops", page: "All desktops", route: "/settings/desktops", anchor: "your-desktops" },
  { id: "desktops.pair", label: "Pair a desktop", keywords: ["pairing link", "pairing code", "qr", "add desktop", "connect"], group: "desktops", page: "All desktops", route: "/settings/desktops", anchor: "pair-desktop" },
  { id: "desktops.browser-access", label: "Browsers with desktop access", keywords: ["devices", "forget device", "this account's devices", "access"], group: "desktops", page: "All desktops", route: "/settings/desktops", anchor: "browser-access" },
  // Desktops > one desktop
  { id: "desktop.status", label: "Status", keywords: ["online", "offline", "connection", "last seen"], group: "desktops", page: "Desktop", route: "/settings/desktops/$id", anchor: "desktop-status", perDesktop: true },
  { id: "desktop.about", label: "About", keywords: ["version", "fingerprint", "key", "system"], group: "desktops", page: "Desktop", route: "/settings/desktops/$id", anchor: "desktop-about", perDesktop: true },
  { id: "desktop.name", label: "Desktop name", keywords: ["rename", "name"], group: "desktops", page: "Desktop", route: "/settings/desktops/$id", anchor: "desktop-name", perDesktop: true },
  { id: "desktop.color", label: "Desktop color", keywords: ["colour", "swatch"], group: "desktops", page: "Desktop", route: "/settings/desktops/$id", anchor: "desktop-color", perDesktop: true },
  { id: "desktop.avatar", label: "Desktop avatar", keywords: ["emoji", "image", "icon", "picture"], group: "desktops", page: "Desktop", route: "/settings/desktops/$id", anchor: "desktop-avatar", perDesktop: true },
  { id: "desktop.models", label: "Models", keywords: ["default model", "harness", "access mode", "claude", "codex"], group: "desktops", page: "Desktop", route: "/settings/desktops/$id", anchor: "desktop-models", perDesktop: true },
  { id: "desktop.notifications", label: "Desktop notifications", keywords: ["push", "approvals", "agent finished", "messages from monos"], group: "desktops", page: "Desktop", route: "/settings/desktops/$id", anchor: "desktop-notifications", perDesktop: true },
  { id: "desktop.projects", label: "Projects visible in this browser", keywords: ["projects", "monos", "hide", "show", "choose projects"], group: "desktops", page: "Desktop", route: "/settings/desktops/$id", anchor: "desktop-projects", perDesktop: true },
  { id: "desktop.update", label: "Desktop app update", keywords: ["update", "upgrade", "version", "restart"], group: "desktops", page: "Desktop", route: "/settings/desktops/$id", anchor: "desktop-update", perDesktop: true },
  { id: "desktop.access", label: "Access", keywords: ["grants", "devices", "permission", "read only", "control", "revoke access", "add another device"], group: "desktops", page: "Desktop", route: "/settings/desktops/$id", anchor: "desktop-access", perDesktop: true },
  { id: "desktop.reconnect", label: "Reconnect", keywords: ["troubleshoot", "connection", "handshake", "refresh"], group: "desktops", page: "Desktop", route: "/settings/desktops/$id", anchor: "desktop-troubleshoot", perDesktop: true },
  { id: "desktop.forget", label: "Forget in this browser", keywords: ["remove", "forget", "leave"], group: "desktops", page: "Desktop", route: "/settings/desktops/$id", anchor: "desktop-remove", perDesktop: true },
  { id: "desktop.revoke", label: "Revoke from account", keywords: ["revoke desktop", "delete desktop", "remove desktop"], group: "desktops", page: "Desktop", route: "/settings/desktops/$id", anchor: "desktop-remove", perDesktop: true },
  // Admin
  { id: "admin", label: "Admin", keywords: ["administration", "users", "audit log", "invites"], group: "admin", page: "Admin", route: "/admin", adminOnly: true },
];

export interface SettingsSearchHit {
  entry: SettingsIndexEntry;
  score: number;
  /** Matched character positions in `entry.label`; empty when only a keyword or the page name matched. */
  positions: number[];
}

/**
 * The entries matching `query`, best first. The label is matched first (its positions are returned for highlighting),
 * then the keywords and the page name (a lower score, no positions). An empty query returns nothing: the nav then shows
 * its normal list. Admin-only entries need `admin`.
 */
export function searchSettings(query: string, options: { admin?: boolean; entries?: readonly SettingsIndexEntry[] } = {}): SettingsSearchHit[] {
  if (query.trim() === "") return [];
  const hits: SettingsSearchHit[] = [];
  for (const entry of options.entries ?? SETTINGS_INDEX) {
    if (entry.adminOnly && !options.admin) continue;
    if (!entryFitsViewport(entry)) continue;
    const label = fuzzyMatch(query, entry.label);
    if (label) {
      hits.push({ entry, score: label.score, positions: label.positions });
      continue;
    }
    let best: number | null = null;
    for (const text of [...entry.keywords, entry.page]) {
      const hit = fuzzyMatch(query, text);
      if (hit && (best === null || hit.score > best)) best = hit.score;
    }
    // Below every label match of a similar quality: the label is what the person reads.
    if (best !== null) hits.push({ entry, score: best - 1000, positions: [] });
  }
  return hits.sort((a, b) => b.score - a.score);
}

/** The anchors one page must render (the settingsIndex test checks them against the pages). */
export function anchorsOf(route: SettingsRoute): string[] {
  return [...new Set(SETTINGS_INDEX.filter((entry) => entry.route === route && entry.anchor).map((entry) => entry.anchor as string))];
}
