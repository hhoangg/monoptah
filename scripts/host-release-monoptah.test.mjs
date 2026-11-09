// Fork-local (Monoptah). Tests the Host release script: version validation,
// dirty-tree refusal and that nothing is ever pushed.
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";
import {
  commitMessage,
  compareVersions,
  hostVersion,
  prepareHostRelease,
  pushCommands,
  tagFor,
  validateVersion,
} from "./host-release-monoptah.mjs";

const script = fileURLToPath(
  new URL("./host-release-monoptah.mjs", import.meta.url),
);
const git = (cwd, ...args) => {
  const r = spawnSync("git", args, { cwd, encoding: "utf8" });
  assert.equal(r.status, 0, r.stderr);
  return r.stdout.trim();
};

async function repo(initial = "1.0.0+monoptah") {
  const dir = await mkdtemp(join(tmpdir(), "host-release-test-"));
  git(dir, "init", "-q");
  git(dir, "config", "user.email", "t@example.com");
  git(dir, "config", "user.name", "t");
  git(dir, "config", "commit.gpgsign", "false");
  await mkdir(join(dir, "host"));
  await writeFile(join(dir, "host", "VERSION"), `${initial}\n`);
  git(dir, "add", ".");
  git(dir, "commit", "-q", "-m", "init");
  return dir;
}

test("validates plain X.Y.Z only", () => {
  assert.equal(validateVersion("1.2.3"), "1.2.3");
  for (const bad of [
    "",
    "1.2",
    "v1.2.3",
    "1.2.3+monoptah",
    "1.2.3-rc.1",
    "01.2.3",
    undefined,
  ])
    assert.throws(() => validateVersion(bad), /Invalid Host version/);
  assert.equal(tagFor("1.2.3"), "host-v1.2.3");
  assert.equal(hostVersion("1.2.3"), "1.2.3+monoptah");
  assert.equal(commitMessage("1.2.3"), "Release Monoptah Host 1.2.3");
  assert.deepEqual(pushCommands("1.2.3"), [
    "git push origin HEAD",
    "git push origin host-v1.2.3",
  ]);
});

test("compares versions numerically and ignores build metadata", () => {
  assert.equal(compareVersions("1.0.10", "1.0.9"), 1);
  assert.equal(compareVersions("1.0.0", "1.0.0+monoptah"), 0);
  assert.equal(compareVersions("0.9.9", "1.0.0+monoptah"), -1);
});

test("writes host/VERSION, commits and tags", async () => {
  const dir = await repo();
  try {
    const result = await prepareHostRelease("1.0.1", { cwd: dir });
    assert.equal(result.tag, "host-v1.0.1");
    assert.equal(
      await readFile(join(dir, "host", "VERSION"), "utf8"),
      "1.0.1+monoptah\n",
    );
    assert.equal(
      git(dir, "log", "-1", "--format=%s"),
      "Release Monoptah Host 1.0.1",
    );
    assert.equal(git(dir, "tag", "--list"), "host-v1.0.1");
    assert.equal(git(dir, "status", "--porcelain"), "");
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});

test("refuses a dirty tree and leaves it untouched", async () => {
  const dir = await repo();
  try {
    await writeFile(join(dir, "dirty.txt"), "x");
    await assert.rejects(
      prepareHostRelease("1.0.1", { cwd: dir }),
      /not clean/,
    );
    assert.equal(
      await readFile(join(dir, "host", "VERSION"), "utf8"),
      "1.0.0+monoptah\n",
    );
    assert.equal(git(dir, "tag", "--list"), "");
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});

test("refuses an invalid, existing or non-newer version", async () => {
  const dir = await repo();
  try {
    await assert.rejects(
      prepareHostRelease("1.0", { cwd: dir }),
      /Invalid Host version/,
    );
    await assert.rejects(
      prepareHostRelease("1.0.0", { cwd: dir }),
      /must be newer/,
    );
    git(dir, "tag", "host-v2.0.0");
    await assert.rejects(
      prepareHostRelease("2.0.0", { cwd: dir }),
      /already exists/,
    );
    assert.equal(git(dir, "status", "--porcelain"), "");
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});

test("never pushes: works with no remote and only prints the commands", async () => {
  const dir = await repo();
  try {
    assert.equal(git(dir, "remote"), "");
    const r = spawnSync(process.execPath, [script, "1.0.1"], {
      cwd: dir,
      encoding: "utf8",
    });
    assert.equal(r.status, 0, r.stderr);
    assert.match(r.stdout, /Nothing was pushed/);
    assert.match(r.stdout, /git push origin host-v1\.0\.1/);
    assert.equal(git(dir, "remote"), "");
    const bad = spawnSync(process.execPath, [script, "nope"], {
      cwd: dir,
      encoding: "utf8",
    });
    assert.equal(bad.status, 1);
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});
