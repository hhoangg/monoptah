import { DEFAULT_CONFIG } from "./config";
import { matchesAnyIdentity } from "./patterns";
import type {
  ContributorsConfig,
  GitIdentity,
  IdentityKey,
  Person,
} from "./types";

const NOREPLY_RE = /^(?:\d+\+)?([^@+\s]+)@users\.noreply\.github\.com$/i;

/** Lowercased email; `gh:<login>` when only a login is known; else the lowercased name. */
export function identityKey(id: GitIdentity): IdentityKey {
  const email = id.email?.trim().toLowerCase();
  if (email) return email;
  const login = id.login?.trim().toLowerCase();
  if (login) return `gh:${login}`;
  return (id.name ?? "").trim().toLowerCase();
}

/** `123+login@users.noreply.github.com` or `login@users.noreply.github.com`. */
export function githubLoginFromEmail(email: string): string | undefined {
  const m = NOREPLY_RE.exec((email ?? "").trim());
  return m ? m[1].toLowerCase() : undefined;
}

/**
 * Whether an identity is an agent or excluded, remembered per config. A model
 * build asks this for every author, committer and co-author of every commit,
 * PR and review, but only a few dozen distinct identities exist, so each is
 * matched against the pattern lists once. The memo is dropped when the config
 * lists are replaced or resized (every edit builds new lists).
 */
type Verdicts = { agent: boolean; excluded: boolean };
type Memo = {
  agentList: string[];
  agentCount: number;
  excludeList: string[];
  excludeCount: number;
  verdicts: Map<string, Verdicts>;
};
const memos = new WeakMap<ContributorsConfig, Memo>();

function verdicts(config: ContributorsConfig, id: GitIdentity): Verdicts {
  let memo = memos.get(config);
  if (
    !memo ||
    memo.agentList !== config.agentIdentities ||
    memo.agentCount !== config.agentIdentities.length ||
    memo.excludeList !== config.excludeIdentities ||
    memo.excludeCount !== config.excludeIdentities.length
  ) {
    memo = {
      agentList: config.agentIdentities,
      agentCount: config.agentIdentities.length,
      excludeList: config.excludeIdentities,
      excludeCount: config.excludeIdentities.length,
      verdicts: new Map(),
    };
    memos.set(config, memo);
  }
  const key = `${id.email ?? ""}\u0000${id.name ?? ""}\u0000${id.login ?? ""}`;
  let found = memo.verdicts.get(key);
  if (!found) {
    // An identity matching both lists counts as an agent, not as excluded.
    const agent = matchesAnyIdentity(config.agentIdentities, id);
    found = {
      agent,
      excluded: !agent && matchesAnyIdentity(config.excludeIdentities, id),
    };
    memo.verdicts.set(key, found);
  }
  return found;
}

export function isAgent(config: ContributorsConfig, id: GitIdentity): boolean {
  return verdicts(config, id).agent;
}

/** An identity matching both lists counts as an agent, not as excluded. */
export function isExcluded(
  config: ContributorsConfig,
  id: GitIdentity,
): boolean {
  return verdicts(config, id).excluded;
}

export function isHuman(config: ContributorsConfig, id: GitIdentity): boolean {
  const v = verdicts(config, id);
  return !v.agent && !v.excluded;
}

export type MailmapPerson = { name: string; identities: string[] };

/**
 * Parses the four git `.mailmap` forms. Groups by proper email; identities are
 * lowercased emails, the proper one first. `name` is "" when no line names it.
 */
export function parseMailmap(text: string): MailmapPerson[] {
  const groups = new Map<string, MailmapPerson>();
  for (const raw of text.split(/\r?\n/)) {
    const line = raw.trim();
    if (!line || line.startsWith("#")) continue;
    const m = /^([^<>]*)<([^<>]*)>(?:\s*([^<>]*)<([^<>]*)>)?\s*(?:#.*)?$/.exec(
      line,
    );
    if (!m) continue;
    const name = m[1].trim();
    const proper = m[2].trim().toLowerCase();
    const commit = m[4]?.trim().toLowerCase();
    if (!proper) continue;
    let group = groups.get(proper);
    if (!group) {
      group = { name: "", identities: [proper] };
      groups.set(proper, group);
    }
    if (!group.name && name) group.name = name;
    if (commit && !group.identities.includes(commit)) {
      group.identities.push(commit);
    }
  }
  return Array.from(groups.values());
}

export type ResolvedPerson = Person & { observed: GitIdentity[] };

export type Resolver = {
  /** Person id of an identity. */
  personOf(id: GitIdentity): string;
  nameOf(personId: string): string;
  /** Every person seen so far plus all config people. */
  readonly people: ResolvedPerson[];
};

