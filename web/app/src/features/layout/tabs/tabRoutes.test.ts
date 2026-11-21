import { describe, expect, it } from "vitest";
import { monoTab, sessionTab, terminalTab } from "./paneTree";
import { tabLocation, tabOfPath } from "./tabRoutes";

describe("tab routes", () => {
  it("maps session, mono and terminal URLs to their tab and back", () => {
    expect(tabOfPath("/d/dk/s/s1")).toBe(sessionTab("s1"));
    expect(tabOfPath("/d/dk/monos/m1/")).toBe(monoTab("m1"));
    expect(tabOfPath("/d/dk/t/abc")).toBe(terminalTab("abc"));
    expect(tabOfPath("/d/dk/terminal")).toBeNull();
    expect(tabLocation("dk", terminalTab("abc"))).toEqual({ to: "/d/$desktopId/t/$terminalId", params: { desktopId: "dk", terminalId: "abc" } });
    expect(tabLocation("dk", sessionTab("s1"))).toEqual({ to: "/d/$desktopId/s/$sessionId", params: { desktopId: "dk", sessionId: "s1" } });
  });
});
