// F-70: the menu at the right of a Mono's header. Details only on the web: "Reset conversation" and
// "Delete Mono" are denied for web devices in v1 (security.md section 7), so they are never offered and
// `mono.reset` / `mono.delete` are never called. Shown when the desktop advertises `monoTools`.
import { Link } from "@tanstack/react-router";
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from "@/ui/dropdown-menu";
import { Icon } from "@/ui/Icon";

export function MonoMenu({ desktopId, monoId }: { desktopId: string; monoId: string }) {
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <button
          type="button"
          aria-label="Mono actions"
          className="inline-flex size-11 shrink-0 items-center justify-center rounded-pill bg-fill text-content hover:bg-selection pointer-fine:size-9"
        >
          <Icon name="more" size={18} />
        </button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end">
        <DropdownMenuItem asChild>
          <Link to="/d/$desktopId/monos/$monoId/profile" params={{ desktopId, monoId }}>
            <Icon name="info" size={18} />
            View details
          </Link>
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
