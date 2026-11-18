import { QueryClient } from "@tanstack/react-query";
import { beforeEach, describe, expect, it, vi } from "vitest";

const grant = vi.hoisted(() => ({ requestOnce: vi.fn() }));
vi.mock("./useLazyGrant", () => ({ requestOnce: grant.requestOnce }));

import { createGrantHook } from "./preConnect";
import { markGrantRequested } from "./storage";

const queryClient = new QueryClient();
const hook = createGrantHook(queryClient);

beforeEach(() => {
  localStorage.clear();
  grant.requestOnce.mockReset();
});

describe("pre-connect grant hook", () => {
  it("requests the grant first and lets the link go on when it exists", async () => {
    grant.requestOnce.mockResolvedValue("posted");
    expect(await hook({ desktopId: "d1", deviceId: "dev1" })).toBe("ok");
    expect(grant.requestOnce).toHaveBeenCalledWith(queryClient, "d1", "dev1");
  });

  it("skips the request when this device already asked", async () => {
    markGrantRequested("d1", "dev1");
    expect(await hook({ desktopId: "d1", deviceId: "dev1" })).toBe("ok");
    expect(grant.requestOnce).not.toHaveBeenCalled();
  });

  it("denies a revoked grant or an unknown device", async () => {
    grant.requestOnce.mockResolvedValue("revoked");
    expect(await hook({ desktopId: "d1", deviceId: "dev1" })).toBe("denied");
  });

  it("lets the relay decide when the request itself failed", async () => {
    grant.requestOnce.mockResolvedValue("failed");
    expect(await hook({ desktopId: "d1", deviceId: "dev1" })).toBe("ok");
  });
});
