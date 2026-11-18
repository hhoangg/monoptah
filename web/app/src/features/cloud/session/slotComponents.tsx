// Adapters from the open slot props (`SlotBaseProps`) to the typed props of the cloud components, so
// `register.ts` stays a list of one-line registrations.
import type { SessionCard } from "@monoptah/protocol";
import type { SlotBaseProps } from "@/ui/slots";
import { ApprovalCard } from "@/features/approvals";
import { MoveDialogHost } from "../move/MoveDialogs";
import { MoveMenuItems } from "../move/MoveMenuItems";
import { NewSessionTarget } from "../start/NewSessionTarget";
import { CloudArchiveDialog } from "./ArchiveDialog";
import { SessionCardBadge, SessionHeaderChip } from "./CloudChip";
import { CloudComposerTop } from "./CloudComposerTop";
import { CloudNavItem } from "./CloudNavItem";

/** The one `composer-top` registration: the approval card (gates itself) above the cloud banners. */
export function ComposerTopCloud({ desktopId, sessionId }: SlotBaseProps) {
  return (
    <>
      <ApprovalCard sessionId={String(sessionId)} />
      <CloudComposerTop desktopId={String(desktopId)} sessionId={String(sessionId)} />
    </>
  );
}

// The badge and the chip also mount the move dialogs: they outlive the session menu that opens them.
export function CardBadgeCloud({ desktopId, session }: SlotBaseProps) {
  return (
    <>
      <SessionCardBadge session={session as SessionCard} />
      <MoveDialogHost desktopId={String(desktopId)} session={session as SessionCard} />
    </>
  );
}

export function HeaderChipCloud({ desktopId, session }: SlotBaseProps) {
  return (
    <>
      <SessionHeaderChip desktopId={String(desktopId)} session={session as SessionCard} />
      <MoveDialogHost desktopId={String(desktopId)} session={session as SessionCard} />
    </>
  );
}

export function MenuItemsCloud({ desktopId, session }: SlotBaseProps) {
  return <MoveMenuItems desktopId={String(desktopId)} session={session as SessionCard} />;
}

export function NewSessionTargetCloud({ desktopId, projectId, value, onChange }: SlotBaseProps & { value?: unknown; onChange?: (target: unknown) => void }) {
  return <NewSessionTarget desktopId={String(desktopId)} projectId={String(projectId)} value={value} onChange={onChange} />;
}

export function ArchiveDialogCloud({ desktopId, session, open, onOpenChange }: SlotBaseProps) {
  return <CloudArchiveDialog desktopId={String(desktopId)} session={session as SessionCard} open={Boolean(open)} onOpenChange={onOpenChange as (open: boolean) => void} />;
}

export function NavItemsCloud({ desktopId }: SlotBaseProps) {
  return <CloudNavItem desktopId={String(desktopId)} />;
}
