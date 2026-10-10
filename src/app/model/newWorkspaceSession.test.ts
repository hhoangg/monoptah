// @vitest-environment happy-dom
import { afterEach, beforeEach, expect, it } from "vitest";
import {
  setProjectDefaultModel,
  setProjectDefaultProvider,
} from "../../features/sessions/model/projectProviders";
import { saveProviderSurface } from "../../features/providers/model/providerSurface";
import { newSession } from "../../features/sessions/model/session";
import { setWorktreeFocus } from "../../features/source-control/model/worktreeFocus";
import { newWorkspaceSession } from "./newWorkspaceSession";

beforeEach(() => localStorage.clear());
afterEach(() => {
  setWorktreeFocus("/current", undefined);
  setWorktreeFocus("/other", undefined);
});

it("creates the first session in the visible project despite defaults from another project", () => {
  const defaults = newSession("claude", "/other", undefined, "auto");
  setProjectDefaultProvider("/current", "codex", "codex:gpt-5.4");
  setWorktreeFocus("/other", { path: "/other-tree", branch: "other" });

  const session = newWorkspaceSession("/current", defaults.runtimeMode);

  expect(session.cwd).toBe("/current");
  expect(session.harness).toBe("codex");
  expect(session.runtimeMode).toBe("auto");
  expect(session.worktreeCwd).toBeUndefined();
  expect(session.blocks).toEqual([]);
});

it("uses the visible project's selected worktree for its first session", () => {
  setWorktreeFocus("/current", { path: "/current-tree", branch: "feature" });

  const session = newWorkspaceSession("/current");

  expect(session.cwd).toBe("/current");
  expect(session.worktreeCwd).toBe("/current-tree");
  expect(session.branch).toBe("feature");
});

it("starts in the project root without session defaults or a separate worktree", () => {
  setWorktreeFocus("/current", { path: "/current", branch: "main" });

  const session = newWorkspaceSession("/current");

  expect(session.cwd).toBe("/current");
  expect(session.runtimeMode).toBe("supervised");
  expect(session.worktreeCwd).toBeUndefined();
});

it("uses an explicit provider with its own surface preference and project model", () => {
  setProjectDefaultProvider("/current", "codex", "codex:gpt-5.4");
  setProjectDefaultModel("/current", "claude", "claude:opus-5");
  saveProviderSurface("claude", "tui");
  saveProviderSurface("cursor", "chat");

  const tui = newWorkspaceSession("/current", "auto", "claude");
  expect(tui.harness).toBe("claude");
  expect(tui.surface).toBe("tui");
  expect(tui.model).toBe("claude:opus-5");
  expect(tui.runtimeMode).toBe("auto");
  expect(tui.cwd).toBe("/current");

  const chat = newWorkspaceSession("/current", undefined, "cursor");
  expect(chat.harness).toBe("cursor");
  expect(chat.surface).toBe("chat");
  expect(chat.runtimeMode).toBe("supervised");
});
