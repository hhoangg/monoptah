import { projectName } from "../../../shared/lib/paths";
import type { HarnessId } from "../../sessions/model/session";
import type { ContributorsModel } from "./pipeline";
import { buildAgentPrompt, buildSnapshot, snapshotFileName } from "./snapshot";
import type { Snapshot, SnapshotScope } from "./snapshot";

/** Who an Ask Agent question is about: the whole team, one person, or the user. */
export type AskScope = SnapshotScope;

/**
 * What the Contributors page hands to the app when the user presses "Start
 * session". The app side creates a chat session in the project, writes
 * `snapshotJson` to a file called `fileName` (an attachment), attaches it and
 * sends `prompt` as the first message. Everything is already final: the
 * snapshot reflects the chips and checkboxes the user chose, and `prompt`
 * already contains `question` and the git/gh permission wording.
 */
export type AskAgentRequest = {
  /** The user's question, trimmed, as typed. Useful for a session title. */
  question: string;
  /** Full first message: context lines, tool permission and the question. */
  prompt: string;
  /** Pretty-printed JSON text of the snapshot, to write into the attachment. */
  snapshotJson: string;
  /** Attachment name, `contributors-<period id>.json`. */
  fileName: string;
  /**
   * Provider chosen in the dialog. It is set whenever the dialog knew any
   * provider (the default one is preselected), so its presence does not mean
   * the user changed it; absent only when no provider list was available.
   */
  harness?: HarnessId;
  /** The real scope. Inside the snapshot and prompt it is aliased when the people chip was removed. */
  scope: AskScope;
};

/** Parts of the snapshot the user can leave out with a chip. */
export type AskContextPart = "people" | "scores" | "formula" | "ai";

export type AskOptions = {
  /** Include the per-PR breakdown (`prs`). */
  perPr: boolean;
  /** Include the raw commit list (`commits`). */
  rawCommits: boolean;
  /** Word the prompt so the agent may run read-only git / gh. */
  allowTools: boolean;
  /** Chips the user removed. */
  omit: readonly AskContextPart[];
};

export const DEFAULT_ASK_OPTIONS: AskOptions = {
  perPr: true,
  rawCommits: false,
  allowTools: true,
  omit: [],
};

export type AskChipId =
  "project" | "branch" | "period" | "focus" | AskContextPart;

export type AskChip = {
  id: AskChipId;
  label: string;
  removable: boolean;
  /** Tooltip saying what removing the chip leaves out. */
  hint?: string;
};

export function personNameOf(
  model: ContributorsModel,
  scope: AskScope,
): string | undefined {
  return scope.kind === "team"
    ? undefined
    : model.resolver.nameOf(scope.personId);
}

export function askSubtitle(scope: AskScope, personName?: string): string {
  if (scope.kind === "team") return "about the whole team";
  if (scope.kind === "me") return "about your score";
  return `about ${personName ?? "this person"}`;
}

const plural = (n: number, one: string, many = `${one}s`) =>
  `${n} ${n === 1 ? one : many}`;

/** Everyone the snapshot lists: the period's ranking plus the stats rows. */
function listedPersonIds(model: ContributorsModel): string[] {
  const ids = [...model.leaderboard.rows]
    .sort((a, b) => a.rank - b.rank)
    .map((row) => row.personId);
  for (const row of model.stats.rows) {
    if (!ids.includes(row.personId)) ids.push(row.personId);
  }
  return ids;
}

