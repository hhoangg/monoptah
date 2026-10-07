import type { HarnessId } from "../../sessions/model/session";

/** How a new session for a provider is presented: app chat or the native CLI. */
export type SessionSurface = "chat" | "tui";

const STORAGE_KEY = "monocode.providerSurface.v1";

/** Fired on `window` when any provider's session surface changes. */
export const PROVIDER_SURFACE_CHANGE_EVENT = "monocode:provider-surface-change";

export const DEFAULT_SESSION_SURFACE: SessionSurface = "chat";

type StoredSurfaces = Partial<Record<HarnessId, SessionSurface>>;

function isSessionSurface(value: unknown): value is SessionSurface {
  return value === "chat" || value === "tui";
}

function readProviderSurfaces(): StoredSurfaces {
  try {
    const value = JSON.parse(localStorage.getItem(STORAGE_KEY) ?? "{}");
    if (typeof value !== "object" || value === null || Array.isArray(value)) {
      return {};
    }
    return Object.fromEntries(
      Object.entries(value).filter(([, surface]) => isSessionSurface(surface)),
    ) as StoredSurfaces;
  } catch {
    return {};
  }
}

export function loadProviderSurface(provider: HarnessId): SessionSurface {
  return readProviderSurfaces()[provider] ?? DEFAULT_SESSION_SURFACE;
}

export function saveProviderSurface(
  provider: HarnessId,
  surface: SessionSurface,
): boolean {
  try {
    const stored = readProviderSurfaces();
    stored[provider] = surface;
    localStorage.setItem(STORAGE_KEY, JSON.stringify(stored));
  } catch {
    return false;
  }
  if (typeof window !== "undefined") {
    window.dispatchEvent(new CustomEvent(PROVIDER_SURFACE_CHANGE_EVENT));
  }
  return true;
}

export function subscribeProviderSurface(listener: () => void): () => void {
  if (typeof window === "undefined") return () => {};
  const local = () => listener();
  // Other windows write the same key without firing our custom event.
  const storage = (event: StorageEvent) => {
    if (event.key === STORAGE_KEY) listener();
  };
  window.addEventListener(PROVIDER_SURFACE_CHANGE_EVENT, local);
  window.addEventListener("storage", storage);
  return () => {
    window.removeEventListener(PROVIDER_SURFACE_CHANGE_EVENT, local);
    window.removeEventListener("storage", storage);
  };
}
