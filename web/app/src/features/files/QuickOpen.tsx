// F-61: the quick open dialog. Type to find a file (`project.search`, 150 ms debounce, fuzzy ranked),
// Up/Down move, Enter opens it in the files screen, Esc closes. A dialog from 640px, a sheet below.
import { useNavigate } from "@tanstack/react-router";
import { useEffect, useId, useRef, useState } from "react";
import { cn } from "@/lib/utils";
import { DelayedSpinner } from "@/ui/DelayedSpinner";
import { Input } from "@/ui/input";
import { MatchText } from "@/ui/MatchText";
import { ResponsiveOverlay } from "@/ui/ResponsiveOverlay";
import { FileTypeIcon } from "./FileTypeIcon";
import { useFileSearch } from "./FileSearch";
import { closeQuickOpen, useQuickOpen } from "./quickOpenStore";

function QuickOpenBody({ desktopId, projectId }: { desktopId: string; projectId: string }) {
  const navigate = useNavigate();
  const [text, setText] = useState("");
  // The selection restarts at the first hit whenever the query changes.
  const [picked, setPicked] = useState({ query: "", index: 0 });
  const listId = useId();
  const { query, hits, isPending, isError } = useFileSearch(projectId, text);
  const active = picked.query === query ? Math.min(picked.index, Math.max(hits.length - 1, 0)) : 0;
  const setActive = (update: number | ((index: number) => number)) =>
    setPicked({ query, index: typeof update === "function" ? update(active) : update });


  const open = (path: string) => {
    closeQuickOpen();
    void navigate({ to: "/d/$desktopId/p/$projectId/files/$", params: { desktopId, projectId, _splat: path } });
  };

  // Enter inside the debounce window must open the top hit of what is typed now, not of the previous query: it
  // waits until the search of the typed text has answered (a ref, so no state is set in the effect).
  const enterWaiting = useRef(false);
  const settled = query === text.trim() && !isPending;
  useEffect(() => {
    if (!enterWaiting.current || !settled) return;
    enterWaiting.current = false;
    const hit = hits[0];
    if (hit) open(hit.path);
  });

  return (
    <div className="flex flex-col gap-2">
      <Input
        autoFocus
        role="combobox"
        aria-label="Find a file"
        aria-expanded={hits.length > 0}
        aria-controls={listId}
        aria-activedescendant={hits[active] ? `${listId}-${active}` : undefined}
        placeholder="Find a file by name"
        autoComplete="off"
        spellCheck={false}
        value={text}
        onChange={(event) => setText(event.target.value)}
        onKeyDown={(event) => {
          if (event.nativeEvent.isComposing) return;
          if (event.key === "ArrowDown" && hits.length > 0) {
            event.preventDefault();
            setActive((index) => (index + 1) % hits.length);
          } else if (event.key === "ArrowUp" && hits.length > 0) {
            event.preventDefault();
            setActive((index) => (index - 1 + hits.length) % hits.length);
          } else if (event.key === "Enter") {
            if (text.trim() === "") return;
            event.preventDefault();
            if (!settled) {
              enterWaiting.current = true;
              return;
            }
            const hit = hits[active];
            if (hit) open(hit.path);
          }
        }}
      />
      <ul id={listId} role="listbox" aria-label="Matching files" className="flex flex-col">
        {hits.map((hit, index) => (
          <li
            key={hit.path}
            id={`${listId}-${index}`}
            role="option"
            aria-selected={index === active}
            onPointerMove={() => setActive(index)}
            onClick={() => open(hit.path)}
            className={cn("flex min-h-11 cursor-pointer items-center gap-2 rounded-row px-2 text-label pointer-fine:min-h-9", index === active && "bg-selection")}
          >
            <FileTypeIcon name={hit.name} isDir={false} />
            <span className="min-w-0 flex-1 truncate [unicode-bidi:isolate]">
              <MatchText text={hit.path} positions={hit.positions} active />
            </span>
          </li>
        ))}
      </ul>
      {isError ? <p className="px-2 text-small text-deletions">The search failed.</p> : null}
      {isPending ? <DelayedSpinner label="Searching" className="mx-2" /> : null}
      {query && !isPending && !isError && hits.length === 0 ? <p className="px-2 text-small text-muted-text">No files match "{query}".</p> : null}
      {!query ? <p className="px-2 text-small text-muted-text">Type a file name. Enter opens it, Esc closes.</p> : null}
    </div>
  );
}

/** Renders the dialog when this host owns it and Mod+P opened it for its project. */
export function QuickOpen({ owner, desktopId, projectId }: { owner: boolean; desktopId: string; projectId: string }) {
  const target = useQuickOpen((state) => state.target);
  const shown = owner && target?.desktopId === desktopId && target.projectId === projectId;
  return (
    <ResponsiveOverlay open={!!shown} onOpenChange={(open) => (open ? undefined : closeQuickOpen())} title="Quick open" fullHeight>
      {shown ? <QuickOpenBody desktopId={desktopId} projectId={projectId} /> : null}
    </ResponsiveOverlay>
  );
}
