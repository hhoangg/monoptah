// Fork-local (Monoptah). Tests the release notes generator without git or gh.
import assert from "node:assert/strict";
import { test } from "node:test";
import {
  DOWNLOADS_SECTION,
  chooseHighlights,
  classify,
  compareVersions,
  fetchPullRequest,
  firstParentCommits,
  generateNotes,
  highlightsFrom,
  parseTitle,
  prNumberOf,
  previousTag,
  renderNotes,
  resolveSince,
} from "./native-release-notes.mjs";
import { checkNotes } from "./native-release-monoptah.mjs";

const entry = (number, title, labels = []) => ({ number, title, labels });

test("previousTag picks the highest lower plain tag that is an ancestor of HEAD", () => {
  const tags = [
    "native-v0.52.0",
    "native-v0.53.3",
    "native-v0.53.4-preview.1", // pre-release: ignored
    "native-v0.54.0", // not lower than the version: ignored
    "native-v0.53.10", // a non-ancestor (another branch): ignored
    "host-v1.2.0",
    "native-v0.9.0",
    "native-v01.0.0",
  ];
  const ancestors = new Set(["native-v0.52.0", "native-v0.53.3", "native-v0.9.0", "native-v0.54.0"]);
  const found = previousTag(tags, "0.53.4", (tag) => ancestors.has(tag));
  assert.deepEqual(found, { tag: "native-v0.53.3", version: "0.53.3" });
  // Numeric, not lexical, order.
  assert.equal(previousTag(["native-v0.9.0", "native-v0.10.0"], "0.11.0", () => true).version, "0.10.0");
  assert.ok(compareVersions("0.10.0", "0.9.0") > 0);
  // The same version is not "strictly lower".
  assert.throws(() => previousTag(["native-v0.53.4"], "0.53.4", () => true), /--since REF/);
  assert.throws(() => previousTag([], "0.53.4", () => true), /--since REF/);
  assert.throws(() => previousTag(["native-v0.53.3"], "0.53.4", () => false), /--since REF/);
});

test("the PR number comes from a squash or a merge subject", () => {
  assert.equal(prNumberOf("fix(ui): close the drawer (#151)"), 151);
  assert.equal(prNumberOf("Merge pull request #136 from hhoangg/docs/git-conventions"), 136);
  assert.equal(prNumberOf("fix: a title with (#12) inside it"), null);
  assert.equal(prNumberOf("chore: direct commit"), null);
});

test("titles parse as Conventional Commits", () => {
  assert.deepEqual(parseTitle("feat(terminals): split view"), {
    type: "feat",
    scope: "terminals",
    breaking: false,
    summary: "split view",
  });
  assert.equal(parseTitle("refactor!: drop the old API").breaking, true);
  assert.equal(parseTitle("fix: x").scope, "");
  assert.equal(parseTitle("Update stuff"), null);
  assert.equal(parseTitle("fix:nospace"), null);
});

test("classification groups, orders and capitalizes", () => {
  const groups = classify([
    entry(1, "fix(ui): drawer closes"),
    entry(2, "feat(terminals): split view"),
    entry(3, "perf(rail): open large projects faster"),
    entry(4, "revert: remove the split view"),
    entry(5, "refactor(core): simplify sessions", ["user-visible"]),
    entry(6, "feat(api)!: rename the host protocol"),
    entry(7, "improve things"),
  ]);
  assert.deepEqual(
    groups.map((g) => g.group),
    ["Breaking changes", "Features", "Fixes", "Performance", "Reverts", "Other changes"],
  );
  assert.deepEqual(groups[0].lines, ["- Rename the host protocol (#6)"]);
  assert.deepEqual(groups[1].lines, ["- Split view (#2)"]);
  // Opted-in refactors and unparsable titles share "Other changes".
  assert.deepEqual(groups[5].lines, ["- Simplify sessions (#5)", "- Improve things (#7)"]);
});

test("docs, chore, ci, test, build and refactor are skipped unless the PR is labeled user-visible", () => {
  const skipped = classify([
    entry(6, "refactor(core): tidy"),
    entry(1, "docs: git conventions"),
    entry(2, "chore(deps): bump"),
    entry(3, "ci: faster"),
    entry(4, "test: more"),
    entry(5, "build: tooling"),
  ]);
  assert.deepEqual(skipped, []);
  const labeled = classify([
    entry(1, "docs: new setup guide", ["user-visible"]),
    entry(2, "ci(release): faster releases", ["bug", "user-visible"]),
    entry(3, "chore: bump", ["bug"]),
  ]);
  assert.deepEqual(labeled, [
    { group: "Other changes", lines: ["- New setup guide (#1)", "- Faster releases (#2)"] },
  ]);
});

