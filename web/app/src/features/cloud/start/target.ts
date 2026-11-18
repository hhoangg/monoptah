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
