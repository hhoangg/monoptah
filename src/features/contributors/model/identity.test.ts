import { describe, expect, it } from "vitest";
import { DEFAULT_CONFIG } from "./config";
import {
  buildResolver,
  createPerson,
  dismissMerge,
  excludeIdentity,
  githubLoginFromEmail,
  identityKey,
  includeIdentity,
  isAgent,
  isExcluded,
  isHuman,
  mergePeople,
  movePersonIdentity,
  parseMailmap,
  personIdFor,
  renamePerson,
  suggestMerges,
} from "./identity";
import type { ContributorsConfig, GitIdentity } from "./types";

const cfg = (patch: Partial<ContributorsConfig> = {}): ContributorsConfig => ({
  ...DEFAULT_CONFIG,
  people: [],
  dismissedMerges: [],
  ...patch,
});

const id = (name: string, email: string, login?: string): GitIdentity =>
  login ? { name, email, login } : { name, email };

describe("identityKey", () => {
  it("uses the lowercased email", () => {
    expect(identityKey(id("Alice", "Alice@Example.COM"))).toBe(
      "alice@example.com",
    );
  });

  it("trims the email", () => {
    expect(identityKey(id("A", "  a@x.com "))).toBe("a@x.com");
  });

  it("uses gh:<login> when the email is empty", () => {
    expect(identityKey(id("Alice", "", "Alice-Dev"))).toBe("gh:alice-dev");
  });

  it("prefers the email over the login", () => {
    expect(identityKey(id("Alice", "a@x.com", "alice"))).toBe("a@x.com");
  });

  it("falls back to the lowercased name", () => {
    expect(identityKey(id("Nguyễn Văn Đạt", ""))).toBe("nguyễn văn đạt");
    expect(identityKey(id("  Bob ", "  "))).toBe("bob");
  });

  it("returns an empty string when nothing is known", () => {
    expect(identityKey(id("", ""))).toBe("");
  });
});

describe("githubLoginFromEmail", () => {
  it("parses the id+login form", () => {
    expect(githubLoginFromEmail("12345+TuanN@users.noreply.github.com")).toBe(
      "tuann",
    );
  });

  it("parses the bare login form", () => {
    expect(githubLoginFromEmail("tuann@users.noreply.github.com")).toBe(
      "tuann",
    );
  });

  it("is case-insensitive on the domain and trims", () => {
    expect(githubLoginFromEmail(" 1+a@Users.NoReply.GitHub.com ")).toBe("a");
  });

  it("keeps bot suffixes", () => {
    expect(
      githubLoginFromEmail("49699333+dependabot[bot]@users.noreply.github.com"),
    ).toBe("dependabot[bot]");
  });

  it("returns undefined for other addresses", () => {
    expect(githubLoginFromEmail("a@example.com")).toBeUndefined();
    expect(githubLoginFromEmail("noreply@github.com")).toBeUndefined();
    expect(githubLoginFromEmail("a@noreply.github.com")).toBeUndefined();
    expect(githubLoginFromEmail("")).toBeUndefined();
    expect(githubLoginFromEmail("@users.noreply.github.com")).toBeUndefined();
    expect(githubLoginFromEmail("1+@users.noreply.github.com")).toBeUndefined();
  });
});

describe("isAgent / isExcluded / isHuman", () => {
  const config = cfg();

  it("recognizes AI agents by email, name and login", () => {
    expect(isAgent(config, id("Claude", "noreply@anthropic.com"))).toBe(true);
    expect(isAgent(config, id("Claude", "x@y.z"))).toBe(true);
    expect(isAgent(config, id("", "", "copilot-swe-agent[bot]"))).toBe(true);
    expect(isAgent(config, id("Devin", "devin@cognition.ai"))).toBe(true);
    expect(isAgent(config, id("Codex", "codex@openai.com"))).toBe(true);
    expect(isAgent(config, id("Cursor Agent", "cursoragent@cursor.com"))).toBe(
      true,
    );
  });

  it("does not treat humans who share a first name with an agent as agents", () => {
    for (const human of [
      id("Claude Dupont", "claude.dupont@corp.io"),
      id("Devin Nguyen", "devin@acme.com"),
      id("Gemini Rodriguez", "gemini.r@acme.com"),
      id("Codex Fan", "fan@acme.com"),
    ]) {
      expect(isAgent(config, human), human.name).toBe(false);
      expect(isHuman(config, human), human.name).toBe(true);
    }
  });

  it("recognizes generic bots as excluded, not agents", () => {
    const dependabot = id(
      "dependabot[bot]",
      "49699333+dependabot[bot]@users.noreply.github.com",
    );
    expect(isAgent(config, dependabot)).toBe(false);
    expect(isExcluded(config, dependabot)).toBe(true);
    expect(isHuman(config, dependabot)).toBe(false);
    expect(isExcluded(config, id("GitHub", "noreply@github.com"))).toBe(true);
    expect(isExcluded(config, id("CI", "build@ci-runner-3.corp"))).toBe(true);
  });

  it("lets agent win over excluded", () => {
    const bot = id(
      "copilot-swe-agent[bot]",
      "1+Copilot@users.noreply.github.com",
    );
    expect(isAgent(config, bot)).toBe(true);
    expect(isExcluded(config, bot)).toBe(false);
    expect(isHuman(config, bot)).toBe(false);
  });

  it("treats everyone else as human", () => {
    const alice = id("Alice", "alice@example.com");
    expect(isAgent(config, alice)).toBe(false);
    expect(isExcluded(config, alice)).toBe(false);
    expect(isHuman(config, alice)).toBe(true);
  });

  it("honors custom patterns", () => {
    const custom = cfg({
      excludeIdentities: ["old@x.com"],
      agentIdentities: [],
    });
    expect(isExcluded(custom, id("Old", "OLD@x.com"))).toBe(true);
    expect(isAgent(custom, id("Claude", "noreply@anthropic.com"))).toBe(false);
  });
});