test("a BREAKING CHANGE footer in the commit body makes the entry a breaking change", () => {
  const body = "Some text\n\nBREAKING CHANGE: the host protocol changed";
  const groups = classify([{ number: 3, title: "feat(host): new protocol", labels: [], body }, entry(4, "fix: other")]);
  assert.deepEqual(groups.map((g) => g.group), ["Breaking changes", "Fixes"]);
  assert.deepEqual(groups[0].lines, ["- New protocol (#3)"]);
  const hyphen = classify([{ number: 5, title: "fix: x", labels: [], body: "BREAKING-CHANGE: y" }]);
  assert.equal(hyphen[0].group, "Breaking changes");
  // The words in the middle of a line are not a footer.
  assert.equal(classify([{ number: 6, title: "fix: z", labels: [], body: "this is not a BREAKING CHANGE: footer" }])[0].group, "Fixes");
  // An unparsable title with a footer is still breaking.
  assert.equal(classify([{ number: 7, title: "Rework it", labels: [], body }])[0].group, "Breaking changes");
});

test("the commit log is NUL-separated, so a body holding the old delimiters is read correctly", () => {
  const body = "line one \x1e with \x1f old delimiters\n\nBREAKING CHANGE: yes";
  const { run } = runnerFor({
    "git log --first-parent -z --format=%H%n%s%n%b v0..HEAD": {
      status: 0,
      stdout: `a1\nfix: first (#1)\n${body}\0a2\nfeat: second (#2)\n\0`,
    },
  });
  const commits = firstParentCommits(run, "v0");
  assert.equal(commits.length, 2);
  assert.deepEqual(commits[0], { sha: "a1", subject: "fix: first (#1)", body });
  assert.deepEqual(commits[1], { sha: "a2", subject: "feat: second (#2)", body: "" });
});

test("unparsable titles are kept under Other changes, not dropped", () => {
  const groups = classify([entry(9, "Merge branch 'x'"), entry(10, "wip: something", []), entry(null, "Direct commit")]);
  assert.deepEqual(groups, [
    { group: "Other changes", lines: ["- Merge branch 'x' (#9)", "- Wip: something (#10)", "- Direct commit"] },
  ]);
});

test("a PR that appears twice is listed once, commits without a number have no suffix", () => {
  const groups = classify([entry(5, "fix: a"), entry(5, "fix: a"), entry(null, "fix: b")]);
  assert.deepEqual(groups, [{ group: "Fixes", lines: ["- A (#5)", "- B"] }]);
});

test("highlights come from --notes, from the docs file of this version, and a mismatched docs file is ignored", () => {
  assert.equal(highlightsFrom("# Title\n\nHello\n"), "Hello");
  assert.equal(highlightsFrom("No heading\n"), "No heading");
  const fromNotes = chooseHighlights({
    version: "0.54.0",
    notesFileText: "# Anything\n\nBig news\n",
    docsText: "# Monoptah Native 0.54.0\n\nFrom docs\n",
  });
  assert.equal(fromNotes.highlights, "Big news");
  const fromDocs = chooseHighlights({ version: "0.54.0", docsText: "# Monoptah Native 0.54.0\n\nFrom docs\n" });
  assert.equal(fromDocs.highlights, "From docs");
  const ignored = chooseHighlights({ version: "0.54.0", docsText: "# Monoptah Native 0.53.4\n\nOld\n" });
  assert.equal(ignored.highlights, "");
  assert.match(ignored.note, /Ignoring .*0\.53\.4.*not for 0\.54\.0/);
  assert.equal(chooseHighlights({ version: "0.54.0", docsText: "" }).highlights, "");
});

