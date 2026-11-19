// The model controls of the composer toolbar and the new-session page: a pill with the model and its
// effort that opens the ModelPicker, and the access picker beside it. It loads `models.list` itself
// (needs a LinkProvider). Task 5 mounts it; see the API notes in the Task 6 report.
//
// - New chat: `onChange(selection)` hands the selection to the page, which keeps it in its state and
//   spreads `buildStartModelParams(selection)` into `session.start`.
// - Running session (`conversationId` set): each change also goes to `session.configure`, debounced
//   400 ms, built by `buildModelSettingsParams`. `full-access` is never sent: a session that runs in
//   full access shows "Full access (set on desktop)" read only.
import { useMemo, useState } from "react";
import type { SendableRuntimeMode } from "@monoptah/protocol";
import { cn } from "@/lib/utils";
import { HarnessIcon } from "@/ui/HarnessIcon";
import { ChevronDown } from "@/ui/icons";
import { AccessPicker } from "./AccessPicker";
import { useAccountUsage, useModelCatalog, useSessionConfigure } from "./hooks";
import { ModelPicker, summarizeSelection } from "./ModelPicker";
import { currentSelection, toEmitted, type ModelSelection, type ModelSelectionInput } from "./modelSettings";

export interface ModelControlsProps {
  /** A running session: changes are sent with `session.configure`. */
  conversationId?: string | undefined;
  /** The project a new chat starts in (scopes `models.list`). */
  projectId?: string | undefined;
  /** Controlled selection; without it the controls start from what the desktop reports as current. */
  value?: ModelSelectionInput | undefined;
  /** Called on every change with a selection whose access is typed sendable. */
  onChange?: (next: ModelSelection) => void;
  /** The account of a running session was picked: the page opens a new chat instead. */
  onSwitchAccount?: (accountId: string, selection: ModelSelection) => void;
  /** The model id marked "Default" in the picker (U-07). */
  defaultModelId?: string | undefined;
  /** A read-only device cannot change anything. */
  disabled?: boolean;
  /** A turn is running: access changes apply to the next turn. */
  busy?: boolean;
  /** Where the access menu opens. */
  side?: "top" | "bottom";
  onError?: (message: string) => void;
  className?: string;
}

export function ModelControls({
  conversationId,
  projectId,
  value,
  onChange,
  onSwitchAccount,
  defaultModelId,
  disabled = false,
  busy = false,
  side = "top",
  onError,
  className,
}: ModelControlsProps) {
  const catalog = useModelCatalog({ conversationId, projectId });
  const [draft, setDraft] = useState<ModelSelection | null>(null);
  const [open, setOpen] = useState(false);
  const { configure } = useSessionConfigure(conversationId, onError);
  const current = catalog.data;
  const selection = useMemo<ModelSelectionInput | null>(() => {
    if (value) return value;
    if (!current) return null;
    return { ...currentSelection(current), ...(draft ?? {}), runtimeMode: draft?.runtimeMode ?? current.current.runtimeMode };
  }, [value, current, draft]);
  const usage = useAccountUsage(open && selection ? selection.harness : undefined);

  if (!current || !selection) {
    return (
      <div className={cn("flex items-center gap-2", className)}>
        <button type="button" disabled className="flex min-h-11 items-center rounded-pill bg-selection px-3 text-caption text-muted-text opacity-60 pointer-fine:min-h-9">
          {catalog.isError ? "Models unavailable" : "Model"}
        </button>
      </div>
    );
  }

  const apply = (next: ModelSelection) => {
    setDraft(next);
    onChange?.(next);
    if (conversationId) configure(next);
  };
  const summary = summarizeSelection(current, selection);

  return (
    <div className={cn("flex min-w-0 items-center gap-1.5", className)} data-model-controls>
      <button
        type="button"
        aria-label={`Model: ${summary.name}${summary.effort ? `, ${summary.effort}` : ""}`}
        onClick={() => setOpen(true)}
        className="flex max-w-64 min-h-11 min-w-0 items-center gap-1.5 rounded-pill bg-selection px-3 text-caption text-content pointer-fine:min-h-9"
      >
        <HarnessIcon harness={selection.harness} className="size-3.5 shrink-0" />
        <span className="min-w-[6ch] truncate">{summary.name}</span>
        {summary.effort ? <span className="min-w-0 shrink-[8] truncate text-muted-text">{summary.effort}</span> : null}
        <ChevronDown size={12} className="shrink-0 text-muted-text" />
      </button>
      {selection.harness !== "fx" ? (
        <AccessPicker
          value={selection.runtimeMode}
          onChange={(mode: SendableRuntimeMode) => apply(toEmitted(selection, mode))}
          disabled={disabled}
          busy={busy}
          side={side}
        />
      ) : null}
      <ModelPicker
        open={open}
        onOpenChange={setOpen}
        catalog={current}
        value={selection}
        onChange={apply}
        existingSession={Boolean(conversationId)}
        {...(onSwitchAccount ? { onSwitchAccount } : {})}
        defaultModelId={defaultModelId}
        usage={usage}
        disabled={disabled}
        busy={busy}
      />
    </div>
  );
}
