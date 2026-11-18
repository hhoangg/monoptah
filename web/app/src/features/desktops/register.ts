// Slot registrations of WS-M (desktop manager and link state screens).
//
// Owner: WS-M. This module is imported for its side effects by one line in src/main.tsx (the slot
// registration contract of workstreams.md 3.2). The `/d/:desktopId` layout renders both slots next to
// the page: the switcher is a slim bar, the link-state screens are a blocking layer.
import { registerSlot } from "@/ui/slots";
import { DesktopSwitcher } from "./DesktopSwitcher";
import { LinkStateScreens } from "./LinkStateScreens";

registerSlot("desktop-switcher", DesktopSwitcher);
registerSlot("link-state-screens", LinkStateScreens);
