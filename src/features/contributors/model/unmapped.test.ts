import { describe, expect, it } from "vitest";
import { DEFAULT_CONFIG } from "./config";
import {
  ensureConfigPerson,
  excludedIdentities,
  personRows,
  removePerson,
  unmappedIdentities,
} from "./unmapped";
import {
  ANA,
  BOB,
  CAT,
  CLAUDE,
  DEPENDABOT,
  ident,
  makeCommit,
  makeRaw,
  makeResolver,
  oct,
  withConfig,
} from "./testFixtures";
import { buildResolver } from "./identity";
import { observedIdentities } from "./scoring";

const BUILD_BOX = ident("Build Box", "ci@build-box");
const RUNNER = ident("runner", "runner@ip-10-0-0-5.ec2.internal");

describe("unmappedIdentities", () => {
  it("lists agents and machine identities, most commits first", () => {
    const raw = makeRaw({
      commits: [
        makeCommit("c1", CLAUDE, oct(1)),
        makeCommit("b1", BUILD_BOX, oct(2)),
        makeCommit("b2", BUILD_BOX, oct(3)),
      ],
    });
    const out = unmappedIdentities({
      raw,
      config: DEFAULT_CONFIG,
      mailmap: [],
    });
    expect(out.map((u) => [u.key, u.reason, u.commits])).toEqual([
      ["ci@build-box", "machine", 2],
      ["noreply@anthropic.com", "agent", 1],
    ]);
    expect(out[1].agent).toBe(true);
    expect(out[0].agent).toBe(false);
  });

  it("does not list ordinary people, who are simply their own person", () => {
    const raw = makeRaw({
      commits: [makeCommit("a1", ANA, oct(1)), makeCommit("b1", BOB, oct(2))],
    });
    expect(
      unmappedIdentities({ raw, config: DEFAULT_CONFIG, mailmap: [] }),
    ).toEqual([]);
  });

  it.each([
    ["a domain without a dot", ident("Dev", "dev@laptop")],
    ["a .local domain", ident("Dev", "dev@Mac-Studio.local")],
    ["a root local-part", ident("root", "root@example.com")],
    ["an admin local-part", ident("Admin", "admin@example.com")],
    ["an ubuntu local-part", ident("ubuntu", "ubuntu@example.com")],
    ["a jenkins local-part", ident("Jenkins", "jenkins@example.com")],
    ["a git local-part", ident("git", "git@example.com")],
    ["a github local-part", ident("GH", "github@example.com")],
    ["a build local-part", ident("b", "build@example.com")],
    ["a ci local-part", ident("ci", "ci@example.com")],
    ["a runner local-part", RUNNER],
    ["a ci-runner local-part", ident("x", "ci-runner-7@example.com")],
    ["a gitlab-runner local-part", ident("x", "gitlab-runner@example.com")],
  ])("lists %s as a machine identity", (_label, who) => {
    const raw = makeRaw({ commits: [makeCommit("m1", who, oct(1))] });
    const out = unmappedIdentities({
      raw,
      config: DEFAULT_CONFIG,
      mailmap: [],
    });
    expect(out.map((u) => u.reason)).toEqual(["machine"]);
  });

  it("does not mistake a human for a machine by a lookalike name", () => {
    const raw = makeRaw({
      commits: [
        makeCommit(
          "h1",
          ident("Ann Brunner", "ann.brunner@example.com"),
          oct(1),
        ),
        makeCommit("h2", ident("Gita", "gita@example.com"), oct(2)),
        makeCommit("h3", ident("Cia", "cian@example.com"), oct(3)),
        makeCommit("h4", ident("Dev", "dev@corp.example.co.uk"), oct(4)),
      ],
    });
    expect(
      unmappedIdentities({ raw, config: DEFAULT_CONFIG, mailmap: [] }),
    ).toEqual([]);
  });

  it("skips identities listed by a config person, by email or login", () => {
    const raw = makeRaw({
      commits: [
        makeCommit("a1", BUILD_BOX, oct(1)),
        makeCommit(
          "b1",
          ident("Jenkins", "jenkins@example.com", "jenk"),
          oct(2),
        ),
      ],
    });
    const config = withConfig({
      people: [
        { id: "ops", name: "Ops", identities: ["ci@build-box"] },
        { id: "jenk", name: "Jenkins", identities: ["gh:jenk"] },
      ],
    });
    expect(unmappedIdentities({ raw, config, mailmap: [] })).toEqual([]);
  });

  it("skips identities that a mailmap group lists", () => {
    const raw = makeRaw({ commits: [makeCommit("a1", BUILD_BOX, oct(1))] });
    const out = unmappedIdentities({
      raw,
      config: DEFAULT_CONFIG,
      mailmap: [{ name: "Ops", identities: ["CI@Build-Box"] }],
    });
    expect(out).toEqual([]);
  });

  it("skips excluded machine identities and keeps agents, flagged", () => {
    const raw = makeRaw({
      commits: [
        makeCommit("d1", DEPENDABOT, oct(1)),
        makeCommit("c1", CLAUDE, oct(2)),
        makeCommit("a1", ANA, oct(3)),
        makeCommit("b1", BUILD_BOX, oct(4)),
      ],
    });
    const config = withConfig({
      excludeIdentities: [...DEFAULT_CONFIG.excludeIdentities, "ci@build-box"],
    });
    const out = unmappedIdentities({ raw, config, mailmap: [] });
    expect(out.map((u) => [u.key, u.agent])).toEqual([
      ["noreply@anthropic.com", true],
    ]);
  });
});

