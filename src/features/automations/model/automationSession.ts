import { TUI_CAPS } from "../../../integrations/harness/core/tuiLaunch";
import { isRemoteProjectPath } from "../../projects/model/recents";
import {
  loadProviderSurface,
  type SessionSurface,
} from "../../providers/model/providerSurface";
import type { HarnessId } from "../../sessions/model/session";
import { tuiExitNotice, type TuiExit } from "../../sessions/model/tuiSession";

/**
 * The surface an automation run opens in. A run follows the provider's
 * "Session surface" setting, but only where a terminal can honour the run.
 */
export function automationRunSurface(
  harness: HarnessId,
  cwd: string,
): SessionSurface {
  return loadProviderSurface(harness) === "tui" &&
    // A CLI that cannot take a first prompt would silently drop the prompt.
    TUI_CAPS[harness].initialPrompt &&
    // A remote project has no local terminal to run the CLI in.
    !isRemoteProjectPath(cwd)
    ? "tui"
    : "chat";
}

/** The error stored on a terminal run whose CLI died or never started. */
export function tuiRunError(harness: HarnessId, exit: TuiExit): string {
  const notice = tuiExitNotice(harness, exit);
  return notice.hint ? `${notice.message}. ${notice.hint}` : notice.message;
}
