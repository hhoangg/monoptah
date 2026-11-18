import { QueryClient } from "@tanstack/react-query";
import { beforeEach, describe, expect, it, vi } from "vitest";

const grant = vi.hoisted(() => ({ requestOnce: vi.fn() }));
vi.mock("./useLazyGrant", () => ({ requestOnce: grant.requestOnce }));

import { createGrantHook } from "./preConnect";
import { markGrantRequested } from "./storage";

const queryClient = new QueryClient();
let hook = createGrantHook(queryClient);

beforeEach(() => {
  localStorage.clear();
  grant.requestOnce.mockReset();
  hook = createGrantHook(queryClient);
});

describe("pre-connect grant hook", () => {
  it("requests the grant first and lets the link go on when it exists", async () => {
    grant.requestOnce.mockResolvedValue("posted");
    expect(await hook({ desktopId: "d1", deviceId: "dev1" })).toBe("ok");
    expect(grant.requestOnce).toHaveBeenCalledWith(queryClient, "d1", "dev1");
  });

  it("asks again on the first connect of a load even when the marker is set, so a deleted pending grant is recreated", async () => {
    markGrantRequested("d1", "dev1");
    grant.requestOnce.mockResolvedValue("posted");
    expect(await hook({ desktopId: "d1", deviceId: "dev1" })).toBe("ok");
    expect(grant.requestOnce).toHaveBeenCalledTimes(1);
  });

  it("skips the request on a later reconnect of the same load when the marker is set", async () => {
    markGrantRequested("d1", "dev1");
    grant.requestOnce.mockResolvedValue("posted");
    await hook({ desktopId: "d1", deviceId: "dev1" });
    await hook({ desktopId: "d1", deviceId: "dev1" });
    expect(grant.requestOnce).toHaveBeenCalledTimes(1);
  });

  it("lets a desktop or device the server does not know go on, so the link ends quietly as before", async () => {
    grant.requestOnce.mockResolvedValue("missing");
    expect(await hook({ desktopId: "d1", deviceId: "dev1" })).toBe("ok");
  });

  it("denies a revoked grant", async () => {
    grant.requestOnce.mockResolvedValue("revoked");
    expect(await hook({ desktopId: "d1", deviceId: "dev1" })).toBe("denied");
  });

  it("lets the relay decide when the request itself failed", async () => {
    grant.requestOnce.mockResolvedValue("failed");
    expect(await hook({ desktopId: "d1", deviceId: "dev1" })).toBe("ok");
  });
});
