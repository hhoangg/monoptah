// One open conversation: the `conversation.watch` registered on the link, its frames written to the
// query cache, the optimistic sends, and the typed hooks over the cache entry (architecture.md 3.2, 3.3).
//
// How a screen opens a conversation: `useConversation(conversationId)`. It shares one watcher per
// (query client, desktop, conversation), registers the watch with the connection (opened when `ready`,
// reopened after every reconnect with `from` set to the first turn held) and unregisters it on unmount.
import { queryOptions, useQuery, useQueryClient, type QueryClient } from "@tanstack/react-query";
import { useEffect, useMemo } from "react";
import type { Approval, Attachment, CloudField, ResultOf } from "@monoptah/protocol";
import { getLinkConnection, useLinkConnection, useLinkDesktopId, type LinkRequestError, type WatchRegistrationHandle } from "@/link";
import type { LinkClient } from "./client.ts";
import { getMutator } from "./mutations.ts";
import { loadPersistedTurns, persistTurns } from "./persist.ts";
import { LINK_QUERY_DEFAULTS, conversationKey } from "./queries.ts";
import {
  NO_APPROVALS,
  attachmentCount,
  countUserItems,
  emptyConversation,
  reconcile,
  withFrames,
  withOptimistic,
  withOptimisticStatus,
  withRead,
  type ConversationState,
} from "./conversationState.ts";
import { useLinkDataSync } from "./sync.ts";
import { applyFrames, type WatchFrameInput } from "./watchUpdates.ts";

/** How many of the latest turns a watch opens on (and how many the persisted cache keeps). */
export const WATCH_TAIL = 30;

export interface SendResult {
  /** True when the desktop accepted the message. False means it shows "Not sent, retry". */
  sent: boolean;
  error?: string;
}

function errorCode(error: unknown): string {
  if (error && typeof error === "object" && "code" in error && typeof (error as { code: unknown }).code === "string") {
    return (error as { code: string }).code;
  }
  return error instanceof Error ? error.message : String(error);
}

export interface ConversationWatcherDeps {
  client: LinkClient;
  queryClient: QueryClient;
  conversationId: string;
}

/**
 * Owns the watch and the optimistic turns of one conversation. Frames and reads are written with
 * `queryClient.setQueryData` to `conversationKey`.
 */
export class ConversationWatcher {
  readonly conversationId: string;
  readonly #client: LinkClient;
  readonly #queryClient: QueryClient;
  readonly #key: ReturnType<typeof conversationKey>;
  readonly #params = new Map<string, { text: string; attachments: Attachment[] | undefined }>();
  #registration: WatchRegistrationHandle | null = null;
  /** Invalidates the callbacks of an earlier registration. */
  #gen = 0;
  #nulls = 0;
  #reopening = false;
  #nextPending = 1;
  #started = false;

  constructor(deps: ConversationWatcherDeps) {
    this.#client = deps.client;
    this.#queryClient = deps.queryClient;
    this.conversationId = deps.conversationId;
    this.#key = conversationKey(deps.client.desktopId, deps.conversationId);
  }

  get state(): ConversationState | undefined {
    return this.#queryClient.getQueryData<ConversationState>(this.#key);
  }