describe("classification memo", () => {
  it("returns the same verdicts on repeated asks and for other fields of the same identity", () => {
    const config = cfg();
    const bot = id("dependabot[bot]", "bot@users.noreply.github.com");
    for (let i = 0; i < 3; i += 1) {
      expect(isExcluded(config, bot)).toBe(true);
      expect(isAgent(config, bot)).toBe(false);
    }
    // Same email, different name or login: not the same identity.
    expect(isAgent(config, id("Claude", "x@y.z"))).toBe(true);
    expect(isAgent(config, id("Ann", "x@y.z"))).toBe(false);
    expect(isAgent(config, id("Ann", "x@y.z", "copilot-swe-agent[bot]"))).toBe(
      true,
    );
  });

  it("follows a config whose lists were replaced", () => {
    const config = cfg({ excludeIdentities: [], agentIdentities: [] });
    const dev = id("Dev", "dev@x.com");
    expect(isExcluded(config, dev)).toBe(false);
    config.excludeIdentities = ["dev@x.com"];
    expect(isExcluded(config, dev)).toBe(true);
  });

  it("follows a list that grew in place", () => {
    const config = cfg({ excludeIdentities: [], agentIdentities: [] });
    const dev = id("Dev", "dev@x.com");
    expect(isAgent(config, dev)).toBe(false);
    config.agentIdentities.push("dev@x.com");
    expect(isAgent(config, dev)).toBe(true);
    expect(isExcluded(config, dev)).toBe(false);
  });

  it("keeps separate verdicts for separate configs", () => {
    const a = cfg({ excludeIdentities: ["dev@x.com"] });
    const b = cfg({ excludeIdentities: [] });
    const dev = id("Dev", "dev@x.com");
    expect(isExcluded(a, dev)).toBe(true);
    expect(isExcluded(b, dev)).toBe(false);
    expect(isHuman(b, dev)).toBe(true);
  });
});

describe("parseMailmap", () => {
  it("returns nothing for empty input", () => {
    expect(parseMailmap("")).toEqual([]);
    expect(parseMailmap("\n\n  \n")).toEqual([]);
  });

  it("parses the `Proper <proper>` form", () => {
    expect(parseMailmap("Jane Doe <Jane@X.com>")).toEqual([
      { name: "Jane Doe", identities: ["jane@x.com"] },
    ]);
  });

  it("parses the `<proper> <commit>` form", () => {
    expect(parseMailmap("<jane@x.com> <jd@old.com>")).toEqual([
      { name: "", identities: ["jane@x.com", "jd@old.com"] },
    ]);
  });

  it("parses the `Proper <proper> <commit>` form", () => {
    expect(parseMailmap("Jane Doe <jane@x.com> <jd@old.com>")).toEqual([
      { name: "Jane Doe", identities: ["jane@x.com", "jd@old.com"] },
    ]);
  });

  it("parses the `Proper <proper> Commit Name <commit>` form", () => {
    expect(
      parseMailmap("Jane Doe <jane@x.com> Jane D <JANE.D@laptop>"),
    ).toEqual([
      { name: "Jane Doe", identities: ["jane@x.com", "jane.d@laptop"] },
    ]);
  });

  it("groups lines by proper email and skips comments and junk", () => {
    const text = [
      "# authors",
      "",
      "Jane Doe <jane@x.com>",
      "<jane@x.com> <jd@old.com>",
      "Jane Doe <jane@x.com> Jane D <j@other>",
      "garbage line",
      "<unclosed",
      "Bob <bob@x.com>",
      "   # indented comment",
    ].join("\n");
    expect(parseMailmap(text)).toEqual([
      {
        name: "Jane Doe",
        identities: ["jane@x.com", "jd@old.com", "j@other"],
      },
      { name: "Bob", identities: ["bob@x.com"] },
    ]);
  });

  it("dedupes identities and handles CRLF", () => {
    const text = "A <a@x.com> <b@x.com>\r\nA <a@x.com> <B@X.com>\r\n";
    expect(parseMailmap(text)).toEqual([
      { name: "A", identities: ["a@x.com", "b@x.com"] },
    ]);
  });

  it("groups case-insensitively by proper email and keeps the first name", () => {
    const text = "<a@x.com> <b@x.com>\nAnna <A@x.com> <c@x.com>";
    expect(parseMailmap(text)).toEqual([
      { name: "Anna", identities: ["a@x.com", "b@x.com", "c@x.com"] },
    ]);
  });

  it("strips a trailing comment", () => {
    expect(
      parseMailmap("Jane <jane@new.com> <jane@old.com> # moved in 2020"),
    ).toEqual([{ name: "Jane", identities: ["jane@new.com", "jane@old.com"] }]);
    expect(parseMailmap("Bob <bob@x.com>   # just a name")).toEqual([
      { name: "Bob", identities: ["bob@x.com"] },
    ]);
  });

  it("handles unicode names", () => {
    expect(parseMailmap("Nguyễn Văn Đạt <dat@x.com> <d@old.com>")).toEqual([
      { name: "Nguyễn Văn Đạt", identities: ["dat@x.com", "d@old.com"] },
    ]);
  });
});

