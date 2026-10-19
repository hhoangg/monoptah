// @vitest-environment happy-dom
import { beforeEach, expect, it } from "vitest";
import { saveProviderSurface } from "../../providers/model/providerSurface";
import { setProjectDefaultProvider } from "../../sessions/model/projectProviders";
import { mobileNewSession } from "./mobileWorkspace";

beforeEach(() => localStorage.clear());

it("starts a phone session as a chat even when its provider opens in a terminal", () => {
  setProjectDefaultProvider("/project", "claude", "claude:opus-5");
  saveProviderSurface("claude", "tui");

  const session = mobileNewSession("/project", "auto");

  expect(session.harness).toBe("claude");
  expect(session.surface).toBe("chat");
  expect(session.runtimeMode).toBe("auto");
});