export type ResolverOptions = {
  mailmap?: MailmapPerson[];
  observedIdentities?: GitIdentity[];
  merge: boolean;
};

function bestName(identities: GitIdentity[]): string {
  const counts = new Map<string, number>();
  for (const id of identities) {
    const name = id.name?.trim();
    if (name) counts.set(name, (counts.get(name) ?? 0) + 1);
  }
  let best = "";
  let bestCount = 0;
  for (const [name, count] of counts) {
    if (
      count > bestCount ||
      (count === bestCount && name.length > best.length)
    ) {
      best = name;
      bestCount = count;
    }
  }
  return best;
}

/** Aliases an identity string can be looked up by: itself plus the login of a noreply email. */
function listedAliases(identity: string): string[] {
  const lower = identity.trim().toLowerCase();
  const login = githubLoginFromEmail(lower);
  return login ? [lower, `gh:${login}`] : [lower];
}

/**
 * Layers, lowest to highest priority: observed identity < mailmap < config people.
 * Observed identities passed up front also teach the resolver which `gh:<login>`
 * belongs to which commit identity, so login-only reviewers can be attributed.
 * The link table is fixed when the resolver is built; identities first seen
 * through `personOf` are listed in `people` but never change any answer, so the
 * result does not depend on call order.
 */
export function buildResolver(
  config: ContributorsConfig,
  opts: ResolverOptions,
): Resolver {
  const { merge } = opts;
  const configPeople = merge ? config.people : [];
  const configById = new Map(configPeople.map((p) => [p.id, p]));
  const configIndex = new Map<string, string>();
  for (const person of configPeople) {
    for (const identity of person.identities) {
      for (const alias of listedAliases(identity)) {
        if (!configIndex.has(alias)) configIndex.set(alias, person.id);
      }
    }
  }

  type Group = { name: string; identities: string[]; canonical: string };
  const groups: Group[] = [];
  const mailmapIndex = new Map<string, Group>();
  const mailmapByCanonical = new Map<string, Group>();
  if (merge) {
    for (const entry of opts.mailmap ?? []) {
      const identities = Array.from(
        new Set(
          entry.identities.map((s) => s.trim().toLowerCase()).filter(Boolean),
        ),
      );
      if (identities.length === 0) continue;
      const group = {
        name: entry.name.trim(),
        identities,
        canonical: identities[0],
      };
      groups.push(group);
      mailmapByCanonical.set(group.canonical, group);
      for (const identity of identities) {
        for (const alias of listedAliases(identity)) {
          if (!mailmapIndex.has(alias)) mailmapIndex.set(alias, group);
        }
      }
    }
  }

  const observed: GitIdentity[] = [];
  const seen = new Set<string>();
  const seenKey = (id: GitIdentity) =>
    `${identityKey(id)}\u0000${id.login?.trim().toLowerCase() ?? ""}`;

  // gh:<login> -> identity key of a commit identity seen with that login
  const ghLinks = new Map<string, string>();
  const linkLogins = (id: GitIdentity) => {
    if (!merge) return;
    const key = identityKey(id);
    if (!key || key.startsWith("gh:")) return;
    const logins = new Set<string>();
    const own = id.login?.trim().toLowerCase();
    if (own) logins.add(own);
    const fromEmail = githubLoginFromEmail(id.email ?? "");
    if (fromEmail) logins.add(fromEmail);
    for (const login of logins) {
      const alias = `gh:${login}`;
      const current = ghLinks.get(alias);
      const currentIsNoreply = current
        ? !!githubLoginFromEmail(current)
        : false;
      if (!current || (currentIsNoreply && !githubLoginFromEmail(key))) {
        ghLinks.set(alias, key);
      }
    }
  };

  const register = (id: GitIdentity) => {
    observed.push(id);
    seen.add(seenKey(id));
  };
  for (const id of opts.observedIdentities ?? []) {
    register(id);
    linkLogins(id);
  }

  const resolved = new Map<string, string>();
  const resolve = (id: GitIdentity): string => {
    const cacheKey = seenKey(id);
    const hit = resolved.get(cacheKey);
    if (hit !== undefined) return hit;
    const key = identityKey(id);
    let out = key;
    if (merge) {
      const candidates = [key];
      const login = id.login?.trim().toLowerCase();
      if (login) candidates.push(`gh:${login}`);
      const fromEmail = githubLoginFromEmail(id.email ?? "");
      if (fromEmail) candidates.push(`gh:${fromEmail}`);
      const linked = key.startsWith("gh:") ? ghLinks.get(key) : undefined;
      if (linked) candidates.push(linked);
      out = lookup(Array.from(new Set(candidates))) ?? linked ?? key;
    }
    resolved.set(cacheKey, out);
    return out;
  };

  const lookup = (candidates: string[]): string | undefined => {
    for (const c of candidates) {
      const personId = configIndex.get(c);
      if (personId) return personId;
    }
    for (const c of candidates) {
      const group = mailmapIndex.get(c);
      if (!group) continue;
      for (const identity of group.identities) {
        for (const alias of listedAliases(identity)) {
          const personId = configIndex.get(alias);
          if (personId) return personId;
        }
      }
      return group.canonical;
    }
    return undefined;
  };

  let cache: ResolvedPerson[] | null = null;
  const build = (): ResolvedPerson[] => {
    const byPerson = new Map<string, GitIdentity[]>();
    for (const id of observed) {
      const personId = resolve(id);
      const list = byPerson.get(personId);
      if (list) list.push(id);
      else byPerson.set(personId, [id]);
    }
    const out: ResolvedPerson[] = [];
    for (const [personId, ids] of byPerson) {
      out.push(describe(personId, ids));
    }
    for (const person of configPeople) {
      if (!byPerson.has(person.id)) out.push(describe(person.id, []));
    }
    return out;
  };

  const describe = (personId: string, ids: GitIdentity[]): ResolvedPerson => {
    const configured = configById.get(personId);
    if (configured) {
      return {
        id: personId,
        name: configured.name || personId,
        identities: [...configured.identities],
        observed: ids,
      };
    }
    const group = mailmapByCanonical.get(personId);
    if (group) {
      return {
        id: personId,
        name: group.name || bestName(ids) || personId,
        identities: [...group.identities],
        observed: ids,
      };
    }
    const login = ids.find((i) => i.login?.trim())?.login?.trim();
    return {
      id: personId,
      name: bestName(ids) || login || personId,
      identities: [personId],
      observed: ids,
    };
  };

  const getPeople = (): ResolvedPerson[] => {
    cache ??= build();
    return cache;
  };

  return {
    personOf(id) {
      if (!seen.has(seenKey(id))) {
        register(id);
        cache = null;
      }
      return resolve(id);
    },
    nameOf(personId) {
      return getPeople().find((p) => p.id === personId)?.name ?? personId;
    },
    get people() {
      return getPeople();
    },
  };
}

