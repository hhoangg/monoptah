import { describe, expect, it } from "vitest";
import { DESKTOP_COLORS } from "./colors";
import { desktopAvatarColor, desktopInitials, grantCue, onAvatarColor } from "./avatar";

describe("desktop avatar", () => {
  it.each([
    ["Studio Mac", "SM"],
    ["Cty-PC", "CP"],
    ["linux", "L"],
    ["  office   pc  extra", "OP"],
    ["Văn phòng", "VP"],
    ["***", "?"],
    ["", "?"],
  ])("takes the initials of %j as %j", (name, initials) => {
    expect(desktopInitials(name)).toBe(initials);
  });

  it("uses the chosen swatch, case-insensitively", () => {
    expect(desktopAvatarColor("d1", "#EA603E")).toBe("#ea603e");
  });

  it("picks a stable swatch from the id when no color is chosen or the stored one is unknown", () => {
    const swatches = DESKTOP_COLORS.map((color) => color.value);
    for (const stored of [null, undefined, "", "#123456", "javascript:alert(1)"]) {
      const color = desktopAvatarColor("desktop-abc", stored);
      expect(swatches).toContain(color);
      expect(desktopAvatarColor("desktop-abc", stored)).toBe(color);
    }
    // Different ids spread over the palette.
    const used = new Set(Array.from({ length: 60 }, (_, index) => desktopAvatarColor(`desktop-${index}`, null)));
    expect(used.size).toBeGreaterThan(4);
  });

  it("picks the text color with the better contrast, and every swatch reaches 4.5:1 with it", () => {
    for (const { value } of DESKTOP_COLORS) expect(onAvatarColor(value)).toBe("var(--on-swatch-dark)");
    expect(onAvatarColor("#b168d4")).toBe("var(--on-swatch-dark)"); // white is only 3.62:1 here
    expect(onAvatarColor("#1a1a1a")).toBe("var(--on-swatch-light)");
  });

  it("reads the grant cue of this device", () => {
    const grants = [
      { deviceId: "me", state: "pending" },
      { deviceId: "other", state: "confirmed" },
    ];
    expect(grantCue(grants, "me")).toBe("pending");
    expect(grantCue(grants, "other")).toBeNull();
    expect(grantCue([{ deviceId: "me", state: "revoked" }], "me")).toBe("revoked");
    expect(grantCue([], "me")).toBe("none");
    expect(grantCue(undefined, "me")).toBeNull();
    expect(grantCue(grants, null)).toBeNull();
  });
});