describe("unmappedIdentities login handling", () => {
  it("marks the login of a noreply email as known even when login is an empty string", () => {
    const noreply = {
      name: "Ana",
      email: "12345+ana@users.noreply.github.com",
      login: "",
    };
    const raw = makeRaw({
      commits: [makeCommit("a1", noreply, oct(1))],
      reviews: [
        {
          prId: "1",
          reviewer: ident("", "", "ana"),
          state: "APPROVED",
          submittedAt: oct(2),
          comments: 0,
        },
      ],
    });
    const out = unmappedIdentities({
      raw,
      config: DEFAULT_CONFIG,
      mailmap: [],
    });
    // Ana is an ordinary person, and her reviewer login is not a second one.
    expect(out).toEqual([]);
  });

  it("lists a noreply agent once even when a reviewer login names the same agent", () => {
    const copilot = {
      name: "Copilot",
      email: "198982749+Copilot@users.noreply.github.com",
      login: "",
    };
    const raw = makeRaw({
      commits: [makeCommit("a1", copilot, oct(1))],
      reviews: [
        {
          prId: "1",
          reviewer: ident("", "", "Copilot"),
          state: "APPROVED",
          submittedAt: oct(2),
          comments: 0,
        },
      ],
    });
    const out = unmappedIdentities({
      raw,
      config: DEFAULT_CONFIG,
      mailmap: [],
    });
    expect(out.map((u) => u.key)).toEqual([
      "198982749+copilot@users.noreply.github.com",
    ]);
  });
});

describe("excludedIdentities", () => {
  it("lists excluded observed identities with their commits and matching patterns", () => {
    const raw = makeRaw({
      commits: [
        makeCommit("d1", DEPENDABOT, oct(1)),
        makeCommit("d2", DEPENDABOT, oct(2)),
        makeCommit("c1", CLAUDE, oct(3)),
        makeCommit("a1", ANA, oct(4)),
      ],
    });
    const out = excludedIdentities({ raw, config: DEFAULT_CONFIG });
    expect(out.map((e) => e.key)).toEqual([
      "49699333+dependabot[bot]@users.noreply.github.com",
    ]);
    expect(out[0].commits).toBe(2);
    expect(out[0].patterns).toContain("*[bot]*");
  });

  it("is removable only when every matching pattern is a literal", () => {
    const raw = makeRaw({
      commits: [
        makeCommit(
          "g1",
          { name: "GitHub", email: "noreply@github.com" },
          oct(1),
        ),
        makeCommit("d1", DEPENDABOT, oct(2)),
        makeCommit("a1", ANA, oct(3)),
      ],
    });
    const config = withConfig({
      excludeIdentities: ["noreply@github.com", "*[bot]*", "ana@example.com"],
    });
    const out = excludedIdentities({ raw, config });
    const byKey = new Map(out.map((e) => [e.key, e]));
    expect(byKey.get("noreply@github.com")?.removable).toBe(true);
    expect(byKey.get("ana@example.com")?.removable).toBe(true);
    expect(byKey.get("ana@example.com")?.patterns).toEqual(["ana@example.com"]);
    const bot = out.find((e) => e.key.includes("dependabot"));
    expect(bot?.removable).toBe(false);
  });

  it("never lists agents (agents win over exclude)", () => {
    const raw = makeRaw({ commits: [makeCommit("c1", CLAUDE, oct(1))] });
    const config = withConfig({ excludeIdentities: ["noreply@anthropic.com"] });
    expect(excludedIdentities({ raw, config })).toEqual([]);
  });
});

