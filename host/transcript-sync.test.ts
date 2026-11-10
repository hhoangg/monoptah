import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import {
  claudeProjectDirName,
  decide,
  forkTranscript,
  parseChain,
  probeLine,
  rewriteCwd,
} from "./transcript-sync";

// The SAME fixture files as crates/monocode-core/src/cloud_sessions/transcript_sync.rs.
const DIR = join(__dirname, "..", "crates/monocode-core/tests/fixtures/transcript_sync");
const fixture = (name: string) => readFileSync(join(DIR, name), "utf8");
const bytes = (name: string) => readFileSync(join(DIR, name));
const manifest = JSON.parse(fixture("cases.json")) as {
  cases: { name: string; existing: string | null; incoming: string; decision: string }[];
  chains: { file: string; uuids: string[] }[];
  edgeRules: string[];
};

describe("shared transcript_sync fixtures", () => {
  it.each(manifest.chains.map((c) => [c.file, c] as const))("chain of %s", (_file, chain) => {
    expect(parseChain(bytes(chain.file))).toEqual(chain.uuids);
  });

  it.each(manifest.cases.map((c) => [c.name, c] as const))("decision: %s", (_name, c) => {
    const existing = c.existing === null ? null : bytes(c.existing);
    expect(decide(existing, bytes(c.incoming))).toBe(c.decision);
  });

  it("covers the edge rules the manifest documents", () => {
    expect(manifest.edgeRules.length).toBeGreaterThan(0);
    // A line that repeats `uuid` or `sessionId` is skipped as a whole.
    expect(probeLine('{"uuid":"a","uuid":"b"}')).toBeNull();
    expect(probeLine('{"sessionId":"a","sessionId":"b"}')).toBeNull();
    expect(probeLine('{"x":{"uuid":"a","uuid":"b"},"uuid":"c"}')?.uuid).toBe("c");
    // Non-object, non-array values and invalid JSON contribute nothing.
    for (const bad of ['"s"', "42", "null", '{"uuid":"a"', '{"uuid":"a"} x', '{"uuid":"a",}', "[1,]"])
      expect(probeLine(bad), bad).toBeNull();
    // Only a string uuid joins the chain.
    expect(probeLine('{"uuid":1}')?.uuid).toBeUndefined();
    expect(probeLine('{"uuid":null}')?.uuid).toBeUndefined();
    // Escapes in keys and values are decoded.
    expect(probeLine('{"u\\u0075id":"a"}')?.uuid).toBe("a");
    // Strict only on top-level keys and on uuid / sessionId values.
    expect(probeLine('{"\\ud800":1,"uuid":"a"}')).toBeNull();
    expect(probeLine('{"uuid":"\\ud800"}')).toBeNull();
    expect(probeLine('{"uuid":1e400}')).toBeNull();
    // Everything else needs correct JSON syntax only.
    expect(probeLine('{"uuid":"a","x":"\\ud800","n":1e400,"o":{"\\udc00":[]}}')?.uuid).toBe("a");
    expect(probeLine(`{"uuid":"a","x":${"[".repeat(5000)}${"]".repeat(5000)}}`)?.uuid).toBe("a");
    expect(probeLine('{"uuid":"a","x":"bad\u0001"}'.replace("\\u0001", "\u0001"))).toBeNull();
    expect(probeLine('{"uuid":"a","x":"\\q"}')).toBeNull();
    // A two-value array is read positionally like serde reads a struct.
    expect(probeLine('["u","s"]')).toMatchObject({ uuid: "u", sessionId: "s" });
    for (const arr of ["[]", '["a"]', '["a","b","c"]']) expect(probeLine(arr), arr).toBeNull();
    // Invalid UTF-8: kept in ignored fields, dropped in uuid.
    const withByte = (head: string, bad: number, tail: string) =>
      Buffer.concat([Buffer.from(head), Buffer.from([bad]), Buffer.from(tail)]);
    expect(parseChain(withByte('{"uuid":"a","x":"', 0xff, '"}'))).toEqual(["a"]);
    expect(parseChain(withByte('{"uuid":"', 0xff, '"}'))).toEqual([]);
    expect(decide('{"uuid":"a"}\n', '{"uuid":"a"}\n{"uuid":"b","x":"\\udc00"}\n')).toBe("replace");
  });

  it("ignores the cwd rewrite when comparing", () => {
    const cloud = fixture("extended.jsonl");
    const localBase = rewriteCwd(fixture("base.jsonl"), ["/workspaces/app"], "/Users/me/app");
    expect(localBase).not.toBe(fixture("base.jsonl"));
    const localExt = rewriteCwd(cloud, ["/workspaces/app"], "/Users/me/app");
    expect(decide(localBase, localExt)).toBe("replace");
    expect(decide(localBase, cloud)).toBe("replace");
    expect(decide(localExt, cloud)).toBe("already_current");
  });

  it("round-trips a cwd rewrite to the identity", () => {
    for (const name of ["base.jsonl", "extended.jsonl", "diverged.jsonl"]) {
      const original = fixture(name);
      const local = rewriteCwd(original, ["/workspaces/app"], "/Users/me/my app");
      expect(local).toContain("/Users/me/my app");
      expect(local).not.toContain("/workspaces/app");
      expect(rewriteCwd(local, ["/Users/me/my app"], "/workspaces/app"), name).toBe(original);
    }
  });

  it("respects path boundaries when rewriting", () => {
    const text = '{"cwd":"/workspaces/app"}\n{"cwd":"/workspaces/app-two"}\n{"cwd":"/tmp/workspaces/app"}\n{"cwd":"/workspaces/app.git"}\n';
    expect(rewriteCwd(text, ["/workspaces/app"], "/x")).toBe(
      '{"cwd":"/x"}\n{"cwd":"/workspaces/app-two"}\n{"cwd":"/tmp/workspaces/app"}\n{"cwd":"/workspaces/app.git"}\n',
    );
  });

  it("forks only the session ids", () => {
    const original = fixture("extended.jsonl");
    const oldId = "11111111-1111-4111-8111-111111111111";
    const newId = "22222222-2222-4222-8222-222222222222";
    const forked = forkTranscript(original, newId);
    expect(forked).not.toContain(oldId);
    expect(forked.split("\n").length).toBe(original.split("\n").length);
    expect(parseChain(forked)).toEqual(parseChain(original));
    expect(forked.split(newId).join(oldId)).toBe(original);
    expect(forked).toContain('"futureField":{"nested":[1,2,3]}');
    expect(forked.split("\n")[0]).toMatch(/^\{"type":"file-history-snapshot"/);
  });

  it("forks spaced JSON and a missing final newline", () => {
    const out = forkTranscript('{ "sessionId": "old", "uuid": "a" }\n{"sessionId":"old"}', "new");
    const [first, second] = out.split("\n");
    expect(JSON.parse(first)).toEqual({ sessionId: "new", uuid: "a" });
    expect(second).toBe('{"sessionId":"new"}');
    expect(out.endsWith("\n")).toBe(false);
  });
});

describe("claude project dir name", () => {
  it("replaces every non-alphanumeric character", () => {
    expect(claudeProjectDirName("/Users/me/my app")).toBe("-Users-me-my-app");
    expect(claudeProjectDirName("/a/b.c_d")).toBe("-a-b-c-d");
    expect(claudeProjectDirName("/a/\u{1F600}")).toBe("-a--");
  });
});
