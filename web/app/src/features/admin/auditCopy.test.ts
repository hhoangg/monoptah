import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";
import type { AdminAuditEvent } from "./adminApi";
import { auditTitle } from "./copy";

/**
 * The app package may not import the worker, so the action ids are read from the worker's registry source:
 * every key of AUDIT_ACTIONS (a quoted `"area.name": spec(` entry) must have a plain-words title.
 */
function workerActions(): string[] {
  const source = readFileSync(resolve(__dirname, "../../../../worker/src/modules/audit/actions.ts"), "utf8");
  const block = source.slice(source.indexOf("export const AUDIT_ACTIONS"));
  return [...block.matchAll(/^\s*"([a-z_]+\.[a-z_]+)": spec\(/gm)].map((match) => match[1] as string);
}

function event(action: string): AdminAuditEvent {
  return { id: "e", ts: 1, actorKind: "user", actorUserId: "u", action, targetType: null, targetId: null, meta: {} } as unknown as AdminAuditEvent;
}

describe("auditTitle", () => {
  const actions = workerActions();

  it("finds the worker's action list", () => {
    expect(actions.length).toBeGreaterThan(20);
    expect(actions).toContain("grant.change");
  });

  it.each(actions)("%s has a plain-words title, not its raw id", (action) => {
    const title = auditTitle(event(action));
    expect(title).not.toBe(action);
    expect(title).not.toBe("Other change");
    expect(title).not.toMatch(/^[a-z_]+\.[a-z_]+$/);
  });

  it("falls back to Other change for an unknown action", () => {
    expect(auditTitle(event("future.thing"))).toBe("Other change");
  });
});
