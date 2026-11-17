import { afterEach, describe, expect, it, vi } from "vitest";
import { installTrustedTypesPolicy } from "./trusted-types";

type Options = NonNullable<Parameters<TrustedTypePolicyFactory["createPolicy"]>[1]>;

function installAndCapture(): { name: string; options: Options } {
  let captured: { name: string; options: Options } | undefined;
  window.trustedTypes = {
    createPolicy: (name: string, options?: Options) => {
      captured = { name, options: options ?? {} };
      return {};
    },
  };
  installTrustedTypesPolicy();
  if (!captured) throw new Error("no policy created");
  return captured;
}

afterEach(() => {
  delete window.trustedTypes;
  vi.restoreAllMocks();
});

describe("Trusted Types default policy", () => {
  it("is named default", () => {
    expect(installAndCapture().name).toBe("default");
  });

  it("blocks HTML and script sinks", () => {
    const { options } = installAndCapture();
    expect(() => options.createHTML?.("<img src=x onerror=alert(1)>")).toThrow("HTML sinks are disabled");
    expect(() => options.createScript?.("alert(1)")).toThrow("Script sinks are disabled");
  });

  it("allows /sw.js and same-origin /assets/*.js", () => {
    const { options } = installAndCapture();
    const origin = location.origin;
    expect(options.createScriptURL?.("/sw.js")).toBe(`${origin}/sw.js`);
    expect(options.createScriptURL?.("/assets/index-abc123.js")).toBe(`${origin}/assets/index-abc123.js`);
    expect(options.createScriptURL?.(`${origin}/assets/chunk.js`)).toBe(`${origin}/assets/chunk.js`);
  });

  it("blocks foreign origins, other paths and non-js assets", () => {
    const { options } = installAndCapture();
    expect(() => options.createScriptURL?.("https://evil.example/assets/x.js")).toThrow("Blocked script URL");
    expect(() => options.createScriptURL?.("https://evil.example/sw.js")).toThrow("Blocked script URL");
    expect(() => options.createScriptURL?.("/other.js")).toThrow("Blocked script URL");
    expect(() => options.createScriptURL?.("/assets/data.json")).toThrow("Blocked script URL");
    expect(() => options.createScriptURL?.("javascript:alert(1)")).toThrow("Blocked script URL");
    expect(() => options.createScriptURL?.("data:text/javascript,alert(1)")).toThrow("Blocked script URL");
  });

  it("does nothing where Trusted Types do not exist", () => {
    delete window.trustedTypes;
    expect(() => installTrustedTypesPolicy()).not.toThrow();
  });
});
