// Every desktop method the web can call (architecture.md 7.1, 6.6, features.md 3):
// params and result schemas, permission class and transport. Derived from
//  - crates/monocode-core/src/mobile/host.rs (APP_METHODS, host-answered methods)
//  - crates/monocode-core/src/mobile/catalog.rs (workspace.list, sessions.list)
//  - crates/monocode-gpui/src/shell/mobile_host.rs, mobile/project.rs, monos/mobile.rs
//  - mobile/src/features/** (read only, for the result shapes)
// Results type the fields the web needs and stay open (`z.looseObject`) so a newer
// desktop can add fields. `methods.json` is generated from METHODS (scripts/gen-vectors.ts).
import { z } from "zod";
import { MAX_MESSAGE_CHARS } from "./limits.ts";
import {
  AttachmentsSchema,
  HarnessSchema,
  ModelCatalogSchema,
  QuestionPromptSchema,
  QuestionReplySchema,
  SendableRuntimeModeSchema,
  TurnSchema,
  UsageLimitSchema,
} from "./wire.ts";
import {
  ApprovalsFieldSchema,
  CatalogStartupSchema,
  CloudFieldSchema,
  CloudStateSchema,
  SessionTargetSchema,
  WatchFrameSchema,
} from "./watch.ts";

// Wire types are re-exported here so the WS-T stubs (question.ts, models.ts, session.ts)
// and consumers have one place for them.
export * from "./wire.ts";

// --- Classes and transports ---

/**
 * - `read` / `control`: fixed permission class (a read-only device may call `read`).
 * - `param`: depends on params; resolve with `classify()`.
 * - `denied`: never exposed, or denied for web in v1.
 * - `unsupportedWeb`: answered `code: unsupported` on the web transport.
 */
export const METHOD_CLASSES = ["read", "control", "param", "denied", "unsupportedWeb"] as const;
export type MethodClass = (typeof METHOD_CLASSES)[number];

/** `app`: forwarded to the app (APP_METHODS). `host`: answered in core (MobileHost::handle / watch). */
export const METHOD_TRANSPORTS = ["app", "host"] as const;
export type MethodTransport = (typeof METHOD_TRANSPORTS)[number];

export interface MethodDef<P extends z.ZodType = z.ZodType, R extends z.ZodType = z.ZodType> {
  class: MethodClass;
  transport: MethodTransport;
  /** Only the QUIC transport can call it (`device.enroll`); the Noise link never does. */
  quicOnly?: boolean;
  params: P;
  result: R;
}

function method<P extends z.ZodType, R extends z.ZodType>(def: MethodDef<P, R>): MethodDef<P, R> {
  return def;
}

// --- Shared schemas ---

const ConversationId = z.string().min(1).max(256);
const ProjectId = z.string().min(1);
const Empty = z.object({});
const Open = z.looseObject({});
const Json = z.unknown();
/** `{sessionId}` as most session commands answer (`json!({"sessionId": ...})`). */
const SessionAck = z.looseObject({ sessionId: z.string().optional() });
const ModelSettings = z.record(z.string().max(64), z.string().max(64));
const WorktreePath = z.string().optional();

// --- Catalog (workspace.list, sessions.list) ---

export const SessionCardSchema = z.looseObject({
  id: z.string(),
  projectId: z.string(),
  monoId: z.string().optional(),
  title: z.string(),
  harness: z.string(),
  model: z.string(),
  modelName: z.string().optional(),
  branch: z.string(),
  worktreePath: z.string().optional(),
  /** What the desktop reports: may be `full-access`. */
  runtimeMode: z.string().optional(),
  updatedAt: z.number(),
  busy: z.boolean().optional(),
  waiting: z.enum(["approval", "question"]).optional(),
  // Web transport only (architecture.md 6.6)
  target: SessionTargetSchema.optional(),
  cloudState: CloudStateSchema.optional(),
  startup: CatalogStartupSchema.optional(),
});
export type SessionCard = z.infer<typeof SessionCardSchema>;

export const ProjectCardSchema = z.looseObject({
  id: z.string(),
  name: z.string(),
  path: z.string(),
  machine: z.string().optional(),
  branch: z.string().optional(),
  color: z.string().optional(),
  mascot: z.string().optional(),
  additions: z.number().optional(),
  deletions: z.number().optional(),
  busy: z.boolean().optional(),
  defaultHarness: z.string().optional(),
  defaultModel: z.string().optional(),
  defaultModelName: z.string().optional(),
  defaultRuntimeMode: z.string().optional(),
});

