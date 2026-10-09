import { nativeModelId } from "../../../features/sessions/model/models";
import type {
  HarnessId,
  Session,
} from "../../../features/sessions/model/session";
import { runtimeModeToPermission } from "../providers/claude/claudeProtocol";

/** What a provider's interactive CLI is known to accept at launch. */
export type TuiCaps = {
  /** Reopens its own conversation by id, so a restored session continues. */
  resume: boolean;
  /** Takes the first prompt as a trailing positional argument. */
  initialPrompt: boolean;
  /** Takes `--model` and `--permission-mode`. */
  launchFlags: boolean;
};

const NONE: Readonly<TuiCaps> = {
  resume: false,
  initialPrompt: false,
  launchFlags: false,
};

/**
 * Typed as a full `Record<HarnessId, …>` so a new harness id fails to compile
 * until it gets an entry here. v1 only knows Claude's interactive flags;
 * everyone else launches the bare binary until their syntax is verified, then
 * a flag flips here (and the builder grows a formatter for it).
 */
export const TUI_CAPS: Record<HarnessId, TuiCaps> = {
  claude: { resume: true, initialPrompt: true, launchFlags: true },
  codex: { ...NONE },
  cursor: { ...NONE },
  grok: { ...NONE },
  opencode: { ...NONE },
  pi: { ...NONE },
  omp: { ...NONE },
  fx: { ...NONE },
  hermes: { ...NONE },
  devin: { ...NONE },
  antigravity: { ...NONE },
};

export type TuiLaunch = {
  program: string;
  args: string[];
  /** Set when the launch minted a new provider id the caller must persist. */
  providerSessionId?: string;
};

export type TuiLaunchOptions = {
  /** Resolved CLI path, so a user-configured binary is honoured. */
  binaryPath: string;
  /** Sent as the first prompt of a new session only. */
  initialPrompt?: string;
  /**
   * Id for a new conversation, instead of minting one. Used to claim an id that
   * was saved but never reached the CLI, which `--resume` cannot open.
   */
  newProviderSessionId?: string;
};

/** Program and arguments that start `session`'s interactive CLI in a PTY. */
export function buildTuiLaunch(
  session: Pick<
    Session,
    "harness" | "model" | "runtimeMode" | "providerSessionId"
  >,
  options: TuiLaunchOptions,
): TuiLaunch {
  const caps = TUI_CAPS[session.harness];
  const args: string[] = [];
  let providerSessionId: string | undefined;

  if (caps.launchFlags) {
    args.push(
      "--model",
      nativeModelId(session.model),
      "--permission-mode",
      runtimeModeToPermission(session.runtimeMode),
    );
  }

  const resuming = caps.resume && !!session.providerSessionId;
  if (caps.resume) {
    if (session.providerSessionId) {
      args.push("--resume", session.providerSessionId);
    } else {
      providerSessionId = options.newProviderSessionId ?? crypto.randomUUID();
      args.push("--session-id", providerSessionId);
    }
  }

  // A resumed conversation already has its first prompt.
  const prompt = options.initialPrompt;
  if (caps.initialPrompt && !resuming && prompt?.trim()) {
    // `--` keeps a prompt that starts with a dash from parsing as a flag.
    if (prompt.startsWith("-")) args.push("--");
    args.push(prompt);
  }

  return providerSessionId
    ? { program: options.binaryPath, args, providerSessionId }
    : { program: options.binaryPath, args };
}
