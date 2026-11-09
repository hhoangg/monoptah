// Fork-local (Monoptah). Prepares a Monoptah Host release:
//   npm run host:release -- X.Y.Z
// It requires a clean tree, writes host/VERSION (X.Y.Z+monoptah), commits
// "Release Monoptah Host X.Y.Z" and creates the tag host-vX.Y.Z. It never pushes:
// it prints the push commands, and publishing stays an explicit step the user
// approves. Pushing the tag starts .github/workflows/host-release-monoptah.yml.
import { spawnSync } from "node:child_process";
import { readFile, writeFile } from "node:fs/promises";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

export const FLAVOR = "monoptah";
const VERSION_RE = /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/;

// Plain X.Y.Z only: the tag, the release name and the pinned hashes assume a final
// version, and "+" in a tag would have to be percent-encoded in download URLs.
export function validateVersion(version) {
  if (typeof version !== "string" || !VERSION_RE.test(version))
    throw new Error(
      `Invalid Host version "${version ?? ""}": use plain X.Y.Z (no pre-release, no build metadata)`,
    );
  return version;
}

export const hostVersion = (version) => `${version}+${FLAVOR}`;
export const tagFor = (version) => `host-v${validateVersion(version)}`;
export const commitMessage = (version) => `Release Monoptah Host ${version}`;

// Compares X.Y.Z numerically; build metadata is ignored.
export function compareVersions(a, b) {
  const parts = (v) => v.split("+")[0].split(".").map(Number);
  const [x, y] = [parts(a), parts(b)];
  for (let i = 0; i < 3; i++) if (x[i] !== y[i]) return x[i] < y[i] ? -1 : 1;
  return 0;
}

export function pushCommands(version, remote = "origin") {
  return [`git push ${remote} HEAD`, `git push ${remote} ${tagFor(version)}`];
}

const git = (cwd, args) => {
  const r = spawnSync("git", args, { cwd, encoding: "utf8" });
  if (r.error) throw r.error;
  return { status: r.status, stdout: r.stdout ?? "", stderr: r.stderr ?? "" };
};
const gitOk = (cwd, args) => {
  const r = git(cwd, args);
  if (r.status !== 0)
    throw new Error(`git ${args.join(" ")} failed: ${r.stderr.trim()}`);
  return r.stdout.trim();
};

// Prepares the release in `cwd` and returns { tag, version, commands }.
export async function prepareHostRelease(
  version,
  { cwd = process.cwd() } = {},
) {
  validateVersion(version);
  const tag = tagFor(version);
  if (gitOk(cwd, ["status", "--porcelain"]) !== "")
    throw new Error("The working tree is not clean; commit or stash first");
  if (
    git(cwd, ["rev-parse", "-q", "--verify", `refs/tags/${tag}`]).status === 0
  )
    throw new Error(`Tag ${tag} already exists`);
  const file = join(cwd, "host", "VERSION");
  const current = (await readFile(file, "utf8")).trim();
  if (compareVersions(version, current) <= 0)
    throw new Error(
      `Version ${version} must be newer than the current Host version ${current}`,
    );
  await writeFile(file, `${hostVersion(version)}\n`);
  gitOk(cwd, ["add", "host/VERSION"]);
  gitOk(cwd, ["commit", "-m", commitMessage(version)]);
  gitOk(cwd, ["tag", tag]);
  return {
    tag,
    version: hostVersion(version),
    commands: pushCommands(version),
  };
}

if (
  process.argv[1] &&
  resolve(process.argv[1]) === fileURLToPath(import.meta.url)
) {
  try {
    const [version] = process.argv.slice(2);
    const { tag, commands } = await prepareHostRelease(version);
    console.log(`Created commit and tag ${tag}. Nothing was pushed.`);
    console.log("To publish, review the commit, then run:");
    for (const command of commands) console.log(`  ${command}`);
  } catch (error) {
    console.error(error.message);
    process.exit(1);
  }
}
