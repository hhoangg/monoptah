// The Settings group of the command palette: `paletteItems` lists the settings hits (from `searchSettings`) after the
// other groups, and never for an empty query.
import { afterEach, describe, expect, it, vi } from "vitest";
import { searchSettings } from "@/features/settings/settingsIndex";
import { paletteItems } from "./CommandPalette";

const catalog = {
  projects: [{ id: "p1", name: "Passkey Project", path: "/work/passkey" }],
  monos: [],
  sessions: [],
};

function settingsOf(query: string, admin = false) {
  return paletteItems({ ...catalog, settings: searchSettings(query, { admin }) }, query).filter((item) => item.kind === "setting");
}

afterEach(() => vi.unstubAllGlobals());

function stubViewport(wide: boolean) {
  vi.stubGlobal("matchMedia", (query: string) => ({ matches: wide && query.includes("min-width"), media: query, addEventListener: () => undefined, removeEventListener: () => undefined }));
}

describe("paletteItems settings", () => {
  it("lists the Layout setting only from 640px, where its picker exists", () => {
    expect(settingsOf("layout").map((item) => item.id)).toContain("appearance.layout");
    stubViewport(false);
    expect(settingsOf("layout").map((item) => item.id)).not.toContain("appearance.layout");
    stubViewport(true);
    expect(settingsOf("layout").map((item) => item.id)).toContain("appearance.layout");
  });

  it("lists no settings for an empty query", () => {
    expect(settingsOf("")).toEqual([]);
    expect(settingsOf("   ")).toEqual([]);
  });

  it("finds Passkeys with its page as the detail", () => {
    const hit = settingsOf("passkey").find((item) => item.id === "security.passkeys");
    expect(hit).toMatchObject({ kind: "setting", label: "Passkeys", detail: "Security" });
    expect(hit?.positions.length).toBeGreaterThan(0);
  });

  it("finds a per-desktop entry such as Reconnect", () => {
    expect(settingsOf("reconnect").map((item) => item.id)).toContain("desktop.reconnect");
  });

  it("lists Admin to admins only", () => {
    expect(settingsOf("admin").map((item) => item.id)).not.toContain("admin");
    expect(settingsOf("admin", true).map((item) => item.id)).toContain("admin");
  });

  it("puts the settings after the projects, Monos and sessions", () => {
    const kinds = paletteItems({ ...catalog, settings: searchSettings("passkey") }, "passkey").map((item) => item.kind);
    expect(kinds[0]).toBe("project");
    expect(kinds[kinds.length - 1]).toBe("setting");
  });
});
