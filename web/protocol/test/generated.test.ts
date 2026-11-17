import { describe, expect, it } from "vitest";
import { buildMethodsJson, buildVectors, serialize } from "../scripts/vectors-lib.ts";
import { KNOWN_CAPABILITIES, METHOD_LIST, StateMessageSchema } from "../src/index.ts";
import { readText } from "./helpers.ts";

describe("generated files are fresh", () => {
  it("vectors.json equals a fresh generation (run `pnpm --filter @monoptah/protocol vectors:gen`)", () => {
    expect(readText("../vectors.json")).toBe(serialize(buildVectors()));
  });

  it("methods.json equals a fresh generation", () => {
    expect(readText("../methods.json")).toBe(serialize(buildMethodsJson()));
  });

  it("has no Noise vectors yet", () => {
    expect("noise" in JSON.parse(readText("../vectors.json"))).toBe(false);
  });

  it("vectors.json has a state key whose messages parse with the state schema", () => {
    const state = JSON.parse(readText("../vectors.json")).state as { capabilities: string[]; messages: unknown[] };
    expect(state.capabilities).toEqual([...KNOWN_CAPABILITIES]);
    const parsed = state.messages.map((message) => StateMessageSchema.parse(message));
    expect(parsed.map((message) => message.state)).toEqual(["pending", "confirmed", "revoked"]);
    expect(parsed[1]?.capabilities).toEqual([...KNOWN_CAPABILITIES]);
    expect(parsed[1]?.permission).toBeDefined();
    expect(parsed[1]?.proto).toBeDefined();
    expect(parsed[1]?.desktop).toBeDefined();
  });

  it("methods.json has the shape the Rust test reads, sorted by name", () => {
    const parsed = JSON.parse(readText("../methods.json")) as { version: number; methods: Record<string, unknown>[] };
    expect(parsed.version).toBe(1);
    expect(parsed.methods.length).toBe(METHOD_LIST.length);
    const names = parsed.methods.map((method) => method.name as string);
    expect(names).toEqual([...names].sort());
    for (const method of parsed.methods) {
      expect(Object.keys(method).sort()).toEqual(["class", "name", "transport"]);
      expect(["read", "control", "param", "denied", "unsupportedWeb"]).toContain(method.class);
      expect(["app", "host"]).toContain(method.transport);
    }
  });
});
