// @vitest-environment happy-dom
import { expect, it, vi } from "vitest";
import { newSession } from "./session";

// A partial platform mock, as other suites use. Building a chat session must
// not reach for exports such as IS_WIN that such a mock leaves out.
vi.mock("../../../platform/tauri/platform", () => ({ IS_MAC: true }));

it("builds a chat session without the platform's IS_WIN export", () => {
  expect(newSession("claude", "/repo").surface).toBe("chat");
});