export type SuggestPerson = {
  id: string;
  name: string;
  identities: string[];
  observed?: GitIdentity[];
};

export type MergeSuggestion = {
  id: string;
  personIds: string[];
  reason: "same-name" | "similar-email" | "github-noreply";
  label: string;
};

const GENERIC_LOCALS = new Set([
  "dev",
  "info",
  "admin",
  "noreply",
  "no-reply",
  "root",
  "me",
  "mail",
  "hello",
  "contact",
  "support",
  "team",
  "git",
  "github",
  "test",
  "user",
  "bot",
  "build",
  "office",
  "ci",
  "jenkins",
]);

const PLACEHOLDER_NAMES = new Set([
  "your name",
  "root",
  "unknown",
  "admin",
  "user",
  "ubuntu",
  "github",
]);

const REASON_SUFFIX = /:(?:same-name|similar-email|github-noreply)$/;

function normalizeName(name: string): string {
  return name
    .toLowerCase()
    .replace(/đ/g, "d")
    .normalize("NFD")
    .replace(/[̀-ͯ]/g, "")
    .replace(/\s+/g, " ")
    .trim();
}

/** Local part without dots, `+tag` suffix and the `12345+` GitHub id prefix. */
function normalizeLocal(raw: string): string {
  let local = raw.toLowerCase();
  const prefixed = /^\d+\+(.+)$/.exec(local);
  if (prefixed) local = prefixed[1];
  else {
    const plus = local.indexOf("+");
    if (plus > 0) local = local.slice(0, plus);
  }
  return local.replace(/\./g, "");
}

function pseudoIdentity(value: string): GitIdentity {
  if (value.includes("@")) return { name: "", email: value };
  if (value.startsWith("gh:")) {
    return { name: "", email: "", login: value.slice(3) };
  }
  return { name: value, email: "" };
}

type LocalEntry = { personId: string; domain: string; noreply: boolean };