describe("buildResolver", () => {
  it("without merging, every distinct key is its own person", () => {
    const r = buildResolver(
      cfg({ people: [{ id: "p", name: "P", identities: ["a@x.com"] }] }),
      {
        merge: false,
        observedIdentities: [
          id("Alice", "a@x.com"),
          id("Alice", "A@X.com"),
          id("Alice", "a@y.com"),
        ],
      },
    );
    expect(r.personOf(id("Alice", "A@x.com"))).toBe("a@x.com");
    expect(r.personOf(id("Alice", "a@y.com"))).toBe("a@y.com");
    expect(r.people.map((p) => p.id).sort()).toEqual(["a@x.com", "a@y.com"]);
    expect(r.nameOf("a@x.com")).toBe("Alice");
  });

  it("maps unmapped identities to themselves when merging", () => {
    const r = buildResolver(cfg(), {
      merge: true,
      observedIdentities: [id("Bob", "bob@x.com")],
    });
    expect(r.personOf(id("Bob", "BOB@x.com"))).toBe("bob@x.com");
    expect(r.nameOf("bob@x.com")).toBe("Bob");
  });

  it("maps listed identities to the configured person", () => {
    const r = buildResolver(
      cfg({
        people: [
          {
            id: "alice",
            name: "Alice A",
            identities: ["a@x.com", "alice@y.com", "gh:alicea"],
          },
        ],
      }),
      {
        merge: true,
        observedIdentities: [id("A", "a@x.com"), id("Alice", "Alice@Y.com")],
      },
    );
    expect(r.personOf(id("A", "a@x.com"))).toBe("alice");
    expect(r.personOf(id("Alice", "ALICE@y.com"))).toBe("alice");
    expect(r.personOf(id("", "", "AliceA"))).toBe("alice");
    expect(r.nameOf("alice")).toBe("Alice A");
    const alice = r.people.find((p) => p.id === "alice");
    // the login-only identity looked up above is registered too
    expect(alice?.observed).toHaveLength(3);
  });

  it("maps a gh login to a person listing a noreply email with that login", () => {
    const r = buildResolver(
      cfg({
        people: [
          {
            id: "tuan",
            name: "Tuan",
            identities: ["12345+tuann@users.noreply.github.com"],
          },
        ],
      }),
      { merge: true },
    );
    expect(r.personOf(id("", "", "TuanN"))).toBe("tuan");
  });

  it("maps a noreply email to a person listing the gh login", () => {
    const r = buildResolver(
      cfg({ people: [{ id: "tuan", name: "Tuan", identities: ["gh:tuann"] }] }),
      { merge: true },
    );
    expect(r.personOf(id("T", "12345+tuann@users.noreply.github.com"))).toBe(
      "tuan",
    );
    expect(r.personOf(id("T", "other@x.com", "tuann"))).toBe("tuan");
  });

  it("links a login-only reviewer to the commit identity seen with that login", () => {
    const r = buildResolver(cfg(), {
      merge: true,
      observedIdentities: [
        id("Carol", "carol@corp.com", "carolc"),
        id("", "", "carolc"),
      ],
    });
    expect(r.personOf(id("", "", "CarolC"))).toBe("carol@corp.com");
  });

  it("links a login-only reviewer to a noreply email seen on commits", () => {
    const r = buildResolver(cfg(), {
      merge: true,
      observedIdentities: [id("Dan", "99+dand@users.noreply.github.com")],
    });
    expect(r.personOf(id("", "", "dand"))).toBe(
      "99+dand@users.noreply.github.com",
    );
  });

  it("does not link a login-only reviewer when merging is off", () => {
    const r = buildResolver(cfg(), {
      merge: false,
      observedIdentities: [id("Carol", "carol@corp.com", "carolc")],
    });
    expect(r.personOf(id("", "", "carolc"))).toBe("gh:carolc");
  });

  it("does not link two commit emails that share a login", () => {
    const r = buildResolver(cfg(), {
      merge: true,
      observedIdentities: [id("E", "e@a.com", "ee"), id("E", "e@b.com", "ee")],
    });
    expect(r.personOf(id("E", "e@a.com", "ee"))).toBe("e@a.com");
    expect(r.personOf(id("E", "e@b.com", "ee"))).toBe("e@b.com");
  });

  it("applies mailmap people under config people", () => {
    const r = buildResolver(
      cfg({
        people: [{ id: "boss", name: "The Boss", identities: ["jd@old.com"] }],
      }),
      {
        merge: true,
        mailmap: [
          {
            name: "Jane Doe",
            identities: ["jane@x.com", "jd@old.com", "j@other"],
          },
          { name: "Bob", identities: ["bob@x.com", "b@old.com"] },
        ],
        observedIdentities: [
          id("Jane", "jane@x.com"),
          id("J", "j@other"),
          id("B", "b@old.com"),
          id("Bobby", "bob@x.com"),
        ],
      },
    );
    // config wins: the config person lists one email of the mailmap group
    expect(r.personOf(id("Jane", "jane@x.com"))).toBe("boss");
    expect(r.personOf(id("J", "j@other"))).toBe("boss");
    expect(r.personOf(id("B", "b@old.com"))).toBe("bob@x.com");
    expect(r.personOf(id("Bobby", "bob@x.com"))).toBe("bob@x.com");
    expect(r.nameOf("bob@x.com")).toBe("Bob");
    expect(r.nameOf("boss")).toBe("The Boss");
  });

  it("falls back to observed names when the mailmap name is empty", () => {
    const r = buildResolver(cfg(), {
      merge: true,
      mailmap: [{ name: "", identities: ["a@x.com", "b@x.com"] }],
      observedIdentities: [id("Anna", "a@x.com"), id("Anna", "b@x.com")],
    });
    expect(r.personOf(id("Anna", "b@x.com"))).toBe("a@x.com");
    expect(r.nameOf("a@x.com")).toBe("Anna");
  });

  it("resolves an identity whose key equals a config person id", () => {
    const r = buildResolver(
      cfg({
        people: [{ id: "a@x.com", name: "Alice", identities: ["b@x.com"] }],
      }),
      {
        merge: true,
        observedIdentities: [id("A", "a@x.com"), id("B", "b@x.com")],
      },
    );
    expect(r.personOf(id("A", "a@x.com"))).toBe("a@x.com");
    expect(r.personOf(id("B", "b@x.com"))).toBe("a@x.com");
    expect(r.people).toHaveLength(1);
    expect(r.nameOf("a@x.com")).toBe("Alice");
  });

  it("works when destructured", () => {
    const { personOf, nameOf, people } = buildResolver(cfg(), {
      merge: true,
      observedIdentities: [id("Bob", "bob@x.com")],
    });
    expect(personOf(id("Bob", "bob@x.com"))).toBe("bob@x.com");
    expect(nameOf("bob@x.com")).toBe("Bob");
    expect(people.map((p) => p.id)).toEqual(["bob@x.com"]);
  });

  describe("order independence", () => {
    const commit = id("Alice", "alice@acme.com", "alice");
    const reviewer = id("", "", "alice");

    it("resolves a login-only identity the same before and after the commit identity", () => {
      const r = buildResolver(cfg(), {
        merge: true,
        observedIdentities: [commit, reviewer],
      });
      const first = r.personOf(reviewer);
      r.personOf(commit);
      const second = r.personOf(reviewer);
      expect(first).toBe("alice@acme.com");
      expect(second).toBe(first);
      expect(r.people.map((p) => p.id)).toEqual(["alice@acme.com"]);
    });

    it("does not learn links lazily from identities that were not observed up front", () => {
      const r = buildResolver(cfg(), { merge: true });
      const first = r.personOf(reviewer);
      expect(r.personOf(commit)).toBe("alice@acme.com");
      expect(r.personOf(reviewer)).toBe(first);

      const reversed = buildResolver(cfg(), { merge: true });
      expect(reversed.personOf(commit)).toBe("alice@acme.com");
      expect(reversed.personOf(reviewer)).toBe(first);
    });

    it("gives the same answers for any call order", () => {
      const observed = [commit, reviewer, id("Bob", "bob@x.com", "bobb")];
      const calls = [
        reviewer,
        commit,
        id("Bob", "bob@x.com", "bobb"),
        id("", "", "bobb"),
      ];
      const answers = (order: GitIdentity[]) => {
        const r = buildResolver(cfg(), {
          merge: true,
          observedIdentities: observed,
        });
        return new Map(order.map((i) => [identityKey(i), r.personOf(i)]));
      };
      const a = answers(calls);
      const b = answers([...calls].reverse());
      expect([...a].sort()).toEqual([...b].sort());
    });

    it("links a config person listing a noreply email to the login regardless of order", () => {
      const config = cfg({
        people: [
          {
            id: "t",
            name: "T",
            identities: ["1+tuan@users.noreply.github.com"],
          },
        ],
      });
      const r = buildResolver(config, { merge: true });
      const before = r.personOf(id("", "", "tuan"));
      r.personOf(id("T", "1+tuan@users.noreply.github.com"));
      expect(r.personOf(id("", "", "tuan"))).toBe(before);
      expect(before).toBe("t");
    });
  });

  it("picks the most frequent display name, ties to the longest", () => {
    const r = buildResolver(cfg(), {
      merge: true,
      observedIdentities: [
        id("Al", "a@x.com"),
        id("Alice", "a@x.com"),
        id("Al", "a@x.com"),
        id("Bo", "b@x.com"),
        id("Bobby", "b@x.com"),
      ],
    });
    expect(r.nameOf("a@x.com")).toBe("Al");
    expect(r.nameOf("b@x.com")).toBe("Bobby");
  });

  it("uses the login or key when no name is known", () => {
    const r = buildResolver(cfg(), {
      merge: true,
      observedIdentities: [id("", "", "ghost")],
    });
    expect(r.nameOf("gh:ghost")).toBe("ghost");
    expect(r.nameOf("unknown-id")).toBe("unknown-id");
  });

  it("lists config people even when never observed", () => {
    const r = buildResolver(
      cfg({ people: [{ id: "p", name: "P", identities: ["p@x.com"] }] }),
      { merge: true },
    );
    expect(r.people).toEqual([
      { id: "p", name: "P", identities: ["p@x.com"], observed: [] },
    ]);
  });

  it("registers identities first seen through personOf", () => {
    const r = buildResolver(cfg(), { merge: true });
    expect(r.personOf(id("Late", "late@x.com"))).toBe("late@x.com");
    expect(r.people.map((p) => p.id)).toEqual(["late@x.com"]);
    expect(r.nameOf("late@x.com")).toBe("Late");
  });

  it("is stable for repeated calls", () => {
    const r = buildResolver(cfg(), {
      merge: true,
      observedIdentities: [id("A", "a@x.com")],
    });
    expect(r.personOf(id("A", "a@x.com"))).toBe(r.personOf(id("A", "a@x.com")));
  });

  it("handles unicode names and uppercase emails", () => {
    const r = buildResolver(cfg(), {
      merge: true,
      observedIdentities: [id("Nguyễn Văn Đạt", "DAT@X.COM")],
    });
    expect(r.personOf(id("x", "dat@x.com"))).toBe("dat@x.com");
    expect(r.nameOf("dat@x.com")).toBe("Nguyễn Văn Đạt");
  });
});

