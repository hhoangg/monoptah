// Public API of the link client for the data layer and the screens of other workstreams.
export { LinkProvider, useLinkConnection, useLinkDesktopId, useLinkState } from "./LinkProvider.tsx";
export {
  LinkConnection,
  backoffDelay,
  getLinkConnection,
  planForClose,
  setPreConnectHook,
  resetLinkConnections,
  type ClosePlan,
  type GrantCheck,
  type PreConnectHook,
  type LinkConnectionDeps,
  type WatchRegistration,
  type WatchRegistrationHandle,
} from "./connection.ts";
export { DeviceRegistrationError, deviceNameFromUserAgent, ensureDevice, setDeviceRegistrar, type DeviceIdentity, type DeviceRegistrar } from "./device.ts";
export {
  LINK_BLOCKED_REASONS,
  LINK_STATUSES,
  getLinkState,
  getLinkStore,
  useLink,
  type LinkBlockedReason,
  type LinkErrorInfo,
  type LinkState,
  type LinkStatus,
  type LinkWatchInfo,
} from "./store.ts";
export { LinkRequestError, type RequestOptions } from "./session.ts";
