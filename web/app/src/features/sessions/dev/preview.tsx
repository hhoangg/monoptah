// Dev-only preview of the real router against an in-browser MockDesktop. Served by `vite dev` at
// /src/features/sessions/dev/. Vite builds only the root index.html, so this page never
// reaches app/dist (the build step `scripts/check-dist.mjs` and the guard in the report check it).
//
// Usage (from web/app):
//   VITE_MOCK_API=1 pnpm exec vite --port 5173 --strictPort
//   http://localhost:5173/src/features/sessions/dev/?to=/d/<id>/p/proj-alpha&theme=dark
// Query:
//   to=/d/<id>/...        the first route (default: `/d/<DESKTOP_ID>`); DESKTOP_ID is in dev/mockLink.ts
//   theme=light|dark      the colour scheme (default light)
//   permission=read       the device is read-only (default control)
//   empty=1               an empty workspace (the F-14 empty state)
//   sessions=N&working=N  how many extra sessions the seed adds (default 45) and how many of them work (default 2)
//   turns=N&tail=N        the long conversation `sess-long` (open it with ?to=/d/<id>/s/sess-long): N turns (default 200),
//                         the last `tail` (default 24) sent on open and the rest paged in by conversation.older
// Screenshots: `node /tmp/ws-t/shoot.mjs --url "/d/<id>/p/proj-alpha" --name W-06/project-list` writes the
// six PNGs (390x844, 768x1024, 1280x800, light and dark) to /tmp/evidence/ws-t/W-06/.
//
// The accounts mock API replaces `/api` so the auth guard passes; the desktop link is the real link
// client over an InMemoryRelay to a MockDesktop seeded with extra projects, Monos and sessions.
import "@/trusted-types-init";
import { QueryClientProvider } from "@tanstack/react-query";
import { RouterProvider } from "@tanstack/react-router";
import { MotionConfig } from "motion/react";
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { installMockApi } from "@/features/accounts/mock";
import "@/styles/index.css";
import { seedLongSession } from "./longSession";
import { DESKTOP_ID, connectMockDesktop, emptyWorkspace, seedWorkspace } from "./mockLink";

const params = new URLSearchParams(window.location.search);
const theme = params.get("theme") === "dark" ? "dark" : "light";
const count = (name: string, fallback: number) => {
  const value = Number(params.get(name));
  return params.has(name) && Number.isFinite(value) ? value : fallback;
};

// The signed-in mock account, before the router asks `/api/me`.
sessionStorage.setItem("monoptah.mock.signedIn", "1");
installMockApi();

connectMockDesktop({
  permission: params.get("permission") === "read" ? "read" : "control",
  seed: (desktop) => {
    if (params.get("empty") === "1") emptyWorkspace(desktop);
    else {
      seedWorkspace(desktop, { sessions: count("sessions", 45), working: count("working", 2) });
      seedLongSession(desktop, { total: count("turns", 200), tail: count("tail", 24) });
    }
  },
});

// The router reads the location when it is created, so the target is set first and the router is
// imported afterwards.
const target = params.get("to");
window.history.replaceState(null, "", target && target.startsWith("/") && !target.startsWith("//") ? target : `/d/${DESKTOP_ID}`);

async function start() {
  const { useAppearance } = await import("@/lib/appearance");
  useAppearance.getState().setAppearance(theme);
  const [{ router }, { queryClient }, { installSessionExpiryHandler }, { Toaster }] = await Promise.all([
    import("@/router"),
    import("@/lib/queryClient"),
    import("@/features/accounts/sessionExpiry"),
    import("@/ui/sonner"),
  ]);
  installSessionExpiryHandler(queryClient, router);
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
