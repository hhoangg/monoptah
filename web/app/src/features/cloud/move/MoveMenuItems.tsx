// `session-menu-items` slot (U-19): Move to cloud, Continue locally and Cancel move, rendered inside the
// session menus WS-T hosts (the card menu and the header menu) with the same menu item component. Shown only
// when the desktop advertises `cloud` and `cloudMove` and this device has `control`: a read-only device
// does not see them. The item only records the request; the dialog lives in `MoveDialogHost` (see moveStore).
import type { SessionCard } from "@monoptah/protocol";
import { useCloud } from "@/data";
import { DropdownMenuItem } from "@/ui/dropdown-menu";
import { Icon } from "@/ui/Icon";
import { useCapability } from "@/version";
import { isBillableKind, placementOf, useCanControl } from "../model";
import { openMove } from "./moveStore";

export function MoveMenuItems({ desktopId, session }: { desktopId: string; session: SessionCard }) {
  const cloudOn = useCapability("cloud");
  const moveOn = useCapability("cloudMove");
  const canControl = useCanControl();
  // The live field exists only for a session whose conversation is loaded (its screen); a list card has just the catalog card.
  const cloud = useCloud(session.id);
  if (!cloudOn || !moveOn || !canControl) return null;
  const placement = placementOf(session, cloud);
  const kind = placement?.kind ?? "local";
  if (cloud?.move) {
    return (
      <DropdownMenuItem onSelect={() => openMove({ kind: "cancel", desktopId, session })}>
        <Icon name="close" size={18} />
        Cancel move
      </DropdownMenuItem>
    );
  }
  if (kind === "local") {
    return (
      <DropdownMenuItem onSelect={() => openMove({ kind: "toCloud", desktopId, session })}>
        <Icon name="externalLink" size={18} />
        Move to cloud
      </DropdownMenuItem>
    );
  }
  if (isBillableKind(kind)) {
    return (
      <DropdownMenuItem onSelect={() => openMove({ kind: "toLocal", desktopId, session })}>
        <Icon name="laptop" size={18} />
        Continue locally
      </DropdownMenuItem>
    );
  }
  return null;
}
