import "fake-indexeddb/auto";
import { describe, expect, it } from "vitest";
import { getSlot } from "@/ui/slots";
import { forgetDesktopLocally } from "@/features/settings/desktopLink";
import { loadMonoSeen, markMonoSeen } from "./monoSeen";
import "./register";

describe("sessions register", () => {
  it("registers the default model row of the models feature (U-07)", () => {
    expect(getSlot("desktop-settings-sections")).toBeDefined();
  });

  it("wipes the Mono seen map and the last place when the device forgets a desktop", async () => {
    localStorage.setItem("monoptah.lastPlace.dk", "/d/dk/p/1");
    loadMonoSeen("dk");
    markMonoSeen("dk", { id: "m", status: "idle", unread: 0, lastMessage: "x", lastActiveAt: 1 });
    await forgetDesktopLocally("dk");
    expect(localStorage.getItem("monoptah.lastPlace.dk")).toBeNull();
  });
});
