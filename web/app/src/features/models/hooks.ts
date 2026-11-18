// Data hooks of the models feature over the link (architecture.md 6.x): the catalog, account usage,
// the debounced `session.configure` and the usage-limit actions. They need a `LinkProvider`.
import { useQueryClient } from "@tanstack/react-query";
import { useCallback, useEffect, useMemo, useRef } from "react";
import type { ResultOf, UsageLimit } from "@monoptah/protocol";
import { linkKey, useLinkMutation, useLinkQuery } from "@/data";
import { useLinkDesktopId } from "@/link";
import { validateUsageLimit } from "@/features/sessions/usageLimit";
import type { ModelSelection } from "./modelSettings";
import { buildModelSettingsParams } from "./params";

/** The desktop's `models.list`, for a project or a running session (`models.list` scopes by either). */
export function useModelCatalog(scope: { conversationId?: string; projectId?: string } = {}) {
  const params = useMemo(
    () => ({
      ...(scope.conversationId ? { conversationId: scope.conversationId } : {}),
      ...(scope.projectId ? { projectId: scope.projectId } : {}),
    }),
    [scope.conversationId, scope.projectId],
  );
  return useLinkQuery("models.list", params);
}

export type AccountUsageResult = ResultOf<"accounts.usage">;

export interface AccountUsage {
  accounts: AccountUsageResult["accounts"];
  loading: boolean;
  refreshing: boolean;
  error: string | null;
  /** A fresh probe from the provider (`refresh: true`, a control request). */
  refresh: () => void;
}

/** Usage meters of the claude and codex accounts. Other providers have none and never query. */
export function useAccountUsage(provider: string | undefined): AccountUsage {
  const enabled = provider === "claude" || provider === "codex";
  const desktopId = useLinkDesktopId();
  const queryClient = useQueryClient();
  const query = useLinkQuery("accounts.usage", { provider: enabled ? provider : "claude" }, { enabled });
  const refreshMutation = useLinkMutation("accounts.usage");
  const refresh = useCallback(() => {
    if (!enabled || refreshMutation.isPending) return;
    refreshMutation.mutate(
      { provider, refresh: true },
      { onSuccess: (result) => queryClient.setQueryData(linkKey(desktopId, "accounts.usage", { provider }), result) },
    );
  }, [enabled, provider, refreshMutation, queryClient, desktopId]);
  const failure = refreshMutation.error ?? query.error;
  return {
    accounts: enabled ? (query.data?.accounts ?? {}) : {},
    loading: enabled && query.isPending,
    refreshing: refreshMutation.isPending,
    error: enabled && failure ? "Usage unavailable" : null,
    refresh,
  };
}

/** Existing session changes go to the desktop after this quiet period (F-51). */
export const CONFIGURE_DEBOUNCE_MS = 400;

/**
 * `session.configure` for a running session. `configure(selection)` replaces any pending call and
 * sends the latest selection after `CONFIGURE_DEBOUNCE_MS`; `flush` sends it now. The params come
 * from `buildModelSettingsParams`, so `full-access` can never be sent.
 */
export function useSessionConfigure(conversationId: string | undefined, onError?: (message: string) => void) {
  const mutation = useLinkMutation("session.configure");
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const pending = useRef<ModelSelection | null>(null);
  // Refs keep `flush` stable across renders, so the unmount flush below only runs on unmount or when
  // the session changes.
  const sendRef = useRef(mutation.mutate);
  const onErrorRef = useRef(onError);
  useEffect(() => {
    sendRef.current = mutation.mutate;
    onErrorRef.current = onError;
  });

  const flush = useCallback(() => {
    if (timer.current) clearTimeout(timer.current);
    timer.current = null;
    const next = pending.current;
    pending.current = null;
    if (!next || !conversationId) return;
    sendRef.current(buildModelSettingsParams(conversationId, next), {
      onError: (error) => onErrorRef.current?.(error.message),
    });
  }, [conversationId]);

  const configure = useCallback(
    (selection: ModelSelection) => {
      pending.current = selection;
      if (timer.current) clearTimeout(timer.current);
      timer.current = setTimeout(flush, CONFIGURE_DEBOUNCE_MS);
    },
    [flush],
  );

  // A selection still waiting when the picker goes away is not lost.
  useEffect(() => flush, [flush]);

  return { configure, flush, isPending: mutation.isPending };
}

/**
 * Resume and "resume at reset" of a session paused by a usage limit (F-55). `latest` is the newest
 * `usageLimit` of the watch frame; the state is validated against it before anything is sent.
 */
export function useUsageLimitActions(conversationId: string, latest: UsageLimit | null | undefined) {
  const resumeNow = useLinkMutation("session.resumeUsage");
  const resumeLater = useLinkMutation("session.resumeUsageAtReset");
  return {
    resume: async (token: string): Promise<void> => {
      const state = validateUsageLimit(latest, token, false);
      await resumeNow.mutateAsync({ conversationId, token: state.token });
    },
    resumeAtReset: async (token: string, enabled: boolean): Promise<void> => {
      const state = validateUsageLimit(latest, token, true);
      await resumeLater.mutateAsync({ conversationId, token: state.token, enabled });
    },
  };
}
