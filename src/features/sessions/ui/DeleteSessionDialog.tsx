import { useState } from "react";
import { prettyCwd } from "../../../shared/lib/paths";
import { Modal } from "../../../shared/ui/Modal";

export type SessionDeleteChoice = {
  confirmed: boolean;
  deleteWorktree: boolean;
  deleteProviderFiles: boolean;
};

/** Which CLI owns the on-disk session files, or "mixed" for a bulk selection. */
export type ProviderFilesKind = "claude" | "codex" | "mixed";

const PROVIDER_FILES_LABEL: Record<ProviderFilesKind, string> = {
  claude: "the Claude Code session files",
  codex: "the Codex session files",
  mixed: "the harness session files",
};

/** The on-disk session files a delete can also remove, if the session has any. */
export function providerFilesKind(session: {
  harness: string;
  providerSessionId?: string;
}): "claude" | "codex" | undefined {
  if (!session.providerSessionId) return undefined;
  return session.harness === "claude" || session.harness === "codex"
    ? session.harness
    : undefined;
}

const CANCELLED: SessionDeleteChoice = {
  confirmed: false,
  deleteWorktree: false,
  deleteProviderFiles: false,
};

export function DeleteSessionDialog({
  title,
  count = 1,
  unusedWorktrees = [],
  providerFiles,
  onClose,
}: {
  /** Title of the session being deleted; unused when deleting several. */
  title: string;
  count?: number;
  unusedWorktrees?: readonly string[];
  providerFiles?: ProviderFilesKind;
  onClose: (choice: SessionDeleteChoice) => void;
}) {
  const hasWorktree = unusedWorktrees.length > 0;
  const hasProviderFiles = providerFiles !== undefined;
  const [deleteWorktree, setDeleteWorktree] = useState(true);
  const [deleteProviderFiles, setDeleteProviderFiles] = useState(true);
  const bulk = count > 1;
  return (
    <Modal
      title={bulk ? `Delete ${count} sessions?` : "Delete session?"}
      size="sm"
      onClose={() => onClose(CANCELLED)}
    >
      <div className="flex flex-col gap-4 p-4 text-[12px]">
        <p>
          {bulk
            ? `${count} sessions will be permanently deleted.`
            : `“${title}” will be permanently deleted.`}
        </p>
        {hasProviderFiles && (
          <label className="flex items-start gap-2">
            <input
              type="checkbox"
              checked={deleteProviderFiles}
              onChange={(e) => setDeleteProviderFiles(e.target.checked)}
              className="mt-0.5 accent-accent"
            />
            <span>
              Also delete {PROVIDER_FILES_LABEL[providerFiles]} on this
              computer
              <span className="mt-1 block text-[11px] text-content/45">
                Frees disk space. The conversation can no longer be resumed
                from the CLI.
              </span>
            </span>
          </label>
        )}
        {hasWorktree && (
          <label className="flex items-start gap-2">
            <input
              type="checkbox"
              checked={deleteWorktree}
              onChange={(e) => setDeleteWorktree(e.target.checked)}
              className="mt-0.5 accent-accent"
            />
            <span>
              {unusedWorktrees.length > 1
                ? "Also delete unused worktrees"
                : "Also delete the unused worktree"}
              {unusedWorktrees.map((path) => (
                <span
                  key={path}
                  className="mt-1 block break-all text-[11px] text-content/45"
                >
                  {prettyCwd(path)}
                </span>
              ))}
              <span className="mt-1 block text-[11px] text-content/45">
                The branch is kept. If files have uncommitted changes, the
                worktree stays.
              </span>
            </span>
          </label>
        )}
        <div className="flex justify-end gap-2">
          <button
            type="button"
            onClick={() => onClose(CANCELLED)}
            className="rounded-md px-3 py-1.5 hover:bg-content/8 active:scale-[0.97]"
          >
            Cancel
          </button>
          <button
            type="button"
            onClick={() =>
              onClose({
                confirmed: true,
                deleteWorktree: hasWorktree && deleteWorktree,
                deleteProviderFiles: hasProviderFiles && deleteProviderFiles,
              })
            }
            className="rounded-md bg-red-500/20 px-3 py-1.5 font-medium text-red-400 hover:bg-red-500/30 active:scale-[0.97]"
          >
            {bulk ? `Delete ${count} sessions` : "Delete session"}
          </button>
        </div>
      </div>
    </Modal>
  );
}
