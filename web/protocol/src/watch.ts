// Watch frame result fields (crates/monocode-core/src/mobile/watch.rs) plus the
// cloud and approvals fields of architecture.md 6.6. A watch frame is the `result`
// of `{rid, ok: true, result}` after `{rid, ok: true, watching: true}`.
import "./zod-config.ts";
import { z } from "zod";
import { QuestionPromptSchema, TurnSchema, UsageLimitSchema } from "./wire.ts";

// --- Cloud (architecture.md 6.6 "Fields") ---

/** `target.kind`: `remote` is new (a session on a monocode-host machine, no cloud binding). */
export const TARGET_KINDS = ["local", "remote", "codespace", "sandbox"] as const;
export const TargetKindSchema = z.enum(TARGET_KINDS);
export type TargetKind = z.infer<typeof TargetKindSchema>;

/** `cloudState` (cloud_sessions.rs:465). `unknown` is shown as "Not connected". */
export const CLOUD_STATES = ["available", "starting", "stopping", "stopped", "unavailable", "unknown"] as const;
export const CloudStateSchema = z.enum(CLOUD_STATES);
export type CloudState = z.infer<typeof CloudStateSchema>;

/** The 9 startup steps (startup.rs:96, 202). */
export const STARTUP_STEP_IDS = [
  "create",
  "provision",
  "buildContainer",
  "blockingCommands",
  "configureCodespace",
  "connect",
  "openProject",
  "claudeSetup",
  "lifecycleCommands",
] as const;
export const StartupStepIdSchema = z.enum(STARTUP_STEP_IDS);
export type StartupStepId = z.infer<typeof StartupStepIdSchema>;

export const STEP_STATUSES = ["pending", "running", "done", "failed", "skipped"] as const;
export const StepStatusSchema = z.enum(STEP_STATUSES);
export type StepStatus = z.infer<typeof StepStatusSchema>;

/** The 7 move steps (move.rs:164-176). */
export const MOVE_STEP_IDS = ["snapshot", "push", "workspace", "upload", "restoreCode", "restoreConversation", "flip"] as const;
export const MoveStepIdSchema = z.enum(MOVE_STEP_IDS);
export type MoveStepId = z.infer<typeof MoveStepIdSchema>;

/** Catalog session card `target`: `instance` is the codespace or sandbox name (metadata only). */
export const SessionTargetSchema = z.looseObject({
  kind: TargetKindSchema,
  instance: z.string().optional(),
  state: CloudStateSchema.optional(),
});
export type SessionTarget = z.infer<typeof SessionTargetSchema>;

/** Catalog session card `startup`: only while the instance starts or its start failed. `error` is sanitized by the desktop. */
export const CatalogStartupSchema = z.looseObject({
  step: StartupStepIdSchema,
  error: z.string().optional(),
});
export type CatalogStartup = z.infer<typeof CatalogStartupSchema>;

export const StartupStepSchema = z.looseObject({
  id: StartupStepIdSchema,
  status: StepStatusSchema,
  startedAt: z.number().nullable().optional(),
  finishedAt: z.number().nullable().optional(),
});

export const CloudStartupSchema = z.looseObject({
  steps: z.array(StartupStepSchema),
  current: StartupStepIdSchema.nullable().optional(),
  error: z.string().optional(),
});

/** `cloud` field of `conversation.read` and watch frames (sent on change only). */
export const CloudFieldSchema = z.looseObject({
  kind: TargetKindSchema,
  instance: z.string(),
  state: CloudStateSchema,
  startup: CloudStartupSchema.optional(),
  /** A failed start left a billable instance (Stop is allowed on it). */
  kept: z.literal(true).optional(),
  /** An auto-stop is announced: `at` is ms epoch. */
  autoStop: z.looseObject({ at: z.number(), reason: z.string() }).optional(),
  move: z.looseObject({ step: MoveStepIdSchema, status: StepStatusSchema }).optional(),
});
export type CloudField = z.infer<typeof CloudFieldSchema>;

/**
 * `approvals` field (sent on change only; an empty array clears). `label` is
 * `Approval.title`, `detail` is `Approval.detail`: provider text, untrusted. Only
 * present when the desktop advertises `approvals`.
 */
export const ApprovalSchema = z.looseObject({
  requestId: z.union([z.number().int().nonnegative(), z.string()]),
  label: z.string(),
  detail: z.string(),
});
export type Approval = z.infer<typeof ApprovalSchema>;
export const ApprovalsFieldSchema = z.array(ApprovalSchema);

/** Fields added to `conversation.read` results and watch frames for cloud sessions and approvals. */
export const CloudApprovalFields = {
  cloud: CloudFieldSchema.optional(),
  approvals: ApprovalsFieldSchema.optional(),
};

// --- Watch frame (watch.rs `update`) ---

/**
 * One edit of a turn. `path` uses JSON array indexes and object keys; `from` and
 * `append` count UTF-16 code units; `remove` deletes the key or array item.
 */
export const WatchEditSchema = z.union([
  z.object({ path: z.array(z.union([z.string(), z.number()])), from: z.number().int().nonnegative(), append: z.string() }),
  z.object({ path: z.array(z.union([z.string(), z.number()])), remove: z.literal(true) }),
  z.object({ path: z.array(z.union([z.string(), z.number()])), value: z.unknown() }),
]);
export type WatchEdit = z.infer<typeof WatchEditSchema>;

export const WatchPatchSchema = z.object({ id: z.string(), edits: z.array(WatchEditSchema) });
export type WatchPatch = z.infer<typeof WatchPatchSchema>;

export const WatchFrameSchema = z.looseObject({
  usageLimit: UsageLimitSchema.nullable(),
  busy: z.boolean(),
  older: z.boolean(),
  /** Every turn id in order; `turns` carries only the changed ones. */
  order: z.array(z.string()),
  /** Present on the first frame and whenever it changes; `null` once answered. */
  question: QuestionPromptSchema.nullable().optional(),
  turns: z.array(TurnSchema),
  patches: z.array(WatchPatchSchema).optional(),
  ...CloudApprovalFields,
});
export type WatchFrame = z.infer<typeof WatchFrameSchema>;
