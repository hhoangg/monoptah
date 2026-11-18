// Public API of the data layer over the desktop link (W-05). Screens import from "@/data".
export type { LinkClient } from "./client.ts";
export { LINK_GC_TIME_MS, LINK_QUERY_DEFAULTS, catalogKey, conversationKey, desktopKey, linkKey, linkQueryOptions, useCatalog, useLinkQuery } from "./queries.ts";
export { MAX_PENDING_COMMAND_IDS, Mutator, commandKey, commandSignature, getMutator, makeCommandId, stableStringify, useLinkMutation } from "./mutations.ts";
export {
  ConversationWatcher,
  WATCH_TAIL,
  conversationQueryOptions,
  openConversation,
  useApprovals,
  useApprovalsSeq,
  useCloud,
  useConversation,
  type ConversationHandle,
  type SendResult,
  type UseConversationResult,
} from "./conversation.ts";
export { NO_APPROVALS, countUserItems, emptyConversation, type ConversationState, type OptimisticTurn } from "./conversationState.ts";
export { CATALOG_POLL_MS, acquireLinkDataSync, startLinkDataSync, useLinkDataSync } from "./sync.ts";
export {
  MemoryPersistAdapter,
  PERSISTED_SESSION_LIMIT,
  PERSISTED_TURN_LIMIT,
  clearPersistedCache,
  getPersistAdapter,
  keyBelongsToDesktop,
  loadPersistedCatalog,
  loadPersistedTurns,
  persistCatalog,
  persistTurns,
  registerPersistAdapter,
  type PersistAdapter,
} from "./persist.ts";
export { applyFrames, type AppliedFrames, type WatchFrameInput } from "./watchUpdates.ts";
export { registerLastPlaceResolver, resolveLastPlace, type LastPlaceResolver } from "./lastPlace.ts";
