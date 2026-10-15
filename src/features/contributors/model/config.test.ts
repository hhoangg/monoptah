import { describe, expect, it } from "vitest";
import {
  CONFIG_PATH,
  DEFAULT_CONFIG,
  parseConfig,
  parseConfigResult,
  serializeConfig,
} from "./config";
import { isIgnoredPath, matchesAnyIdentity } from "./patterns";

describe("CONFIG_PATH", () => {
  it("points at the fork-local config file", () => {
    expect(CONFIG_PATH).toBe(".monoptah/contributors.json");
  });
});

describe("DEFAULT_CONFIG", () => {
  it("has the documented shape", () => {
    expect(DEFAULT_CONFIG.version).toBe(1);
    expect(DEFAULT_CONFIG.season).toBe("monthly");
    expect(DEFAULT_CONFIG.weights).toEqual({
      delivery: 1,
      review: 1,
      care: 1,
      consistency: 1,
      breadth: 1,
    });
    expect(DEFAULT_CONFIG.people).toEqual([]);
    expect(DEFAULT_CONFIG.dismissedMerges).toEqual([]);
  });

  it("ignores lockfiles, build output and generated files", () => {
    for (const path of [
      "yarn.lock",
      "web/package-lock.json",
      "pnpm-lock.yaml",
      "src-tauri/Cargo.lock",
      "go.sum",
      "dist/a.js",
      "build/a.js",
      "vendor/x/y.go",
      "src/__snapshots__/a.test.ts.snap",
      "public/app.min.js",
      "src/api.generated.ts",
      "src/generated/x.ts",
    ]) {
      expect(isIgnoredPath(DEFAULT_CONFIG.ignore, path), path).toBe(true);
    }
    expect(isIgnoredPath(DEFAULT_CONFIG.ignore, "src/app.ts")).toBe(false);
  });

  it("lists exactly the narrowed agent identities", () => {
    expect(DEFAULT_CONFIG.agentIdentities).toEqual([
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
    ]);
  });

  it("classifies the Claude GitHub app as an agent, not excluded", () => {
    const app = {
      name: "claude[bot]",
      email: "209825114+claude[bot]@users.noreply.github.com",
      login: "claude[bot]",
    };
    expect(matchesAnyIdentity(DEFAULT_CONFIG.agentIdentities, app)).toBe(true);
    expect(
      matchesAnyIdentity(DEFAULT_CONFIG.agentIdentities, {
        name: "",
        email: app.email,
      }),
    ).toBe(true);
  });

  it("ignores build output at any depth in monorepos", () => {
    for (const path of [
      "packages/app/dist/x.js",
      "apps/web/build/x.js",
      "services/api/vendor/x.go",
    ]) {
      expect(isIgnoredPath(DEFAULT_CONFIG.ignore, path), path).toBe(true);
    }
    for (const g of ["**/dist/**", "**/build/**", "**/vendor/**", "dist/**"]) {
      expect(DEFAULT_CONFIG.ignore).toContain(g);
    }
  });

  it("keeps generic bots out of the agent list and in the exclude list", () => {
    for (const p of [
      "dependabot[bot]",
      "renovate[bot]",
      "github-actions[bot]",
      "noreply@github.com",
      "*@ci-runner*",
    ]) {
      expect(DEFAULT_CONFIG.excludeIdentities).toContain(p);
      expect(DEFAULT_CONFIG.agentIdentities).not.toContain(p);
    }
    for (const name of [
      "dependabot[bot]",
      "renovate[bot]",
      "github-actions[bot]",
    ]) {
      expect(
        matchesAnyIdentity(DEFAULT_CONFIG.agentIdentities, {
          name,
          email: `1+${name}@users.noreply.github.com`,
        }),
      ).toBe(false);
      expect(
        matchesAnyIdentity(DEFAULT_CONFIG.excludeIdentities, {
          name,
          email: `1+${name}@users.noreply.github.com`,
        }),
      ).toBe(true);
    }
  });
});