  #update(change: (state: ConversationState) => ConversationState): void {
    this.#queryClient.setQueryData<ConversationState>(this.#key, (previous) => change(previous ?? emptyConversation()));
  }

  /** Registers the watch. The persisted turns show first when the cache has none (marked `cachedOnly`). */
  start(): void {
    if (this.#started) return;
    this.#started = true;
    globalThis.document?.addEventListener("visibilitychange", this.#onVisibility);
    this.#register();
    if (!this.state) {
      void loadPersistedTurns(this.#client.desktopId, this.conversationId).then((turns) => {
        if (!this.#started || !turns || this.state) return;
        this.#queryClient.setQueryData<ConversationState>(this.#key, { ...emptyConversation(), turns, cachedOnly: true });
      });
    }
  }

  /** Unregisters the watch and keeps the last turns for next time. */
  stop(): void {
    if (!this.#started) return;
    this.#started = false;
    globalThis.document?.removeEventListener("visibilitychange", this.#onVisibility);
    this.#gen += 1;
    this.#registration?.unregister();
    this.#registration = null;
    this.#save();
  }

  readonly #onVisibility = (): void => {
    if (globalThis.document?.hidden === true) this.#save();
  };

  #save(): void {
    const state = this.state;
    // A revoked desktop's data was wiped; never write it back (the adapter may be durable, W-12).
    if (this.#client.store.getState().revoked) return;
    if (state && !state.cachedOnly && state.turns.length > 0) void persistTurns(this.#client.desktopId, this.conversationId, state.turns);
  }

  /** Params of every open of the watch: from the first turn held, so history joins up (WorkspaceProvider.tsx:370-389). */
  watchParams(): { conversationId: string; tail: number; incremental: true; from?: string } {
    const state = this.state;
    const from = state && !state.cachedOnly ? state.turns[0]?.id : undefined;
    return { conversationId: this.conversationId, tail: WATCH_TAIL, incremental: true, ...(from ? { from } : {}) };
  }

  #register(): void {
    const gen = ++this.#gen;
    this.#registration = this.#client.registerWatch({
      method: "conversation.watch",
      reopenParams: () => this.watchParams(),
      onFrame: (frame) => {
        if (gen === this.#gen) this.#onFrame(frame);
      },
      onError: (error) => {
        if (gen === this.#gen) this.#update((state) => ({ ...state, error: error.message }));
      },
    });
  }

  #onFrame(frame: WatchFrameInput): void {
    const current = this.state ?? emptyConversation();
    const applied = applyFrames(current.turns, [frame]);
    if (!applied) {
      this.#reopen();
      return;
    }
    this.#nulls = 0;
    this.#update((state) => withFrames(state, applied));
  }

  /** A frame named a turn the client never got: close the watch and open it again with `from`. */
  #reopen(): void {
    this.#nulls += 1;
    // A second miss in a row means the held turns do not join up: open on the latest turns instead.
    if (this.#nulls >= 2) this.#update((state) => ({ ...state, turns: [] }));
    this.#gen += 1;
    if (this.#reopening) return;
    this.#reopening = true;
    queueMicrotask(() => {
      this.#reopening = false;
      if (!this.#started) return;
      this.#registration?.unregister();
      this.#register();
    });
  }

  /** The fallback of mobile: read the whole conversation once (resync after a reconnect). */
  async read(): Promise<void> {
    const result = await this.#client.request("conversation.read", { conversationId: this.conversationId });
    this.#update((state) => withRead(state, result));
  }

  /** Sends a message: an optimistic turn first, then the desktop answer decides what it shows. */
  async send(text: string, attachments?: Attachment[]): Promise<SendResult> {
    const id = `pending:${this.#nextPending++}`;
    this.#params.set(id, { text, attachments });
    this.#update((state) =>
      withOptimistic(state, { id, text, at: Date.now(), attachments: attachmentCount(attachments), status: "sending", baseline: this.#baseline(state, id) }),
    );
    return this.#deliver(id);
  }

  /** User action only: sends a "Not sent" message again with the same command id. */
  async retry(id: string): Promise<SendResult> {
    const turn = this.state?.optimistic.find((other) => other.id === id);
    if (!turn || turn.status !== "not_sent" || !this.#params.has(id)) return { sent: false, error: "nothing_to_retry" };
    this.#update((state) => withOptimisticStatus({ ...state, optimistic: state.optimistic.map((o) => (o.id === id ? { ...o, baseline: this.#baseline(state, id) } : o)) }, id, { status: "sending" }));
    return this.#deliver(id);
  }

  #baseline(state: ConversationState, ownId: string): number {
    return countUserItems(state.turns) + state.optimistic.filter((turn) => turn.id !== ownId && turn.status !== "not_sent").length;
  }

  async #deliver(id: string): Promise<SendResult> {
    const params = this.#params.get(id);
    if (!params) return { sent: false, error: "nothing_to_retry" };
    try {
      await getMutator(this.#client).mutate("conversation.send", {
        conversationId: this.conversationId,
        text: params.text,
        ...(params.attachments?.length ? { attachments: params.attachments } : {}),
      });
    } catch (error) {
      // Never re-sent here: after a reconnect the state is re-read, and the turn reconciles or stays "Not sent".
      const code = errorCode(error);
      this.#update((state) => reconcile(withOptimisticStatus(state, id, { status: "not_sent", error: code })));
      return { sent: false, error: code };
    }
    this.#params.delete(id);
    this.#update((state) => reconcile(withOptimisticStatus(state, id, { status: "sent" })));
    return { sent: true };
  }
}

// --- Registry: one watcher per query client, desktop and conversation ---

interface Entry {
  watcher: ConversationWatcher;
  refs: number;
}

const registries = new WeakMap<QueryClient, Map<string, Entry>>();

/** Conversations whose watch is open for this client and desktop (the resync reads them again). */
export function openConversationWatchers(queryClient: QueryClient, desktopId: string): ConversationWatcher[] {
  const out: ConversationWatcher[] = [];
  for (const [key, entry] of registries.get(queryClient) ?? []) if (key.startsWith(`${desktopId}\u0000`)) out.push(entry.watcher);
  return out;
}

export interface ConversationHandle {
  readonly watcher: ConversationWatcher;
  /** Releases this user of the watcher; the last release unregisters the watch. */
  close(): void;
}

/** Non-React way to open a conversation (the hook uses it). Ref counted. */
export function openConversation(queryClient: QueryClient, client: LinkClient, conversationId: string): ConversationHandle {
  let registry = registries.get(queryClient);
  if (!registry) {
    registry = new Map();
    registries.set(queryClient, registry);
  }
  const key = `${client.desktopId}\u0000${conversationId}`;
  let entry = registry.get(key);
  if (!entry) {
    entry = { watcher: new ConversationWatcher({ client, queryClient, conversationId }), refs: 0 };
    registry.set(key, entry);
  }
  entry.refs += 1;
  entry.watcher.start();
  const owned = entry;
  let closed = false;
  return {
    watcher: owned.watcher,
    close() {
      if (closed) return;
      closed = true;
      owned.refs -= 1;
      if (owned.refs === 0) {
        owned.watcher.stop();
        registry.delete(key);
      }
    },
  };
}

// --- Query options and hooks ---

/** `conversation.read` into the conversation entry; also the fallback for a caller that does not watch. */
export function conversationQueryOptions(desktopId: string, conversationId: string, extras: { connection?: LinkClient } = {}) {
  return queryOptions<ConversationState, LinkRequestError>({
    ...LINK_QUERY_DEFAULTS,
    queryKey: conversationKey(desktopId, conversationId),
    queryFn: async ({ client: queryClient, signal }) => {
      const result: ResultOf<"conversation.read"> = await (extras.connection ?? getLinkConnection(desktopId)).request(
        "conversation.read",
        { conversationId },
        { signal },
      );
      return withRead(queryClient.getQueryData<ConversationState>(conversationKey(desktopId, conversationId)) ?? emptyConversation(), result);
    },
  });
}

export interface UseConversationResult {
  /** Undefined until the first frame, read or persisted entry arrives. */
  state: ConversationState | undefined;
  send(text: string, attachments?: Attachment[]): Promise<SendResult>;
  /** Re-sends a "Not sent" message with its command id. Call it from a user action only. */
  retry(optimisticId: string): Promise<SendResult>;
}

/** Opens the conversation watch for the lifetime of the component and returns its state and send API. */
export function useConversation(conversationId: string): UseConversationResult {
  const desktopId = useLinkDesktopId();
  const client = useLinkConnection();
  const queryClient = useQueryClient();
  useLinkDataSync();
  useEffect(() => {
    const handle = openConversation(queryClient, client, conversationId);
    return () => handle.close();
  }, [queryClient, client, conversationId]);
  const { data } = useQuery({ ...conversationQueryOptions(desktopId, conversationId), enabled: false });
  return useMemo(
    () => ({
      state: data,
      send: async (text, attachments) => {
        const handle = openConversation(queryClient, client, conversationId);
        try {
          return await handle.watcher.send(text, attachments);
        } finally {
          handle.close();
        }
      },
      retry: async (id) => {
        const handle = openConversation(queryClient, client, conversationId);
        try {
          return await handle.watcher.retry(id);
        } finally {
          handle.close();
        }
      },
    }),
    [data, queryClient, client, conversationId],
  );
}

function useConversationSelect<T>(sessionId: string, select: (state: ConversationState) => T): T | undefined {
  const desktopId = useLinkDesktopId();
  return useQuery({ ...conversationQueryOptions(desktopId, sessionId), enabled: false, select }).data;
}

/** The `cloud` field of the conversation (from reads and frames), or undefined for a local session. */
export function useCloud(sessionId: string): CloudField | undefined {
  return useConversationSelect(sessionId, (state) => state.cloud);
}

/** The pending tool approvals of the conversation. Empty when there are none or the desktop does not advertise `approvals`. */
export function useApprovals(sessionId: string): Approval[] {
  return useConversationSelect(sessionId, (state) => state.approvals) ?? NO_APPROVALS;
}

/** How many times the desktop sent the `approvals` field for this conversation; see `ConversationState.approvalsSeq`. */
export function useApprovalsSeq(sessionId: string): number {
  return useConversationSelect(sessionId, (state) => state.approvalsSeq) ?? 0;
}
