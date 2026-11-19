// Public API of features/offline for the other workstreams.
export { setSendInProgressCheck, isSendInProgress } from "./swUpdate";
export { describeConnectivity, useOnline, type Connectivity, type ConnectivityKind } from "./connectivity";
export { OfflineBanner, useConnectivity, useDesktopConnectivity } from "./OfflineBanner";
export { getPersistentStorageState, type PersistentStorageState } from "./storage";
