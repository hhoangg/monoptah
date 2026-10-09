import type { HarnessId } from "../../../features/sessions/model/session";
import * as child from "./child";

// Looked up on `child` at call time so a user-configured binary path is read
// fresh and an isolated test can mock a single resolver.
const RESOLVERS: Record<
  Exclude<HarnessId, "antigravity">,
  () => Promise<{ path: string }>
> = {
  claude: () => child.resolveClaudeBinary(),
  codex: () => child.resolveCodexBinary(),
  cursor: () => child.resolveCursorBinary(),
  grok: () => child.resolveGrokBinary(),
  opencode: () => child.resolveOpenCodeBinary(),
  pi: () => child.resolvePiBinary(),
  omp: () => child.resolveOmpBinary(),
  fx: () => child.resolveFxBinary(),
  hermes: () => child.resolveHermesBinary(),
  devin: () => child.resolveDevinBinary(),
};

/**
 * Program that starts a provider's interactive CLI in a PTY.
 *
 * Antigravity is the exception: its resolver finds the ACP server the chat
 * path talks to, which waits for JSON-RPC and is not a terminal UI. The
 * interactive CLI is `agy`, found on PATH when the PTY starts.
 */
export async function resolveTuiBinary(harness: HarnessId): Promise<string> {
  if (harness === "antigravity") return "agy";
  const { path } = await RESOLVERS[harness]();
  return path;
}