export function suggestMerges(
  people: SuggestPerson[],
  dismissed: string[],
  config: ContributorsConfig = DEFAULT_CONFIG,
): MergeSuggestion[] {
  const names = new Map<string, { display: string; ids: Set<string> }>();
  const locals = new Map<string, LocalEntry[]>();

  for (const person of people) {
    const observed = person.observed ?? [];
    const probes: GitIdentity[] = [
      { name: person.name, email: "" },
      ...person.identities.map(pseudoIdentity),
      ...observed,
    ];
    if (probes.some((p) => isAgent(config, p) || isExcluded(config, p))) {
      continue;
    }

    for (const raw of [person.name, ...observed.map((o) => o.name)]) {
      const norm = normalizeName(raw ?? "");
      // One-word names (Minh, Sam) are too common to link people on.
      if (
        norm.length < 3 ||
        norm.includes("@") ||
        !norm.includes(" ") ||
        PLACEHOLDER_NAMES.has(norm)
      ) {
        continue;
      }
      const bucket = names.get(norm) ?? { display: raw.trim(), ids: new Set() };
      bucket.ids.add(person.id);
      names.set(norm, bucket);
    }

    const addLocal = (raw: string, domain: string, noreply: boolean) => {
      const local = normalizeLocal(raw);
      if (local.length < 3 || GENERIC_LOCALS.has(local)) return;
      const list = locals.get(local) ?? [];
      list.push({ personId: person.id, domain, noreply });
      locals.set(local, list);
    };
    const emails = [
      ...person.identities.filter((i) => i.includes("@")),
      ...observed.map((o) => o.email ?? "").filter(Boolean),
    ].map((e) => e.trim().toLowerCase());
    for (const email of new Set(emails)) {
      const login = githubLoginFromEmail(email);
      if (login) addLocal(login, "users.noreply.github.com", true);
      else {
        const at = email.lastIndexOf("@");
        if (at > 0) addLocal(email.slice(0, at), email.slice(at + 1), false);
      }
    }
    const logins = [
      ...person.identities
        .filter((i) => i.startsWith("gh:"))
        .map((i) => i.slice(3)),
      ...observed.map((o) => o.login ?? "").filter(Boolean),
    ];
    for (const login of new Set(logins)) addLocal(login, "", true);
  }

  const out: MergeSuggestion[] = [];
  const seenSets = new Set<string>();
  const push = (
    ids: Iterable<string>,
    reason: MergeSuggestion["reason"],
    label: string,
  ) => {
    const personIds = Array.from(new Set(ids)).sort();
    if (personIds.length < 2) return;
    const setKey = personIds.join("|");
    if (seenSets.has(setKey)) return;
    seenSets.add(setKey);
    // No reason in the id, so a dismissal survives the reason changing.
    out.push({ id: setKey, personIds, reason, label });
  };

  for (const { display, ids } of names.values()) {
    push(ids, "same-name", `Same name "${display}"`);
  }
  for (const [local, entries] of locals) {
    const included = entries.filter((e) =>
      entries.some((o) => o.personId !== e.personId && o.domain !== e.domain),
    );
    const noreply = included.some((e) => e.noreply);
    push(
      included.map((e) => e.personId),
      noreply ? "github-noreply" : "similar-email",
      noreply
        ? `GitHub login "${local}" matches another email`
        : `Similar email "${local}"`,
    );
  }

  const skip = new Set(dismissed.map((d) => d.replace(REASON_SUFFIX, "")));
  return out.filter((s) => !skip.has(s.id));
}

// ---- Immutable config editing helpers ----

const lowerKey = (key: string) => key.trim().toLowerCase();

function slugify(name: string): string {
  return normalizeName(name)
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "");
}

/** The id `createPerson` will give a person with this name. */
export function personIdFor(config: ContributorsConfig, name: string): string {
  const base = `person-${slugify(name) || "person"}`;
  const taken = new Set(config.people.map((p) => p.id));
  let candidate = base;
  for (let n = 2; taken.has(candidate); n++) candidate = `${base}-${n}`;
  return candidate;
}

export function createPerson(
  config: ContributorsConfig,
  name: string,
): ContributorsConfig {
  const trimmed = name.trim();
  if (!trimmed) return config;
  return {
    ...config,
    people: [
      ...config.people,
      { id: personIdFor(config, trimmed), name: trimmed, identities: [] },
    ],
  };
}

/** Renaming a person that only exists through observation creates a config person for it. */
export function renamePerson(
  config: ContributorsConfig,
  personId: string,
  name: string,
): ContributorsConfig {
  const trimmed = name.trim();
  if (!trimmed) return config;
  if (config.people.some((p) => p.id === personId)) {
    return {
      ...config,
      people: config.people.map((p) =>
        p.id === personId ? { ...p, name: trimmed } : p,
      ),
    };
  }
  return {
    ...config,
    people: [
      ...config.people,
      { id: personId, name: trimmed, identities: [personId] },
    ],
  };
}

