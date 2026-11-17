import { afterEach, describe, expect, it } from "vitest";
import { clearHooks, registerSendGuard, registerSessionCapabilities, runSendGuard, sessionCan } from "./hooks";

afterEach(clearHooks);

const ctx = { desktopId: "d1", sessionId: "s1", target: { kind: "cloud" } };

describe("send-guard", () => {
  it("allows by default", async () => {
    expect(await runSendGuard(ctx)).toEqual({ kind: "allow" });
  });

  it("returns what the registered guard decides", async () => {
    registerSendGuard((context) => Promise.resolve(context.target?.kind === "cloud" ? { kind: "confirm", notice: "cost" } : { kind: "allow" }));
    expect(await runSendGuard(ctx)).toEqual({ kind: "confirm", notice: "cost" });
    expect(await runSendGuard({ ...ctx, target: { kind: "local" } })).toEqual({ kind: "allow" });
  });

  it("throws on a second registration", () => {
    registerSendGuard(() => Promise.resolve({ kind: "refuse", reason: "starting" }));
    expect(() => registerSendGuard(() => Promise.resolve({ kind: "allow" }))).toThrow(/already registered/);
  });
});

describe("session-capabilities", () => {
  const local = { target: { kind: "local" }, desktopCapabilities: [] };

  it("offers every capability by default", () => {
    for (const capability of ["delete", "files", "changes", "git"] as const) expect(sessionCan(capability, local)).toBe(true);
  });

  it("uses the registered predicate", () => {
    registerSessionCapabilities((capability, context) => context.target?.kind !== "cloud" || capability === "delete");
    expect(sessionCan("files", { ...local, target: { kind: "cloud" } })).toBe(false);
    expect(sessionCan("delete", { ...local, target: { kind: "cloud" } })).toBe(true);
    expect(sessionCan("git", local)).toBe(true);
  });

  it("throws on a second registration", () => {
    registerSessionCapabilities(() => true);
    expect(() => registerSessionCapabilities(() => false)).toThrow(/already registered/);
  });
});