describe("DEFAULT_CONFIG service accounts", () => {
  it("excludes review and CI services but keeps them out of the agent list", () => {
    for (const name of [
      "coderabbitai[bot]",
      "coderabbitai",
      "copilot-pull-request-reviewer",
      "copilot-pull-request-reviewer[bot]",
      "codecov-commenter",
      "codecov[bot]",
      "sonarcloud[bot]",
      "SonarCloud",
      "gemini-code-assist[bot]",
      "sourcery-ai[bot]",
      "github-advanced-security[bot]",
      "vercel[bot]",
      "vercel",
      "netlify[bot]",
      "netlify",
    ]) {
      const identity = { name, email: "", login: name };
      expect(
        matchesAnyIdentity(DEFAULT_CONFIG.excludeIdentities, identity),
        name,
      ).toBe(true);
      expect(
        matchesAnyIdentity(DEFAULT_CONFIG.agentIdentities, identity),
        name,
      ).toBe(false);
    }
  });

  it("adds the patterns without dropping the earlier defaults", () => {
    for (const p of [
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
    ]) {
      expect(DEFAULT_CONFIG.excludeIdentities).toContain(p);
    }
  });

  it("does not exclude ordinary people", () => {
    for (const name of ["Alice Smith", "Vera Cruz", "Nate Lify"]) {
      expect(
        matchesAnyIdentity(DEFAULT_CONFIG.excludeIdentities, {
          name,
          email: "x@acme.com",
        }),
      ).toBe(false);
    }
  });
});

