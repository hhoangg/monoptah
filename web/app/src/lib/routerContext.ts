import type { QueryClient } from "@tanstack/react-query";

/** Router context available to every `beforeLoad` and `loader`. */
export interface RouterContext {
  queryClient: QueryClient;
}
