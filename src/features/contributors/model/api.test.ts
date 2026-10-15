import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

import { invoke } from "@tauri-apps/api/core";
import {
  collectRaw,
  gitUser,
  listBranches,
  readConfig,
  readMailmap,
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

  it("collects raw data with explicit nulls for omitted options", async () => {
    mocked.mockResolvedValueOnce({ head: "abc" });
    await collectRaw("/repo", { since: "2026-09-01T00:00:00Z" });
    expect(mocked).toHaveBeenCalledWith("contributors_collect", {
      cwd: "/repo",
      since: "2026-09-01T00:00:00Z",
      until: null,
      branch: null,
      force: false,
    });
  });

  it("forwards until, branch and force", async () => {
    mocked.mockResolvedValueOnce({ head: "abc" });
    await collectRaw("/repo", {
      since: "2026-09-01T00:00:00Z",
      until: "2026-10-01T00:00:00Z",
      branch: "dev",
      force: true,
    });
    expect(mocked).toHaveBeenCalledWith("contributors_collect", {
      cwd: "/repo",
      since: "2026-09-01T00:00:00Z",
      until: "2026-10-01T00:00:00Z",
      branch: "dev",
      force: true,
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

  it("propagates a rejected command", async () => {
    mocked.mockRejectedValueOnce("not a git repository");
    await expect(readConfig("/repo")).rejects.toBe("not a git repository");
  });
});