describe("suggestMerges", () => {
  const p = (
    pid: string,
    name: string,
    identities: string[],
    observed?: GitIdentity[],
  ) => ({ id: pid, name, identities, observed });

  it("suggests persons with the same normalized display name", () => {
    const out = suggestMerges(
      [
        p("a@x.com", "Nguyễn  Văn A", ["a@x.com"]),
        p("b@y.com", "nguyen van a", ["b@y.com"]),
        p("c@z.com", "Someone Else", ["c@z.com"]),
      ],
      [],
    );
    expect(out).toHaveLength(1);
    expect(out[0].reason).toBe("same-name");
    expect(out[0].personIds).toEqual(["a@x.com", "b@y.com"]);
    expect(out[0].id).toBe("a@x.com|b@y.com");
    expect(out[0].label).toContain("name");
  });

  it("matches on any observed display name", () => {
    const out = suggestMerges(
      [
        p("a@x.com", "A", ["a@x.com"], [id("Dana Scully", "a@x.com")]),
        p("b@y.com", "B", ["b@y.com"], [id("dana scully", "b@y.com")]),
      ],
      [],
    );
    expect(out.map((s) => s.reason)).toEqual(["same-name"]);
  });

  it("groups three persons into one suggestion", () => {
    const out = suggestMerges(
      [
        p("c@x.com", "Sam Lee", ["c@x.com"]),
        p("a@x.com", "sam lee", ["a@x.com"]),
        p("b@x.com", "SAM LEE", ["b@x.com"]),
      ],
      [],
    );
    expect(out).toHaveLength(1);
    expect(out[0].personIds).toEqual(["a@x.com", "b@x.com", "c@x.com"]);
    expect(out[0].id).toBe("a@x.com|b@x.com|c@x.com");
  });

  it("never suggests a person with itself", () => {
    const out = suggestMerges(
      [
        p(
          "a@x.com",
          "Sam",
          ["a@x.com", "sam@y.com"],
          [id("Sam", "a@x.com"), id("sam", "sam@y.com")],
        ),
      ],
      [],
    );
    expect(out).toEqual([]);
  });

  it("suggests similar emails across different domains", () => {
    const out = suggestMerges(
      [
        p("p1", "One", ["john.doe@a.com"]),
        p("p2", "Two", ["johndoe+work@b.com"]),
      ],
      [],
    );
    expect(out).toHaveLength(1);
    expect(out[0].reason).toBe("similar-email");
    expect(out[0].personIds).toEqual(["p1", "p2"]);
  });

  it("does not suggest similar emails on the same domain", () => {
    const out = suggestMerges(
      [p("p1", "One", ["john.doe@a.com"]), p("p2", "Two", ["johndoe@a.com"])],
      [],
    );
    expect(out).toEqual([]);
  });

  it("does not suggest tiny or generic local parts", () => {
    const out = suggestMerges(
      [
        p("p1", "One", ["ab@a.com"]),
        p("p2", "Two", ["ab@b.com"]),
        p("p3", "Three", ["admin@a.com"]),
        p("p4", "Four", ["admin@b.com"]),
      ],
      [],
    );
    expect(out).toEqual([]);
  });

  it("suggests a GitHub noreply login that equals another email local part", () => {
    const out = suggestMerges(
      [
        p("p1", "One", ["12345+tuann@users.noreply.github.com"]),
        p("p2", "Two", ["tuann@corp.com"]),
      ],
      [],
    );
    expect(out).toHaveLength(1);
    expect(out[0].reason).toBe("github-noreply");
    expect(out[0].id).toBe("p1|p2");
  });

  it("suggests a gh: identity that equals another email local part", () => {
    const out = suggestMerges(
      [p("p1", "One", ["gh:tuann"]), p("p2", "Two", ["tuann@corp.com"])],
      [],
    );
    expect(out.map((s) => s.reason)).toEqual(["github-noreply"]);
  });

  it("keeps one suggestion per person set, preferring same-name", () => {
    const out = suggestMerges(
      [
        p("p1", "Tuan Nguyen", ["tuann@a.com"]),
        p("p2", "Tuan Nguyen", ["tuann@b.com"]),
      ],
      [],
    );
    expect(out).toHaveLength(1);
    expect(out[0].reason).toBe("same-name");
  });

  it("never suggests agents or excluded identities", () => {
    const out = suggestMerges(
      [
        p(
          "c1",
          "Claude",
          ["noreply@anthropic.com"],
          [id("Claude", "noreply@anthropic.com")],
        ),
        p(
          "c2",
          "Claude",
          ["claude@other.com"],
          [id("Claude", "claude@other.com")],
        ),
        p("d1", "dependabot[bot]", [
          "1+dependabot[bot]@users.noreply.github.com",
        ]),
        p("d2", "dependabot[bot]", [
          "2+dependabot[bot]@users.noreply.github.com",
        ]),
        p("g1", "GitHub", ["noreply@github.com"]),
        p("g2", "GitHub", ["gh:github"]),
      ],
      [],
    );
    expect(out).toEqual([]);
  });

  it("respects a custom config", () => {
    const custom = cfg({
      agentIdentities: [],
      excludeIdentities: ["*@ci.com"],
    });
    const out = suggestMerges(
      [
        p("a", "Robo Cop", ["r1@ci.com"]),
        p("b", "Robo Cop", ["r2@ci.com"]),
        p("c", "Real Person", ["r@x.com"]),
        p("d", "Real Person", ["r@y.org"]),
      ],
      [],
      custom,
    );
    expect(out.map((s) => s.personIds)).toEqual([["c", "d"]]);
  });

  it("filters dismissed suggestions", () => {
    const people = [
      p("a@x.com", "Sam Lee", ["a@x.com"]),
      p("b@x.com", "Sam Lee", ["b@x.com"]),
    ];
    const first = suggestMerges(people, []);
    expect(first).toHaveLength(1);
    expect(suggestMerges(people, [first[0].id])).toEqual([]);
    expect(suggestMerges(people, ["other|ids"])).toHaveLength(1);
  });

  it("keeps a dismissal valid when the reason changes", () => {
    const byEmail = [
      p("p1", "One", ["john.doe@a.com"]),
      p("p2", "Two", ["johndoe@b.com"]),
    ];
    const byName = [
      p("p1", "John Doe", ["john.doe@a.com"]),
      p("p2", "John Doe", ["johndoe@b.com"]),
    ];
    const first = suggestMerges(byEmail, []);
    expect(first[0].reason).toBe("similar-email");
    expect(suggestMerges(byName, [first[0].id])).toEqual([]);
  });

  it("still honors legacy dismissals that carry a reason suffix", () => {
    const people = [
      p("a@x.com", "Sam Lee", ["a@x.com"]),
      p("b@x.com", "Sam Lee", ["b@x.com"]),
    ];
    expect(suggestMerges(people, ["a@x.com|b@x.com:same-name"])).toEqual([]);
  });

  it("does not suggest single-word names", () => {
    expect(
      suggestMerges(
        [p("a@x.com", "Minh", ["a@x.com"]), p("b@y.com", "Minh", ["b@y.com"])],
        [],
      ),
    ).toEqual([]);
  });

  it("does not suggest placeholder names", () => {
    for (const name of [
      "Your Name",
      "root",
      "unknown",
      "admin",
      "user",
      "ubuntu",
      "github",
      "GitHub",
    ]) {
      expect(
        suggestMerges(
          [p("a@x.com", name, ["a@x.com"]), p("b@y.com", name, ["b@y.com"])],
          [],
        ),
        name,
      ).toEqual([]);
    }
  });

  it("does not suggest generic local parts", () => {
    for (const local of [
      "dev",
      "info",
      "admin",
      "noreply",
      "root",
      "me",
      "mail",
      "hello",
      "contact",
      "support",
      "team",
      "test",
      "user",
      "bot",
      "build",
      "office",
      "ci",
      "jenkins",
      "git",
      "github",
      "no-reply",
    ]) {
      expect(
        suggestMerges(
          [
            p("p1", "One", [`${local}@a.com`]),
            p("p2", "Two", [`${local}@b.com`]),
          ],
          [],
        ),
        local,
      ).toEqual([]);
    }
  });

  it("returns nothing for empty input", () => {
    expect(suggestMerges([], [])).toEqual([]);
    expect(suggestMerges([p("a", "", [])], [])).toEqual([]);
  });
});

