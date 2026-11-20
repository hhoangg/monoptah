// Query options of the per-desktop settings. Keys sit under `["settings", "desktop", id]` so they
// never clash with the data layer keys of W-05 (`["link", id, ...]`); the link is passed in because
// it comes from the registry of desktopLink.ts.
import { queryOptions } from "@tanstack/react-query";
import type { DesktopLink } from "./desktopLink";

export const selectionQuery = (desktopId: string, link: DesktopLink) =>
  queryOptions({
    queryKey: ["settings", "desktop", desktopId, "selection"] as const,
    queryFn: () => link.call("device.selection", {}),
    staleTime: 0,
    retry: false,
  });

export const workspaceQuery = (desktopId: string, link: DesktopLink) =>
  queryOptions({
    queryKey: ["settings", "desktop", desktopId, "workspace"] as const,
    queryFn: () => link.call("workspace.list", {}),
    staleTime: 0,
    retry: false,
  });

export const updateStatusQuery = (desktopId: string, link: DesktopLink) =>
  queryOptions({
    queryKey: ["settings", "desktop", desktopId, "update"] as const,
    queryFn: () => link.call("desktop.updateStatus", {}),
    staleTime: 0,
    retry: false,
  });
