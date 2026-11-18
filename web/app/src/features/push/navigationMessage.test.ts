import { describe, expect, it, vi } from "vitest";
import { NAVIGATE_MESSAGE_TYPE } from "@/sw/pushHandlers";
import { installNavigationListener, navigationTarget } from "./navigationMessage";

describe("navigationTarget", () => {
  it("accepts a navigate message with a safe URL", () => {
    expect(navigationTarget({ type: NAVIGATE_MESSAGE_TYPE, url: "/d/dk1/monos/m7" })).toBe("/d/dk1/monos/m7");
  });

  it("accepts the root, which a generic notice opens", () => {
    expect(navigationTarget({ type: NAVIGATE_MESSAGE_TYPE, url: "/" })).toBe("/");
  });

  it.each([
    { type: NAVIGATE_MESSAGE_TYPE, url: "//evil.example" },
    { type: NAVIGATE_MESSAGE_TYPE, url: "https://evil.example/d/x" },
    { type: NAVIGATE_MESSAGE_TYPE },
    { type: "SKIP_WAITING", url: "/d/dk1/s/s9" },
    null,
    "text",
  ])("ignores %j", (message) => {
    expect(navigationTarget(message)).toBeNull();
  });
});

describe("installNavigationListener", () => {
  it("routes safe messages and stops after cleanup", () => {
    const source = new EventTarget();
    const navigate = vi.fn();
    const stop = installNavigationListener(navigate, source);
    source.dispatchEvent(new MessageEvent("message", { data: { type: NAVIGATE_MESSAGE_TYPE, url: "/d/dk1/s/s9" } }));
    source.dispatchEvent(new MessageEvent("message", { data: { type: NAVIGATE_MESSAGE_TYPE, url: "/\\evil" } }));
    expect(navigate).toHaveBeenCalledExactlyOnceWith("/d/dk1/s/s9");
    stop();
    source.dispatchEvent(new MessageEvent("message", { data: { type: NAVIGATE_MESSAGE_TYPE, url: "/d/dk1/s/s9" } }));
    expect(navigate).toHaveBeenCalledOnce();
  });
});
