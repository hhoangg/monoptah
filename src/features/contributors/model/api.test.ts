import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

import { invoke } from "@tauri-apps/api/core";
import {
  cancelSync,
  clearSynced,
  firstCommit,
  gitUser,
  listBranches,
  loadStored,
  readConfig,
  readMailmap,
  startSync,
  syncEstimate,
  syncStatus,
  writeConfig,
} from "./api";

const mocked = vi.mocked(invoke);

beforeEach(() => {
  mocked.mockReset();
});

describe("contributors api", () => {
  it("reads the config text and passes null through for a missing file", async () => {
    mocked.mockResolvedValueOnce(null);
    await expect(readConfig("/repo")).resolves.toBeNull();
    expect(mocked).toHaveBeenCalledWith("contributors_read_config", {
      cwd: "/repo",
    });
  });

  it("reads the mailmap", async () => {
    mocked.mockResolvedValueOnce("A <a@x.io>");
    await expect(readMailmap("/repo")).resolves.toBe("A <a@x.io>");
    expect(mocked).toHaveBeenCalledWith("contributors_read_mailmap", {
      cwd: "/repo",
    });
  });

  it("writes the config content", async () => {
    mocked.mockResolvedValueOnce(undefined);
    await writeConfig("/repo", "{}\n");
    expect(mocked).toHaveBeenCalledWith("contributors_write_config", {
      cwd: "/repo",
      content: "{}\n",
    });
  });

  it("loads a period from the store with an explicit null branch", async () => {
    mocked.mockResolvedValueOnce({ head: "abc" });
    await loadStored("/repo", {
      since: "2026-09-01T00:00:00Z",
      until: "2026-10-01T00:00:00Z",
    });
    expect(mocked).toHaveBeenCalledWith("contributors_load", {
      cwd: "/repo",
      branch: null,
      since: "2026-09-01T00:00:00Z",
      until: "2026-10-01T00:00:00Z",
    });
  });

  it("forwards the branch of a load, an estimate and a sync start", async () => {
    mocked.mockResolvedValue(undefined);
    const window = { from: "2026-09-01T00:00:00Z", to: "2026-10-01T00:00:00Z" };
    await loadStored("/repo", {
      since: window.from,
      until: window.to,
      branch: "dev",
    });
    await syncEstimate("/repo", { ...window, branch: "dev" });
    await startSync("/repo", { ...window, branch: "dev", mode: "full" });
    expect(
      mocked.mock.calls.map(([, args]) => (args as { branch: string }).branch),
    ).toEqual(["dev", "dev", "dev"]);
  });

  it("estimates a period", async () => {
    mocked.mockResolvedValueOnce({
      commits: 5,
      storedCommits: 2,
      firstParent: 4,
      prs: null,
    });
    await expect(
      syncEstimate("/repo", {
        from: "2026-09-01T00:00:00Z",
        to: "2026-10-01T00:00:00Z",
      }),
    ).resolves.toMatchObject({ commits: 5, prs: null });
    expect(mocked).toHaveBeenCalledWith("contributors_sync_estimate", {
      cwd: "/repo",
      branch: null,
      from: "2026-09-01T00:00:00Z",
      to: "2026-10-01T00:00:00Z",
    });
  });

  it("starts a sync with explicit nulls for the optional arguments", async () => {
    mocked.mockResolvedValue(undefined);
    const window = { from: "2026-09-01T00:00:00Z", to: "2026-10-01T00:00:00Z" };
    await startSync("/repo", { ...window, mode: "full" });
    expect(mocked).toHaveBeenLastCalledWith("contributors_sync_start", {
      cwd: "/repo",
      branch: null,
      ...window,
      mode: "full",
      oldHead: null,
    });
    await startSync("/repo", {
      ...window,
      branch: "dev",
      mode: "topup",
      oldHead: "abc",
    });
    expect(mocked).toHaveBeenLastCalledWith("contributors_sync_start", {
      cwd: "/repo",
      branch: "dev",
      ...window,
      mode: "topup",
      oldHead: "abc",
    });
  });

  it("cancels, lists and clears", async () => {
    mocked.mockResolvedValue(undefined);
    await cancelSync("/repo");
    expect(mocked).toHaveBeenLastCalledWith("contributors_sync_cancel", {
      cwd: "/repo",
    });
    mocked.mockResolvedValueOnce([]);
    await expect(syncStatus()).resolves.toEqual([]);
    expect(mocked).toHaveBeenLastCalledWith("contributors_sync_status");
    await clearSynced("/repo");
    expect(mocked).toHaveBeenLastCalledWith("contributors_clear", {
      cwd: "/repo",
    });
  });

  it("lists branches", async () => {
    mocked.mockResolvedValueOnce({
      current: "main",
      branches: ["main", "dev"],
    });
    await expect(listBranches("/repo")).resolves.toEqual({
      current: "main",
      branches: ["main", "dev"],
    });
    expect(mocked).toHaveBeenCalledWith("contributors_branches", {
      cwd: "/repo",
    });
  });

  it("reads the git user", async () => {
    mocked.mockResolvedValueOnce({ name: "Ana", email: "ana@example.com" });
    await expect(gitUser("/repo")).resolves.toEqual({
      name: "Ana",
      email: "ana@example.com",
    });
    expect(mocked).toHaveBeenCalledWith("contributors_git_user", {
      cwd: "/repo",
    });
  });

  it("reads the first commit date of a branch, null for none", async () => {
    mocked.mockResolvedValueOnce("2023-05-17T15:40:00+02:00");
    await expect(firstCommit("/repo", "dev")).resolves.toBe(
      "2023-05-17T15:40:00+02:00",
    );
    expect(mocked).toHaveBeenLastCalledWith("contributors_first_commit", {
      cwd: "/repo",
      branch: "dev",
    });
    mocked.mockResolvedValueOnce(null);
    await expect(firstCommit("/repo")).resolves.toBeNull();
    expect(mocked).toHaveBeenLastCalledWith("contributors_first_commit", {
      cwd: "/repo",
      branch: null,
    });
  });

  it("propagates a rejected command", async () => {
    mocked.mockRejectedValueOnce("not a git repository");
    await expect(readConfig("/repo")).rejects.toBe("not a git repository");
  });
});
