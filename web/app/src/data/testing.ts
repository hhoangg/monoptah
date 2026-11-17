// Test support: a fake link client (no network, no Noise) for the data layer tests.
import { vi } from "vitest";
import { createStore } from "zustand/vanilla";
import type { WatchFrame } from "@monoptah/protocol";
import type { LinkRequestError, WatchRegistration } from "@/link";
import { initialLinkState, type LinkState, type LinkStatus } from "@/link/store.ts";
import type { LinkClient } from "./client.ts";

export interface FakeWatch {
  registration: WatchRegistration;
  /** The params the watch was opened with (what `reopenParams` returned at open time). */
  openedWith: Record<string, unknown>[];
  unregistered: boolean;
}

export class FakeClient {
  readonly desktopId: string;
  readonly store = createStore<LinkState>(() => ({ ...initialLinkState(), status: "ready" }));
  readonly watches: FakeWatch[] = [];
  readonly listeners = new Set<() => void>();
  readonly request = vi.fn(async (_method: string, _params?: unknown, _options?: unknown): Promise<unknown> => ({}));

  constructor(desktopId = "dk1") {
    this.desktopId = desktopId;
  }

  setStatus(status: LinkStatus): void {
    this.store.setState({ status });
  }

  registerWatch = (registration: WatchRegistration) => {
    const watch: FakeWatch = { registration, openedWith: [], unregistered: false };
    this.watches.push(watch);
    if (this.store.getState().status === "ready") watch.openedWith.push(registration.reopenParams());
    return {
      id: this.watches.length,
      unregister: () => {
        watch.unregistered = true;
      },
    };
  };

  onResynced = (listener: () => void) => {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  };

  /** The last watch that is still registered. */
  get live(): FakeWatch {
    const watch = [...this.watches].reverse().find((candidate) => !candidate.unregistered);
    if (!watch) throw new Error("no live watch");
    return watch;
  }

  frame(frame: Partial<WatchFrame> & { error?: string }): void {
    this.live.registration.onFrame(frame as WatchFrame);
  }

  /** What the connection does after a reconnect: reopen every live watch with fresh params, then emit `resynced`. */
  reconnect(): void {
    for (const watch of this.watches) if (!watch.unregistered) watch.openedWith.push(watch.registration.reopenParams());
    for (const listener of this.listeners) listener();
  }

  asClient(): LinkClient {
    return this as unknown as LinkClient;
  }
}

export function requestError(code: string): LinkRequestError {
  return Object.assign(new Error(code), { name: "LinkRequestError", code }) as LinkRequestError;
}

export const flush = async (): Promise<void> => {
  for (let i = 0; i < 5; i += 1) await Promise.resolve();
};
