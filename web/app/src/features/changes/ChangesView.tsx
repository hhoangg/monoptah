// F-64/F-65: the changes list. Staged and unstaged sections, status letters, +/- counts, stage and unstage per
// file and for all (optimistic, rolled back on error), polled every 10 s while the screen is up, and the
// context-aware bottom action. Read-only devices see the list without any control.
import { useState } from "react";
import { cn } from "@/lib/utils";
import { DelayedSpinner } from "@/ui/DelayedSpinner";
import { Icon } from "@/ui/Icon";
import { Notice } from "@/ui/Notice";
import { FileTypeIcon } from "@/features/files/FileTypeIcon";
import { GitActionBar } from "./GitActionBar";
import { STATUS_LABEL, STATUS_LETTER, errorText, formatCount, isPartlyStaged, isStaged, sections, splitPath, withWanted, type ChangedFile } from "./gitModel";
import { useCanPush, useChangesQuery, useGitQuery, useReadOnlyDevice, useRunGitAction } from "./useGit";

type SectionId = "staged" | "changes";

export interface ChangesViewProps {
  projectId: string;
  onOpenFile: (file: ChangedFile) => void;
}

export function ChangesView({ projectId, onOpenFile }: ChangesViewProps) {
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

  const stage = async (paths: string[], toStaged: boolean, all = false) => {
    const mark = (next: boolean | null) =>
      setWanted((current) => {
        const copy = new Map(current);
        for (const path of paths) {
          if (next === null) copy.delete(path);
          else copy.set(path, next);
        }
        return copy;
      });
    mark(toStaged);
    setError(null);
    try {
      await run(all ? { action: toStaged ? "stageAll" : "unstageAll" } : { action: toStaged ? "stage" : "unstage", paths });
      await reread();
    } catch (reason) {
      // Roll the optimistic state back.
      setError(`Could not ${toStaged ? "stage" : "unstage"}: ${errorText(reason)}`);
    }
    mark(null);
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
            className="flex min-h-11 min-w-0 flex-1 items-center gap-2 text-left text-label font-semibold"
          >
            <Icon name={collapsed.has(id) ? "chevronRight" : "chevronDown"} size={16} className="text-muted-text" />
            <span className="truncate">{title}</span>
            <span className="rounded-pill bg-fill px-2 text-small text-muted-text">{formatCount(list.length)}</span>
          </button>
          {readOnly ? null : (
            <button
              type="button"
              aria-label={id === "staged" ? "Unstage All" : "Stage All"}
              disabled={stagingAll}
              onClick={() => void stageAll(id !== "staged")}
              className="inline-flex size-11 items-center justify-center rounded-pill text-muted-text hover:bg-selection pointer-fine:size-9"
            >
              <Icon name={id === "staged" ? "minus" : "add"} size={18} />
            </button>
          )}
        </div>
        {collapsed.has(id) ? null : (
          <ul className="border-t border-stroke">
            {list.map((file) => (
              <FileRow key={file.relative} file={file} readOnly={readOnly} onOpen={() => onOpenFile(file)} onToggle={() => void stage([file.relative], !isStaged(file))} />
            ))}
          </ul>
        )}
      </section>
    );

  const additions = files.reduce((sum, file) => sum + file.additions, 0);
  const deletions = files.reduce((sum, file) => sum + file.deletions, 0);

  return (
    <div className="flex min-h-0 flex-col gap-3 p-4" data-testid="changes-view">
      <p className="flex items-center gap-2 text-small text-muted-text">
        <Icon name="branch" size={16} />
        <span className="truncate font-medium text-content">{changes.data.branch}</span>
        {files.length > 0 ? (
          <span>
            {files.length} {files.length === 1 ? "file" : "files"}, <span className="text-additions">+{formatCount(additions)}</span>{" "}
            <span className="text-deletions">-{formatCount(deletions)}</span>
          </span>
        ) : null}
      </p>
      {readOnly ? <Notice>This device is read-only. You can look at changes but not stage or commit them.</Notice> : null}
      {error ? <Notice variant="error">{error}</Notice> : null}
      {files.length === 0 ? <Notice title="No changes">Your working tree is clean.</Notice> : null}
      {renderSection("staged", "Staged Changes", staged)}
      {renderSection("changes", "Changes", unstaged)}
      {readOnly ? null : (
        <GitActionBar
          projectId={projectId}
          files={files}
          status={git.data}
          canPush={canPush}
          stagingAll={stagingAll}
          onStageAll={() => void stageAll(true)}
          onChanged={() => void reread()}
        />
      )}
    </div>
  );
}

function FileRow({ file, readOnly, onOpen, onToggle }: { file: ChangedFile; readOnly: boolean; onOpen: () => void; onToggle: () => void }) {
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
          onClick={onToggle}
          className="inline-flex size-11 shrink-0 items-center justify-center pointer-fine:size-9"
        >
          <span className={cn("flex size-5 items-center justify-center rounded-pill border border-stroke", staged && "border-transparent bg-primary text-primary-foreground", partly && "bg-selection")}>
            {staged ? <Icon name="check" size={13} /> : partly ? <Icon name="minus" size={13} /> : null}
          </span>
        </button>
      )}
      <button
        type="button"
        disabled={!openable}
        onClick={onOpen}
        aria-label={`Open diff of ${file.relative}`}
        className={cn("flex min-h-11 min-w-0 flex-1 items-center gap-2 py-1 pr-3 text-left hover:bg-selection disabled:hover:bg-transparent pointer-fine:min-h-9", readOnly && "pl-3")}
      >
        <FileTypeIcon name={name} isDir={false} />
        <span className="min-w-0 flex-1">
          <span className="block truncate text-label">{name}</span>
          {dir ? <span className="block truncate text-small text-faint">{dir}</span> : null}
        </span>
        {file.additions > 0 ? <span className="text-small text-additions">+{formatCount(file.additions)}</span> : null}
        {file.deletions > 0 ? <span className="text-small text-deletions">-{formatCount(file.deletions)}</span> : null}
        <span title={STATUS_LABEL[file.status]} aria-label={STATUS_LABEL[file.status]} className="w-4 shrink-0 text-center font-mono text-small text-muted-text">
          {STATUS_LETTER[file.status]}
        </span>
      </button>
    </li>
  );
}
