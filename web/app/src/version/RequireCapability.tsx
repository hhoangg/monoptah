import type { ReactNode } from "react";
import { FEATURES, useCapability, type KnownCapability } from "./capabilities";

export interface RequireCapabilityProps {
  name: KnownCapability;
  /** `hide` renders nothing when the capability is missing; `notice` renders the update hint. */
  fallback: "hide" | "notice";
  children: ReactNode;
}

/** Renders `children` only when the desktop advertises `name` (W-22). */
export function RequireCapability({ name, fallback, children }: RequireCapabilityProps) {
  const available = useCapability(name);
  if (available) return <>{children}</>;
  if (fallback === "hide") return null;
  return (
    <p data-testid="capability-notice" data-capability={name} className="text-small text-muted-text">
      Update Monoptah on your desktop to use {FEATURES[name]}
    </p>
  );
}
