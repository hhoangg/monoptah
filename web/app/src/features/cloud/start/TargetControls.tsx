// The controls of the target picker, shared by the New session page (U-18) and the "Move to cloud" dialog
// (U-19): kind, instance, branch, and machine (codespace) or size (sandbox), all from `cloud.startOptions`
// and `cloud.instances`. There is deliberately no kit, template, image, cpu, memory or ttl control.
import { useId, type ComponentProps } from "react";
import type { SessionStartTarget } from "@monoptah/protocol";
import { cn } from "@/lib/utils";
import { SegmentedControl } from "@/ui/SegmentedControl";
import { CLAUDE_ONLY_NOTE } from "../copy";
import { useCloudInstances } from "../instances/useCloudInstances";
import { kindLabel, stateLabel, type BillableKind } from "../model";
import { NEW_INSTANCE, buildTarget, choicesOf, readyKinds, type StartOptions } from "./target";

const NONE = "none";

function SelectField({ label, className, children, ...props }: { label: string } & ComponentProps<"select">) {
  const id = useId();
  return (
    <div className={cn("flex flex-col gap-1", className)}>
      <label htmlFor={id} className="text-meta font-medium text-muted-text">
        {label}
      </label>
      <select id={id} {...props} className="min-h-11 w-full min-w-0 rounded-row border border-stroke bg-fill px-3 py-2 text-row text-content disabled:pointer-events-none disabled:opacity-40 pointer-fine:min-h-9">
        {children}
      </select>
    </div>
  );
}

export interface TargetControlsProps {
  options: StartOptions;
  value: SessionStartTarget | null;
  onChange: (target: SessionStartTarget | null) => void;
  /** Label of the option that means "no cloud target": Local on the New session page, a desktop default when moving. */
  noneLabel: string;
  disabled?: boolean;
}

export function TargetControls({ options, value, onChange, noneLabel, disabled = false }: TargetControlsProps) {
  const instances = useCloudInstances();
  const kinds = readyKinds(options);
  const kind = value?.kind;
  const instanceValue = value?.instance ?? NEW_INSTANCE;
  const existing = (instances.data?.instances ?? []).filter((candidate) => candidate.kind === kind);
  const branches = options.branches ?? [];
  const machines = choicesOf(options.machines);
  const sizes = choicesOf(options.sizes);

  const update = (patch: Partial<{ instance: string; branch: string; machine: string; size: string }>) => {
    if (!kind) return;
    onChange(buildTarget({ kind, instance: instanceValue, branch: value?.branch, machine: value?.machine, size: value?.size, ...patch }));
  };

  const kindOptions = [{ value: NONE, label: noneLabel }, ...kinds.map((candidate) => ({ value: candidate as string, label: kindLabel(candidate) }))];
  const pickKind = (next: string) => {
    if (next === NONE) return onChange(null);
    if (next === kind) return;
    onChange(buildTarget({ kind: next as BillableKind }));
  };

  return (
    <fieldset disabled={disabled} className="flex min-w-0 flex-col gap-3 border-0 p-0">
      <SegmentedControl label="Where it runs" options={kindOptions} value={kind ?? NONE} onChange={pickKind} />
      {kind ? (
        <>
          <p className="text-small text-muted-text">{CLAUDE_ONLY_NOTE}</p>
          <SelectField label="Instance" value={instanceValue} onChange={(event) => update({ instance: event.target.value })}>
            <option value={NEW_INSTANCE}>New instance</option>
            {existing.map((candidate) => (
              <option key={candidate.instance} value={candidate.instance}>
                {candidate.displayName} ({stateLabel(candidate.state)})
              </option>
            ))}
          </SelectField>
          {branches.length > 0 ? (
            <SelectField label="Branch" value={value?.branch ?? ""} onChange={(event) => update({ branch: event.target.value })}>
              <option value="">Default branch</option>
              {branches.map((branch) => (
                <option key={branch} value={branch}>
                  {branch}
                </option>
              ))}
            </SelectField>
          ) : null}
          {kind === "codespace" && instanceValue === NEW_INSTANCE && machines.length > 0 ? (
            <SelectField label="Machine" value={value?.machine ?? ""} onChange={(event) => update({ machine: event.target.value })}>
              <option value="">Default machine</option>
              {machines.map((machine) => (
                <option key={machine.id} value={machine.id}>
                  {machine.name}
                </option>
              ))}
            </SelectField>
          ) : null}
          {kind === "sandbox" && instanceValue === NEW_INSTANCE && sizes.length > 0 ? (
            <SelectField label="Size" value={value?.size ?? ""} onChange={(event) => update({ size: event.target.value })}>
              <option value="">Default size</option>
              {sizes.map((size) => (
                <option key={size.id} value={size.id}>
                  {size.name}
                </option>
              ))}
            </SelectField>
          ) : null}
        </>
      ) : null}
    </fieldset>
  );
}
