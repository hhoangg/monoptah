// Order matters: the Trusted Types policy must exist before any other module runs (security.md 8.2).
import "./trusted-types-init";

import { QueryClientProvider } from "@tanstack/react-query";
import { RouterProvider } from "@tanstack/react-router";
import { MotionConfig } from "motion/react";
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { startAppearanceSync } from "@/lib/appearance";
import { queryClient } from "@/lib/queryClient";
import { Toaster } from "@/ui/sonner";
import { router } from "./router";
import "./styles/index.css";

// Slot registration contract (workstreams.md 3.2): one import line per owner, side effects only.
import "./features/cloud/register";
import "./features/desktops/register";
import "./features/layout/register";

startAppearanceSync();

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
