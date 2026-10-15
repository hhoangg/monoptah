import type { ContributorsConfig, Person } from "./types";

export const CONFIG_PATH = ".monoptah/contributors.json";

export const DEFAULT_CONFIG: ContributorsConfig = {
  version: 1,
  season: "monthly",
  weights: { delivery: 1, review: 1, care: 1, consistency: 1, breadth: 1 },
  ignore: [
    "*.lock",
    "package-lock.json",
    "pnpm-lock.yaml",
    "yarn.lock",
    "Cargo.lock",
    "go.sum",
    "dist/**",
    "build/**",
    "vendor/**",
    "**/dist/**",
    "**/build/**",
    "**/vendor/**",
    "**/*.snap",
    "**/*.min.js",
    "**/*.generated.*",
    "**/generated/**",
  ],
  // Generic bots are excluded outright; AI agents are kept (see agentIdentities)
  // and an identity matching both counts as an agent.
  excludeIdentities: [
    "*[bot]*",
    "dependabot[bot]",
    "renovate[bot]",
    "github-actions[bot]",
    "noreply@github.com",
    "*@ci-runner*",
    "coderabbitai*",
    "copilot-pull-request-reviewer*",
    "*codecov*",
    "*sonarcloud*",
    "gemini-code-assist*",
    "sourcery-ai*",
    "github-advanced-security*",
    "vercel*",
    "netlify*",
  ],
  // Whole-string matches, so a human named Claude Dupont is not an agent.
  agentIdentities: [
    "*@anthropic.com",
    "claude",
    "codex",
    "copilot",
    "devin",
    "gemini",
    "cursor agent",
    "cursoragent@cursor.com",
    "*@cursor.com",
    "copilot@github.com",
    "*@openai.com",
    "*@cognition.ai",
    "copilot-swe-agent[bot]",
    "*copilot-swe-agent*",
    "devin-ai-integration[bot]",
    "*devin-ai-integration*",
    "claude[bot]",
    "*claude[bot]*",
  ],
  people: [],
  dismissedMerges: [],
};

const WEIGHT_KEYS = [
  "delivery",
  "review",
  "care",
  "consistency",
  "breadth",
] as const;

const isRecord = (v: unknown): v is Record<string, unknown> =>
  typeof v === "object" && v !== null && !Array.isArray(v);

function uniqueStrings(values: string[]): string[] {
  return Array.from(new Set(values));
}

/** Non-string and blank entries are dropped; `undefined` when the field is not an array. */
function stringList(value: unknown, lowercase = false): string[] | undefined {
  if (!Array.isArray(value)) return undefined;
  return uniqueStrings(
    value
      .filter((v): v is string => typeof v === "string")
      .map((v) => (lowercase ? v.trim().toLowerCase() : v.trim()))
      .filter((v) => v !== ""),
  );
}

function parseWeights(value: unknown): ContributorsConfig["weights"] {
  const out = { ...DEFAULT_CONFIG.weights };
  if (!isRecord(value)) return out;
  for (const key of WEIGHT_KEYS) {
    const w = value[key];
    if (typeof w === "number" && Number.isFinite(w)) {
      out[key] = Math.min(5, Math.max(0, w));
    }
  }
  return out;
}

function parsePeople(value: unknown): Person[] {
  if (!Array.isArray(value)) return [];
  const seen = new Set<string>();
  const people: Person[] = [];
  for (const entry of value) {
    if (!isRecord(entry)) continue;
    const id = typeof entry.id === "string" ? entry.id.trim() : "";
    if (!id || seen.has(id)) continue;
    seen.add(id);
    const name =
      typeof entry.name === "string" && entry.name.trim()
        ? entry.name.trim()
        : id;
    people.push({
      id,
      name,
      identities: stringList(entry.identities, true) ?? [],
    });
  }
  return people;
}

const describeType = (value: unknown) =>
  value === null
    ? "null"
    : Array.isArray(value)
      ? "an array"
      : `a ${typeof value}`;

/**
 * Never throws; every field falls back to its default on bad input. `error` is
 * set when the file exists but is not usable as a whole (broken JSON, or not an
 * object), so callers can refuse to save over it. The returned config is still
 * the defaults, for display.
 */
export function parseConfigResult(text: string | null | undefined): {
  config: ContributorsConfig;
  error?: string;
} {
  let raw: unknown = null;
  let error: string | undefined;
  if (text && text.trim()) {
    try {
      raw = JSON.parse(text);
      if (!isRecord(raw)) {
        error = `Expected a JSON object at the top level, found ${describeType(raw)}`;
      }
    } catch (e) {
      error = `Invalid JSON: ${e instanceof Error ? e.message : String(e)}`;
    }
  }
  const src = isRecord(raw) ? raw : {};
  const config: ContributorsConfig = {
    version: 1,
    season: "monthly",
    weights: parseWeights(src.weights),
    ignore: stringList(src.ignore) ?? [...DEFAULT_CONFIG.ignore],
    excludeIdentities: stringList(src.excludeIdentities) ?? [
      ...DEFAULT_CONFIG.excludeIdentities,
    ],
    agentIdentities: stringList(src.agentIdentities) ?? [
      ...DEFAULT_CONFIG.agentIdentities,
    ],
    people: parsePeople(src.people),
    dismissedMerges: stringList(src.dismissedMerges) ?? [],
  };
  return error ? { config, error } : { config };
}

/** Never throws; see `parseConfigResult` to learn whether the file was valid. */
export function parseConfig(
  text: string | null | undefined,
): ContributorsConfig {
  return parseConfigResult(text).config;
}

/** Stable key order, 2-space JSON, trailing newline, so diffs stay small. */
export function serializeConfig(config: ContributorsConfig): string {
  const ordered = {
    version: config.version,
    season: config.season,
    weights: {
      delivery: config.weights.delivery,
      review: config.weights.review,
      care: config.weights.care,
      consistency: config.weights.consistency,
      breadth: config.weights.breadth,
    },
    ignore: config.ignore,
    excludeIdentities: config.excludeIdentities,
    agentIdentities: config.agentIdentities,
    people: config.people.map((p) => ({
      id: p.id,
      name: p.name,
      identities: p.identities,
    })),
    dismissedMerges: config.dismissedMerges,
  };
  return `${JSON.stringify(ordered, null, 2)}\n`;
}
