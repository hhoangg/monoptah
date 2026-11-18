// Slot registrations of WS-C (cloud sessions, U-16 to U-19).
//
// This module is imported for its side effects by one line in src/main.tsx (the slot registration
// contract of workstreams.md 3.2), so registrations are written as top-level calls. The approval card,
// the start picker and the move actions (Tasks 3 and 4) add their entries here.
import { registerSendGuard, registerSessionCapabilities } from "@/ui/hooks";
import { registerSlot } from "@/ui/slots";
import { cloudSendGuard, cloudSessionCapabilities } from "./session/guard";
import { ArchiveDialogCloud, CardBadgeCloud, ComposerTopCloud, HeaderChipCloud, NavItemsCloud } from "./session/slotComponents";

registerSlot("composer-top", ComposerTopCloud);
registerSlot("session-card-badge", CardBadgeCloud);
registerSlot("session-header-chip", HeaderChipCloud);
registerSlot("archive-dialog", ArchiveDialogCloud);
registerSlot("desktop-nav-items", NavItemsCloud);
registerSendGuard(cloudSendGuard);
registerSessionCapabilities(cloudSessionCapabilities);