/** Rewrites dismissed suggestion ids so they follow a person that got a new id. */
function renameInDismissed(
  dismissed: string[],
  oldId: string,
  newId: string,
): string[] {
  const out = dismissed.map((entry) => {
    const suffix = REASON_SUFFIX.exec(entry)?.[0] ?? "";
    const ids = entry.slice(0, entry.length - suffix.length).split("|");
    if (!ids.includes(oldId)) return entry;
    const renamed = ids.map((i) => (i === oldId ? newId : i));
    return `${Array.from(new Set(renamed)).sort().join("|")}${suffix}`;
  });
  return Array.from(new Set(out));
}

export function movePersonIdentity(
  config: ContributorsConfig,
  identityKey: string,
  toPersonId: string | null,
  opts: { createdName?: string } = {},
): ContributorsConfig {
  const key = lowerKey(identityKey);
  const emptied = new Set<string>();
  let people = config.people.map((p) => {
    const identities = p.identities.filter((i) => i !== key);
    if (identities.length === 0 && p.identities.length > 0) emptied.add(p.id);
    return { ...p, identities };
  });
  if (toPersonId !== null) {
    if (people.some((p) => p.id === toPersonId)) {
      people = people.map((p) =>
        p.id === toPersonId ? { ...p, identities: [...p.identities, key] } : p,
      );
    } else {
      people.push({
        id: toPersonId,
        name: opts.createdName?.trim() || key,
        identities: [key],
      });
    }
  }
  // Only people this move emptied go away; others may be empty on purpose.
  people = people.filter((p) => !emptied.has(p.id) || p.identities.length > 0);
  // A person whose id is the moved key would otherwise keep claiming that
  // identity (an unmapped identity resolves to its own key), so give it a new id.
  let dismissedMerges = config.dismissedMerges;
  if (toPersonId !== key) {
    const taken = people.filter((p) => p.id !== key);
    people = people.map((p) => {
      if (p.id !== key) return p;
      const id = personIdFor({ ...config, people: taken }, p.name);
      taken.push({ ...p, id });
      dismissedMerges = renameInDismissed(dismissedMerges, key, id);
      return { ...p, id };
    });
  }
  return { ...config, people, dismissedMerges };
}

/**
 * Merges `ids` into the first one. Ids that are config people have their
 * identities moved; any other id is an observed identity key and is added as
 * an identity. The first id becomes a config person when it is not one yet.
 */
export function mergePeople(
  config: ContributorsConfig,
  ids: string[],
  intoName?: string,
): ContributorsConfig {
  const distinct = Array.from(new Set(ids));
  if (distinct.length < 2) return config;
  const [targetId, ...rest] = distinct;
  const byId = new Map(config.people.map((p) => [p.id, p]));
  const existing = byId.get(targetId);

  const identities: string[] = existing
    ? [...existing.identities]
    : [lowerKey(targetId)];
  for (const id of rest) {
    const other = byId.get(id);
    if (other) identities.push(...other.identities);
    else identities.push(lowerKey(id));
  }
  const merged: Person = {
    id: targetId,
    name: intoName?.trim() || existing?.name || targetId,
    identities: Array.from(new Set(identities)),
  };

  const removed = new Set(rest);
  const people = config.people
    .filter((p) => !removed.has(p.id))
    .map((p) => (p.id === targetId ? merged : p));
  if (!existing) people.push(merged);
  return { ...config, people };
}

/** Adds an exact (lowercased) pattern. Wildcard characters in the key act as wildcards. */
export function excludeIdentity(
  config: ContributorsConfig,
  identityKey: string,
): ContributorsConfig {
  const key = lowerKey(identityKey);
  if (!key) return config;
  if (config.excludeIdentities.some((p) => lowerKey(p) === key)) return config;
  return { ...config, excludeIdentities: [...config.excludeIdentities, key] };
}

/** Removes an exact exclude pattern; wildcard patterns that cover the identity are not touched. */
export function includeIdentity(
  config: ContributorsConfig,
  pattern: string,
): ContributorsConfig {
  const key = lowerKey(pattern);
  const next = config.excludeIdentities.filter((p) => lowerKey(p) !== key);
  if (next.length === config.excludeIdentities.length) return config;
  return { ...config, excludeIdentities: next };
}

export function dismissMerge(
  config: ContributorsConfig,
  suggestionId: string,
): ContributorsConfig {
  if (config.dismissedMerges.includes(suggestionId)) return config;
  return {
    ...config,
    dismissedMerges: [...config.dismissedMerges, suggestionId],
  };
}
