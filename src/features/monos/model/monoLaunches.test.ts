import { describe, expect, it } from "vitest";
import { createMonoLaunchRegistry } from "./monoLaunches";

describe("Mono launch registry", () => {
  it("returns the owning Mono and parented flag of a recorded launch", () => {
    const launches = createMonoLaunchRegistry();
    launches.record("app-lead-dev", "mono", false);
    launches.record("app-app-lead-dev-rev", "mono", true);
    expect(launches.get("app-lead-dev")).toEqual({
      monoId: "mono",
      parented: false,
    });
    expect(launches.get("app-app-lead-dev-rev")).toEqual({
      monoId: "mono",
      parented: true,
    });
  });

  it("returns undefined for an unknown session", () => {
    expect(createMonoLaunchRegistry().get("nope")).toBeUndefined();
  });

  it("keeps the latest record when a session is recorded again", () => {
    const launches = createMonoLaunchRegistry();
    launches.record("app-lead-dev", "mono", false);
    launches.record("app-lead-dev", "mono", false);
    expect(launches.get("app-lead-dev")).toEqual({
      monoId: "mono",
      parented: false,
    });
    launches.record("app-lead-dev", "other", true);
    expect(launches.get("app-lead-dev")).toEqual({
      monoId: "other",
      parented: true,
    });
  });

  it("keeps registries independent", () => {
    const a = createMonoLaunchRegistry();
    const b = createMonoLaunchRegistry();
    a.record("x", "mono", false);
    expect(b.get("x")).toBeUndefined();
  });
});
