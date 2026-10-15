import { invoke } from "@tauri-apps/api/core";
import type { RawData } from "./types";

// Plain `invoke`: these commands read the local repository and are not routed
// to remote machines.

export type ContributorsBranches = { current: string; branches: string[] };
export type GitUser = { name: string; email: string };

export type CollectOptions = {
  since: string;
  until?: string;
  branch?: string;
  /** Skip the Rust cache and re-run git and gh. */
  force?: boolean;
};

/** The config file text, or null when the file does not exist. */
export function readConfig(cwd: string): Promise<string | null> {
  return invoke<string | null>("contributors_read_config", { cwd });
}

/** The repo `.mailmap` text, or null when the file does not exist. */
export function readMailmap(cwd: string): Promise<string | null> {
  return invoke<string | null>("contributors_read_mailmap", { cwd });
}

export function writeConfig(cwd: string, text: string): Promise<void> {
  return invoke<void>("contributors_write_config", { cwd, content: text });
}

export function collectRaw(
  cwd: string,
  { since, until, branch, force }: CollectOptions,
): Promise<RawData> {
  return invoke<RawData>("contributors_collect", {
    cwd,
    since,
    until: until ?? null,
    branch: branch ?? null,
    force: force ?? false,
  });
}

export function listBranches(cwd: string): Promise<ContributorsBranches> {
  return invoke<ContributorsBranches>("contributors_branches", { cwd });
}

/** `git config user.name` and `user.email`; empty strings when unset. */
export function gitUser(cwd: string): Promise<GitUser> {
  return invoke<GitUser>("contributors_git_user", { cwd });
}