describe("parseConfig", () => {
  it("returns defaults for missing input", () => {
    expect(parseConfig(null)).toEqual(DEFAULT_CONFIG);
    expect(parseConfig(undefined)).toEqual(DEFAULT_CONFIG);
    expect(parseConfig("")).toEqual(DEFAULT_CONFIG);
    expect(parseConfig("   \n")).toEqual(DEFAULT_CONFIG);
  });

  it("returns defaults for invalid JSON", () => {
    expect(parseConfig("{not json")).toEqual(DEFAULT_CONFIG);
    expect(parseConfig("<<<<<<< HEAD")).toEqual(DEFAULT_CONFIG);
  });

  it("returns defaults for non-object JSON", () => {
    expect(parseConfig("[]")).toEqual(DEFAULT_CONFIG);
    expect(parseConfig("42")).toEqual(DEFAULT_CONFIG);
    expect(parseConfig('"x"')).toEqual(DEFAULT_CONFIG);
    expect(parseConfig("null")).toEqual(DEFAULT_CONFIG);
    expect(parseConfig("true")).toEqual(DEFAULT_CONFIG);
  });

  it("never returns shared references to the defaults", () => {
    const a = parseConfig(null);
    a.ignore.push("mutated");
    a.weights.delivery = 4;
    expect(DEFAULT_CONFIG.ignore).not.toContain("mutated");
    expect(DEFAULT_CONFIG.weights.delivery).toBe(1);
    expect(parseConfig(null).ignore).not.toContain("mutated");
  });

  it("falls back field by field", () => {
    const cfg = parseConfig(
      JSON.stringify({
        weights: { delivery: 2 },
        ignore: "oops",
        excludeIdentities: ["me@x.com"],
        people: "nope",
        dismissedMerges: 7,
      }),
    );
    expect(cfg.weights).toEqual({ ...DEFAULT_CONFIG.weights, delivery: 2 });
    expect(cfg.ignore).toEqual(DEFAULT_CONFIG.ignore);
    expect(cfg.excludeIdentities).toEqual(["me@x.com"]);
    expect(cfg.agentIdentities).toEqual(DEFAULT_CONFIG.agentIdentities);
    expect(cfg.people).toEqual([]);
    expect(cfg.dismissedMerges).toEqual([]);
  });

  it("clamps weights to [0, 5]", () => {
    const cfg = parseConfig(
      JSON.stringify({
        weights: {
          delivery: -3,
          review: 9,
          care: 0,
          consistency: 2.5,
          breadth: 5,
        },
      }),
    );
    expect(cfg.weights).toEqual({
      delivery: 0,
      review: 5,
      care: 0,
      consistency: 2.5,
      breadth: 5,
    });
  });

  it("falls back to 1 for non-numeric or non-finite weights", () => {
    const cfg = parseConfig(
      '{"weights":{"delivery":"3","review":null,"care":true,"consistency":[],"breadth":1e999}}',
    );
    expect(cfg.weights).toEqual(DEFAULT_CONFIG.weights);
  });

  it("ignores a weights field that is not an object", () => {
    expect(parseConfig('{"weights":3}').weights).toEqual(
      DEFAULT_CONFIG.weights,
    );
    expect(parseConfig('{"weights":[1,2]}').weights).toEqual(
      DEFAULT_CONFIG.weights,
    );
  });

  it("drops non-string entries, trims, drops blanks and dedupes", () => {
    const cfg = parseConfig(
      JSON.stringify({
        ignore: ["*.lock", 5, null, " *.lock ", "", "  ", "dist/**", {}],
        excludeIdentities: ["a@x.com", "a@x.com", 1],
        agentIdentities: [],
        dismissedMerges: ["x:same-name", "x:same-name", false],
      }),
    );
    expect(cfg.ignore).toEqual(["*.lock", "dist/**"]);
    expect(cfg.excludeIdentities).toEqual(["a@x.com"]);
    expect(cfg.agentIdentities).toEqual([]);
    expect(cfg.dismissedMerges).toEqual(["x:same-name"]);
  });

  it("keeps an explicitly emptied list instead of restoring defaults", () => {
    expect(parseConfig('{"ignore":[]}').ignore).toEqual([]);
  });

  it("parses people and normalizes their identities", () => {
    const cfg = parseConfig(
      JSON.stringify({
        people: [
          {
            id: "p1",
            name: "Alice",
            identities: [
              "Alice@Example.COM",
              "gh:Alice",
              " alice@example.com ",
              3,
            ],
          },
          { id: "p2", name: "Bob" },
        ],
      }),
    );
    expect(cfg.people).toEqual([
      {
        id: "p1",
        name: "Alice",
        identities: ["alice@example.com", "gh:alice"],
      },
      { id: "p2", name: "Bob", identities: [] },
    ]);
  });

  it("drops malformed people and duplicate ids", () => {
    const cfg = parseConfig(
      JSON.stringify({
        people: [
          null,
          "x",
          [],
          { name: "No id" },
          { id: "", name: "Empty id" },
          { id: 5, name: "Numeric id" },
          { id: "dup", name: "First", identities: ["a@x.com"] },
          { id: "dup", name: "Second", identities: ["b@x.com"] },
          { id: "noname", identities: ["c@x.com"] },
        ],
      }),
    );
    expect(cfg.people.map((p) => p.id)).toEqual(["dup", "noname"]);
    expect(cfg.people[0].name).toBe("First");
    expect(cfg.people[1].name).toBe("noname");
  });

  it("forces version 1 and monthly season", () => {
    const cfg = parseConfig('{"version":9,"season":"weekly"}');
    expect(cfg.version).toBe(1);
    expect(cfg.season).toBe("monthly");
  });

  it("handles unicode names", () => {
    const cfg = parseConfig(
      JSON.stringify({
        people: [{ id: "p", name: "Nguyễn Văn Đạt", identities: ["Đạt"] }],
      }),
    );
    expect(cfg.people[0].name).toBe("Nguyễn Văn Đạt");
    expect(cfg.people[0].identities).toEqual(["đạt"]);
  });
});

