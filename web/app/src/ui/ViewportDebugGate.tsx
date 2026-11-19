// TEMPORARY: renders the viewport debug overlay only when enabled; otherwise nothing, and the overlay chunk never loads.
import { lazy, Suspense, useState } from "react";
import { viewportDebugEnabled } from "./viewportDebugFlag";

const ViewportDebug = lazy(() => import("./ViewportDebug"));

export function ViewportDebugGate() {
  const [enabled] = useState(() => viewportDebugEnabled());
  return enabled ? (
    <Suspense fallback={null}>
      <ViewportDebug />
    </Suspense>
  ) : null;
}
