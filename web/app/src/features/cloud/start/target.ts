// Pure helpers of the target picker (U-18, U-19): the value the `new-session-target` slot holds, the
// options derived from `cloud.startOptions`, and the one place that builds an emitted target. Only keys
// of `SessionStartTargetSchema` (kind, instance, branch, machine, size) can ever be produced.
import { SessionStartTargetSchema, type ResultOf, type SessionStartTarget } from "@monoptah/protocol";
import type { BillableKind } from "../model";

export type StartOptions = ResultOf<"cloud.startOptions">;

export const NEW_INSTANCE = "new";

/** The target a slot value holds, or null (local) when the value is empty or not a valid target. */
export function parseTarget(value: unknown): SessionStartTarget | null {
  if (value === null || value === undefined) return null;
  const parsed = SessionStartTargetSchema.safeParse(value);
  return parsed.success ? parsed.data : null;
}

/** Backends the desktop reports as ready, narrowed to the kinds a web start may use. */
export function readyKinds(options: StartOptions | undefined): BillableKind[] {
  const kinds: BillableKind[] = [];
  for (const backend of options?.backends ?? []) {
    if (!backend.ready) continue;
    if ((backend.kind === "codespace" || backend.kind === "sandbox") && !kinds.includes(backend.kind)) kinds.push(backend.kind);
  }
  return kinds;
}

export interface Choice {
  id: string;
  name: string;
}

/** Machines or sizes from startOptions that have an id; a missing name falls back to the id. */
export function choicesOf(list: ReadonlyArray<{ id?: string | undefined; name?: string | undefined }> | undefined): Choice[] {
  const out: Choice[] = [];
  for (const entry of list ?? []) {
    if (typeof entry.id === "string" && entry.id !== "") out.push({ id: entry.id, name: entry.name?.trim() ? entry.name : entry.id });
  }
  return out;
}

/** Builds a target from the picker fields. Empty fields are left out, so the desktop applies its default. */
export function buildTarget(fields: { kind: BillableKind; instance?: string | undefined; branch?: string | undefined; machine?: string | undefined; size?: string | undefined }): SessionStartTarget {
  const target: SessionStartTarget = { kind: fields.kind, instance: fields.instance || NEW_INSTANCE };
  if (fields.branch) target.branch = fields.branch;
  if (fields.kind === "codespace" && target.instance === NEW_INSTANCE && fields.machine) target.machine = fields.machine;
  if (fields.kind === "sandbox" && target.instance === NEW_INSTANCE && fields.size) target.size = fields.size;
  return target;
}

/**
 * Drops what the current options no longer offer: a backend that is not ready or an instance that no longer
 * exists (the whole target goes back to local: never silently to a NEW billable instance, the user picks again), a branch, machine or size that is not in the lists. `instances`
 * is undefined while the instance list is not loaded, and then an instance is kept. Returns the same object
 * when nothing changed.
 */
export function sanitizeTarget(
  value: SessionStartTarget | null,
  options: StartOptions,
  instances: ReadonlyArray<{ kind: string; instance: string }> | undefined,
): SessionStartTarget | null {
  if (!value) return value;
  const kind = value.kind;
  if (kind !== "codespace" && kind !== "sandbox") return null;
  if (!readyKinds(options).includes(kind)) return null;
  const instance = value.instance;
  if (instance && instance !== NEW_INSTANCE && instances && !instances.some((candidate) => candidate.kind === kind && candidate.instance === instance)) return null;
  const branch = value.branch !== undefined && (options.branches ?? []).includes(value.branch) ? value.branch : undefined;
  const machine = value.machine !== undefined && choicesOf(options.machines).some((choice) => choice.id === value.machine) ? value.machine : undefined;
  const size = value.size !== undefined && choicesOf(options.sizes).some((choice) => choice.id === value.size) ? value.size : undefined;
  const next = buildTarget({ kind, instance, branch, machine, size });
  return JSON.stringify(next) === JSON.stringify(value) ? value : next;
}
