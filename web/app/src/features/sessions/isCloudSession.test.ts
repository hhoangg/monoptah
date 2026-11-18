import { describe, expect, it } from "vitest";
import type { SessionCard } from "@monoptah/protocol";
import { isCloudSession } from "./SessionActionDialogs";

const card = (target: { kind: string } | undefined) => ({ target }) as unknown as Pick<SessionCard, "target">;

describe("isCloudSession", () => {
  it("is true only for the cloud kinds", () => {
    expect(isCloudSession(card({ kind: "codespace" }))).toBe(true);
    expect(isCloudSession(card({ kind: "sandbox" }))).toBe(true);
  });
  it("is false for local, remote and no target", () => {
    expect(isCloudSession(card({ kind: "local" }))).toBe(false);
    expect(isCloudSession(card({ kind: "remote" }))).toBe(false);
    expect(isCloudSession(card(undefined))).toBe(false);
  });
});
