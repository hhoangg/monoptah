// Adapters from the open slot props (`SlotBaseProps`) to the typed props of the cloud components, so
// `register.ts` stays a list of one-line registrations.
import type { SessionCard } from "@monoptah/protocol";
import type { SlotBaseProps } from "@/ui/slots";
import { CloudArchiveDialog } from "./ArchiveDialog";
import { SessionCardBadge, SessionHeaderChip } from "./CloudChip";
import { CloudComposerTop } from "./CloudComposerTop";
import { CloudNavItem } from "./CloudNavItem";

export function ComposerTopCloud({ desktopId, sessionId }: SlotBaseProps) {
  return <CloudComposerTop desktopId={String(desktopId)} sessionId={String(sessionId)} />;
}

export function CardBadgeCloud({ session }: SlotBaseProps) {
  return <SessionCardBadge session={session as SessionCard} />;
}

export function HeaderChipCloud({ session }: SlotBaseProps) {
  return <SessionHeaderChip session={session as SessionCard} />;
}

export function ArchiveDialogCloud({ desktopId, session, open, onOpenChange }: SlotBaseProps) {
  return <CloudArchiveDialog desktopId={String(desktopId)} session={session as SessionCard} open={Boolean(open)} onOpenChange={onOpenChange as (open: boolean) => void} />;
}

export function NavItemsCloud({ desktopId }: SlotBaseProps) {
  return <CloudNavItem desktopId={String(desktopId)} />;
}
