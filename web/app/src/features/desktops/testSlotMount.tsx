// Mounts one slot component inside a small router (`/d/$desktopId` plus the desktop routes) for the tests
// of the link-state screens and the switcher. Owner: WS-M. The tests mock `@/link`, so no connection runs.
import { QueryClientProvider } from "@tanstack/react-query";
import { createMemoryHistory, createRootRoute, createRoute, createRouter, Outlet, RouterProvider } from "@tanstack/react-router";
import { render } from "@testing-library/react";
import { StrictMode, type ReactElement } from "react";
import { createQueryClient } from "@/lib/queryClient";

export function mountSlot(element: ReactElement, url = "/d/d1", options: { strict?: boolean } = {}) {
  const queryClient = createQueryClient();
  const root = createRootRoute({ component: Outlet });
  const page = (path: string, body: ReactElement | null) => createRoute({ getParentRoute: () => root, path, component: () => body });
  const routeTree = root.addChildren([
    page("/d/$desktopId", element),
    page("/desktops", <p>Desktops page</p>),
    page("/desktops/$id", <p>Manage page</p>),
  ]);
  const router = createRouter({ routeTree, history: createMemoryHistory({ initialEntries: [url] }) });
  const tree = (
    <QueryClientProvider client={queryClient}>
      <RouterProvider router={router} />
    </QueryClientProvider>
  );
  render(options.strict ? <StrictMode>{tree}</StrictMode> : tree);
  return { queryClient, router };
}
