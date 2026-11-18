// `desktop-nav-items` slot: the entry of the cloud instance list (U-17), only when the desktop advertises `cloud`.
import { Link } from "@tanstack/react-router";
import { Icon } from "@/ui/Icon";
import { useCapability } from "@/version";
import { CLOUD_NAV_LABEL } from "../copy";

export function CloudNavItem({ desktopId }: { desktopId: string }) {
  const enabled = useCapability("cloud");
  if (!enabled) return null;
  return (
    <Link to="/d/$desktopId/cloud" params={{ desktopId }} className="flex min-h-11 items-center gap-3 rounded-row px-2.5 text-row hover:bg-content-a04 [&.active]:bg-selection">
      <Icon name="computer" size={20} />
      {CLOUD_NAV_LABEL}
    </Link>
  );
}
