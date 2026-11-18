// Which /d/:desktopId routes already draw their own sidebar.
//
// TEMPORARY BRIDGE: WS-T's `WorkspaceFrame` (features/sessions/WorkspaceFrame.tsx) renders its own
// `aside` with `WorkspaceNav` from 640px up on the home, project and cloud screens. The shell draws the
// same sidebar for every other route, so it skips it on exactly these (no double sidebar). When WS-T
// removes the `aside` from `WorkspaceFrame`, empty `FRAME_OWNED_ROUTES` and delete this bridge.
const FRAME_OWNED_ROUTES: readonly RegExp[] = [
  /^\/d\/[^/]+$/, // home
  /^\/d\/[^/]+\/p\/[^/]+$/, // project
  /^\/d\/[^/]+\/cloud$/, // cloud
];

/** True when the screen at `pathname` renders `WorkspaceFrame` and so its own sidebar. */
export function routeHasOwnSidebar(pathname: string): boolean {
  const path = pathname.length > 1 ? pathname.replace(/\/+$/, "") : pathname;
  return FRAME_OWNED_ROUTES.some((pattern) => pattern.test(path));
}