test("the rendered notes keep the heading checkNotes needs and the constant sections", () => {
  const text = renderNotes({
    version: "0.54.0",
    previous: "0.53.4",
    groups: classify([entry(2, "feat: split view")]),
    highlights: "Big news",
  });
  assert.equal(text.split("\n", 1)[0], "# Monoptah Native 0.54.0");
  assert.equal(checkNotes(text, "0.54.0"), text);
  assert.match(text, /\n\nBig news\n\n## Changes since 0\.53\.4\n\n### Features\n\n- Split view \(#2\)\n\n## Downloads and updates/);
  assert.ok(text.includes(DOWNLOADS_SECTION.trimEnd()));
  assert.ok(text.endsWith("\n"));
  const empty = renderNotes({ version: "0.54.0", previous: "0.53.4", groups: [], highlights: "" });
  assert.match(empty, /## Changes since 0\.53\.4\n\nNo user-visible changes since 0\.53\.4\.\n\n## Downloads/);
  assert.ok(!empty.includes("###"));
});

// A fake runner over a table keyed by the command line.
function runnerFor(table) {
  const calls = [];
  const run = (command, args) => {
    const key = [command, ...args].join(" ");
    calls.push(key);
    for (const [prefix, answer] of Object.entries(table)) if (key === prefix) return { stdout: "", stderr: "", ...answer };
    throw new Error(`unexpected command: ${key}`);
  };
  return { run, calls };
}
const LOG = "git log --first-parent -z --format=%H%n%s%n%b native-v0.53.3..HEAD";
const record = (sha, subject, body = "") => `${sha}\n${subject}\n${body}\0`;
const pull = (n) => `gh api repos/hhoangg/monoptah/pulls/${n} --jq {title: .title, labels: [.labels[].name]}`;

test("generateNotes reads commits, squash and merge PR numbers, and PR titles and labels", () => {
  const { run, calls } = runnerFor({
    "git tag --list native-v*": { status: 0, stdout: "native-v0.53.3\nnative-v0.53.4-preview.1\nnative-v0.52.0\n" },
    "git merge-base --is-ancestor native-v0.53.3 HEAD": { status: 0 },
    [LOG]: {
      status: 0,
      stdout:
        record("a1", "fix(sessions): keep the interrupt notice (#140)", "body line") +
        record("a2", "Merge pull request #141 from hhoangg/feat/x", "feat: x") +
        record("a3", "Direct commit without a PR") +
        record("a4", "docs: guide (#142)"),
    },
    [pull(140)]: { status: 0, stdout: JSON.stringify({ title: "fix(sessions): keep the interrupt notice", labels: [] }) },
    [pull(141)]: { status: 0, stdout: JSON.stringify({ title: "feat(ui): new panel", labels: ["enhancement"] }) },
    [pull(142)]: { status: 0, stdout: JSON.stringify({ title: "docs: guide", labels: ["user-visible"] }) },
  });
  const result = generateNotes({ run, version: "0.54.0", docsText: "# Monoptah Native 0.54.0\n\nHighlight text\n" });
  assert.equal(result.since, "native-v0.53.3");
  assert.equal(result.commits, 4);
  assert.match(result.text, /^# Monoptah Native 0\.54\.0\n\nHighlight text\n\n## Changes since 0\.53\.3/);
  assert.match(result.text, /### Features\n\n- New panel \(#141\)/);
  assert.match(result.text, /### Fixes\n\n- Keep the interrupt notice \(#140\)/);
  assert.match(result.text, /### Other changes\n\n- Direct commit without a PR\n- Guide \(#142\)/);
  assert.equal(calls.filter((c) => c.startsWith("gh api")).length, 3);
});

test("generateNotes with --since uses that ref and labels it", () => {
  const { run } = runnerFor({
    "git rev-parse -q --verify native-v0.50.0^{commit}": { status: 0 },
    "git log --first-parent -z --format=%H%n%s%n%b native-v0.50.0..HEAD": { status: 0, stdout: "" },
  });
  const result = generateNotes({ run, version: "0.54.0", since: "native-v0.50.0" });
  assert.match(result.text, /## Changes since 0\.50\.0\n\nNo user-visible changes since 0\.50\.0\./);
  const sha = runnerFor({ "git rev-parse -q --verify abc1234^{commit}": { status: 0 } });
  assert.equal(resolveSince(sha.run, "0.54.0", "abc1234").label, "abc1234");
  const missing = runnerFor({ "git rev-parse -q --verify nope^{commit}": { status: 1 } });
  assert.throws(() => resolveSince(missing.run, "0.54.0", "nope"), /not a commit or tag/);
});

test("a gh or git failure is a clear error, never a silent fallback", () => {
  const gh = runnerFor({ [pull(7)]: { status: 1, stderr: "HTTP 401" } });
  assert.throws(() => fetchPullRequest(gh.run, 7), /Could not read pull request #7.*HTTP 401.*run again/s);
  const bad = runnerFor({ [pull(7)]: { status: 0, stdout: "not json" } });
  assert.throws(() => fetchPullRequest(bad.run, 7), /Unexpected answer for pull request #7/);
  const log = runnerFor({ "git log --first-parent -z --format=%H%n%s%n%b v1..HEAD": { status: 128, stderr: "bad revision" } });
  assert.throws(() => firstParentCommits(log.run, "v1"), /git log v1\.\.HEAD failed: bad revision/);
  const noTags = runnerFor({ "git tag --list native-v*": { status: 0, stdout: "" } });
  assert.throws(() => generateNotes({ run: noTags.run, version: "0.54.0" }), /--since REF/);
});
