// Leaf wire types shared by methods.ts, watch.ts and the WS-T stubs. Derived from
// the real desktop code (crates/monocode-gpui/src/shell/mobile_host.rs, the mobile
// app types in mobile/src/features/workspace/types.ts and models/types.ts).
// Results are typed for the fields the web needs and stay open (looseObject) so
// a newer desktop can add fields without breaking an older web build.
import "./zod-config.ts";
import { z } from "zod";
import { MAX_ATTACHMENTS } from "./limits.ts";

/** Desktop `HarnessId` (mobile/src/features/workspace/types.ts). Params use the enum; results use plain strings. */
export const HARNESS_IDS = [
  "claude",
  "codex",
  "cursor",
  "grok",
  "opencode",
  "pi",
  "omp",
  "fx",
  "hermes",
  "antigravity",
  "devin",
] as const;
export const HarnessSchema = z.enum(HARNESS_IDS);
export type Harness = z.infer<typeof HarnessSchema>;

/**
 * Permission modes the desktop may REPORT (`models.list`, session cards). Includes
 * `full-access`, which the web must never send (architecture.md 7.1).
 */
export const RuntimeModeSchema = z.enum(["supervised", "auto-accept-edits", "auto", "full-access"]);
export type RuntimeMode = z.infer<typeof RuntimeModeSchema>;

/** Permission modes the web may SEND in `session.configure` and `session.start`: no `full-access`. */
export const SendableRuntimeModeSchema = z.enum(["supervised", "auto-accept-edits", "auto"]);
export type SendableRuntimeMode = z.infer<typeof SendableRuntimeModeSchema>;

/** One attachment on the wire (`WireAttachment`: name, mimeType, base64 data). At most 10 per message (limits.ts). */
export const AttachmentSchema = z.object({
  name: z.string().trim().min(1).max(255),
  mimeType: z.string().max(255),
  /** Standard base64 of the file bytes. */
  data: z.string(),
});
export type Attachment = z.infer<typeof AttachmentSchema>;
export const AttachmentsSchema = z.array(AttachmentSchema).max(MAX_ATTACHMENTS);

// --- Questions (desktop `UserQuestionPrompt`) ---

export const QuestionOptionSchema = z.looseObject({
  id: z.string(),
  label: z.string(),
  description: z.string().optional(),
});
export const QuestionSchema = z.looseObject({
  id: z.string(),
  header: z.string().optional(),
  prompt: z.string(),
  multiSelect: z.boolean(),
  allowCustom: z.boolean(),
  options: z.array(QuestionOptionSchema),
});
export const QuestionPromptSchema = z.looseObject({
  requestId: z.number().int().nonnegative(),
  title: z.string().optional(),
  questions: z.array(QuestionSchema),
  /** When the agent carries on without an answer, if it would (ms epoch). */
  autoResolveAt: z.number().optional(),
});
export type QuestionPrompt = z.infer<typeof QuestionPromptSchema>;

/** `reply` of `session.answerQuestion`. */
export const QuestionReplySchema = z.discriminatedUnion("kind", [
  z.object({
    kind: z.literal("answered"),
    /** Question id -> chosen option ids. */
    answers: z.record(z.string(), z.array(z.string())),
    /** Question id -> free text, when the question allows a custom answer. */
    custom: z.record(z.string(), z.string()).optional(),
  }),
  z.object({ kind: z.literal("skipped") }),
]);
export type QuestionReply = z.infer<typeof QuestionReplySchema>;

// --- Usage limit (desktop `usage_state`; null when there is none) ---

export const UsageLimitSchema = z.looseObject({
  token: z.string(),
  resetsAt: z.number().nullable().optional(),
  resumeAtReset: z.boolean(),
  canResume: z.boolean(),
  canAutoResume: z.boolean(),
  resetLabel: z.string(),
  limitation: z.string(),
});
export type UsageLimit = z.infer<typeof UsageLimitSchema>;

// --- Turns (already projected by the desktop; the web renders them) ---

export const TurnSchema = z.looseObject({
  id: z.string(),
  items: z.array(z.looseObject({ type: z.string(), id: z.string().optional() })),
  stampAt: z.number().optional(),
  completedAt: z.number().optional(),
  footer: z.boolean().optional(),
  copyText: z.string().optional(),
  durationMs: z.number().optional(),
});
export type Turn = z.infer<typeof TurnSchema>;

// --- Models (`models.list`) ---

export const ModelSettingSchema = z.looseObject({
  id: z.string(),
  label: z.string(),
  kind: z.enum(["select", "toggle"]),
  value: z.string(),
  options: z.array(z.object({ value: z.string(), label: z.string() })),
  description: z.string().optional(),
});
export const CatalogModelSchema = z.looseObject({
  id: z.string(),
  harness: z.string(),
  name: z.string(),
  provider: z.string().optional(),
  settings: z.array(ModelSettingSchema),
});
export type CatalogModel = z.infer<typeof CatalogModelSchema>;

/** What a session runs with. `runtimeMode` is what the desktop reports, so it may be `full-access`. */
export const ModelChoiceSchema = z.looseObject({
  harness: z.string(),
  model: z.string(),
  modelSettings: z.record(z.string(), z.string()),
  runtimeMode: RuntimeModeSchema,
  providerAccountId: z.string().optional(),
});
export type ModelChoice = z.infer<typeof ModelChoiceSchema>;

export const ModelCatalogSchema = z.looseObject({
  harnesses: z.array(z.looseObject({ id: z.string(), label: z.string() })),
  models: z.array(CatalogModelSchema),
  /**
   * The desktop drops `full-access` from this list for every remote device
   * (security.md 7); the entry id is still typed with the reported enum.
   */
  runtimeModes: z.array(z.looseObject({ id: RuntimeModeSchema, label: z.string(), hint: z.string() })),
  accounts: z
    .array(z.looseObject({ id: z.string(), provider: z.string(), label: z.string(), isDefault: z.boolean().optional() }))
    .optional(),
  selectedAccounts: z.record(z.string(), z.string()).optional(),
  current: ModelChoiceSchema,
});
export type ModelCatalog = z.infer<typeof ModelCatalogSchema>;
