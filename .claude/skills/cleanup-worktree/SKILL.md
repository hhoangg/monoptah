---
name: cleanup-worktree
description: Remove a merged git worktree and its leftovers (local/remote branch, subagent worktrees, stale native-dev temp logs) to free disk. Use when a PR this session created has been merged, or when asked to clean up worktrees or free disk space.
---

# Clean up worktrees

Tool: `npm run worktrees:clean -- [<worktree path|branch>...] [--merged] [--dry-run] [--yes]`
(`scripts/worktrees-clean.mjs`). `--merged` and `--prune-target` also prune stale cargo build data (below). It fetches with `--prune`, checks every worktree, and only
deletes the ones that pass all checks. Run it from the main checkout, not from inside the
worktree being removed.

## When

- A PR this session created is merged (check with `gh pr view <n> --json state`).
- The user asks to clean up worktrees or free disk space.

## Steps

1. Dry run first, for the session's own worktree (path or branch):
   `npm run worktrees:clean -- <path|branch> --dry-run`
2. If it says `DELETE`, run the same command with `--yes` (no TTY, so confirm yourself first).
3. Report what was removed and the `Freed ...` line.

For a broad sweep the user asked for: `--merged --dry-run`, show the output, then `--merged --yes`.

## Pruning cargo build data

`--merged` (unless `--no-prune-target`) and `--prune-target` (alone, removes no worktree) list
`PRUNE` lines for every worktree that stays, and delete them only after the usual confirmation
(`--dry-run` lists only). A worktree with a process inside, or when processes cannot be listed,
gets `NOPRUNE` and is left alone. Only paths inside a worktree of this repo, never symlinks.

- `crates/monocode-gpui/target/debug`: the unused `dev` profile (the native workspace uses
  `native-dev`). Not touched in the main checkout or in a worktree named `release-main`, which seed
  other worktrees' build caches. The root workspace's `target/debug` (src-tauri) is never removed.
- Incremental data under any `<target>/<profile>/incremental/` (also in `release-main` and the main
  checkout): a `<crate>-<hash>` dir with nothing written for 14 days is removed whole; in the other
  dirs every `s-*` session older than 1 day except the newest is removed with its `.lock`.
  `-working` sessions are never touched.

## Merge rules to know

- A branch with no commits of its own (still equal to an ancestor of `origin/main`) counts as
  merged when you name it explicitly. In `--merged` scans such fresh branches are kept.
- For squash merges, a merged PR whose head equals the local tip replaces the "no commits missing
  from every remote" check, since the remote branch is usually already deleted.
- Ignored files other than regenerable build dirs (`target/`, `node_modules/`, `dist/`, ...), such
  as `.env`, local DBs or notes, make the tool keep the worktree. Tell the user, do not delete them.

## Reading refusals (`KEEP` lines)

The tool refuses unless everything holds: not the main checkout, merged into `origin/main` (or a
merged PR whose head equals the local tip), clean tree, no commits missing from every remote, not
the worktree Native Dev runs from, and no process with its cwd inside.

- uncommitted/untracked: commit or discard it yourself, never delete around it.
- not merged / local commits after merge: stop, tell the user.
- process cwd inside: only your own shell or the user's terminal. Move out of it, or tell the
  user to run the command after closing it.
- `gh`/`lsof` unavailable: tool fails closed. Report it, do not work around it.
- `git branch -d` refused (squash merge): the branch is kept. Report it, never use `-D`.

## Rules

- Never force: no `git worktree remove --force`, no `-D`, no `rm -rf` on worktrees.
- Only clean worktrees this session created. Others (including `.claude/worktrees/agent-*` from
  other sessions) only when the user asks.
- Never touch `~/.cargo`, the dev DB, or app data.
