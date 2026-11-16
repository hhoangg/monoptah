# Git conventions

Branches, commits, pull requests, issues and releases in this repository follow the rules
below. They apply to people and agents alike.

## Types

One set of types is shared by branches, commits and PR titles:

| Type | Use for |
|---|---|
| `feat` | A new user-visible feature |
| `fix` | A bug fix |
| `perf` | A performance improvement with no behavior change |
| `refactor` | A code change that neither fixes a bug nor adds a feature |
| `test` | Adding or correcting tests only |
| `docs` | Documentation only |
| `build` | Build system, packaging or dependencies |
| `ci` | CI workflows and their scripts |
| `chore` | Maintenance that fits nothing above |
| `revert` | Reverting an earlier commit |

## Branches

- Format: `<type>/<issue#>-<short-slug>`, or `<type>/<short-slug>` when there is no issue.
- Lowercase, words joined by hyphens, about 40 characters at most.
- Branches the app creates for a session worktree (`mc/...`) must be renamed before the first
  push: `git branch -m <type>/<slug>`. After the rename the app treats the branch as a user
  branch: deleting the session no longer deletes it, so clean up with the `cleanup-worktree`
  skill once the PR is merged.

```text
feat/142-terminal-split
fix/151-cloud-drawer-close
docs/git-conventions
```

## Commits

Commits follow [Conventional Commits](https://www.conventionalcommits.org/):

```text
<type>(<scope>): <summary>

<body: why and what changed, not how. Wrap at 72 columns.>

Refs: #N            (or Fixes: #N)
BREAKING CHANGE: ... (if any)
```

- Summary: imperative mood, at most 72 characters, no trailing period.
- Scope: the module touched, for example `terminals`, `workflows`, `cloud`, `accounts`,
  `mono`, `ui`, `core`, `release`, `scripts`, `ci`. Omit it when the change has no single
  module.
- Body: required for `feat` and `fix` (the cause or the reason for the change).
- Footer: `Refs: #N`, or `Fixes: #N` when the commit closes the issue; omit it when there is no
  issue.
- One complete, buildable change per commit. Do not mix a refactor and a fix.
- Commits that address review comments say so, for example
  `fix(cloud): address review on drawer close`.
- English only.
- Never commit to `main`. Never force-push a branch that has an open PR.
- No secrets, not even realistic-looking fake tokens (GitHub push protection blocks them).
- Keep the attribution trailer lines the agent harness requires.

Example:

```text
fix(cloud): close the drawer when its instance stops

The drawer kept showing a stopped instance because it only listened for
explicit close clicks. It now also closes when the instance it shows
leaves the running state.

Fixes: #151
```

## Pull requests

- Title: the commit format, for example `feat(terminals): split view for Mono terminals`.
- Body: follow [`.github/pull_request_template.md`](../.github/pull_request_template.md) and
  keep its headings: Summary, Changes, How to test, Screenshots, Risk, Checklist.
  - Summary: what and why, with `Closes #N` when there is an issue.
  - How to test: the steps, plus the commands you ran and their results.
  - Screenshots: for UI changes only.
  - Risk: data, tokens, cloud cost, migrations, platform-specific behavior; "None" if none.
- Agents that create a PR with `gh pr create --body` must read the template and fill every
  section.
- Merge with squash. The squash commit title is the PR title; GitHub appends the PR number.
- Exception: a PR that merges upstream (`hardbeat920/monocode`) into this fork is merged with a merge commit (`gh pr merge --merge`), never squashed. Squashing drops upstream history, so the next upstream merge conflicts again.

## Issues

Open issues with one of the two forms in `.github/ISSUE_TEMPLATE/`:

- **Bug** (`[Bug] ...`, label `bug`): Monoptah version, OS and version, provider CLI and
  version, steps to reproduce, expected and actual behavior, logs or screenshots.
- **Feature** (`[Feature] ...`, label `enhancement`): problem or need, proposal, alternatives
  considered, acceptance criteria.

## Releases

- Native release tags are `native-vX.Y.Z` (semver).
- Host release tags are `host-vX.Y.Z`. Bump `host/VERSION` through a PR, then tag main (see
  "Releasing the Host" in `CLAUDE.md`).
- Version bump from the merged changes: `feat` is minor, `fix` and `perf` are patch, any
  `BREAKING CHANGE` is major.
- Release notes live in `docs/native/monoptah-release-notes.md`. Its first line must stay
  exactly `# Monoptah Native X.Y.Z`: `npm run native:release:mac` and the release workflow
  refuse anything else. Group the changes by type and end each line with its PR number.
  Keep the "Downloads and updates" and "Known limits" sections that follow.
  See "Releasing the native app" in `CLAUDE.md` for the release steps.

Example:

```markdown
# Monoptah Native 0.54.0

## Changes since 0.53.3

### Features

- Mono terminals can be split side by side. (#142)

### Fixes

- The cloud drawer closes when its instance stops. (#151)

### Performance

- The project rail opens large projects faster. (#155)
```
