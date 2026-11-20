import { describe, expect, it } from "vitest";
import { MockWorld } from "../src/index.ts";
import { setup } from "./harness.ts";

const withWorld = (world: MockWorld) => ({ desktop: { world } });

describe("desktop.updateStatus", () => {
  it("reports up to date by default, with the running sessions", async () => {
    const { client } = await setup();
    const status = await client.call("desktop.updateStatus");
    expect(status).toMatchObject({ current: "1.4.0", latest: "1.4.0", updateAvailable: false });
    expect(status.runningSessions).toEqual([{ id: "sess-local-approval", title: "Run the example checks" }]);
    expect(status.error).toBeUndefined();
  });

  it("reports a newer version with its notes", async () => {
    const { client } = await setup(withWorld(new MockWorld({ latestVersion: "1.5.0" })));
    expect(await client.call("desktop.updateStatus")).toMatchObject({ current: "1.4.0", latest: "1.5.0", updateAvailable: true });
  });

  it("an unreadable feed gives latest = current and an error", async () => {
    const world = new MockWorld({ latestVersion: "1.5.0" });
    world.updateFeedError = "Could not read the update feed";
    const { client } = await setup(withWorld(world));
    expect(await client.call("desktop.updateStatus")).toMatchObject({ latest: "1.4.0", updateAvailable: false, error: "Could not read the update feed" });
  });

  it("a read-only device may read it", async () => {
    const { client } = await setup({ permission: "read" });
    expect((await client.request("desktop.updateStatus", {})).ok).toBe(true);
  });
});

describe("desktop.update", () => {
  it("answers up_to_date and starts nothing when there is no newer version", async () => {
    const world = new MockWorld();
    const { client } = await setup(withWorld(world));
    expect(await client.call("desktop.update", { confirm: true })).toMatchObject({ status: "up_to_date", current: "1.4.0" });
    expect(world.updatesStarted).toEqual([]);
  });

  it("never downgrades", async () => {
    const world = new MockWorld({ latestVersion: "1.3.0" });
    world.latestVersion = "1.4.0";
    const { client } = await setup(withWorld(world));
    expect((await client.call("desktop.update", { confirm: true })).status).toBe("up_to_date");
  });

  it("asks for confirmation with the running sessions and does nothing", async () => {
    const world = new MockWorld({ latestVersion: "1.5.0" });
    const { client } = await setup(withWorld(world));
    const result = await client.call("desktop.update", {});
    expect(result).toMatchObject({ status: "confirmation_required", runningSessions: [{ id: "sess-local-approval" }] });
    expect(world.updatesStarted).toEqual([]);
  });

  it("updates after confirm: true, and the version changes once the app relaunches", async () => {
    const world = new MockWorld({ latestVersion: "1.5.0" });
    const { client } = await setup(withWorld(world));
    expect(await client.call("desktop.update", { confirm: true })).toMatchObject({ status: "updating", from: "1.4.0", to: "1.5.0" });
    expect(world.updatesStarted).toEqual([{ from: "1.4.0", to: "1.5.0" }]);
    world.completeUpdate();
    expect(await client.call("desktop.updateStatus")).toMatchObject({ current: "1.5.0", updateAvailable: false });
  });

  it("updates without confirmation when no session runs", async () => {
    const world = new MockWorld({ latestVersion: "1.5.0" });
    world.session("sess-local-approval").busy = false;
    const { client } = await setup(withWorld(world));
    expect((await client.call("desktop.update", {})).status).toBe("updating");
  });

  it("a read-only device is denied and nothing starts", async () => {
    const world = new MockWorld({ latestVersion: "1.5.0" });
    const { client } = await setup({ permission: "read", ...withWorld(world) });
    expect(await client.request("desktop.update", { confirm: true })).toMatchObject({ ok: false, code: "read_only" });
    expect(world.updatesStarted).toEqual([]);
  });
});

describe("a failed install", () => {
  it("replies updating, then reports lastFailure while the link stays up, until the next install starts", async () => {
    const world = new MockWorld({ latestVersion: "1.5.0" });
    world.failNextUpdate = "disk full";
    const { client } = await setup(withWorld(world));
    expect(await client.call("desktop.update", { confirm: true })).toMatchObject({ status: "updating", to: "1.5.0" });
    const status = await client.call("desktop.updateStatus");
    expect(status).toMatchObject({ current: "1.4.0", updateAvailable: true, lastFailure: "Update to 1.5.0 failed: disk full" });
    expect(await client.call("desktop.update", { confirm: true })).toMatchObject({ status: "updating" });
    expect((await client.call("desktop.updateStatus")).lastFailure).toBeUndefined();
  });
});