describe("serializeConfig", () => {
  it("emits 2-space JSON with a trailing newline", () => {
    const text = serializeConfig(DEFAULT_CONFIG);
    expect(text.endsWith("\n")).toBe(true);
    expect(text.endsWith("\n\n")).toBe(false);
    expect(text).toContain('\n  "version": 1,');
  });

  it("uses a stable key order", () => {
    const keys = Object.keys(JSON.parse(serializeConfig(DEFAULT_CONFIG)));
    expect(keys).toEqual([
      "version",
      "season",
      "weights",
      "ignore",
      "excludeIdentities",
      "agentIdentities",
      "people",
      "dismissedMerges",
    ]);
    const weightKeys = Object.keys(
      JSON.parse(serializeConfig(DEFAULT_CONFIG)).weights,
    );
    expect(weightKeys).toEqual([
      "delivery",
      "review",
      "care",
      "consistency",
      "breadth",
    ]);
  });

  it("orders keys the same regardless of input key order", () => {
    const shuffled = parseConfig(
      '{"people":[{"identities":["a@x.com"],"name":"A","id":"a"}],"weights":{"breadth":2,"delivery":3}}',
    );
    const keys = Object.keys(JSON.parse(serializeConfig(shuffled)));
    expect(keys[0]).toBe("version");
    const person = JSON.parse(serializeConfig(shuffled)).people[0];
    expect(Object.keys(person)).toEqual(["id", "name", "identities"]);
  });

  it("round-trips through parseConfig", () => {
    const cfg = parseConfig(
      JSON.stringify({
        weights: { delivery: 2, review: 0.5 },
        ignore: ["a/**"],
        people: [{ id: "p", name: "P", identities: ["p@x.com", "gh:p"] }],
        dismissedMerges: ["a|b:same-name"],
      }),
    );
    expect(parseConfig(serializeConfig(cfg))).toEqual(cfg);
    expect(serializeConfig(parseConfig(serializeConfig(cfg)))).toBe(
      serializeConfig(cfg),
    );
  });

  it("is idempotent for the defaults", () => {
    expect(parseConfig(serializeConfig(DEFAULT_CONFIG))).toEqual(
      DEFAULT_CONFIG,
    );
  });
});

describe("parseConfigResult", () => {
  it("reports no error for a missing, empty or blank file", () => {
    for (const text of [null, undefined, "", "  \n"]) {
      const result = parseConfigResult(text);
      expect(result.error).toBeUndefined();
      expect(result.config).toEqual(DEFAULT_CONFIG);
    }
  });

  it("reports no error for a valid object, even with unknown or bad fields", () => {
    const result = parseConfigResult(
      JSON.stringify({ weights: { delivery: "x" }, extra: 1 }),
    );
    expect(result.error).toBeUndefined();
    expect(result.config.weights.delivery).toBe(1);
  });

  it("reports broken JSON with the parser's reason and falls back to defaults for display", () => {
    const result = parseConfigResult('{ "people": [ ],, }');
    expect(result.error).toMatch(/^Invalid JSON: .+/);
    expect(result.config).toEqual(DEFAULT_CONFIG);
  });

  it("reports merge-conflict markers", () => {
    const result = parseConfigResult("<<<<<<< HEAD\n{}\n=======\n{}\n>>>>>>>");
    expect(result.error).toMatch(/^Invalid JSON/);
  });

  it.each([
    ["[]", "an array"],
    ['"text"', "a string"],
    ["42", "a number"],
    ["true", "a boolean"],
    ["null", "null"],
  ])("reports a wrong top-level type: %s", (text, what) => {
    const result = parseConfigResult(text);
    expect(result.error).toBe(
      `Expected a JSON object at the top level, found ${what}`,
    );
    expect(result.config).toEqual(DEFAULT_CONFIG);
  });

  it("parseConfig stays a never-throwing wrapper over the same config", () => {
    expect(parseConfig("{ nope")).toEqual(DEFAULT_CONFIG);
    expect(parseConfig("[1]")).toEqual(DEFAULT_CONFIG);
    const text = JSON.stringify({ ignore: ["a/**"] });
    expect(parseConfig(text)).toEqual(parseConfigResult(text).config);
  });
});
