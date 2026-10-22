// Output contract for session nodes: the reply ends with one fenced JSON block the engine parses.
import type { OutputField, OutputFieldType } from "./workflow";

export const OUTPUT_FENCE = "workflow-output";

export type ParsedOutput = { ok: true; value: Record<string, unknown> } | { ok: false; error: string };

const TYPE_HINT: Record<OutputFieldType, string> = {
  string: "a JSON string",
  number: "a JSON number",
  boolean: "true or false",
  list: "a JSON array",
  object: "a JSON object",
};

function describeFields(fields: Record<string, OutputField>): string {
  return Object.entries(fields)
    .map(([name, field]) => {
      const note = field.description ? `: ${field.description}` : "";
      return `- ${name} (${field.type})${note}`;
    })
    .join("\n");
}

/** Text appended to a session prompt that tells the model how to report its result. */
export function outputInstruction(fields: Record<string, OutputField>): string {
  if (Object.keys(fields).length === 0) return "";
  return [
    `When you are finished, end your final reply with exactly one fenced code block whose info string is \`${OUTPUT_FENCE}\`.`,
    "It must contain a single JSON object with these fields:",
    describeFields(fields),
    "Example shape:",
    "```" + OUTPUT_FENCE,
    "{ ... }",
    "```",
  ].join("\n");
}

/** The single follow-up message sent when the reply did not satisfy the contract. */
export function outputReask(fields: Record<string, OutputField>, problem: string): string {
  return [
    `Your last reply did not satisfy the output contract: ${problem}`,
    "",
    "Reply again with the result as one fenced code block, nothing else required.",
    outputInstruction(fields),
  ].join("\n");
}

const OPEN_FENCE = /^ {0,3}(`{3,})([^`]*)$/;

/** The body of the last fenced block whose info string is exactly the output fence, or null. */
function lastOutputBlock(reply: string): string | null {
  const lines = reply.split(/\r?\n/);
  let found: string | null = null;
  let i = 0;
  while (i < lines.length) {
    const open = OPEN_FENCE.exec(lines[i]);
    if (!open) {
      i++;
      continue;
    }
    const fenceLen = open[1].length;
    const isOutput = open[2].trim() === OUTPUT_FENCE;
    const closer = new RegExp(`^ {0,3}\`{${fenceLen},}\\s*$`);
    const body: string[] = [];
    let j = i + 1;
    while (j < lines.length && !closer.test(lines[j])) {
      body.push(lines[j]);
      j++;
    }
    // An unclosed fence runs to the end of the reply, as in Markdown.
    if (isOutput) found = body.join("\n");
    i = j + 1;
  }
  return found;
}

function matchesType(value: unknown, type: OutputFieldType): boolean {
  switch (type) {
    case "string":
      return typeof value === "string";
    case "number":
      return typeof value === "number" && Number.isFinite(value);
    case "boolean":
      return typeof value === "boolean";
    case "list":
      return Array.isArray(value);
    case "object":
      return value !== null && typeof value === "object" && !Array.isArray(value);
  }
}

/** Extracts and validates the `workflow-output` block from a session reply. */
export function parseWorkflowOutput(reply: string, fields: Record<string, OutputField>): ParsedOutput {
  const body = lastOutputBlock(reply);
  if (body === null) {
    if (Object.keys(fields).length === 0) return { ok: true, value: {} };
    return { ok: false, error: `No ${OUTPUT_FENCE} block found in the reply.` };
  }
  let parsed: unknown;
  try {
    parsed = JSON.parse(body);
  } catch {
    return { ok: false, error: `The ${OUTPUT_FENCE} block is not valid JSON.` };
  }
  if (parsed === null || typeof parsed !== "object" || Array.isArray(parsed)) {
    return { ok: false, error: `The ${OUTPUT_FENCE} block must contain a JSON object.` };
  }
  const value = parsed as Record<string, unknown>;
  for (const [name, field] of Object.entries(fields)) {
    if (!Object.prototype.hasOwnProperty.call(value, name)) {
      return { ok: false, error: `Field "${name}" is missing; expected ${TYPE_HINT[field.type]} (${field.type}).` };
    }
    if (!matchesType(value[name], field.type)) {
      return { ok: false, error: `Field "${name}" must be ${TYPE_HINT[field.type]} (${field.type}).` };
    }
  }
  return { ok: true, value };
}