describe("personRows", () => {
  const rows = (
    raw = makeRaw(),
    config = DEFAULT_CONFIG,
    merge = true,
    mailmap: { name: string; identities: string[] }[] = [],
  ) => {
    const resolver = buildResolver(config, {
      merge,
      mailmap,
      observedIdentities: observedIdentities(raw),
    });
    return personRows({ raw, config, resolver });
  };

  it("lists resolved people with their identities and commit totals", () => {
    const raw = makeRaw({
      commits: [
        makeCommit("a1", ANA, oct(1)),
        makeCommit("a2", ident("Ana", "ana@laptop.local"), oct(2)),
        makeCommit("b1", BOB, oct(3)),
      ],
    });
    const config = withConfig({
      people: [
        {
          id: "ana",
          name: "Ana Sol",
          identities: ["ana@example.com", "ana@laptop.local"],
        },
      ],
    });
    const out = rows(raw, config);
    expect(out.map((p) => p.name)).toEqual(["Ana Sol", "Bob"]);
    expect(out[0]).toMatchObject({ id: "ana", commits: 2, inConfig: true });
    expect(out[0].identities.map((i) => i.key)).toEqual([
      "ana@example.com",
      "ana@laptop.local",
    ]);
    expect(out[1]).toMatchObject({ id: "bob@example.com", inConfig: false });
  });

  it("keeps an empty config person so a new person is visible", () => {
    const config = withConfig({
      people: [{ id: "person-new", name: "New person", identities: [] }],
    });
    const out = rows(
      makeRaw({ commits: [makeCommit("a", ANA, oct(1))] }),
      config,
    );
    expect(out.find((p) => p.id === "person-new")).toMatchObject({
      identities: [],
      commits: 0,
      inConfig: true,
    });
  });

  it("hides auto people that are only agents or excluded identities", () => {
    const raw = makeRaw({
      commits: [
        makeCommit("d1", DEPENDABOT, oct(1)),
        makeCommit("c1", CLAUDE, oct(2)),
        makeCommit("a1", ANA, oct(3)),
      ],
    });
    expect(rows(raw).map((p) => p.name)).toEqual(["Ana"]);
  });

  it("flags agent identities inside a person", () => {
    const raw = makeRaw({
      commits: [
        makeCommit("a1", ANA, oct(1)),
        makeCommit("c1", CLAUDE, oct(2)),
      ],
    });
    const config = withConfig({
      people: [
        {
          id: "ana",
          name: "Ana",
          identities: ["ana@example.com", "noreply@anthropic.com"],
        },
      ],
    });
    const [ana] = rows(raw, config);
    expect(ana.identities.map((i) => [i.key, i.agent])).toEqual([
      ["ana@example.com", false],
      ["noreply@anthropic.com", true],
    ]);
  });

  it("with merging off every raw identity is its own person", () => {
    const raw = makeRaw({
      commits: [
        makeCommit("a1", ANA, oct(1)),
        makeCommit("a2", ident("Ana", "ana@laptop.local"), oct(2)),
      ],
    });
    const config = withConfig({
      people: [
        {
          id: "ana",
          name: "Ana Sol",
          identities: ["ana@example.com", "ana@laptop.local"],
        },
      ],
    });
    const out = rows(raw, config, false);
    expect(out.map((p) => p.id).sort()).toEqual([
      "ana@example.com",
      "ana@laptop.local",
    ]);
    expect(out.every((p) => !p.inConfig)).toBe(true);
  });

  it("lists identities a mailmap group adds to a config person", () => {
    const raw = makeRaw({
      commits: [
        makeCommit("a1", ANA, oct(1)),
        makeCommit("a2", ident("Ana", "ana@laptop.local"), oct(2)),
      ],
    });
    const config = withConfig({
      people: [{ id: "ana", name: "Ana Sol", identities: ["ana@example.com"] }],
    });
    const out = rows(raw, config, true, [
      { name: "Ana", identities: ["ana@example.com", "ana@laptop.local"] },
    ]);
    expect(out).toHaveLength(1);
    expect(out[0].identities.map((i) => i.key)).toEqual([
      "ana@example.com",
      "ana@laptop.local",
    ]);
  });

  it("does not list a login-only reviewer that is already linked to a listed identity", () => {
    const raw = makeRaw({
      commits: [makeCommit("a1", ANA, oct(1))],
      reviews: [
        {
          prId: "1",
          reviewer: ident("", "", "ana"),
          state: "APPROVED",
          submittedAt: oct(2),
          comments: 0,
        },
      ],
    });
    const config = withConfig({
      people: [{ id: "ana", name: "Ana", identities: ["ana@example.com"] }],
    });
    expect(rows(raw, config)[0].identities.map((i) => i.key)).toEqual([
      "ana@example.com",
    ]);
  });

  it("orders by commits, then name", () => {
    const raw = makeRaw({
      commits: [
        makeCommit("c1", CAT, oct(1)),
        makeCommit("b1", BOB, oct(2)),
        makeCommit("b2", BOB, oct(3)),
        makeCommit("a1", ANA, oct(4)),
      ],
    });
    expect(rows(raw).map((p) => p.name)).toEqual(["Bob", "Ana", "Cat"]);
  });

  it("uses the shared resolver of the model", () => {
    const raw = makeRaw({ commits: [makeCommit("a1", ANA, oct(1))] });
    const resolver = makeResolver(raw);
    expect(personRows({ raw, config: DEFAULT_CONFIG, resolver })).toHaveLength(
      1,
    );
  });
});

