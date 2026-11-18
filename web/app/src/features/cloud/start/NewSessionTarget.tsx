// `new-session-target` slot (U-18): a chip on the New session page that names where the session runs
// (Local by default) and opens the picker. Shown only when the desktop advertises `cloud` and `cloudStart`.
// Controlled: `value` and `onChange(target | null)`, null means local. The start cost notice is not a gate
// here: the confirm comes from the send-guard (an empty session id with a cloud kind).
import { useState } from "react";
import type { SessionStartTarget } from "@monoptah/protocol";
import { useLinkQuery } from "@/data";
import { Icon } from "@/ui/Icon";
import { Notice } from "@/ui/Notice";
import { ResponsiveOverlay } from "@/ui/ResponsiveOverlay";
import { Button } from "@/ui/button";
import { useCapability } from "@/version";
import { COST_NOTICES, READ_ONLY_NOTE, START_DISABLED_NOTE } from "../copy";
import { kindLabel, useCanControl } from "../model";
import { TargetControls } from "./TargetControls";
import { parseTarget, readyKinds } from "./target";

export interface NewSessionTargetProps {
  projectId: string;
  value?: unknown;
  onChange?: (target: unknown) => void;
}

export function NewSessionTarget({ projectId, value, onChange }: NewSessionTargetProps) {
  const cloud = useCapability("cloud");
  const cloudStart = useCapability("cloudStart");
  const enabled = cloud && cloudStart;
  const canControl = useCanControl();
  const options = useLinkQuery("cloud.startOptions", { projectId }, { enabled });
  const [open, setOpen] = useState(false);
  if (!enabled) return null;

  const target = parseTarget(value);
  const disabledByDesktop = options.isError && options.error.code === "disabled";
  const noKinds = options.data !== undefined && readyKinds(options.data).length === 0;
  const emit = (next: SessionStartTarget | null) => onChange?.(next);

  return (
    <>
      <button
        type="button"
        aria-label={`Where it runs: ${target ? kindLabel(target.kind) : "Local"}`}
        aria-haspopup="dialog"
        disabled={!canControl}
        onClick={() => setOpen(true)}
        data-testid="target-chip"
        className="inline-flex min-h-9 items-center gap-1.5 rounded-pill bg-fill px-3 text-label text-content hover:bg-selection disabled:pointer-events-none disabled:opacity-40"
      >
        <Icon name="computer" size={14} />
        {target ? kindLabel(target.kind) : "Local"}
        <Icon name="chevronDown" size={12} />
      </button>
      <ResponsiveOverlay
        open={open}
        onOpenChange={setOpen}
        title="Where it runs"
        footer={
          <Button type="button" onClick={() => setOpen(false)}>
            Done
          </Button>
        }
      >
        <div className="flex flex-col gap-3">
          {!canControl ? <Notice>{READ_ONLY_NOTE}</Notice> : null}
          {disabledByDesktop ? <Notice>{START_DISABLED_NOTE}</Notice> : null}
          {options.isError && !disabledByDesktop ? <Notice variant="error">{options.error.message.trim() || "Could not load the cloud options."}</Notice> : null}
          {noKinds ? <Notice>No cloud backend is ready on your desktop. Sessions start locally.</Notice> : null}
          {options.data ? <TargetControls options={options.data} value={target} onChange={emit} noneLabel="Local" disabled={!canControl || noKinds} /> : null}
          {target ? <Notice>{COST_NOTICES.start}</Notice> : null}
        </div>
      </ResponsiveOverlay>
    </>
  );
}
