// Pure helpers for starting an agent session from the Contributors page. The
// App owns the session and Tauri calls; this module only shapes the data.

const CHUNK = 0x8000;

/** Base64 of the UTF-8 bytes of `text`; `btoa` alone throws on non-Latin1. */
export function utf8ToBase64(text: string): string {
  const bytes = new TextEncoder().encode(text);
  let binary = "";
  for (let i = 0; i < bytes.length; i += CHUNK) {
    binary += String.fromCharCode(...bytes.subarray(i, i + CHUNK));
  }
  return btoa(binary);
}

/** `contributors-2026-10.json` becomes "Contributors 2026-10". */
export function contributorsSessionTitle(fileName: string): string {
  const season = /^contributors-(.+)\.json$/.exec(fileName)?.[1];
  return season ? `Contributors ${season}` : "Contributors";
}

/**
 * The first message. A provider that cannot take attachments still gets the
 * snapshot: the prompt names the file so the agent can read it with its tools.
 */
export function composeSessionPrompt(input: {
  prompt: string;
  path: string;
  attached: boolean;
}): string {
  if (input.attached) return input.prompt;
  return `${input.prompt}\n\nThe snapshot could not be attached to this provider. Read it from this file: ${input.path}`;
}

/** The chosen provider, or undefined for the project default. */
export function resolveHarness<T extends string>(
  raw: string | undefined,
  known: readonly T[],
): T | undefined {
  if (!raw) return undefined;
  const match = known.find((id) => id === raw);
  if (!match) throw new Error(`The provider "${raw}" is not available.`);
  return match;
}

/** Tauri rejects with plain strings; turn any failure into one readable line. */
export function failureMessage(step: string, error: unknown): string {
  const detail = (
    error instanceof Error
      ? error.message
      : typeof error === "string"
        ? error
        : ""
  ).trim();
  return detail ? `Could not ${step}: ${detail}` : `Could not ${step}.`;
}