describe("config helpers for the Identities tab", () => {
  it("ensureConfigPerson materializes a person that only exists through observation", () => {
    const row = {
      id: "ana@example.com",
      name: "Ana",
      identities: [{ key: "ana@example.com", agent: false }],
      commits: 1,
      inConfig: false,
    };
    const next = ensureConfigPerson(DEFAULT_CONFIG, row);
    expect(next.people).toEqual([
      { id: "ana@example.com", name: "Ana", identities: ["ana@example.com"] },
    ]);
  });

  it("ensureConfigPerson returns the same config for a config person", () => {
    const config = withConfig({
      people: [{ id: "ana", name: "Ana", identities: ["ana@example.com"] }],
    });
    const row = {
      id: "ana",
      name: "Ana",
      identities: [{ key: "ana@example.com", agent: false }],
      commits: 1,
      inConfig: true,
    };
    expect(ensureConfigPerson(config, row)).toBe(config);
  });

  it("ensureConfigPerson lists identities a config person only has through observation", () => {
    const config = withConfig({
      people: [{ id: "ana", name: "Ana", identities: ["ana@example.com"] }],
    });
    const row = {
      id: "ana",
      name: "Ana",
      identities: [
        { key: "ana@example.com", agent: false },
        { key: "ana@laptop.local", agent: false },
      ],
      commits: 2,
      inConfig: true,
    };
    expect(ensureConfigPerson(config, row).people[0].identities).toEqual([
      "ana@example.com",
      "ana@laptop.local",
    ]);
  });

  it("removePerson drops a config person and nothing else", () => {
    const config = withConfig({
      people: [
        { id: "ana", name: "Ana", identities: ["ana@example.com"] },
        { id: "bob", name: "Bob", identities: ["bob@example.com"] },
      ],
    });
    expect(removePerson(config, "ana").people.map((p) => p.id)).toEqual([
      "bob",
    ]);
    expect(removePerson(config, "zed")).toBe(config);
  });
});