export const MonoCardSchema = z.looseObject({
  id: z.string(),
  name: z.string(),
  mascot: z.string().optional(),
  color: z.string().optional(),
  projectIds: z.array(z.string()).optional(),
  unread: z.number().optional(),
  status: z.string().optional(),
  lastMessage: z.string().optional(),
  lastActiveAt: z.number().optional(),
  harness: z.string().optional(),
  model: z.string().optional(),
  subsessions: z.looseObject({ harness: z.string().optional(), model: z.string().optional(), effort: z.string().optional() }).nullable().optional(),
});

export const WorkspaceListResultSchema = z.looseObject({
  projects: z.array(ProjectCardSchema),
  monos: z.array(MonoCardSchema),
  sessions: z.array(SessionCardSchema),
  /** Capability flags the catalog has always carried (absent on older desktops). */
  monoTools: z.boolean().optional(),
  monoFiles: z.boolean().optional(),
  push: z.boolean().optional(),
});

export const SessionsListParamsSchema = z.object({
  projectId: ProjectId,
  query: z.string().optional(),
  offset: z.number().int().min(0).optional(),
  /** The desktop clamps to 1..100, default 30. */
  limit: z.number().int().min(1).max(100).optional(),
  since: z.number().optional(),
  hiddenHarnesses: z.array(z.string()).optional(),
});

// --- Session start / configure params ---

/**
 * `target` of `session.start` (architecture.md 6.6): only these keys. `kit`, `template`,
 * `image`, `cpus`, `memory`, `ttl` are rejected here (the desktop answers `forbidden`).
 */
export const SessionStartTargetSchema = z.strictObject({
  kind: z.enum(["codespace", "sandbox"]),
  /** `"new"` or a name from `cloud.instances`. */
  instance: z.string().min(1).optional(),
  branch: z.string().optional(),
  /** Codespace machine, from `cloud.startOptions`. */
  machine: z.string().optional(),
  /** Sandbox preset, from `cloud.startOptions`. */
  size: z.string().optional(),
});
export type SessionStartTarget = z.infer<typeof SessionStartTargetSchema>;

/** `runtimeMode` here is `SendableRuntimeModeSchema`: `full-access` is rejected for every remote device. */
export const SessionConfigureParamsSchema = z.object({
  conversationId: ConversationId,
  harness: HarnessSchema.optional(),
  model: z.string().max(256).optional(),
  modelSettings: ModelSettings.optional(),
  runtimeMode: SendableRuntimeModeSchema.optional(),
  providerAccountId: z.string().optional(),
});

export const SessionStartParamsSchema = z.object({
  projectId: ProjectId,
  text: z.string().max(MAX_MESSAGE_CHARS),
  attachments: AttachmentsSchema.optional(),
  harness: HarnessSchema.optional(),
  model: z.string().max(256).optional(),
  modelSettings: ModelSettings.optional(),
  runtimeMode: SendableRuntimeModeSchema.optional(),
  providerAccountId: z.string().optional(),
  worktreePath: WorktreePath,
  workspaceMode: z.literal("worktree").optional(),
  monoId: z.string().optional(),
  /** Cloud start; allowed only while the desktop toggle "Remote cloud start" is on. */
  target: SessionStartTargetSchema.optional(),
});

// --- Git ---

export const GIT_ACTIONS = ["stage", "unstage", "stageAll", "unstageAll", "generate", "commit", "sync", "pr"] as const;
export const GitActionParamsSchema = z.object({
  projectId: ProjectId,
  worktreePath: WorktreePath,
  action: z.enum(GIT_ACTIONS),
  paths: z.array(z.string()).optional(),
  message: z.string().optional(),
  /** `commit` with `push: true` or `createPr: true` pushes: denied unless "Remote push in Git" is on. */
  push: z.boolean().optional(),
  createPr: z.boolean().optional(),
  harness: HarnessSchema.optional(),
});

const ProjectScope = { projectId: ProjectId, worktreePath: WorktreePath };

// --- Cloud (architecture.md 6.6) ---

export const STOP_REPORTS = ["stopping", "takingLong", "stillRunning"] as const;

export const CloudInstanceSchema = z.looseObject({
  kind: z.enum(["codespace", "sandbox"]),
  instance: z.string(),
  displayName: z.string(),
  state: CloudStateSchema,
  rawState: z.string().optional(),
  lastUsedAt: z.number().nullable().optional(),
  sessions: z.array(z.looseObject({ sessionId: z.string(), title: z.string(), busy: z.boolean() })),
  costEstimatePerHour: z.number().optional(),
  stopReport: z.enum(STOP_REPORTS).optional(),
});