describe("config editing helpers", () => {
  const base = cfg({
    people: [
      { id: "alice", name: "Alice", identities: ["a@x.com", "gh:alice"] },
      { id: "bob", name: "Bob", identities: ["b@x.com"] },
    ],
  });

  it("renamePerson renames an existing person without mutating", () => {
    const snapshot = JSON.stringify(base);
    const next = renamePerson(base, "alice", "  Alice Q ");
    expect(next.people[0].name).toBe("Alice Q");
    expect(next.people[1]).toEqual(base.people[1]);
    expect(JSON.stringify(base)).toBe(snapshot);
  });

  it("renamePerson creates a config person for an unmapped id", () => {
    const next = renamePerson(base, "c@x.com", "Carol");
    expect(next.people).toHaveLength(3);
    expect(next.people[2]).toEqual({
      id: "c@x.com",
      name: "Carol",
      identities: ["c@x.com"],
    });
  });

  it("renamePerson ignores a blank name", () => {
    expect(renamePerson(base, "alice", "   ")).toEqual(base);
  });

  describe("movePersonIdentity", () => {
    it("moves an identity to another existing person", () => {
      const next = movePersonIdentity(base, "gh:alice", "bob");
      expect(next.people.find((p) => p.id === "alice")?.identities).toEqual([
        "a@x.com",
      ]);
      expect(next.people.find((p) => p.id === "bob")?.identities).toEqual([
        "b@x.com",
        "gh:alice",
      ]);
    });

    it("removes the identity from every person when the target is null", () => {
      const next = movePersonIdentity(base, "A@x.com", null);
      expect(next.people.find((p) => p.id === "alice")?.identities).toEqual([
        "gh:alice",
      ]);
    });

    it("creates the target when it does not exist", () => {
      const next = movePersonIdentity(base, "c@x.com", "carol", {
        createdName: "Carol C",
      });
      expect(next.people.find((p) => p.id === "carol")).toEqual({
        id: "carol",
        name: "Carol C",
        identities: ["c@x.com"],
      });
    });

    it("names a created target after the identity key by default", () => {
      const next = movePersonIdentity(base, "c@x.com", "carol");
      expect(next.people.find((p) => p.id === "carol")?.name).toBe("c@x.com");
    });

    it("removes the identity from other persons first", () => {
      const next = movePersonIdentity(base, "a@x.com", "bob");
      const holders = next.people.filter((p) =>
        p.identities.includes("a@x.com"),
      );
      expect(holders.map((p) => p.id)).toEqual(["bob"]);
    });

    it("drops config people left without identities", () => {
      const next = movePersonIdentity(base, "b@x.com", "alice");
      expect(next.people.map((p) => p.id)).toEqual(["alice"]);
      const gone = movePersonIdentity(base, "b@x.com", null);
      expect(gone.people.map((p) => p.id)).toEqual(["alice"]);
    });

    it("keeps unrelated empty people, including one made by createPerson", () => {
      const withEmpty = createPerson(base, "New Hire");
      const next = movePersonIdentity(withEmpty, "c@x.com", "bob");
      expect(next.people.map((p) => p.id)).toEqual([
        "alice",
        "bob",
        "person-new-hire",
      ]);
      const hand = cfg({
        people: [
          { id: "ghost", name: "Ghost", identities: [] },
          { id: "bob", name: "Bob", identities: ["b@x.com"] },
        ],
      });
      expect(
        movePersonIdentity(hand, "z@x.com", "bob").people.map((p) => p.id),
      ).toEqual(["ghost", "bob"]);
    });

    it("can move an identity into a person made by createPerson", () => {
      const withEmpty = createPerson(base, "New Hire");
      const next = movePersonIdentity(withEmpty, "b@x.com", "person-new-hire");
      expect(next.people.map((p) => p.id)).toEqual([
        "alice",
        "person-new-hire",
      ]);
      expect(next.people[1].identities).toEqual(["b@x.com"]);
    });

    it("detaches an identity whose key is also a person id", () => {
      let c = renamePerson(cfg(), "alice@x.com", "Alice");
      c = mergePeople(c, ["alice@x.com", "bob@y.com"]);
      c = movePersonIdentity(c, "alice@x.com", null);
      const r = buildResolver(c, {
        merge: true,
        observedIdentities: [
          id("Alice", "alice@x.com"),
          id("Bob", "bob@y.com"),
        ],
      });
      const alice = r.personOf(id("Alice", "alice@x.com"));
      const bob = r.personOf(id("Bob", "bob@y.com"));
      expect(alice).toBe("alice@x.com");
      expect(bob).not.toBe(alice);
      expect(c.people).toHaveLength(1);
      expect(c.people[0].identities).toEqual(["bob@y.com"]);
      expect(c.people[0].name).toBe("Alice");
    });

    it("re-ids a person whose id equals the moved key when moving elsewhere", () => {
      let c = renamePerson(cfg(), "alice@x.com", "Alice");
      c = mergePeople(c, ["alice@x.com", "bob@y.com"]);
      c = movePersonIdentity(c, "alice@x.com", "carol", {
        createdName: "Carol",
      });
      const r = buildResolver(c, {
        merge: true,
        observedIdentities: [id("A", "alice@x.com"), id("B", "bob@y.com")],
      });
      expect(r.personOf(id("A", "alice@x.com"))).toBe("carol");
      expect(r.personOf(id("B", "bob@y.com"))).not.toBe("carol");
    });

    it("rewrites dismissed merges that reference a re-id'd person", () => {
      let c = renamePerson(cfg(), "alice@x.com", "Alice");
      c = mergePeople(c, ["alice@x.com", "bob@y.com"]);
      c = {
        ...c,
        dismissedMerges: [
          "alice@x.com|zed@z.com",
          "alice@x.com|zed@z.com:same-name",
          "bob@y.com|carol@c.com",
          "carol@c.com|zed@z.com",
        ],
      };
      const next = movePersonIdentity(c, "alice@x.com", null);
      expect(next.people[0].id).toBe("person-alice");
      expect(next.dismissedMerges).toEqual([
        "person-alice|zed@z.com",
        "person-alice|zed@z.com:same-name",
        "bob@y.com|carol@c.com",
        "carol@c.com|zed@z.com",
      ]);
    });

    it("keeps dismissed ids sorted and deduped after a re-id", () => {
      let c = renamePerson(cfg(), "alice@x.com", "Alice");
      c = mergePeople(c, ["alice@x.com", "bob@y.com"]);
      c = {
        ...c,
        dismissedMerges: ["alice@x.com|zzz@z.com", "aaa@a.com|alice@x.com"],
      };
      const next = movePersonIdentity(c, "alice@x.com", null);
      expect(next.dismissedMerges).toEqual([
        "person-alice|zzz@z.com",
        "aaa@a.com|person-alice",
      ]);
    });

    it("does not touch dismissed merges when nothing is re-id'd", () => {
      const c = { ...base, dismissedMerges: ["alice|bob"] };
      expect(movePersonIdentity(c, "b@x.com", "alice").dismissedMerges).toEqual(
        ["alice|bob"],
      );
    });

    it("does not re-id a person that receives its own key", () => {
      let c = renamePerson(cfg(), "alice@x.com", "Alice");
      c = movePersonIdentity(c, "alice@x.com", "alice@x.com");
      expect(c.people.map((p) => p.id)).toEqual(["alice@x.com"]);
    });

    it("is idempotent and does not duplicate", () => {
      const once = movePersonIdentity(base, "b@x.com", "alice");
      const twice = movePersonIdentity(once, "b@x.com", "alice");
      expect(twice).toEqual(once);
    });

    it("does not mutate the input", () => {
      const snapshot = JSON.stringify(base);
      movePersonIdentity(base, "b@x.com", "alice");
      expect(JSON.stringify(base)).toBe(snapshot);
    });
  });

  describe("mergePeople", () => {
    it("merges later people into the first and keeps its name", () => {
      const next = mergePeople(base, ["alice", "bob"]);
      expect(next.people).toEqual([
        {
          id: "alice",
          name: "Alice",
          identities: ["a@x.com", "gh:alice", "b@x.com"],
        },
      ]);
    });

    it("uses the provided name", () => {
      expect(
        mergePeople(base, ["bob", "alice"], "Team Person").people[0].name,
      ).toBe("Team Person");
    });

    it("turns unmapped ids into identities of the target", () => {
      const next = mergePeople(base, ["alice", "c@x.com", "d@y.com"]);
      expect(next.people).toHaveLength(2);
      expect(next.people[0].identities).toEqual([
        "a@x.com",
        "gh:alice",
        "c@x.com",
        "d@y.com",
      ]);
    });

    it("creates the target when the first id is unmapped", () => {
      const next = mergePeople(base, ["c@x.com", "bob"], "Carol");
      const carol = next.people.find((p) => p.id === "c@x.com");
      expect(carol).toEqual({
        id: "c@x.com",
        name: "Carol",
        identities: ["c@x.com", "b@x.com"],
      });
      expect(next.people.find((p) => p.id === "bob")).toBeUndefined();
    });

    it("dedupes identities and ignores duplicate ids", () => {
      const next = mergePeople(base, ["alice", "alice", "a@x.com"]);
      expect(next.people[0].identities).toEqual(["a@x.com", "gh:alice"]);
    });

    it("does nothing for fewer than two distinct ids", () => {
      expect(mergePeople(base, ["alice"])).toEqual(base);
      expect(mergePeople(base, [])).toEqual(base);
      expect(mergePeople(base, ["alice", "alice"])).toEqual(base);
    });

    it("does not mutate the input", () => {
      const snapshot = JSON.stringify(base);
      mergePeople(base, ["alice", "bob"]);
      expect(JSON.stringify(base)).toBe(snapshot);
    });
  });

  describe("createPerson / personIdFor", () => {
    it("appends an empty person with a slug id", () => {
      const next = createPerson(base, " Nguyễn Văn Đạt ");
      const created = next.people[2];
      expect(created.name).toBe("Nguyễn Văn Đạt");
      expect(created.identities).toEqual([]);
      expect(created.id).toBe("person-nguyen-van-dat");
      expect(personIdFor(base, " Nguyễn Văn Đạt ")).toBe(created.id);
    });

    it("makes ids unique", () => {
      const once = createPerson(base, "Sam");
      const twice = createPerson(once, "Sam");
      expect(twice.people.map((p) => p.id).slice(2)).toEqual([
        "person-sam",
        "person-sam-2",
      ]);
    });

    it("falls back to a generic slug for symbol-only names", () => {
      expect(personIdFor(base, "***")).toBe("person-person");
    });

    it("ignores blank names", () => {
      expect(createPerson(base, "  ")).toEqual(base);
    });
  });

  describe("exclude / include / dismiss", () => {
    it("excludeIdentity adds a lowercased key once", () => {
      const once = excludeIdentity(base, "Bot@X.com");
      expect(once.excludeIdentities).toContain("bot@x.com");
      const twice = excludeIdentity(once, "bot@x.com");
      expect(
        twice.excludeIdentities.filter((p) => p === "bot@x.com"),
      ).toHaveLength(1);
      expect(base.excludeIdentities).not.toContain("bot@x.com");
    });

    it("excludeIdentity makes the identity excluded", () => {
      const next = excludeIdentity(base, "gh:ghost");
      expect(isExcluded(next, id("", "", "ghost"))).toBe(true);
    });

    it("includeIdentity removes a pattern case-insensitively", () => {
      const next = includeIdentity(base, "NOREPLY@github.com");
      expect(next.excludeIdentities).not.toContain("noreply@github.com");
      expect(next.excludeIdentities).toContain("*@ci-runner*");
    });

    it("includeIdentity leaves the config alone when the pattern is absent", () => {
      expect(includeIdentity(base, "missing@x.com")).toEqual(base);
    });

    it("dismissMerge adds an id once", () => {
      const once = dismissMerge(base, "a|b:same-name");
      expect(once.dismissedMerges).toEqual(["a|b:same-name"]);
      expect(dismissMerge(once, "a|b:same-name").dismissedMerges).toEqual([
        "a|b:same-name",
      ]);
      expect(base.dismissedMerges).toEqual([]);
    });
  });
});
