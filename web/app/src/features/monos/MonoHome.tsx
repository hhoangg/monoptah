// F-13: the messaging-style Mono home, shown when "Monos only" is on (and at `/d/:desktopId/monos`): the Monos
// newest first with their last reply, an unread dot (F-17) and a context menu (mark read or unread, details,
// hide, and Reset conversation for a Control device whose desktop advertises `monoReset`). "Delete" is denied for web
// (security.md section 7) and not offered. Hiding is
// device-local and can be undone from the "Hidden" line at the foot.
import { Link } from "@tanstack/react-router";
import { useEffect, useMemo, useState } from "react";
import { useCatalog } from "@/data";
import { markMonoSeen, markMonoUnread, useUnreadMonos } from "@/features/sessions/monoSeen";
import { oneLine } from "@/features/sessions/WorkspaceNav";
import { chatListTime } from "@/features/sessions/relativeTime";
import type { CatalogMono } from "@/features/sessions/workspaceFilter";
import { cn } from "@/lib/utils";
import { Button } from "@/ui/button";
import { DelayedSpinner } from "@/ui/DelayedSpinner";
import { Icon } from "@/ui/Icon";
import { Notice } from "@/ui/Notice";
import { WorkspaceFrame, useReadOnly } from "@/features/sessions/WorkspaceFrame";
import { EmptyState } from "@/ui/EmptyState";
import { ContextMenu, ContextMenuContent, ContextMenuItem, ContextMenuTrigger } from "./contextMenu";
import { stripEmoTokens } from "./emoji";
import { ResetMonoDialog, useCanResetMono } from "./ResetMonoDialog";
import { CreateMonoDialog } from "./CreateMonoDialog";
import { clearCreateMonoRequest, useCreateMonoRequest } from "./createRequest";
import { MonoAvatar } from "./MonoAvatar";
import { useMonoPrefs } from "./monoPrefs";

export function MonoHome({ desktopId, titleAs: Title = "h2" }: { desktopId: string; /** The heading element: "h1" when the list is a page of its own. */ titleAs?: "h1" | "h2" }) {
  const catalog = useCatalog();
  const readOnly = useReadOnly();
  const [creating, setCreating] = useState(false);
  const [showHidden, setShowHidden] = useState(false);
  const hiddenIds = useMonoPrefs((state) => state.hidden[desktopId]);
  const setHidden = useMonoPrefs((state) => state.setHidden);
  const monos = catalog.data?.monos;
  const unread = useUnreadMonos(desktopId, monos ?? []);
  const hidden = useMemo(() => new Set(hiddenIds ?? []), [hiddenIds]);
  const sorted = useMemo(() => [...(monos ?? [])].sort((a, b) => (b.lastActiveAt ?? 0) - (a.lastActiveAt ?? 0)), [monos]);
  const canCreate = catalog.data?.monoTools === true && !readOnly;
  // "New Mono" asked for from the command palette: the dialog opens once the catalog says this device may create.
  const createRequested = useCreateMonoRequest((state) => state.requested);
  const catalogLoaded = !!catalog.data;
  useEffect(() => {
    // A device that may not create drops the request, so it does not wait for a later list to act on it.
    if (createRequested && catalogLoaded && !canCreate) clearCreateMonoRequest();
  }, [createRequested, catalogLoaded, canCreate]);
  const dialogOpen = creating || (createRequested && canCreate);
  const shown = sorted.filter((mono) => !hidden.has(mono.id));
  const hiddenMonos = sorted.filter((mono) => hidden.has(mono.id));

  const newMono = canCreate ? (
    <Button type="button" size="sm" variant="outline" onClick={() => setCreating(true)}>
      <Icon name="add" size={16} />
      New Mono
    </Button>
  ) : null;
  // A device that cannot create says why, instead of hiding the button without a word.
  const cannotCreate = !catalog.data ? null : readOnly ? "This device is read-only, so it cannot create Monos." : catalog.data.monoTools !== true ? "This desktop does not offer creating Monos from the web." : null;
  // As a page of its own (`titleAs="h1"`) the list sits in the workspace frame: drawer button, back to the list, New Mono in the header.
  const framed = Title === "h1";

  const body = (
    <section data-testid="mono-home" aria-label="Monos" className={cn("mx-auto flex w-full max-w-[768px] flex-col gap-2", framed ? "" : "px-3 py-3")}>
      {framed ? null : (
        <div className="flex items-center gap-2 px-2">
          <Title className="flex-1 text-headline font-semibold">Monos</Title>
          {newMono}
        </div>
      )}
      {!monos ? (
        catalog.isError ? <Notice variant="error">Could not load your Monos.</Notice> : <DelayedSpinner label="Loading Monos" className="m-6" />
      ) : shown.length === 0 ? (
        <EmptyState
          data-testid="mono-home-empty"
          icon="bot"
          title={hiddenMonos.length > 0 ? "All your Monos are hidden." : "No Monos yet."}
          description={
            hiddenMonos.length > 0
              ? "Show one again from the Hidden list below."
              : `A Mono is a personal agent you chat with, like a contact that remembers you across sessions. ${cannotCreate ?? "Create one with New Mono."}`
          }
        />
      ) : (
        <ul className="flex flex-col">
          {shown.map((mono) => (
            <li key={mono.id}>
              <MonoRow desktopId={desktopId} mono={mono} unread={unread.has(mono.id)} onHide={() => setHidden(desktopId, mono.id, true)} />
            </li>
          ))}
        </ul>
      )}
      {cannotCreate && shown.length > 0 ? <p className="px-2 text-small text-muted-text">{cannotCreate}</p> : null}
      {hiddenMonos.length > 0 ? (
        <div className="px-2">
          <button type="button" className="inline-flex min-h-11 items-center text-small text-muted-text underline-offset-2 hover:underline pointer-fine:min-h-9" aria-expanded={showHidden} onClick={() => setShowHidden(!showHidden)}>
            Hidden ({hiddenMonos.length})
          </button>
          {showHidden ? (
            <ul className="mt-1 flex flex-col">
              {hiddenMonos.map((mono) => (
                <li key={mono.id} className="flex items-center gap-2 py-1">
                  <span className="min-w-0 flex-1 truncate text-body">{mono.name}</span>
                  <Button type="button" size="sm" variant="ghost" onClick={() => setHidden(desktopId, mono.id, false)}>
                    Show {mono.name}
                  </Button>
                </li>
              ))}
            </ul>
          ) : null}
        </div>
      ) : null}
      {canCreate ? <CreateMonoDialog
          desktopId={desktopId}
          open={dialogOpen}
          onOpenChange={(next) => {
            setCreating(next);
            if (!next) clearCreateMonoRequest();
          }}
        /> : null}
    </section>
  );
  return framed ? (
    <WorkspaceFrame desktopId={desktopId} title="Monos" actions={newMono} testId="monos-screen" narrow>
      {body}
    </WorkspaceFrame>
  ) : (
    body
  );
}

