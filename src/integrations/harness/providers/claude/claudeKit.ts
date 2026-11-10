/**
 * The Claude "kit" a Host applied (skills, plugins, MCP servers and their
 * secrets). The app never registers a provider, so only the Host reaches it.
 * This module has no Node imports because it is part of the shared bundle.
 */
export interface ClaudeKit {
  /** Content hash of the applied kit. Part of the process settings key. */
  hash: string;
  /** Hash of the secrets file. A change respawns the process. */
  secretsHash: string;
  /** One `--plugin-dir` per plugin. */
  pluginDirs: string[];
  /** Path for `--mcp-config`, when the kit carries MCP servers. */
  mcpConfig?: string;
  /** Environment overlay for the interactive Claude child only. */
  env: Record<string, string>;
}

let provider: (() => ClaudeKit | undefined) | undefined;

export function setClaudeKitProvider(
  next: (() => ClaudeKit | undefined) | undefined,
): void {
  provider = next;
}

/** The kit for the next interactive spawn, or undefined when none is applied. */
export function currentClaudeKit(): ClaudeKit | undefined {
  return provider?.();
}

/** The part of the kit that decides whether a live process must respawn. */
export function claudeKitKey(kit: ClaudeKit | undefined): string {
  return kit ? `${kit.hash}.${kit.secretsHash}` : "";
}
