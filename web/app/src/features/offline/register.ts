// Startup wiring of W-12 (offline). Imported for its side effects by one line in src/main.tsx
// (the registration contract of workstreams.md 3.2). Order: persisted cache first, so the first
// read or write already goes to the encrypted store.
import { clearPersistedCache, registerPersistAdapter } from "@/data";
import { onForgetDesktop } from "@/features/settings/desktopLink";
import { queryClient } from "@/lib/queryClient";
import { startAccountPersistence } from "./accountCache";
import { EncryptedPersistAdapter, encryptedPersistSupported } from "./encryptedPersist";
import { registerServiceWorker, shouldRegisterServiceWorker } from "./registerSw";

// Encrypted IndexedDB cache; where IndexedDB or WebCrypto is missing the in-memory default stays.
if (encryptedPersistSupported()) registerPersistAdapter(new EncryptedPersistAdapter());

// "Remove this desktop from this device" (F-08) wipes that desktop's cached entries. A revoked link does
// the same from data/sync.ts, and "Forget this device" calls `clearPersistedCache()` for everything.
onForgetDesktop((desktopId) => clearPersistedCache(desktopId));

startAccountPersistence(queryClient);

if (shouldRegisterServiceWorker(import.meta.env)) registerServiceWorker();
