import { beforeEach, describe, expect, it, vi } from "vitest";

const lib = vi.hoisted(() => ({
  startRegistration: vi.fn(),
  startAuthentication: vi.fn(),
  browserSupportsWebAuthn: vi.fn(() => true),
}));
vi.mock("@simplewebauthn/browser", () => lib);

import { createPasskey, getPasskey, passkeysSupported } from "./webauthn";

function domError(name: string): Error {
  const error = new Error(name);
  error.name = name;
  return error;
}

beforeEach(() => vi.clearAllMocks());

describe("webauthn helper", () => {
  it("passes the server options JSON to the library and returns the response", async () => {
    lib.startRegistration.mockResolvedValue({ id: "c", type: "public-key" });
    const result = await createPasskey({ challenge: "abc" });
    expect(lib.startRegistration).toHaveBeenCalledWith({ optionsJSON: { challenge: "abc" } });
    expect(result).toEqual({ status: "ok", response: { id: "c", type: "public-key" } });
  });

  it.each([domError("NotAllowedError"), domError("AbortError"), Object.assign(new Error("x"), { code: "ERROR_CEREMONY_ABORTED" }), Object.assign(new Error("x"), { cause: domError("NotAllowedError") })])(
    "maps a user cancel to a cancelled result (%#)",
    async (error) => {
      lib.startAuthentication.mockRejectedValue(error);
      lib.startRegistration.mockRejectedValue(error);
      expect(await getPasskey({ challenge: "abc" })).toEqual({ status: "cancelled" });
      expect(await createPasskey({ challenge: "abc" })).toEqual({ status: "cancelled" });
    },
  );

  it("rethrows other failures", async () => {
    lib.startAuthentication.mockRejectedValue(domError("SecurityError"));
    await expect(getPasskey({ challenge: "abc" })).rejects.toThrow("SecurityError");
  });

  it("reports support from the library", () => {
    lib.browserSupportsWebAuthn.mockReturnValueOnce(false);
    expect(passkeysSupported()).toBe(false);
    expect(passkeysSupported()).toBe(true);
  });
});
