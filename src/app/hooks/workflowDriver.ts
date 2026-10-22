import { getCurrentWindow } from "@tauri-apps/api/window";

/**
 * Only the main window drives workflows. Every native window runs the whole app, but a run
 * lives in memory of the window that started it, so a second driver would sweep that run to
 * "interrupted", start a second run of the same workflow and be unable to stop the first.
 * Other windows ask the driver through the workflow files instead.
 */
export const DRIVER_WINDOW_LABEL = "main";

export function isDriverWindow(label: string | undefined): boolean {
  return label === DRIVER_WINDOW_LABEL;
}

/** Whether this window is the driver; false outside the desktop shell. */
export function currentWindowDrivesWorkflows(): boolean {
  try {
    return isDriverWindow(getCurrentWindow().label);
  } catch {
    return false;
  }
}