export function askContextChips(
  model: ContributorsModel,
  project: string,
  scope: AskScope,
): AskChip[] {
  const name = personNameOf(model, scope);
  const ids = listedPersonIds(model);
  const identities = ids.reduce(
    (sum, id) =>
      sum +
      (model.resolver.people.find((p) => p.id === id)?.identities.length ?? 1),
    0,
  );
  const count = `${plural(ids.length, "person", "people")} · ${plural(identities, "identity", "identities")}`;
  const chips: AskChip[] = [
    { id: "project", label: projectName(project), removable: false },
    { id: "branch", label: model.raw.branch, removable: false },
    {
      id: "period",
      label: model.period.label,
      removable: false,
    },
  ];
  if (scope.kind !== "team") {
    chips.push({
      id: "focus",
      label: `${name ?? scope.personId} (${scope.kind === "me" ? "you" : "focus"})`,
      removable: false,
    });
  }
  chips.push(
    {
      id: "people",
      label: scope.kind === "team" ? count : `Whole team: ${count}`,
      removable: true,
      hint: "Remove to leave out e-mail addresses and logins: people are then sent under their display name and an alias (person-1, person-2, ...)",
    },
    {
      id: "scores",
      label: "Scores + breakdown",
      removable: true,
      hint: "Remove to send only scores and ranks, without the per-dimension breakdown",
    },
    {
      id: "formula",
      label: "Scoring formula",
      removable: true,
      hint: "Remove to leave out the explanation of how the score is calculated",
    },
    {
      id: "ai",
      label: "AI-assisted stats",
      removable: true,
      hint: "Remove to leave out the share of AI-assisted work",
    },
  );
  return chips;
}

const OMIT_TEXT: Record<AskContextPart, string> = {
  people: "identity lists (e-mails and logins)",
  scores: "score breakdowns",
  formula: "the scoring formula",
  ai: "AI-assisted stats",
};

type Json = Record<string, unknown>;

function without<T>(value: T, fields: readonly string[]): T {
  if (!value || typeof value !== "object") return value;
  const copy: Json = { ...(value as Json) };
  for (const field of fields) delete copy[field];
  return copy as T;
}

/**
 * Stable aliases (person-1, person-2, ...) in rank order, so no e-mail address
 * reaches the agent through an id, a display name or a commit author.
 */
function makeAnonymiser(model: ContributorsModel) {
  const aliases = new Map(
    listedPersonIds(model).map((id, i) => [id, `person-${i + 1}`]),
  );
  const aliasOf = (id: string) => {
    let alias = aliases.get(id);
    if (!alias) {
      alias = `person-${aliases.size + 1}`;
      aliases.set(id, alias);
    }
    return alias;
  };
  const isEmail = (text: string) => text.includes("@");
  /** The display name, or the alias when the name is an address. */
  const nameOf = (id: string, name: string) =>
    isEmail(name) ? aliasOf(id) : name;
  const idOfName = (name: string) =>
    [...aliases.keys()].find((id) => model.resolver.nameOf(id) === name);

  return {
    aliasOf,
    nameOf,
    scope(scope: AskScope): AskScope {
      return scope.kind === "team"
        ? scope
        : { kind: scope.kind, personId: aliasOf(scope.personId) };
    },
    person<T extends { personId: string; name: string }>(p: T): T {
      return {
        ...p,
        personId: aliasOf(p.personId),
        name: nameOf(p.personId, p.name),
      };
    },
    pr<T extends { personId: string; personName: string }>(pr: T): T {
      return {
        ...pr,
        personId: aliasOf(pr.personId),
        personName: nameOf(pr.personId, pr.personName),
      };
    },
    commit<T extends { author: string; person: string | null }>(c: T): T {
      const id = c.person === null ? undefined : idOfName(c.person);
      const person =
        c.person === null
          ? null
          : isEmail(c.person)
            ? id
              ? aliasOf(id)
              : "unknown"
            : c.person;
      const author = c.author.replace(/\s*<[^>]*>\s*$/, "");
      return {
        ...c,
        person,
        author:
          isEmail(author) || !author.trim() ? (person ?? "unknown") : author,
      };
    },
  };
}

type Built = { snapshot: Json; scope: AskScope; personName?: string };

