// Slot registrations of WS-M (desktop manager and link state screens).
//
// Owner: WS-M. This module is imported for its side effects by one line in src/main.tsx (the slot
// registration contract of workstreams.md 3.2). The `/d/:desktopId` layout renders both slots next to
// the page: the switcher is a slim bar, the link-state screens are a blocking layer.
import { setPreConnectHook } from "@/link";
import { queryClient } from "@/lib/queryClient";
import { registerSlot } from "@/ui/slots";
import { DesktopSwitcher } from "./DesktopSwitcher";
import { LinkStateScreens } from "./LinkStateScreens";
import { createGrantHook } from "./preConnect";

registerSlot("desktop-switcher", DesktopSwitcher);
registerSlot("link-state-screens", LinkStateScreens);

// Ask for the grant of this device before the relay is joined (link pre-connect hook).
setPreConnectHook(createGrantHook(queryClient));
