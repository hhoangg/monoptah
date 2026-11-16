// Fork-local (Monoptah). Generates the release notes of a native release from the
// merged pull requests since the previous native-v* tag, so a release needs no
// separate notes PR. The pure functions take their side effects as arguments
// (`run(command, args)` returns { status, stdout, stderr } and never throws), so
// the tests run without git or gh.
//
// Sources and rules (see docs/git-conventions.md, "Releases"):
//   - the first-parent commits on HEAD after the previous tag; the PR number is
//     read from a squash subject "title (#N)" or a merge subject
//     "Merge pull request #N from ..."
//   - the title and labels of each PR come from the GitHub API. There is no
//     fallback to the commit subject: a squash subject can be edited at merge
//     time and a merge subject carries no title, so a silent fallback would
//     publish different words than the PR shows. A gh failure stops the release
//     before the long build instead.
//   - the Conventional Commits type of the title decides the section.
import { REPOSITORY } from "./native-release-monoptah.mjs";
import { releaseVersion } from "./package-native-monoptah.mjs";

export const GROUPS = [
  "Breaking changes",
  "Features",
  "Fixes",
  "Performance",
  "Reverts",
  "Other changes",
];
// Skipped unless the PR carries this label (then listed under "Other changes").
export const USER_VISIBLE_LABEL = "user-visible";
const TYPE_GROUP = {
  feat: "Features",
  fix: "Fixes",
  perf: "Performance",
  revert: "Reverts",
};
const SKIPPED_TYPES = new Set(["docs", "chore", "ci", "test", "build", "refactor"]);
// A footer in the commit body also marks a breaking change.
export const BREAKING_FOOTER = /^BREAKING[ -]CHANGE:/m;

