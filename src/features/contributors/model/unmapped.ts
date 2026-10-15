import {
  githubLoginFromEmail,
  identityKey,
  isAgent,
  isExcluded,
} from "./identity";
import type { MailmapPerson } from "./identity";
import type { ResolvedPerson, Resolver } from "./identity";
import { matchesAnyIdentity } from "./patterns";
import { observedIdentities } from "./scoring";
import type { ContributorsConfig, GitIdentity, RawData } from "./types";

export type UnmappedIdentity = {
  key: string;
  identity: GitIdentity;
  /** Commits in the raw data authored with this identity. */
  commits: number;
  agent: boolean;
  /** Why it needs attention: an agent needs an owner, a machine account an owner or an exclusion. */
  reason: "agent" | "machine";
};

const MACHINE_LOCAL_PARTS = new Set([
  "root",
  "admin",
  "ubuntu",
  "runner",
  "ci",
  "build",
  "jenkins",
  "git",
  "github",
]);

/**
 * An address no person reads: a host without a dot (`dev@laptop`), a `.local`
 * host, a service-account local-part, or a CI runner. `runner` must stand
 * alone as a word, so a surname such as Brunner is not a machine.
 */
export function isMachineIdentity(identity: GitIdentity): boolean {
  const email = identity.email?.trim().toLowerCase() ?? "";
  const at = email.lastIndexOf("@");
  if (at <= 0) return false;
  const local = email.slice(0, at);
  const domain = email.slice(at + 1);
  if (!domain.includes(".") || domain.endsWith(".local")) return true;
  if (MACHINE_LOCAL_PARTS.has(local)) return true;
  return /(^|[^a-z])runner([^a-z]|$)/.test(local);
}

/** Every string a listed identity can be matched by: itself plus the login of a noreply email. */
function aliasesOf(identity: string): string[] {
  const lower = identity.trim().toLowerCase();
  const login = githubLoginFromEmail(lower);
  return login ? [lower, `gh:${login}`] : [lower];
}

/**
 * Identities seen in the raw data that no config person and no mailmap person
 * lists AND that need someone to look at them: agents (they need an owner) and
 * machine accounts such as CI runners. An ordinary author who is simply their
 * own person is not listed. Excluded identities are not listed. One shared
 * selector so the header badge and the Identities tab agree.
 */
export function unmappedIdentities(input: {
  raw: RawData;
  config: ContributorsConfig;
  mailmap: MailmapPerson[];
}): UnmappedIdentity[] {
  const { raw, config, mailmap } = input;
  const listed = new Set<string>();
  for (const person of config.people) {
    for (const id of person.identities)
      aliasesOf(id).forEach((a) => listed.add(a));
  }
  for (const group of mailmap) {
    for (const id of group.identities)
      aliasesOf(id).forEach((a) => listed.add(a));
  }

  const observed = observedIdentities(raw);
  // A login-only identity (a reviewer) belongs to the commit identity that
  // carries the same login; it is not a second unmapped person.
  const knownLogins = new Set<string>();
  for (const id of observed) {
    if (!id.email?.trim()) continue;
    // `||`: an empty login string must still fall back to the noreply email.
    const login =
      id.login?.trim().toLowerCase() || githubLoginFromEmail(id.email);
    if (login) knownLogins.add(login);
  }

  const commits = commitsByAuthorKey(raw);

  const out = new Map<string, UnmappedIdentity>();
  for (const id of observed) {
    const key = identityKey(id);
    if (out.has(key) || !key) continue;
    if (isExcluded(config, id)) continue;
    const candidates = new Set([key, ...aliasesOf(key)]);
    const login = id.login?.trim().toLowerCase();
    if (login) candidates.add(`gh:${login}`);
    if ([...candidates].some((c) => listed.has(c))) continue;
    if (key.startsWith("gh:") && knownLogins.has(key.slice(3))) continue;
    const agent = isAgent(config, id);
    if (!agent && !isMachineIdentity(id)) continue;
    out.set(key, {
      key,
      identity: id,
      commits: commits.get(key) ?? 0,
      agent,
      reason: agent ? "agent" : "machine",
    });
  }
  return Array.from(out.values()).sort(
    (a, b) => b.commits - a.commits || a.key.localeCompare(b.key),
  );
}

function commitsByAuthorKey(raw: RawData): Map<string, number> {
  const commits = new Map<string, number>();
  for (const c of raw.commits) {
    const key = identityKey(c.author);
    commits.set(key, (commits.get(key) ?? 0) + 1);
  }
  return commits;
}

export type ExcludedIdentity = {
  key: string;
  identity: GitIdentity;
  commits: number;
  /** Exclude patterns that match this identity. */
  patterns: string[];
  /**
   * True when every matching pattern is a literal, so "Include again" can lift
   * the exclusion. A wildcard such as `*[bot]*` covers other identities too and
   * is edited in the config file instead.
   */
  removable: boolean;
};

