// Order matters: the Trusted Types policy must exist before any other module runs (security.md 8.2).
import "./trusted-types-init";

import { QueryClientProvider } from "@tanstack/react-query";
import { RouterProvider } from "@tanstack/react-router";
import { MotionConfig } from "motion/react";
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { installSessionExpiryHandler } from "@/features/accounts/sessionExpiry";
import { startAppearanceSync } from "@/lib/appearance";
import { queryClient } from "@/lib/queryClient";
import { Toaster } from "@/ui/sonner";
import { viewportDebugEnabled } from "@/ui/viewportDebugFlag";
import { router } from "./router";
import "./styles/index.css";

// Slot registration contract (workstreams.md 3.2): one import line per owner, side effects only.
import "./features/cloud/register";
import "./features/desktops/register";
import "./features/layout/register";
import "./features/monos/register";
import "./features/offline/register";
import "./features/push/register";
import "./features/settings/register";

startAppearanceSync();
// TEMPORARY: read `?debug=viewport` before the router can redirect it away (`/` redirects to the last place).
viewportDebugEnabled();
installSessionExpiryHandler(queryClient, router);

async function start() {
  // Dev only: the mock API replaces the network. The condition is a build-time constant, so the
  // dynamic import (and the whole mock) is removed from production builds.
  if (import.meta.env.DEV && import.meta.env.VITE_MOCK_API === "1") {
    const { installMockApi } = await import("./features/accounts/mock");
    installMockApi();
    const { installMockDesktopLink } = await import("./features/settings/mockDesktopLink");
    installMockDesktopLink();
  }

  const container = document.getElementById("root");
  if (!container) throw new Error("Missing #root element");

  createRoot(container).render(
    <StrictMode>
      <MotionConfig reducedMotion="user">
        <QueryClientProvider client={queryClient}>
          <RouterProvider router={router} />
          <Toaster />
        </QueryClientProvider>
      </MotionConfig>
    </StrictMode>,
  );
}

void start();