export const CloudInstancesResultSchema = z.looseObject({
  instances: z.array(CloudInstanceSchema),
  backends: z.array(z.looseObject({ kind: z.string(), ready: z.boolean().optional() })).optional(),
  /** Billing instances the device's selection hides: shown as "N other instances". */
  otherCount: z.number().int().nonnegative().optional(),
});

export const CloudStopParamsSchema = z.union([
  z.object({ sessionId: z.string().min(1) }),
  z.object({ kind: z.enum(["codespace", "sandbox"]), instance: z.string().min(1) }),
]);

const SessionIdParams = z.object({ sessionId: z.string().min(1) });

export const CloudStartOptionsResultSchema = z.looseObject({
  backends: z.array(z.looseObject({ kind: z.string(), ready: z.boolean() })),
  branches: z.array(z.string()).optional(),
  machines: z.array(z.looseObject({ id: z.string().optional(), name: z.string().optional() })).optional(),
  sizes: z.array(z.looseObject({ id: z.string().optional(), name: z.string().optional() })).optional(),
});

// --- Mono ---

const MonoFile = z.enum(["soul", "memory"]);
const MonoSubsessions = z.object({
  harness: HarnessSchema.optional(),
  model: z.string().max(256).optional(),
  effort: z.string().max(256).optional(),
});

// --- Device selection (architecture.md 7.1, features.md 3.3) ---

/** `null` means all. */
export const DeviceSelectionSchema = z.object({
  projects: z.array(z.string()).nullable(),
  monos: z.array(z.string()).nullable(),
});
export type DeviceSelection = z.infer<typeof DeviceSelectionSchema>;

// --- The table ---

