// Feature flag of the web approval card (U-08). It is OFF by default: remote approval turns an XSS into
// direct command execution on the desktop, so it ships only after the U-13 security review gate passes.
// Flipping the default (or setting VITE_WEB_APPROVALS=on in a release build) requires U-13 to be recorded
// as passed in the PR. Until then the card renders nothing and the desktop notice stays the only path.
export function isApprovalCardEnabled(): boolean {
  return import.meta.env.VITE_WEB_APPROVALS === "on";
}
