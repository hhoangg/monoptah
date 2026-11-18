// F-64/F-65: the changes list. Staged and unstaged sections, status letters, +/- counts, stage and unstage per
// file and for all (optimistic, rolled back on error), polled every 10 s while the screen is up, and the
// context-aware bottom action. Read-only devices see the list without any control.
import { useRef, useState } from "react";
import { cn } from "@/lib/utils";
import { DelayedSpinner } from "@/ui/DelayedSpinner";
import { Icon } from "@/ui/Icon";
import { IconTip } from "@/ui/tooltip";
import { Notice } from "@/ui/Notice";
import { FileTypeIcon } from "@/features/files/FileTypeIcon";
import { GitActionBar } from "./GitActionBar";
import { STATUS_LABEL, STATUS_LETTER, errorText, formatCount, isPartlyStaged, isStaged, sections, splitPath, withWanted, type ChangedFile } from "./gitModel";
import { useCanPush, useChangesQuery, useGitQuery, useReadOnlyDevice, useRunGitAction } from "./useGit";

type SectionId = "staged" | "changes";

/** Keyboard focus rings are drawn inside the rows: the section cards clip anything outside their rounded edge. */
const INSET_FOCUS = "focus-visible:outline-offset-[-2px]";

export interface ChangesViewProps {
  projectId: string;
  /** The file whose diff is open beside the list (split layout): its row is marked current. */
  selectedPath?: string;
  onOpenFile: (file: ChangedFile) => void;
}

export function ChangesView({ projectId, selectedPath, onOpenFile }: ChangesViewProps) {
  const changes = useChangesQuery(projectId);
  const git = useGitQuery(projectId);
  const readOnly = useReadOnlyDevice();
  const canPush = useCanPush();
  const run = useRunGitAction(projectId);
  // The staging the user asked for, shown at once and kept until the list is read again.
  const [wanted, setWanted] = useState<ReadonlyMap<string, boolean>>(new Map());
  const [stagingAll, setStagingAll] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [collapsed, setCollapsed] = useState<ReadonlySet<SectionId>>(new Set());

  const files = withWanted(changes.data?.files ?? [], wanted);
  const { staged, unstaged } = sections(files);

  const reread = async () => {
    await Promise.all([changes.refetch(), git.refetch()]);
  };

  // Optimistic staging that survives quick toggles: calls go out one after the other (the desktop applies them in
  // the order the user clicked), and a call clears the optimistic value of a path only while it is still the
  // latest one asked for that path, so an earlier answer never flips a row the user has toggled again.
  const queue = useRef<Promise<void>>(Promise.resolve());
  const latest = useRef(new Map<string, number>());
  const counter = useRef(0);

  const stage = (paths: string[], toStaged: boolean, all = false): Promise<void> => {
    const token = ++counter.current;
    for (const path of paths) latest.current.set(path, token);
    setWanted((current) => {
      const copy = new Map(current);
      for (const path of paths) copy.set(path, toStaged);
      return copy;
    });
    setError(null);
    const task = queue.current.then(async () => {
      try {
        await run(all ? { action: toStaged ? "stageAll" : "unstageAll" } : { action: toStaged ? "stage" : "unstage", paths });
        await reread();
      } catch (reason) {
        // The optimistic state is rolled back below (for the paths no later call owns).
        setError(`Could not ${toStaged ? "stage" : "unstage"}: ${errorText(reason)}`);
      }
      const owned = paths.filter((path) => latest.current.get(path) === token);
      for (const path of owned) latest.current.delete(path);
      setWanted((current) => {
        const copy = new Map(current);
        for (const path of owned) copy.delete(path);
        return copy;
      });
    });
    queue.current = task;
    return task;
  };

  const stageAll = async (toStaged: boolean) => {
    const paths = (toStaged ? unstaged : staged).map((file) => file.relative);
    if (paths.length === 0) return;
    setStagingAll(true);
    await stage(paths, toStaged, true);
    setStagingAll(false);
  };

  const toggleSection = (id: SectionId) =>
    setCollapsed((current) => {
      const next = new Set(current);
      if (!next.delete(id)) next.add(id);
      return next;
    });

  if (changes.isError && !changes.data) {
    return (
      <Notice variant="error" className="m-4">
        {errorText(changes.error)}
      </Notice>
    );
  }
  if (!changes.data) return <DelayedSpinner label="Loading changes" className="m-6" />;

  const renderSection = (id: SectionId, title: string, list: ChangedFile[]) =>
    list.length === 0 ? null : (
      <section key={id} aria-label={title} className="overflow-hidden rounded-card border border-stroke">
        <div className="flex min-h-11 items-center gap-1 pr-1 pl-3">
          <button
            type="button"
            aria-expanded={!collapsed.has(id)}
            onClick={() => toggleSection(id)}
            className={cn("flex min-h-11 min-w-0 flex-1 items-center gap-2 text-left text-label font-semibold", INSET_FOCUS)}
          >
            <Icon name={collapsed.has(id) ? "chevronRight" : "chevronDown"} size={16} className="text-muted-text" />
            <span className="truncate">{title}</span>
            <span className="rounded-pill bg-fill px-2 text-small text-muted-text tabular-nums">{formatCount(list.length)}</span>
          </button>
          {readOnly ? null : (
            <IconTip label={id === "staged" ? "Unstage All" : "Stage All"}>
              <button
                type="button"
                aria-label={id === "staged" ? "Unstage All" : "Stage All"}
                disabled={stagingAll}
                onClick={() => void stageAll(id !== "staged")}
                className={cn("inline-flex size-11 items-center justify-center rounded-pill text-muted-text hover:bg-selection disabled:opacity-40 disabled:hover:bg-transparent pointer-fine:size-9", INSET_FOCUS)}
              >
                <Icon name={id === "staged" ? "minus" : "add"} size={18} />
              </button>
            </IconTip>
          )}
        </div>
        {collapsed.has(id) ? null : (
          <ul className="border-t border-stroke">
            {list.map((file) => (
              <FileRow key={file.relative} file={file} current={file.relative === selectedPath} readOnly={readOnly} busy={stagingAll} onOpen={() => onOpenFile(file)} onToggle={() => void stage([file.relative], !isStaged(file))} />
            ))}
          </ul>
        )}
      </section>
    );

  const additions = files.reduce((sum, file) => sum + file.additions, 0);
  const deletions = files.reduce((sum, file) => sum + file.deletions, 0);

  return (
    <div className="flex min-h-0 flex-1 flex-col" data-testid="changes-view">
      <div className="flex min-h-0 flex-1 flex-col gap-3 overflow-y-auto p-4">
        <p className="flex items-start gap-2 text-small text-muted-text">
          <Icon name="branch" size={16} className="mt-0.5 shrink-0" />
          <span className="min-w-0 font-medium text-content wrap-anywhere" title={changes.data.branch}>
            {changes.data.branch}
          </span>
          {files.length > 0 ? (
            <span className="shrink-0">
              {files.length} {files.length === 1 ? "file" : "files"}, <span className="text-additions tabular-nums">+{formatCount(additions)}</span>{" "}
              <span className="text-deletions tabular-nums">-{formatCount(deletions)}</span>
            </span>
          ) : null}
        </p>
        {readOnly ? <Notice>This device is read-only. You can look at changes but not stage or commit them.</Notice> : null}
        {error ? <Notice variant="error">{error}</Notice> : null}
        {files.length === 0 ? <Notice title="No changes">Your working tree is clean.</Notice> : null}
        {renderSection("staged", "Staged Changes", staged)}
        {renderSection("changes", "Changes", unstaged)}
      </div>
      {readOnly ? null : (
        <div className="shrink-0 border-t border-stroke bg-surface p-3 pb-[max(0.75rem,env(safe-area-inset-bottom))]">
          <GitActionBar
            projectId={projectId}
            files={files}
            status={git.data}
            canPush={canPush}
            stagingAll={stagingAll}
            onStageAll={() => void stageAll(true)}
            onChanged={() => void reread()}
          />
        </div>
      )}
    </div>
  );
}