export const METHODS = {
  // Conversations
  "conversation.read": method({
    class: "read",
    transport: "app",
    params: z.object({ conversationId: ConversationId }),
    result: z.looseObject({
      id: z.string().optional(),
      messages: z.array(z.unknown()).optional(),
      turns: z.array(TurnSchema),
      busy: z.boolean().optional(),
      usageLimit: UsageLimitSchema.nullable().optional(),
      older: z.boolean(),
      question: QuestionPromptSchema.nullable().optional(),
      cloud: CloudFieldSchema.optional(),
      approvals: ApprovalsFieldSchema.optional(),
    }),
  }),
  "conversation.older": method({
    class: "read",
    transport: "app",
    params: z.object({ conversationId: ConversationId, before: z.string(), limit: z.number().int().min(1).max(100).optional() }),
    result: z.looseObject({ turns: z.array(TurnSchema), older: z.boolean() }),
  }),
  "conversation.watch": method({
    class: "read",
    transport: "host",
    params: z.object({
      conversationId: ConversationId,
      /** How many of the latest turns to open on. */
      tail: z.number().int().min(0).optional(),
      /** First turn id the client already has. */
      from: z.string().max(256).optional(),
      incremental: z.boolean().optional(),
    }),
    /** Each frame of the stream (the `watching` ack carries no result). */
    result: WatchFrameSchema,
  }),
  "conversation.send": method({
    class: "control",
    transport: "app",
    params: z.object({
      conversationId: ConversationId,
      text: z.string().max(MAX_MESSAGE_CHARS),
      attachments: AttachmentsSchema.optional(),
      /**
       * Consent to wake a stopped cloud instance (billable). Without it a send to a session whose
       * instance would be woken is refused with `wake_required` (architecture.md 6.6).
       */
      wake: z.literal(true).optional(),
    }),
    /** `wake` and `queued`: the send woke a stopped cloud instance (web transport, architecture.md 6.6). */
    result: z.looseObject({ sessionId: z.string().optional(), wake: z.boolean().optional(), queued: z.boolean().optional() }),
  }),

  // Models and accounts
  "models.list": method({
    class: "read",
    transport: "app",
    params: z.object({ conversationId: ConversationId.optional(), projectId: ProjectId.optional() }),
    result: ModelCatalogSchema,
  }),
  "accounts.usage": method({
    class: "param", // read, or control with refresh: true
    transport: "app",
    params: z.object({ provider: z.enum(["claude", "codex"]), refresh: z.boolean().optional() }),
    result: z.looseObject({
      accounts: z.record(
        z.string(),
        z.looseObject({
          status: z.looseObject({ tone: z.string(), label: z.string(), detail: z.string().optional() }),
          updatedAt: z.number(),
          stale: z.boolean(),
          meters: z.array(
            z.looseObject({
              title: z.string(),
              usedPercent: z.number(),
              percent: z.number(),
              label: z.string(),
              reset: z.string(),
            }),
          ),
        }),
      ),
    }),
  }),

  // Sessions
  "session.configure": method({ class: "control", transport: "app", params: SessionConfigureParamsSchema, result: SessionAck }),
  "session.start": method({
    class: "param", // control; with `target` only while "Remote cloud start" is on
    transport: "app",
    params: SessionStartParamsSchema,
    result: SessionAck,
  }),
  "session.resumeUsage": method({
    class: "control",
    transport: "app",
    params: z.object({ conversationId: ConversationId, token: z.string() }),
    result: SessionAck,
  }),
  "session.resumeUsageAtReset": method({
    class: "control",
    transport: "app",
    params: z.object({ conversationId: ConversationId, token: z.string(), enabled: z.boolean() }),
    result: SessionAck,
  }),
  "session.answerQuestion": method({
    class: "control",
    transport: "app",
    params: z.object({ conversationId: ConversationId, requestId: z.number().int().nonnegative(), reply: QuestionReplySchema }),
    result: SessionAck,
  }),
  "session.keepQuestionOpen": method({
    class: "control",
    transport: "app",
    params: z.object({ conversationId: ConversationId, requestId: z.number().int().nonnegative() }),
    result: SessionAck,
  }),
  "session.answerApproval": method({
    class: "control",
    transport: "app",
    // Params are drafted here; WS-T and WS-D2 extend them under X-01.
    params: z.object({
      conversationId: ConversationId,
      requestId: z.union([z.number().int().nonnegative(), z.string()]),
      decision: z.enum(["allow", "deny"]),
    }),
    result: SessionAck,
  }),
  "session.cancel": method({ class: "control", transport: "app", params: z.object({ conversationId: ConversationId }), result: SessionAck }),
  "session.rename": method({
    class: "control",
    transport: "app",
    params: z.object({ conversationId: ConversationId, title: z.string().trim().min(1).max(200) }),
    result: SessionAck,
  }),
  "session.archive": method({
    class: "control",
    transport: "app",
    /** A cloud session needs `cloud: "keep"` or `"stop"` (architecture.md 6.6). */
    params: z.object({ conversationId: ConversationId, cloud: z.enum(["keep", "stop"]).optional() }),
    result: SessionAck,
  }),
  "session.delete": method({ class: "control", transport: "app", params: z.object({ conversationId: ConversationId }), result: SessionAck }),

  // Projects
  "project.changes": method({
    class: "read",
    transport: "app",
    params: z.object(ProjectScope),
    result: z.looseObject({
      branch: z.string(),
      files: z.array(
        z.looseObject({
          relative: z.string(),
          status: z.enum(["modified", "added", "deleted", "untracked"]),
          additions: z.number(),
          deletions: z.number(),
          staged: z.boolean().optional(),
          unstaged: z.boolean().optional(),
          lines: z.array(
            z.looseObject({
              kind: z.enum(["add", "del", "context", "hunk"]),
              text: z.string(),
              oldNumber: z.number().nullable(),
              newNumber: z.number().nullable(),
              runs: z.array(z.tuple([z.string(), z.number()])).optional(),
            }),
          ),
          palette: z.array(z.string()).optional(),
        }),
      ),
      worktreePath: z.string().optional(),
    }),
  }),
  "project.worktrees": method({
    class: "read",
    transport: "app",
    params: z.object({ projectId: ProjectId }),
    result: z.looseObject({ worktrees: z.array(z.looseObject({ path: z.string(), name: z.string() })) }),
  }),
  "project.files": method({
    class: "read",
    transport: "app",
    params: z.object({ ...ProjectScope, path: z.string().optional() }),
    result: z.looseObject({
      entries: z.array(z.looseObject({ name: z.string(), isDir: z.boolean(), ignored: z.boolean() })),
    }),
  }),
  "project.file": method({
    class: "read",
    transport: "app",
    params: z.object({ ...ProjectScope, path: z.string() }),
    result: z.looseObject({
      path: z.string().optional(),
      text: z.string(),
      truncated: z.boolean(),
      highlighted: z.looseObject({ palette: z.array(z.string()), lines: z.array(z.array(z.tuple([z.string(), z.number()]))) }).optional(),
      highlightJob: z.string().optional(),
    }),
  }),
  "project.search": method({
    class: "read",
    transport: "app",
    params: z.object({ ...ProjectScope, query: z.string() }),
    result: z.looseObject({ results: z.array(z.looseObject({ path: z.string(), name: z.string() })) }),
  }),
  // Phones only: the web renders highlights itself (shiki) and gets `unsupported`.
  "project.fileHighlight": method({
    class: "unsupportedWeb",
    transport: "app",
    params: z.object({ job: z.string(), from: z.number().int().min(0).optional() }),
    result: z.looseObject({ palette: z.array(z.string()), lines: z.array(z.unknown()), done: z.boolean() }),
  }),
  "project.git": method({
    class: "read",
    transport: "app",
    params: z.object(ProjectScope),
    result: z.looseObject({
      branch: z.string(),
      files: z.number(),
      remote: z.boolean(),
      upstream: z.boolean(),
      defaultBranch: z.string(),
      ahead: z.number(),
      behind: z.number(),
      aheadOfDefault: z.number().optional(),
      pr: z.looseObject({ number: z.number(), url: z.string() }).nullable().optional(),
    }),
  }),
  "project.gitAction": method({
    class: "param", // control for stage, unstage, generate and a commit that does not push; denied for sync, pr and a pushing commit unless "Remote push in Git" is on
    transport: "app",
    params: GitActionParamsSchema,
    result: z.looseObject({ message: z.string().optional(), url: z.string().optional() }),
  }),

  // Monos
  "mono.document": method({
    class: "read",
    transport: "app",
    params: z.object({ documentId: z.string().min(1) }),
    result: Open,
  }),
  "mono.profile": method({ class: "read", transport: "app", params: z.object({ monoId: z.string().min(1) }), result: Open }),
  "mono.file": method({
    class: "read",
    transport: "app",
    params: z.object({ monoId: z.string().min(1), file: MonoFile }),
    result: z.looseObject({ text: z.string(), hash: z.string() }),
  }),
  "mono.create": method({ class: "control", transport: "app", params: Empty, result: z.looseObject({ monoId: z.string() }) }),
  "mono.update": method({
    class: "control",
    transport: "app",
    params: z.object({
      monoId: z.string().min(1),
      name: z.string().max(40).optional(),
      mascot: z.string().optional(),
      color: z.string().regex(/^#?[0-9a-fA-F]{6}$/).optional(),
      subsessions: MonoSubsessions.nullable().optional(),
      addProject: z.string().optional(),
      removeProject: z.string().optional(),
    }),
    result: Json,
  }),
  // Denied for web in v1 (security.md 7): hidden in the UI.
  "mono.saveFile": method({
    class: "denied",
    transport: "app",
    params: z.object({ monoId: z.string().min(1), file: MonoFile, text: z.string(), expectedHash: z.string().optional() }),
    result: z.looseObject({ hash: z.string() }),
  }),
  "mono.reset": method({ class: "denied", transport: "app", params: z.object({ monoId: z.string().min(1) }), result: Json }),
  "mono.delete": method({ class: "denied", transport: "app", params: z.object({ monoId: z.string().min(1) }), result: Json }),

  // Answered in core (host)
  "workspace.list": method({ class: "read", transport: "host", params: Empty, result: WorkspaceListResultSchema }),
  "sessions.list": method({
    class: "read",
    transport: "host",
    params: SessionsListParamsSchema,
    result: z.looseObject({ sessions: z.array(SessionCardSchema), hasMore: z.boolean() }),
  }),
  "device.leave": method({ class: "read", transport: "host", params: Empty, result: Json }),
  /** Expo push tokens: not applicable on web (Web Push is held by the Worker). */
  "device.push": method({ class: "unsupportedWeb", transport: "host", params: Open, result: Json }),
  /** The Noise-based pairing replaces it; only the QUIC phone invitation still uses it. */
  "device.enroll": method({ class: "denied", transport: "host", quicOnly: true, params: Open, result: Json }),
  "device.selection": method({
    class: "param", // get: read; set: control, narrowing only
    transport: "host",
    /** Empty params read the selection; `projects` or `monos` present sets it (may only narrow). */
    params: z.object({ projects: z.array(z.string()).nullable().optional(), monos: z.array(z.string()).nullable().optional() }),
    result: DeviceSelectionSchema,
  }),

  // Cloud (architecture.md 6.6)
  "cloud.instances": method({ class: "read", transport: "app", params: Empty, result: CloudInstancesResultSchema }),
  "cloud.startOptions": method({
    class: "read",
    transport: "app",
    params: z.object({ projectId: ProjectId }),
    result: CloudStartOptionsResultSchema,
  }),
  "cloud.stop": method({
    class: "control",
    transport: "app",
    params: CloudStopParamsSchema,
    /** `stopped: true` only means requested; progress is `stopReport`. `warning` names interrupted busy sessions. */
    result: z.looseObject({ stopped: z.boolean(), warning: z.string().optional() }),
  }),
  "cloud.resume": method({
    class: "control",
    transport: "app",
    params: SessionIdParams,
    /** Answers at once; progress arrives as `cloud` fields in the watch frame. */
    result: z.looseObject({ accepted: z.boolean() }),
  }),
  "cloud.retryStart": method({ class: "control", transport: "app", params: SessionIdParams, result: Json }),
  "cloud.keepRunning": method({ class: "control", transport: "app", params: SessionIdParams, result: Json }),
  "cloud.moveToCloud": method({
    class: "control", // only while "Remote cloud start" and "Remote push in Git" are on
    transport: "app",
    params: z.object({ sessionId: z.string().min(1), target: SessionStartTargetSchema.optional() }),
    result: Json,
  }),
  "cloud.moveToLocal": method({ class: "control", transport: "app", params: SessionIdParams, result: Json }),
  "cloud.cancelMove": method({ class: "control", transport: "app", params: SessionIdParams, result: Json }),
} as const;

export type MethodName = keyof typeof METHODS;
export type ParamsOf<M extends MethodName> = z.input<(typeof METHODS)[M]["params"]>;
export type ResultOf<M extends MethodName> = z.output<(typeof METHODS)[M]["result"]>;

export function isMethodName(name: string): name is MethodName {
  return Object.hasOwn(METHODS, name);
}

/** Entries of the table with their names, sorted by name. */
export const METHOD_LIST: readonly { name: MethodName; def: MethodDef }[] = (Object.keys(METHODS) as MethodName[])
  .sort()
  .map((name) => ({ name, def: METHODS[name] }));

// --- Classification (architecture.md 7.1) ---

/** What a call needs after the params are known. */
export type ResolvedClass = "read" | "control" | "denied" | "unsupportedWeb";

/** Desktop toggles that open denied paths (default off). */
export interface ClassifyFlags {
  /** "Remote push in Git". */
  remoteGitPush?: boolean;
  /** "Remote cloud start". */
  remoteCloudStart?: boolean;
}

const GIT_CONTROL_ACTIONS: ReadonlySet<string> = new Set(["stage", "unstage", "stageAll", "unstageAll", "generate"]);

function record(value: unknown): Record<string, unknown> {
  return typeof value === "object" && value !== null ? (value as Record<string, unknown>) : {};
}

/**
 * Resolves the permission class of a call. `param` methods look at `params`; fixed
 * classes are returned as they are, except that `full-access` in `session.configure`
 * or `session.start` and the toggle-gated cloud moves are `denied`. Unknown methods
 * return `"unknown"` (the desktop answers `Unsupported mobile operation`).
 */
export function classify(name: string, params: unknown = {}, flags: ClassifyFlags = {}): ResolvedClass | "unknown" {
  if (!isMethodName(name)) return "unknown";
  const p = record(params);
  switch (name) {
    case "accounts.usage":
      return p.refresh === true ? "control" : "read";
    case "device.selection":
      return "projects" in p || "monos" in p ? "control" : "read";
    case "project.gitAction": {
      const action = p.action;
      if (typeof action !== "string") return "denied";
      if (GIT_CONTROL_ACTIONS.has(action)) return "control";
      if (action === "commit") {
        const pushes = p.push === true || p.createPr === true;
        return pushes && !flags.remoteGitPush ? "denied" : "control";
      }
      if (action === "sync" || action === "pr") return flags.remoteGitPush ? "control" : "denied";
      return "denied";
    }
    case "session.start":
      if (p.runtimeMode === "full-access") return "denied";
      if (p.target !== undefined && p.target !== null) return flags.remoteCloudStart ? "control" : "denied";
      return "control";
    case "session.configure":
      return p.runtimeMode === "full-access" ? "denied" : "control";
    case "cloud.moveToCloud":
    case "cloud.moveToLocal":
    case "cloud.cancelMove":
      return flags.remoteCloudStart && flags.remoteGitPush ? "control" : "denied";
    default: {
      const cls = METHODS[name].class;
      // `param` entries are all handled above.
      return cls === "param" ? "denied" : cls;
    }
  }
}