/** Observed identities the config excludes (agents are never excluded). */
export function excludedIdentities(input: {
  raw: RawData;
  config: ContributorsConfig;
}): ExcludedIdentity[] {
  const { raw, config } = input;
  const commits = commitsByAuthorKey(raw);
  const out = new Map<string, ExcludedIdentity>();
  for (const id of observedIdentities(raw)) {
    const key = identityKey(id);
    if (!key || out.has(key) || !isExcluded(config, id)) continue;
    const patterns = config.excludeIdentities.filter((p) =>
      matchesAnyIdentity([p], id),
    );
    out.set(key, {
      key,
      identity: id,
      commits: commits.get(key) ?? 0,
      patterns,
      removable: patterns.length > 0 && patterns.every((p) => !p.includes("*")),
    });
  }
  return Array.from(out.values()).sort(
    (a, b) => b.commits - a.commits || a.key.localeCompare(b.key),
  );
}

export type IdentityRow = { key: string; agent: boolean };

export type PersonRow = {
  id: string;
  name: string;
  identities: IdentityRow[];
  /** Commits in the raw data authored by any identity of this person. */
  commits: number;
  /** A person stored in the config file; the others only exist through observation. */
  inConfig: boolean;
};

function pseudoIdentity(value: string): GitIdentity {
  if (value.includes("@")) return { name: "", email: value };
  if (value.startsWith("gh:")) {
    return { name: "", email: "", login: value.slice(3) };
  }
  return { name: value, email: "" };
}

/** The observed identity a listed identity string stands for, else a stand-in built from the string. */
function identityFor(value: string, observed: GitIdentity[]): GitIdentity {
  const lower = value.trim().toLowerCase();
  const login = githubLoginFromEmail(lower);
  const hit = observed.find((o) => {
    if (identityKey(o) === lower) return true;
    const own = o.login?.trim().toLowerCase();
    return !!own && (lower === `gh:${own}` || own === login);
  });
  return hit ?? pseudoIdentity(lower);
}

/**
 * Listed identities plus observed ones that resolve to the person without being
 * listed (joined through a mailmap group), so none of them vanishes from the
 * tab. A login-only identity already tied to another identity is not extra.
 */
function identityKeysOf(person: ResolvedPerson): string[] {
  const keys = person.identities.map((k) => k.trim().toLowerCase());
  const known = new Set(keys.flatMap(aliasesOf));
  const logins = new Set<string>();
  for (const o of person.observed) {
    if (!o.email?.trim()) continue;
    const login =
      o.login?.trim().toLowerCase() || githubLoginFromEmail(o.email);
    if (login) logins.add(login);
  }
  for (const o of person.observed) {
    const key = identityKey(o);
    if (!key || known.has(key)) continue;
    if (key.startsWith("gh:") && logins.has(key.slice(3))) continue;
    if (key.startsWith("gh:") && known.has(`gh:${key.slice(3)}`)) continue;
    keys.push(key);
    aliasesOf(key).forEach((a) => known.add(a));
  }
  return keys;
}

/**
 * People for the Identities tab, from the model's resolver (so merge off gives
 * one person per raw identity). Auto people made only of agents or excluded
 * identities are left out: they show in the unmapped and excluded lists.
 */
export function personRows(input: {
  raw: RawData;
  config: ContributorsConfig;
  resolver: Resolver;
}): PersonRow[] {
  const { raw, config, resolver } = input;
  const configIds = new Set(config.people.map((p) => p.id));
  const commits = new Map<string, number>();
  for (const c of raw.commits) {
    const id = resolver.personOf(c.author);
    commits.set(id, (commits.get(id) ?? 0) + 1);
  }

  const rows: PersonRow[] = [];
  for (const person of resolver.people) {
    const inConfig = configIds.has(person.id);
    const onlyNonHuman =
      person.observed.length > 0 &&
      person.observed.every((o) => isAgent(config, o) || isExcluded(config, o));
    if (!inConfig && (onlyNonHuman || person.identities.length === 0)) continue;
    rows.push({
      id: person.id,
      name: person.name,
      identities: identityKeysOf(person).map((key) => ({
        key,
        agent: isAgent(config, identityFor(key, person.observed)),
      })),
      commits: commits.get(person.id) ?? 0,
      inConfig,
    });
  }
  return rows.sort(
    (a, b) => b.commits - a.commits || a.name.localeCompare(b.name),
  );
}

/**
 * Makes the person a config person that lists every identity the row shows, so
 * edits can target it and moving one identity out leaves the rest (including
 * mailmap members) with this person.
 */
export function ensureConfigPerson(
  config: ContributorsConfig,
  row: PersonRow,
): ContributorsConfig {
  const existing = config.people.find((p) => p.id === row.id);
  if (existing) {
    const missing = row.identities
      .map((i) => i.key)
      .filter((key) => !existing.identities.includes(key));
    if (missing.length === 0) return config;
    return {
      ...config,
      people: config.people.map((p) =>
        p.id === row.id
          ? { ...p, identities: [...p.identities, ...missing] }
          : p,
      ),
    };
  }
  return {
    ...config,
    people: [
      ...config.people,
      {
        id: row.id,
        name: row.name,
        identities: row.identities.map((i) => i.key),
      },
    ],
  };
}

/** Ungroups a config person; its identities fall back to mailmap or to their own people. */
export function removePerson(
  config: ContributorsConfig,
  personId: string,
): ContributorsConfig {
  const people = config.people.filter((p) => p.id !== personId);
  return people.length === config.people.length
    ? config
    : { ...config, people };
}
