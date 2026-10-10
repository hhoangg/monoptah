import {
  hasProbedHarnessAvailability,
  isHarnessAvailable,
} from "../../../integrations/harness/core/availabilityState";
import { loadProviderSurface } from "../../providers/model/providerSurface";
import { isRemoteProjectPath } from "../../projects/model/recents";
import { defaultSessionChoice, showProviderInModelPicker } from "./models";
import { isProviderHidden } from "./projectProviders";
import { HARNESSES, type HarnessId } from "./session";

/** Providers the model picker would show for this project. */
export function newSessionProviders(
  cwd: string,
  available: (id: HarnessId) => boolean = isHarnessAvailable,
  probed: boolean = hasProbedHarnessAvailability(),
): HarnessId[] {
  return HARNESSES.filter(
    (id) =>
      showProviderInModelPicker(id, available(id), probed) &&
      !isProviderHidden(cwd, id),
  );
}

/** Move the highlight one row, wrapping like the model picker menus. */
export function moveHighlight(
  index: number,
  direction: 1 | -1,
  length: number,
): number {
  if (length === 0) return 0;
  return (index + direction + length) % length;
}

/** Cmd+T should ask which provider to use when the default one opens as a TUI,
 *  because a TUI session gives no in-app way to switch provider afterwards.
 *  With fewer than two providers there is nothing to choose, so it never asks. */
export function shouldPromptForProvider(
  cwd: string,
  available: (id: HarnessId) => boolean = isHarnessAvailable,
  probed: boolean = hasProbedHarnessAvailability(),
): boolean {
  if (isRemoteProjectPath(cwd)) return false;
  if (loadProviderSurface(defaultSessionChoice(cwd).harness) !== "tui") {
    return false;
  }
  return newSessionProviders(cwd, available, probed).length >= 2;
}
