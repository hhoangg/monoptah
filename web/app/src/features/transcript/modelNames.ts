// D-12: the turn header shows the model's display name, not the internal id. The names come from the model
// catalog the app already loads (`models.list`); an id the catalog does not know is shown as it is.
import { createContext, createElement, use, useCallback, type ReactNode } from "react";
import { useModelCatalog } from "@/features/models/hooks";

export const ModelNamesContext = createContext<(id: string) => string>((id) => id);
export const useModelName = (id: string | undefined): string | undefined => {
  const resolve = use(ModelNamesContext);
  return id === undefined ? undefined : resolve(id);
};

/**
 * Provides the resolver over the catalog of the nearest link. Hosts that render a transcript inside a link (the
 * session and Mono screens) wrap it in this; a transcript rendered on its own keeps the identity default.
 */
export function ModelNamesProvider({ children }: { children: ReactNode }) {
  const models = useModelCatalog().data?.models;
  const resolve = useCallback((id: string) => models?.find((model) => model.id === id)?.name || id, [models]);
  return createElement(ModelNamesContext, { value: resolve }, children);
}