function MonoRow({ desktopId, mono, unread, onHide }: { desktopId: string; mono: CatalogMono; unread: boolean; onHide: () => void }) {
  const working = mono.status === "working";
  const needsYou = mono.status === "needs-you";
  const preview = mono.lastMessage ? oneLine(stripEmoTokens(mono.lastMessage)) : "";
  const canReset = useCanResetMono();
  const [resetting, setResetting] = useState(false);
  return (
    <>
    <ContextMenu>
      <ContextMenuTrigger asChild>
        <Link
          to="/d/$desktopId/monos/$monoId"
          params={{ desktopId, monoId: mono.id }}
          data-testid="mono-row"
          data-unread={unread ? "true" : undefined}
          className="flex min-h-16 items-center gap-3 rounded-card px-2 py-2 hover:bg-fill"
        >
          <MonoAvatar monoId={mono.id} name={mono.name} mascot={mono.mascot} color={mono.color} working={working} size={40} />
          <span className="min-w-0 flex-1">
            <span className="flex items-baseline gap-2">
              <span className={cn("min-w-0 flex-1 truncate text-row", unread ? "font-semibold" : "font-medium")}>{mono.name}</span>
              {mono.lastActiveAt ? <span className="shrink-0 text-meta text-muted-text">{chatListTime(mono.lastActiveAt)}</span> : null}
            </span>
            <span className={cn("block truncate text-small text-muted-text", needsYou && "text-caution")}>{working ? "Working..." : needsYou ? "Needs you" : preview || "No messages yet"}</span>
          </span>
          {unread ? <span role="img" aria-label="Unread" className="size-2.5 shrink-0 rounded-pill bg-[var(--accent-color)]" /> : null}
        </Link>
      </ContextMenuTrigger>
      <ContextMenuContent>
        {unread ? (
          <ContextMenuItem onSelect={() => markMonoSeen(desktopId, mono)}>
            <Icon name="check" size={18} />
            Mark as read
          </ContextMenuItem>
        ) : (
          <ContextMenuItem onSelect={() => markMonoUnread(desktopId, mono)}>
            <Icon name="inbox" size={18} />
            Mark as unread
          </ContextMenuItem>
        )}
        <ContextMenuItem asChild>
          <Link to="/d/$desktopId/monos/$monoId/profile" params={{ desktopId, monoId: mono.id }}>
            <Icon name="info" size={18} />
            View details
          </Link>
        </ContextMenuItem>
        <ContextMenuItem onSelect={onHide}>
          <Icon name="archive" size={18} />
          Hide
        </ContextMenuItem>
        {canReset ? (
          <ContextMenuItem destructive onSelect={() => setResetting(true)}>
            <Icon name="refresh" size={18} />
            Reset conversation
          </ContextMenuItem>
        ) : null}
      </ContextMenuContent>
    </ContextMenu>
    {resetting ? <ResetMonoDialog desktopId={desktopId} monoId={mono.id} name={mono.name} onOpenChange={setResetting} /> : null}
    </>
  );
}
