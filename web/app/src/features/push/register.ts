// Startup wiring of P-04, imported for its side effects by one line in src/main.tsx. A notification click either
// arrives as a message from the service worker (a window that is open) or waits as a pending target that the page
// takes when it starts and whenever it becomes visible again (cold start, iOS standalone).
import { router } from "@/router";
import { idbPendingStore } from "@/sw/pendingTarget";
import { installNavigationListener } from "./navigationMessage";
import { consumePendingTarget, createTapGate, type LaunchDeps } from "./launchTarget";

const store = idbPendingStore(typeof indexedDB === "undefined" ? undefined : indexedDB);

/** One tap can arrive as a message and as the saved target (an iOS resume): it routes once, a new tap to the same URL routes again. */
const taps = createTapGate();

function goTo(url: string, at?: number): Promise<unknown> | void {
  if (!taps.first(at)) return;
  if (router.state.location.href === url) return;
  return router.navigate({ href: url });
}

installNavigationListener((url, at) => {
  void Promise.resolve(goTo(url, at)).then(
    // The message carried the target, so the saved copy for this URL must not route a second time (a newer tap stays).
    () => store.clearIf(url),
    () => store.clearIf(url),
  );
});

/** Resolves once the mounted router has finished its first load (the `/` redirect included), so a navigation made now is not replaced by it. */
function routerResolved(): Promise<void> {
  if (router.state.resolvedLocation !== undefined && router.state.status === "idle") return Promise.resolve();
  return new Promise((resolve) => {
    const stop = router.subscribe("onResolved", () => {
      stop();
      resolve();
    });
  });
}

const deps: LaunchDeps = {
  store,
  ready: routerResolved,
  currentHref: () => router.state.location.href,
  navigate: goTo,
  now: () => Date.now(),
};

function consume(): void {
  void consumePendingTarget(deps).catch(() => undefined);
}

consume();
if (typeof document !== "undefined") {
  document.addEventListener("visibilitychange", () => {
    if (document.visibilityState === "visible") consume();
  });
}
