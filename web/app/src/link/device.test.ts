import { IDBFactory } from "fake-indexeddb";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type * as Device from "./device.ts";

async function freshModule(): Promise<typeof Device> {
  vi.resetModules();
  return import("./device.ts");
}

beforeEach(() => {
  globalThis.indexedDB = new IDBFactory();
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("deviceNameFromUserAgent", () => {
  it("names the browser and the OS", async () => {
    const { deviceNameFromUserAgent } = await freshModule();
    expect(deviceNameFromUserAgent("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0 Safari/537.36")).toBe("Chrome on macOS");
    expect(deviceNameFromUserAgent("Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.0 Mobile/15E148 Safari/604.1")).toBe("Safari on iOS");
    expect(deviceNameFromUserAgent("Mozilla/5.0 (X11; Linux x86_64; rv:128.0) Gecko/20100101 Firefox/128.0")).toBe("Firefox on Linux");
    expect(deviceNameFromUserAgent("")).toBe("Browser");
  });
});

describe("ensureDevice", () => {
  it("creates the key and registers once, then reuses the stored server id", async () => {
    const { ensureDevice, setDeviceRegistrar } = await freshModule();
    const registrar = vi.fn(async () => "33333333-3333-4333-8333-333333333333");
    setDeviceRegistrar(registrar);
    const [a, b] = await Promise.all([ensureDevice(), ensureDevice()]);
    expect(registrar).toHaveBeenCalledTimes(1);
    expect(a.deviceId).toBe("33333333-3333-4333-8333-333333333333");
    expect(b.deviceId).toBe(a.deviceId);
    expect(a.publicKey).toHaveLength(32);
    expect(a.fingerprint).toMatch(/^[0-9a-f]{64}$/);

    const again = await ensureDevice();
    expect(registrar).toHaveBeenCalledTimes(1);
    expect(again.deviceId).toBe(a.deviceId);
    expect(again.fingerprint).toBe(a.fingerprint);

    // A fresh module (page reload) reads the id from IndexedDB and does not register again.
    const reloaded = await freshModule();
    const second = vi.fn(async () => "44444444-4444-4444-8444-444444444444");
    reloaded.setDeviceRegistrar(second);
    const after = await reloaded.ensureDevice();
    expect(second).not.toHaveBeenCalled();
    expect(after.deviceId).toBe(a.deviceId);
  });

  it("a failed registration is not remembered: the next call registers again", async () => {
    const { ensureDevice, setDeviceRegistrar } = await freshModule();
    const registrar = vi
      .fn<Device.DeviceRegistrar>()
      .mockRejectedValueOnce(new Error("network"))
      .mockResolvedValueOnce("55555555-5555-4555-8555-555555555555");
    setDeviceRegistrar(registrar);
    await expect(ensureDevice()).rejects.toThrow("network");
    await expect(ensureDevice()).resolves.toMatchObject({ deviceId: "55555555-5555-4555-8555-555555555555" });
    expect(registrar).toHaveBeenCalledTimes(2);
  });

  it("the default registrar posts the public key with the CSRF header and cookies", async () => {
    const { ensureDevice } = await freshModule();
    const fetchMock = vi.fn(async () => new Response(JSON.stringify({ deviceId: "66666666-6666-4666-8666-666666666666" }), { status: 201 }));
    vi.stubGlobal("fetch", fetchMock);
    const device = await ensureDevice();
    expect(device.deviceId).toBe("66666666-6666-4666-8666-666666666666");
    expect(fetchMock).toHaveBeenCalledTimes(1);
    const [url, init] = fetchMock.mock.calls[0] as unknown as [string, RequestInit];
    expect(url).toBe("/api/devices");
    expect(init.method).toBe("POST");
    expect(init.credentials).toBe("same-origin");
    expect(init.headers).toEqual({ "Content-Type": "application/json", "X-Monoptah-CSRF": "1" });
    const body = JSON.parse(init.body as string) as { name: string; publicKey: string };
    expect(Object.keys(body).sort()).toEqual(["name", "publicKey"]);
    expect(body.publicKey).not.toMatch(/[+/=]/);
  });

  it("a rejected registration surfaces the status", async () => {
    const { ensureDevice, DeviceRegistrationError } = await freshModule();
    vi.stubGlobal("fetch", vi.fn(async () => new Response("{}", { status: 401 })));
    await expect(ensureDevice()).rejects.toBeInstanceOf(DeviceRegistrationError);
    await expect(ensureDevice()).rejects.toMatchObject({ status: 401 });
  });
});
