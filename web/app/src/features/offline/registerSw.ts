// Registers /sw.js (production builds, or a dev run with VITE_REGISTER_SW=1, never in vitest) and wires
// the update prompt. Registration goes through the Trusted Types `default` policy of
// trusted-types.ts: under `require-trusted-types-for 'script'` the browser turns the string into a
// TrustedScriptURL with that policy, which allows exactly /sw.js (security.md 8.2).
import { Workbox } from "workbox-window";
import { showUpdateToast } from "./swUpdate";

export const SW_URL = "/sw.js";

export interface SwEnv {
  PROD?: boolean;
  VITE_REGISTER_SW?: string;
}

/** Production always; development only with the opt-in. A vitest run is neither (PROD false, no opt-in). */
export function shouldRegisterServiceWorker(env: SwEnv): boolean {
  return env.PROD === true || env.VITE_REGISTER_SW === "1";
}

/** Registers the worker and shows the update toast when a new version is waiting. Returns the Workbox handle. */
export function registerServiceWorker(): Workbox | undefined {
  if (typeof navigator === "undefined" || !("serviceWorker" in navigator)) return undefined;
  const workbox = new Workbox(SW_URL);
  // `waiting` fires for a worker found by this page and for one that was already waiting at load.
  workbox.addEventListener("waiting", () => {
    showUpdateToast(() => {
      // The page reloads when the new worker takes control, never before the user tapped Reload.
      workbox.addEventListener("controlling", () => window.location.reload());
      workbox.messageSkipWaiting();
    });
  });
  void workbox.register().catch(() => {
    // No worker (private mode, blocked): the app works online as before.
  });
  return workbox;
}
