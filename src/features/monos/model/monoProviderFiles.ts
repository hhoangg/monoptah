import { HARNESS_TITLE, type HarnessId } from "../../sessions/model/session";

/**
 * A provider session whose on-disk files Monoptah knows how to remove. These
 * are the only providers with a known layout; anything else is never sent to
 * the backend.
 */
export type ProviderSessionRef = { harness: "claude" | "codex"; id: string };

const PROVIDER_ID =
  /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

/** A canonical UUID: the shape of every Claude Code and Codex session id. */
export function isProviderSessionId(id: unknown): id is string {
  return typeof id === "string" && PROVIDER_ID.test(id);
}

export function isDeletableHarness(
  harness: unknown,
): harness is ProviderSessionRef["harness"] {
  return harness === "claude" || harness === "codex";
}

/** The reference for a harness and id, or undefined when either is not safe to send. */
export function providerSessionRef(
  harness: unknown,
  id: unknown,
): ProviderSessionRef | undefined {
  return isDeletableHarness(harness) && isProviderSessionId(id)
    ? { harness, id }
    : undefined;
}

/** Drop repeats, keeping the first occurrence of each session. */
export function dedupeProviderSessions(
  sessions: readonly ProviderSessionRef[],
): ProviderSessionRef[] {
  const seen = new Set<string>();
  const out: ProviderSessionRef[] = [];
  for (const session of sessions) {
    const key = `${session.harness}:${session.id.toLowerCase()}`;
    if (seen.has(key)) continue;
    seen.add(key);
    out.push(session);
  }
  return out;
}

/**
 * The valid, deduped sessions in a value read from storage. Anything that is
 * not a list, or any entry that is malformed, is dropped.
 */
export function sanitizeProviderSessions(
  value: unknown,
): ProviderSessionRef[] {
  if (!Array.isArray(value)) return [];
  return dedupeProviderSessions(
    value.flatMap((entry: { harness?: unknown; id?: unknown } | null) => {
      const ref = providerSessionRef(entry?.harness, entry?.id);
      return ref ? [ref] : [];
    }),
  );
}

/** What a reset says when some provider session files may have stayed behind. */
export function providerFilesProblemMessage(
  label: string,
  problems: number,
): string {
  return `Conversation reset. Some ${label} session files may not have been deleted (${problems} ${
    problems === 1 ? "problem" : "problems"
  }); you can remove them manually.`;
}

export type MonoProviderFiles = {
  /** What a reset may ask the backend to delete. */
  sessions: ProviderSessionRef[];
  /** Whether there is anything to delete. */
  supported: boolean;
  /** The provider or providers the files belong to, for the checkbox. */
  label: string;
  /** Why nothing, or not everything, can be deleted. */
  note?: string;
};

function titleOf(harness: string): string {
  return HARNESS_TITLE[harness as HarnessId] ?? harness;
}

/**
 * The provider session files a Mono reset can also delete: the conversation's
 * current provider session plus the ones context rotation retired earlier.
 */
export function monoProviderFiles(
  session: { harness: string; providerSessionId?: string },
  retired: readonly ProviderSessionRef[],
): MonoProviderFiles {
  const current = providerSessionRef(
    session.harness,
    session.providerSessionId,
  );
  const sessions = dedupeProviderSessions([
    ...(current ? [current] : []),
    ...sanitizeProviderSessions(retired),
  ]);
  const providers = (["claude", "codex"] as const).filter((harness) =>
    sessions.some((entry) => entry.harness === harness),
  );
  const currentSupported = isDeletableHarness(session.harness);
  const label = providers.length
    ? providers.map(titleOf).join(" and ")
    : titleOf(session.harness);
  if (!sessions.length) {
    return {
      sessions,
      supported: false,
      label,
      note: currentSupported
        ? `There are no ${label} session files to delete yet.`
        : `Monoptah can't delete ${label} session files yet.`,
    };
  }
  return {
    sessions,
    supported: true,
    label,
    ...(currentSupported
      ? {}
      : {
          note: `Files from the current ${titleOf(session.harness)} session are not covered.`,
        }),
  };
}
