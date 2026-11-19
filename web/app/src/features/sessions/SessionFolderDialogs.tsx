// Control devices edit the desktop's session folders: add a session to a folder (or start a new one), rename a
// folder, ungroup it. Each is one folder method (`folder.move`, `folder.create`, `folder.rename`, `folder.delete`);
// the desktop applies it through its sidebar and the catalog poll brings the result back.
import { useState, type FormEvent } from "react";
import { FOLDER_NAME_MAX, type FolderCard, type SessionCard } from "@monoptah/protocol";
import { useCatalog, useLinkMutation } from "@/data";
import { Button } from "@/ui/button";
import { ConfirmDialog } from "@/ui/ConfirmDialog";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/ui/dialog";
import { Field } from "@/ui/Field";
import { Icon } from "@/ui/Icon";
import { cn } from "@/lib/utils";
import { folderColor, folderOf, projectFolders } from "./folders";
import { useRefreshSessions } from "./SessionActionDialogs";

interface SessionDialogProps {
  desktopId: string;
  session: SessionCard;
  open: boolean;
  onOpenChange: (open: boolean) => void;
}

/** "Add to folder": the project's folders to move the session into, or a name for a new one. */
export function AddToFolderDialog({ desktopId, session, open, onOpenChange }: SessionDialogProps) {
  // Re-mounted per opening, so the field starts empty.
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      {open ? <AddToFolderForm desktopId={desktopId} session={session} onOpenChange={onOpenChange} /> : null}
    </Dialog>
  );
}

function AddToFolderForm({ desktopId, session, onOpenChange }: Omit<SessionDialogProps, "open">) {
  const catalog = useCatalog();
  const folders = projectFolders(catalog.data?.folders, session.projectId);
  const current = folderOf(folders, session.id);
  const [name, setName] = useState("");
  const move = useLinkMutation("folder.move");
  const create = useLinkMutation("folder.create");
  const refresh = useRefreshSessions(desktopId);
  const pending = move.isPending || create.isPending;
  const trimmed = name.trim();
  const valid = trimmed.length > 0 && trimmed.length <= FOLDER_NAME_MAX;
  const done = () => {
    refresh();
    onOpenChange(false);
  };
  const pick = (folder: FolderCard) => {
    if (pending) return;
    if (folder.id === current?.id) return onOpenChange(false);
    move.mutate({ projectId: session.projectId, conversationId: session.id, folderId: folder.id }, { onSuccess: done });
  };
  const submit = (event: FormEvent) => {
    event.preventDefault();
    if (!valid || pending) return;
    // A folder with this name already there is the one to use: the desktop's picker never makes a twin.
    const same = folders.find((folder) => folder.name.toLowerCase() === trimmed.toLowerCase());
    if (same) return pick(same);
    create.mutate({ projectId: session.projectId, conversationIds: [session.id], name: trimmed }, { onSuccess: done });
  };
  const failed = move.isError || create.isError;
  return (
    <DialogContent showClose={false}>
      <DialogHeader className="pr-0">
        <DialogTitle>Add to folder</DialogTitle>
        <DialogDescription className="sr-only">Choose a folder for this session, or name a new one.</DialogDescription>
      </DialogHeader>
      {folders.length > 0 ? (
        <ul aria-label="Folders" className="-mx-1.5 flex max-h-64 flex-col overflow-y-auto">
          {folders.map((folder) => (
            <li key={folder.id}>
              <button
                type="button"
                disabled={pending}
                aria-current={folder.id === current?.id ? "true" : undefined}
                onClick={() => pick(folder)}
                className="flex min-h-11 w-full items-center gap-3 rounded-row px-3 text-left text-row hover:bg-selection disabled:opacity-40 pointer-fine:min-h-9"
              >
                <Icon name="folder" size={18} className="shrink-0 text-muted-text" />
                <span className="min-w-0 flex-1 truncate">{folder.name}</span>
                {folder.id === current?.id ? <Icon name="check" size={16} className="shrink-0 text-brand-text" /> : <span className="shrink-0 text-meta tabular text-muted-text">{folder.sessionIds.length}</span>}
              </button>
            </li>
          ))}
        </ul>
      ) : null}
      <form onSubmit={submit} className="grid gap-4">
        <Field
          label="New folder"
          value={name}
          maxLength={FOLDER_NAME_MAX}
          autoComplete="off"
          placeholder="Folder name"
          onChange={(event) => setName(event.target.value)}
          error={failed ? "Could not update the folders." : null}
        />
        <DialogFooter>
          <Button type="button" variant="ghost" disabled={pending} onClick={() => onOpenChange(false)}>
            Cancel
          </Button>
          <Button type="submit" disabled={!valid || pending} aria-busy={create.isPending}>
            Create folder
          </Button>
        </DialogFooter>
      </form>
    </DialogContent>
  );
}

