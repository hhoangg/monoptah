// Feature flag of the web approval card (U-08). It is ON by default since the U-13 security review gate
// passed (2026-11-18). The card still renders only when the desktop advertises the `approvals` capability,
// which it does only while its own "Remote approvals" toggle is on (default off). VITE_WEB_APPROVALS=off
// stays as a kill switch for a build: the card then renders nothing and the desktop notice is the only path.
export function isApprovalCardEnabled(): boolean {
  return import.meta.env.VITE_WEB_APPROVALS !== "off";
}
