// The registrations of WS-C: one component per slot (a second registration throws), composer-top is the
// composition of the approval card and the cloud banners.
import { beforeAll, describe, expect, it } from "vitest";
import { clearSlots, getSlot, registerSlot } from "@/ui/slots";
import { ComposerTopCloud, MenuItemsCloud, NewSessionTargetCloud } from "./session/slotComponents";

beforeAll(async () => {
  clearSlots();
  await import("./register");
});

describe("register.ts", () => {
  it("registers composer-top once, as the approval card plus the cloud banners", () => {
    expect(getSlot("composer-top")).toBe(ComposerTopCloud);
    expect(() => registerSlot("composer-top", ComposerTopCloud)).toThrow(/already registered/);
  });

  it("registers the start picker and the move menu items", () => {
    expect(getSlot("new-session-target")).toBe(NewSessionTargetCloud);
    expect(getSlot("session-menu-items")).toBe(MenuItemsCloud);
  });
});
