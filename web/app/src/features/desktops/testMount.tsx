// Mounts the real app router on a memory history for the desktop screen tests. Owner: WS-M.
import { QueryClientProvider } from "@tanstack/react-query";
import { createMemoryHistory, RouterProvider } from "@tanstack/react-router";
import { render } from "@testing-library/react";
import { StrictMode } from "react";
import { createQueryClient } from "@/lib/queryClient";
import { createAppRouter } from "@/router";

export function mountApp(url: string, options: { strict?: boolean } = {}) {
  const queryClient = createQueryClient();
  const router = createAppRouter({ history: createMemoryHistory({ initialEntries: [url] }), queryClient });
  const tree = (
    <QueryClientProvider client={queryClient}>
      <RouterProvider router={router} />
    </QueryClientProvider>
  );
  render(options.strict ? <StrictMode>{tree}</StrictMode> : tree);
  return { queryClient, router };
}