function buildAskSnapshotParts(input: {
  model: ContributorsModel;
  project: string;
  scope: AskScope;
  options: AskOptions;
  now: Date;
}): Built {
  const { model, scope, options } = input;
  const snapshot: Snapshot = buildSnapshot({
    project: projectName(input.project),
    branch: model.raw.branch,
    period: model.period,
    scores: model.current,
    leaderboard: model.leaderboard,
    stats: model.stats,
    config: model.config,
    scope,
    include: { perPr: options.perPr, rawCommits: options.rawCommits },
    raw: model.raw,
    ghAvailable: model.raw.ghAvailable,
    resolver: model.resolver,
    now: input.now,
  });
  const out: Json = { ...snapshot };
  const omit = new Set(options.omit);
  const anon = omit.has("people") ? makeAnonymiser(model) : null;

  const personFields: string[] = [];
  const statsFields: string[] = [];
  const prFields: string[] = [];
  if (omit.has("scores")) personFields.push("parts", "breakdown");
  if (omit.has("ai")) {
    personFields.push("aiAssistedPct");
    statsFields.push("aiAssistedPct");
    prFields.push("aiAssisted");
  }
  if (omit.has("people")) {
    personFields.push("identities");
    statsFields.push("identities");
  }

  const person = <T extends { personId: string; name: string }>(p: T): T =>
    without(anon ? anon.person(p) : p, personFields);

  if (omit.has("formula")) delete out.formula;
  if (anon) out.scope = anon.scope(scope);
  out.people = snapshot.people.map(person);
  if (snapshot.focus) out.focus = person(snapshot.focus);
  if (snapshot.stats) {
    out.stats = {
      ...snapshot.stats,
      rows: snapshot.stats.rows.map((row) =>
        without(anon ? anon.person(row) : row, statsFields),
      ),
    };
  }
  if (snapshot.prs) {
    out.prs = snapshot.prs.map((pr) =>
      without(anon ? anon.pr(pr) : pr, prFields),
    );
  }
  if (snapshot.commits && anon) out.commits = snapshot.commits.map(anon.commit);

  const notes = [...snapshot.notes];
  if (omit.size > 0) {
    notes.push(
      `Left out at the user's request: ${[...omit].map((part) => OMIT_TEXT[part]).join(", ")}.`,
    );
  }
  if (anon) {
    notes.push(
      "People are identified by aliases (person-1, person-2, ... in rank order) in every personId field; e-mail addresses and logins were removed.",
    );
  }
  out.notes = notes;

  const name = personNameOf(model, scope);
  return {
    snapshot: out,
    scope: anon ? anon.scope(scope) : scope,
    personName:
      anon && name !== undefined && scope.kind !== "team"
        ? anon.nameOf(scope.personId, name)
        : name,
  };
}

/** The snapshot as the user configured it: the model's snapshot minus the removed chips. */
export function buildAskSnapshot(input: {
  model: ContributorsModel;
  project: string;
  scope: AskScope;
  options: AskOptions;
  now: Date;
}): Json {
  return buildAskSnapshotParts(input).snapshot;
}

/** What the attached file holds, for the prompt; mirrors the snapshot built above. */
function contentsOf(options: AskOptions): string[] {
  const omit = new Set(options.omit);
  return [
    "scores and ranks",
    ...(omit.has("scores") ? [] : ["per-dimension breakdowns"]),
    ...(omit.has("people") ? [] : ["identity mapping"]),
    ...(omit.has("formula") ? [] : ["the scoring formula"]),
    ...(options.perPr ? ["per-PR details"] : []),
    ...(options.rawCommits ? ["the raw commit list"] : []),
    ...(omit.has("ai") ? [] : ["AI-assisted shares"]),
  ];
}

export function buildAskRequest(input: {
  model: ContributorsModel;
  project: string;
  scope: AskScope;
  question: string;
  options: AskOptions;
  harness?: HarnessId;
  now: Date;
}): AskAgentRequest {
  const { model, scope } = input;
  const question = input.question.trim();
  const fileName = snapshotFileName(model.period);
  const built = buildAskSnapshotParts(input);
  return {
    question,
    prompt: buildAgentPrompt({
      question,
      scope: built.scope,
      fileName,
      allowTools: input.options.allowTools,
      projectName: projectName(input.project),
      periodLabel: model.period.label,
      personName: built.personName,
      contains: contentsOf(input.options),
    }),
    snapshotJson: JSON.stringify(built.snapshot, null, 2),
    fileName,
    ...(input.harness ? { harness: input.harness } : {}),
    scope,
  };
}