interface FolderDialogProps {
  desktopId: string;
  folder: FolderCard;
  open: boolean;
  onOpenChange: (open: boolean) => void;
}

export function RenameFolderDialog({ desktopId, folder, open, onOpenChange }: FolderDialogProps) {
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      {open ? <RenameFolderForm desktopId={desktopId} folder={folder} onOpenChange={onOpenChange} /> : null}
    </Dialog>
  );
}

function RenameFolderForm({ desktopId, folder, onOpenChange }: Omit<FolderDialogProps, "open">) {
  const [name, setName] = useState(folder.name);
  const rename = useLinkMutation("folder.rename");
  const refresh = useRefreshSessions(desktopId);
  const trimmed = name.trim();
  const valid = trimmed.length > 0 && trimmed.length <= FOLDER_NAME_MAX;
  const submit = (event: FormEvent) => {
    event.preventDefault();
    if (!valid || rename.isPending) return;
    if (trimmed === folder.name) return onOpenChange(false);
    rename.mutate(
      { projectId: folder.projectId, folderId: folder.id, name: trimmed },
      {
        onSuccess: () => {
          refresh();
          onOpenChange(false);
        },
      },
    );
  };
  return (
    <DialogContent showClose={false}>
      <form onSubmit={submit} className="grid gap-4">
        <DialogHeader className="pr-0">
          <DialogTitle>Rename folder</DialogTitle>
          <DialogDescription className="sr-only">Give the folder a new name.</DialogDescription>
        </DialogHeader>
        <Field
          label="Name"
          value={name}
          maxLength={FOLDER_NAME_MAX}
          autoComplete="off"
          autoFocus
          onChange={(event) => setName(event.target.value)}
          error={rename.isError ? "Could not rename the folder." : null}
        />
        <DialogFooter>
          <Button type="button" variant="ghost" disabled={rename.isPending} onClick={() => onOpenChange(false)}>
            Cancel
          </Button>
          <Button type="submit" disabled={!valid || rename.isPending} aria-busy={rename.isPending}>
            Save
          </Button>
        </DialogFooter>
      </form>
    </DialogContent>
  );
}

/** Ungroup: the folder goes and its sessions stay in the list. */
export function UngroupFolderDialog({ desktopId, folder, open, onOpenChange }: FolderDialogProps) {
  const remove = useLinkMutation("folder.delete");
  const refresh = useRefreshSessions(desktopId);
  return (
    <ConfirmDialog
      open={open}
      onOpenChange={onOpenChange}
      title="Ungroup this folder?"
      description={`The folder "${folder.name}" is removed on your desktop. Its sessions stay in the list.`}
      confirmLabel="Ungroup"
      busy={remove.isPending}
      error={remove.isError ? "Could not ungroup the folder." : null}
      onConfirm={() =>
        remove.mutate(
          { projectId: folder.projectId, folderId: folder.id },
          {
            onSuccess: () => {
              refresh();
              onOpenChange(false);
            },
          },
        )
      }
    />
  );
}

/** The folder icon in the folder's own color (the desktop's palette), else the muted default. */
export function FolderIcon({ folder, className }: { folder: Pick<FolderCard, "colorIndex" | "customColor">; className?: string }) {
  const color = folderColor(folder);
  return (
    <span style={color ? { color } : undefined} className={cn("inline-flex shrink-0", !color && "text-muted-text", className)}>
      <Icon name="folder" size={18} />
    </span>
  );
}

/** Tells a device that a folder change from a menu did not go through (the menu has closed, so nothing else would). */
export function FolderErrorDialog({ open, onClose }: { open: boolean; onClose: () => void }) {
  return (
    <Dialog open={open} onOpenChange={(next) => (next ? undefined : onClose())}>
      <DialogContent showClose={false}>
        <DialogHeader className="pr-0">
          <DialogTitle>Could not update the folders</DialogTitle>
          <DialogDescription>The change did not go through on your desktop. Try again.</DialogDescription>
        </DialogHeader>
        <DialogFooter>
          <Button type="button" onClick={onClose}>
            OK
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