const TAG_PATTERN = /^native-v(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/;

export const DOWNLOADS_SECTION = `## Downloads and updates

- macOS Apple silicon: app archive and ZIP; Homebrew cask \`hhoangg/monoptah/monoptah-native\`.
- Linux x86_64: AppImage, DEB and RPM (built on AlmaLinux 10).
- Windows x86_64: NSIS installer.
- Every updater package and the shared native update feed are signed. Native releases use their own feed and do not replace the Tauri update feed.

## Known limits

- macOS packages are ad-hoc signed and are not Apple-notarized. The Windows installer is unsigned, so SmartScreen warns on first install.
- Linux and Windows desktop interaction still need broader runtime testing. The in-app HTML viewer is macOS only; on Linux and Windows the viewer offers to open HTML with the default app.
`;

const versionParts = (version) => version.split(".").map(Number);
export function compareVersions(a, b) {
  const [x, y] = [versionParts(a), versionParts(b)];
  for (let i = 0; i < 3; i++) if (x[i] !== y[i]) return x[i] - y[i];
  return 0;
}

// The highest plain native-vA.B.C tag that is an ancestor of HEAD and strictly
// lower than `version`. `isAncestor(tag)` answers git merge-base --is-ancestor.
export function previousTag(tags, version, isAncestor) {
  releaseVersion(version);
  const candidates = tags
    .map((tag) => tag.trim())
    .filter((tag) => TAG_PATTERN.test(tag))
    .map((tag) => ({ tag, version: tag.slice("native-v".length) }))
    .filter((entry) => compareVersions(entry.version, version) < 0)
    .sort((a, b) => compareVersions(b.version, a.version));
  for (const entry of candidates) if (isAncestor(entry.tag)) return entry;
  throw new Error(
    `No earlier native-vX.Y.Z tag below ${version} is an ancestor of HEAD, so the start of the release notes is unknown. Pass --since REF (a tag or commit) to name it.`,
  );
}

// "title (#12)" (squash) or "Merge pull request #12 from owner/branch" (merge).
export function prNumberOf(subject) {
  const merge = /^Merge pull request #(\d+) from /.exec(subject);
  if (merge) return Number(merge[1]);
  const squash = /\(#(\d+)\)\s*$/.exec(subject);
  return squash ? Number(squash[1]) : null;
}

// `type(scope)!: summary`. Returns null when the title is not in that form.
export function parseTitle(title) {
  const match = /^([A-Za-z]+)(?:\(([^)]*)\))?(!)?:\s+(\S.*)$/.exec(title.trim());
  if (!match) return null;
  return {
    type: match[1].toLowerCase(),
    scope: match[2] ?? "",
    breaking: match[3] === "!",
    summary: match[4].trim(),
  };
}

const capitalize = (text) => text.charAt(0).toUpperCase() + text.slice(1);

// Entries: { number: number | null, title: string, labels: string[], body?: string }.
// Returns [{ group, lines: [string] }] in GROUPS order, without empty groups.
export function classify(entries) {
  const seen = new Set();
  const byGroup = new Map(GROUPS.map((group) => [group, []]));
  for (const entry of entries) {
    if (entry.number !== null) {
      if (seen.has(entry.number)) continue;
      seen.add(entry.number);
    }
    const parsed = parseTitle(entry.title);
    const visible = (entry.labels ?? []).includes(USER_VISIBLE_LABEL);
    const breakingFooter = BREAKING_FOOTER.test(entry.body ?? "");
    let group;
    let text;
    if (!parsed) {
      group = breakingFooter ? "Breaking changes" : "Other changes";
      text = entry.title.trim();
    } else {
      text = parsed.summary;
      if (parsed.breaking || breakingFooter) group = "Breaking changes";
      else if (TYPE_GROUP[parsed.type]) group = TYPE_GROUP[parsed.type];
      else if (SKIPPED_TYPES.has(parsed.type)) group = visible ? "Other changes" : null;
      else {
        // An unknown type keeps its whole title, so the type stays visible.
        group = "Other changes";
        text = entry.title.trim();
      }
    }
    if (group === null) continue;
    const line = `- ${capitalize(text)}${entry.number === null ? "" : ` (#${entry.number})`}`;
    byGroup.get(group).push(line);
  }
  return GROUPS.filter((group) => byGroup.get(group).length > 0).map((group) => ({
    group,
    lines: byGroup.get(group),
  }));
}

// Highlights typed by hand: the text without its leading "# ..." heading line.
export function highlightsFrom(text) {
  const lines = text.replace(/\r\n/g, "\n").split("\n");
  if (/^#\s/.test(lines[0] ?? "")) lines.shift();
  return lines.join("\n").trim();
}

// Chooses the highlights. `--notes FILE` wins; otherwise the docs file counts
// only when its heading is for this version. Returns { highlights, note }.
export function chooseHighlights({ version, notesFileText, docsText }) {
  if (notesFileText !== undefined && notesFileText !== null)
    return { highlights: highlightsFrom(notesFileText), note: "Highlights: taken from --notes." };
  if (docsText) {
    const heading = docsText.split("\n", 1)[0].trim();
    if (heading === `# Monoptah Native ${version}`)
      return { highlights: highlightsFrom(docsText), note: "Highlights: taken from docs/native/monoptah-release-notes.md." };
    return {
      highlights: "",
      note: `Ignoring docs/native/monoptah-release-notes.md: its heading is "${heading}", not for ${version}.`,
    };
  }
  return { highlights: "", note: "Highlights: none." };
}

export function renderNotes({ version, previous, groups, highlights }) {
  const parts = [`# Monoptah Native ${version}`];
  if (highlights) parts.push(highlights);
  const body = [`## Changes since ${previous}`];
  if (groups.length === 0) body.push(`No user-visible changes since ${previous}.`);
  for (const { group, lines } of groups) body.push(`### ${group}\n\n${lines.join("\n")}`);
  parts.push(body.join("\n\n"));
  parts.push(DOWNLOADS_SECTION.trimEnd());
  return `${parts.join("\n\n")}\n`;
}

const errorOf = (r) => (r.stderr || r.error?.message || "").trim();

// First-parent commits on HEAD after `since`: [{ sha, subject, body }]. The log
// is NUL-separated (-z) and each record is "sha NL subject NL body", so nothing a
// commit message contains can break the split (a body cannot hold a NUL).
export function firstParentCommits(run, since) {
  const log = run("git", ["log", "--first-parent", "-z", "--format=%H%n%s%n%b", `${since}..HEAD`]);
  if (log.status !== 0) throw new Error(`git log ${since}..HEAD failed: ${errorOf(log)}`);
  return (log.stdout ?? "")
    .split("\0")
    .filter((record) => record.trim() !== "")
    .map((record) => {
      const [sha, subject = "", ...body] = record.replace(/^\n+/, "").split("\n");
      return { sha, subject, body: body.join("\n").trim() };
    });
}

export function pullRequestPath(number) {
  return `repos/${REPOSITORY}/pulls/${number}`;
}

export function fetchPullRequest(run, number) {
  const result = run("gh", ["api", pullRequestPath(number), "--jq", "{title: .title, labels: [.labels[].name]}"]);
  if (result.status !== 0)
    throw new Error(
      `Could not read pull request #${number} from GitHub (${errorOf(result)}). The release notes are built from PR titles; fix gh access and run again.`,
    );
  try {
    const data = JSON.parse(result.stdout);
    if (typeof data.title !== "string") throw new Error("no title");
    return { title: data.title, labels: Array.isArray(data.labels) ? data.labels : [] };
  } catch (error) {
    throw new Error(`Unexpected answer for pull request #${number}: ${error.message}`);
  }
}

// Resolves where the notes start. Returns { ref, label }.
export function resolveSince(run, version, since) {
  if (since) {
    const known = run("git", ["rev-parse", "-q", "--verify", `${since}^{commit}`]);
    if (known.status !== 0) throw new Error(`--since ${since} is not a commit or tag in this repository.`);
    return { ref: since, label: TAG_PATTERN.test(since) ? since.slice("native-v".length) : since };
  }
  const list = run("git", ["tag", "--list", "native-v*"]);
  if (list.status !== 0) throw new Error(`git tag --list failed: ${errorOf(list)}`);
  const tags = (list.stdout ?? "").split("\n").filter(Boolean);
  const found = previousTag(tags, version, (tag) => run("git", ["merge-base", "--is-ancestor", tag, "HEAD"]).status === 0);
  return { ref: found.tag, label: found.version };
}

// Builds the whole notes text. `docsText` is the content of the docs notes file
// (or ""), `notesFileText` the content of --notes (or undefined).
export function generateNotes({ run, version, since, docsText = "", notesFileText }) {
  releaseVersion(version);
  const start = resolveSince(run, version, since);
  const entries = firstParentCommits(run, start.ref).map((commit) => {
    const number = prNumberOf(commit.subject);
    if (number === null) return { number: null, title: commit.subject, labels: [], body: commit.body };
    return { number, ...fetchPullRequest(run, number), body: commit.body };
  });
  const groups = classify(entries);
  const { highlights, note } = chooseHighlights({ version, notesFileText, docsText });
  return {
    text: renderNotes({ version, previous: start.label, groups, highlights }),
    note,
    since: start.ref,
    commits: entries.length,
  };
}
