import { QueryClient } from "@tanstack/react-query";

/**
 * Shared TanStack Query client (architecture.md 3.2).
 *
 * Defaults suit Worker API data (`['me']`, `['desktops']`, ...). The desktop link keys
 * (`['link', desktopId, method, params]`) get `staleTime: Infinity`, `networkMode: 'always'` and
 * `gcTime` 10 min from the link adapters in W-05, set per query there, not here.
 */
export function createQueryClient(): QueryClient {
  return new QueryClient({
    defaultOptions: {
      queries: {
        staleTime: 30_000,
        gcTime: 5 * 60_000,
        retry: 1,
        refetchOnWindowFocus: false,
      },
      mutations: { retry: 0 },
    },
  });
}

export const queryClient = createQueryClient();
