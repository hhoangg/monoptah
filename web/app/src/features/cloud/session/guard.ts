// `send-guard` and `session-capabilities` of U-16 (workstreams.md 3.2).
import type { CloudState } from "@monoptah/protocol";
import type { SendGuard, SessionCapabilityPredicate } from "@/ui/hooks";
import { COST_NOTICES, STARTING_REFUSAL, STOPPING_REFUSAL } from "../copy";
import { isBillableKind, isNonLocalKind } from "../model";
import { getLiveCloudState } from "./liveState";

/**
 * Sends to a cloud instance: `starting` and `stopping` are refused, `stopped` asks to confirm the wake cost,
 * an empty session id (the New session page) asks to confirm the start cost. Local or absent targets pass.
 */
export const cloudSendGuard: SendGuard = ({ sessionId, target }) => {
  if (!target || !isBillableKind(target.kind)) return Promise.resolve({ kind: "allow" });
  if (sessionId === "") return Promise.resolve({ kind: "confirm", notice: COST_NOTICES.start });
  const catalog = (target as { state?: CloudState }).state;
  const state = getLiveCloudState(sessionId) ?? catalog;
  if (state === "starting") return Promise.resolve({ kind: "refuse", reason: STARTING_REFUSAL });
  if (state === "stopping") return Promise.resolve({ kind: "refuse", reason: STOPPING_REFUSAL });
  if (state === "stopped") return Promise.resolve({ kind: "confirm", notice: COST_NOTICES.wake });
  return Promise.resolve({ kind: "allow" });
};

/** A session that is not local hides Delete, Files, Changes and Git (the desktop owns them). */
export const cloudSessionCapabilities: SessionCapabilityPredicate = (_capability, { target }) => !isNonLocalKind(target?.kind);
