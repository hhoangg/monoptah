import { describe, expect, it } from "vitest";
import { registerServiceWorker, shouldRegisterServiceWorker } from "./registerSw";

describe("service worker registration gate", () => {
  it("registers in production", () => {
    expect(shouldRegisterServiceWorker({ PROD: true })).toBe(true);
  });

  it("does not register in development or in vitest without the opt-in", () => {
    expect(shouldRegisterServiceWorker({ PROD: false })).toBe(false);
    expect(shouldRegisterServiceWorker(import.meta.env)).toBe(false);
  });

  it("registers in development with VITE_REGISTER_SW=1", () => {
    expect(shouldRegisterServiceWorker({ PROD: false, VITE_REGISTER_SW: "1" })).toBe(true);
    expect(shouldRegisterServiceWorker({ PROD: false, VITE_REGISTER_SW: "0" })).toBe(false);
  });

  it("does nothing where service workers do not exist (jsdom)", () => {
    expect(registerServiceWorker()).toBeUndefined();
  });
});
