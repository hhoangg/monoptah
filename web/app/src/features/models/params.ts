// Request param builders for the model, effort and access of a session. Every call that carries a
// `runtimeMode` goes through `sendableAccess`, so `full-access` (or anything unknown) can never reach
// the wire from this browser: it is dropped and the desktop keeps what it has.
import type { ParamsOf, SendableRuntimeMode } from "@monoptah/protocol";
import { sendableAccess } from "./access";
import { parseStringRecord, type ModelSelection } from "./modelSettings";

export type SessionConfigureParams = ParamsOf<"session.configure">;

/** The model fields shared by `session.configure` and `session.start`. */
export interface ModelStartFields {
  harness: string;
  model: string;
  modelSettings: Record<string, string>;
  runtimeMode?: SendableRuntimeMode;
  providerAccountId?: string;
}

/** The fields of a selection for `session.start`; spread them next to `projectId` and `text`. */
export function buildStartModelParams(selection: ModelSelection | undefined): Partial<ModelStartFields> {
  if (!selection) return {};
  const runtimeMode = sendableAccess(selection.runtimeMode);
  return {
    harness: selection.harness,
    model: selection.model,
    modelSettings: parseStringRecord(selection.modelSettings),
    ...(runtimeMode ? { runtimeMode } : {}),
    ...(selection.providerAccountId ? { providerAccountId: selection.providerAccountId } : {}),
  };
}

/** `session.configure` params for a running session. */
export function buildModelSettingsParams(conversationId: string, selection: ModelSelection): SessionConfigureParams {
  return { conversationId, ...buildStartModelParams(selection) } as SessionConfigureParams;
}