function FileRow({ file, current, readOnly, busy, onOpen, onToggle }: { file: ChangedFile; current: boolean; readOnly: boolean; busy: boolean; onOpen: () => void; onToggle: () => void }) {
  const { name, dir } = splitPath(file.relative);
  const staged = isStaged(file);
  const partly = isPartlyStaged(file);
  // A deleted file has no diff to open (as on the desktop).
  const openable = file.lines.length > 0;
  return (
    <li className="flex items-center border-b border-stroke last:border-b-0" data-testid="change-row" data-staged={staged}>
      {readOnly ? null : (
        <button
          type="button"
          role="checkbox"
          aria-checked={partly ? "mixed" : staged}
          aria-label={staged ? `Unstage ${name}` : `Stage ${name}`}
          disabled={busy}
          onClick={onToggle}
          className={cn("inline-flex size-11 shrink-0 items-center justify-center disabled:opacity-40 pointer-fine:size-9", INSET_FOCUS)}
        >
          <span className={cn("flex size-5 items-center justify-center rounded-pill border border-stroke", staged && "border-transparent bg-primary text-primary-foreground", partly && "bg-selection")}>
            {staged ? <Icon name="check" size={13} /> : partly ? <Icon name="minus" size={13} /> : null}
          </span>
        </button>
      )}
      <button
        type="button"
        disabled={!openable}
        aria-current={current ? "true" : undefined}
        onClick={onOpen}
        aria-label={`Open diff of ${file.relative}, ${STATUS_LABEL[file.status]}${file.additions > 0 ? `, ${file.additions} added` : ""}${file.deletions > 0 ? `, ${file.deletions} removed` : ""}`}
        className={cn("flex min-h-11 min-w-0 flex-1 items-center gap-2 py-1 pr-3 text-left hover:bg-selection disabled:cursor-default disabled:hover:bg-transparent pointer-fine:min-h-9", INSET_FOCUS, current && "bg-selection", readOnly && "pl-3")}
      >
        <FileTypeIcon name={name} isDir={false} />
        <span className="min-w-0 flex-1">
          <span className="block truncate [unicode-bidi:isolate] text-label">{name}</span>
          {dir ? <span className="block truncate [unicode-bidi:isolate] text-small text-muted-text">{dir}</span> : null}
          {openable ? null : <span className="block text-small text-muted-text">{file.status === "deleted" ? "Deleted, no diff" : "No diff"}</span>}
        </span>
        {file.additions > 0 ? <span className="text-small text-additions tabular-nums">+{formatCount(file.additions)}</span> : null}
        {file.deletions > 0 ? <span className="text-small text-deletions tabular-nums">-{formatCount(file.deletions)}</span> : null}
        <span title={STATUS_LABEL[file.status]} aria-label={STATUS_LABEL[file.status]} className="w-4 shrink-0 text-center font-mono text-small text-muted-text">
          {STATUS_LETTER[file.status]}
        </span>
      </button>
    </li>
  );
}
