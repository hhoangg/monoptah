// The header of a session folder in a project's list: a button that folds it (the chevron, the folder's own icon,
// its name and how many sessions it holds) and, for a device that may control, a menu with Rename and Ungroup. The
// header is as wide as a session card and every part of it is at least 44 px high on touch (design-system.md).
import { useState } from "react";
import type { FolderCard } from "@monoptah/protocol";
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from "@/ui/dropdown-menu";
import { Icon } from "@/ui/Icon";
import { cn } from "@/lib/utils";
import { FolderIcon, RenameFolderDialog, UngroupFolderDialog } from "./SessionFolderDialogs";

export interface FolderHeaderProps {
  desktopId: string;
  folder: FolderCard;
  /** How many of the folder's sessions the list shows. */
  count: number;
  open: boolean;
  /** Folds or unfolds the folder on this device. */
  onToggle: () => void;
  readOnly: boolean;
  /** The id of the list the header folds, for `aria-controls`. */
  controls: string;
}

export function FolderHeader({ desktopId, folder, count, open, onToggle, readOnly, controls }: FolderHeaderProps) {
  const [renaming, setRenaming] = useState(false);
  const [ungrouping, setUngrouping] = useState(false);
  return (
    <div className="relative" data-testid="folder-header" data-folder-id={folder.id}>
      <button
        type="button"
        aria-expanded={open}
        aria-controls={controls}
        onClick={onToggle}
        className={cn(
          "flex min-h-11 w-full items-center gap-2 rounded-row px-3.5 text-left text-content hover:bg-content-a04 active:bg-content-a04 pointer-fine:min-h-9",
          readOnly ? "" : "pr-14",
        )}
      >
        <Icon name="chevronRight" size={16} className={cn("shrink-0 text-muted-text transition-transform", open && "rotate-90")} />
        <FolderIcon folder={folder} />
        <span className="min-w-0 flex-1 truncate text-body font-semibold">{folder.name}</span>
        <span className="shrink-0 text-meta tabular text-muted-text" aria-label={`${count} ${count === 1 ? "session" : "sessions"}`}>
          {count}
        </span>
      </button>
      {readOnly ? null : (
        <>
          <DropdownMenu>
            <DropdownMenuTrigger asChild>
              <button
                type="button"
                aria-label={`Actions for folder ${folder.name}`}
                className="absolute top-1/2 right-1.5 inline-flex size-11 -translate-y-1/2 items-center justify-center rounded-pill text-muted-text hover:bg-selection pointer-fine:size-9"
              >
                <Icon name="more" size={18} />
              </button>
            </DropdownMenuTrigger>
            <DropdownMenuContent align="end">
              <DropdownMenuItem onSelect={() => setRenaming(true)}>
                <Icon name="edit" size={18} />
                Rename
              </DropdownMenuItem>
              <DropdownMenuItem destructive onSelect={() => setUngrouping(true)}>
                <Icon name="trash" size={18} />
                Ungroup
              </DropdownMenuItem>
            </DropdownMenuContent>
          </DropdownMenu>
          <RenameFolderDialog desktopId={desktopId} folder={folder} open={renaming} onOpenChange={setRenaming} />
          <UngroupFolderDialog desktopId={desktopId} folder={folder} open={ungrouping} onOpenChange={setUngrouping} />
        </>
      )}
    </div>
  );
}
