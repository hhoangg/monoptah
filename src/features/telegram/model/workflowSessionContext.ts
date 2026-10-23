/** Where a workflow node's session came from, so a forwarded question can say so. */
export type WorkflowSessionContext = {
  workflowName: string;
  nodeTitle: string;
  itemLabel?: string;
  itemId?: string;
  round?: number;
};

const contexts = new Map<string, WorkflowSessionContext>();

export function setWorkflowSessionContext(
  sessionId: string,
  ctx: WorkflowSessionContext,
): void {
  contexts.set(sessionId, ctx);
}

export function workflowSessionContext(
  sessionId: string,
): WorkflowSessionContext | undefined {
  return contexts.get(sessionId);
}

export function clearWorkflowSessionContext(sessionId: string): void {
  contexts.delete(sessionId);
}

const LABEL_FIELDS = ["title", "name", "summary", "subject"];
const ID_FIELDS = ["identifier", "key", "id", "number"];

/** A readable label and id for a For each item, when it has the usual fields. */
export function workflowItemInfo(item: unknown): {
  itemLabel?: string;
  itemId?: string;
} {
  if (typeof item === "string") {
    return item.trim() ? { itemLabel: item.trim() } : {};
  }
  if (typeof item !== "object" || item === null) return {};
  const record = item as Record<string, unknown>;
  const label = firstText(record, LABEL_FIELDS);
  const id = firstText(record, ID_FIELDS);
  return {
    ...(id ? { itemId: id } : {}),
    ...(label ? { itemLabel: label } : {}),
  };
}

function firstText(
  record: Record<string, unknown>,
  fields: string[],
): string | undefined {
  for (const field of fields) {
    const value = record[field];
    if (typeof value === "string" && value.trim()) return value.trim();
    if (typeof value === "number" && Number.isFinite(value)) {
      return String(value);
    }
  }
  return undefined;
}
