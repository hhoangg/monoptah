export const ADD_TO_CHAT_EVENT = "monocode:add-to-chat";

/**
 * "quote" blockquotes the text, "plain" adds it as written, and "prefill"
 * starts a sentence for the user to finish: it keeps the text exactly,
 * including a trailing space, and leaves no blank line after it.
 */
export type AddToChatMode = "quote" | "plain" | "prefill";

export type AddToChatRequest = {
  text: string;
  mode: AddToChatMode;
  /** Only this session's composer takes the request; any other ignores it. */
  sessionId?: string;
};

export type QuoteRequest = {
  id: number;
  text: string;
  mode?: AddToChatMode;
};

export function requestAddToChat(
  text: string,
  mode: AddToChatMode = "quote",
  sessionId?: string,
) {
  if (typeof window === "undefined") return;
  const normalized = text.replace(/\r\n?/g, "\n");
  const value = mode === "prefill" ? normalized : normalized.trim();
  if (!value.trim()) return;
  window.dispatchEvent(
    new CustomEvent<AddToChatRequest>(ADD_TO_CHAT_EVENT, {
      detail: { text: value, mode, ...(sessionId ? { sessionId } : {}) },
    }),
  );
}

/** Whether the composer of `sessionId` should take this request. */
export function addToChatTargets(
  request: AddToChatRequest,
  sessionId: string,
): boolean {
  return !request.sessionId || request.sessionId === sessionId;
}

/** Initial composer text for an add-to-chat request that opens a new session. */
export function composerSeedForAddToChat(
  text: string,
  mode: AddToChatMode = "quote",
): string {
  return addToDraft("", text, mode);
}

export type QuoteConsumption = {
  draft: string;
  consumedId: number | null;
  changed: boolean;
};

export function isMarkdownBlockquotePosition(
  text: string,
  position: number,
): boolean {
  const index = Math.max(0, Math.min(position, text.length));
  const lineStart = text.lastIndexOf("\n", Math.max(0, index - 1)) + 1;
  return /^ {0,3}>/.test(text.slice(lineStart, index));
}

export function appendSelectionQuote(draft: string, text: string): string {
  const selected = text.replace(/\r\n?/g, "\n").trim();
  if (!selected) return draft;

  const quote = selected
    .split("\n")
    .map((line) => (line ? `> ${line}` : ">"))
    .join("\n");
  return joinComposerInsert(draft, quote);
}

export function appendComposerInsert(draft: string, text: string): string {
  const selected = text.replace(/\r\n?/g, "\n").trim();
  if (!selected) return draft;
  return joinComposerInsert(draft, selected);
}

export function consumeQuoteRequest(
  draft: string,
  consumedId: number | null,
  request: QuoteRequest | undefined,
): QuoteConsumption {
  if (!request || request.id === consumedId) {
    return { draft, consumedId, changed: false };
  }

  const next = addToDraft(draft, request.text, request.mode);
  return {
    draft: next,
    consumedId: request.id,
    changed: next !== draft,
  };
}

export function acknowledgeQuoteRequest(
  current: QuoteRequest | undefined,
  handledId: number,
): QuoteRequest | undefined {
  return current?.id === handledId ? undefined : current;
}

export function appendComposerPrefill(draft: string, text: string): string {
  const value = text.replace(/\r\n?/g, "\n");
  if (!value.trim()) return draft;
  return `${draft}${composerInsertSeparator(draft)}${value}`;
}

function addToDraft(
  draft: string,
  text: string,
  mode: AddToChatMode = "quote",
): string {
  if (mode === "plain") return appendComposerInsert(draft, text);
  if (mode === "prefill") return appendComposerPrefill(draft, text);
  return appendSelectionQuote(draft, text);
}

function composerInsertSeparator(draft: string): string {
  return draft.length === 0
    ? ""
    : draft.endsWith("\n\n")
      ? ""
      : draft.endsWith("\n")
        ? "\n"
        : "\n\n";
}

function joinComposerInsert(draft: string, block: string): string {
  return `${draft}${composerInsertSeparator(draft)}${block}\n\n`;
}
