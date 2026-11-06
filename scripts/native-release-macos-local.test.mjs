// Fork-local (Monoptah). Tests the pure parts of the local macOS release script:
// nothing here runs cargo, pushes to git or calls gh.
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";
import {
  BINARY,
  FEED_URL,
  InterruptedError,
  NOTES_FILE,
  STAMP_FILE,
  buildCommand,
  buildEnv,
  ciRunsPath,
  evaluatePreconditions,
  failureOf,
  formatCommand,
  gatherState,
  interruptedMessage,
  macosReleaseFiles,
  packageCommand,
  parseArgs,
  publishCommands,
  readUpdaterPubkey,
  recoveryMessage,
  runRelease,
  sdkProblem,
  stampContent,
  stampMatches,
  tagFor,
} from "./native-release-macos-local.mjs";
import { assetNames } from "./package-native-monoptah.mjs";

const root = fileURLToPath(new URL("../", import.meta.url));
const read = (path) => readFileSync(new URL(`../${path}`, import.meta.url), "utf8");
const V = "0.40.5";
const SHA = "a".repeat(40);

test("arguments: a plain version, optional --dry-run, nothing else", () => {
  assert.deepEqual(parseArgs([V]), { version: V, dryRun: false, recheck: false });
  assert.deepEqual(parseArgs([V, "--dry-run"]), { version: V, dryRun: true, recheck: false });
  assert.deepEqual(parseArgs(["--dry-run", V, "--recheck"]), { version: V, dryRun: true, recheck: true });
  assert.throws(() => parseArgs([]), /usage/);
  assert.throws(() => parseArgs(["--dry-run"]), /usage/);
  assert.throws(() => parseArgs(["0.40.5-preview.1"]), /plain native release version/);
  assert.throws(() => parseArgs(["v0.40.5"]), /plain native release version/);
  assert.throws(() => parseArgs([V, "0.40.6"]), /extra argument/);
  assert.throws(() => parseArgs([V, "--force"]), /Unknown option/);
  assert.equal(tagFor(V), "native-v0.40.5");
});

test("the compiled-in feed URL and public key equal the release workflow's", () => {
  const workflow = read(".github/workflows/native-release-monoptah.yml");
  const value = (name) => new RegExp(`^  ${name}: (\\S+)$`, "m").exec(workflow)?.[1];
  assert.ok(value("NATIVE_FEED_URL"));
  assert.equal(FEED_URL, value("NATIVE_FEED_URL"));
  const pubkey = readUpdaterPubkey(read("src-tauri/tauri.conf.json"));
  assert.equal(pubkey, value("NATIVE_UPDATER_PUBKEY"));
  assert.throws(() => readUpdaterPubkey("{}"), /no plugins.updater.pubkey/);
});

test("the build environment carries the release settings and never the signing key", () => {
  const env = buildEnv(V, "PUBKEY", {
    PATH: "/bin",
    CARGO_TARGET_DIR: "/elsewhere",
    CARGO_BUILD_TARGET_DIR: "/elsewhere",
    TAURI_SIGNING_PRIVATE_KEY: "secret",
    TAURI_SIGNING_PRIVATE_KEY_PASSWORD: "secret",
  });
  assert.equal(env.MONOCODE_NATIVE_VERSION, V);
  assert.equal(env.MONOCODE_NATIVE_UPDATER_ENDPOINT, FEED_URL);
  assert.equal(env.MONOCODE_NATIVE_UPDATER_PUBKEY, "PUBKEY");
  assert.equal(env.PATH, "/bin");
  assert.deepEqual(Object.keys(env).filter((k) => k.startsWith("TAURI_SIGNING")), []);
  // The binary path is fixed, so cargo must not build into another directory.
  assert.equal(env.CARGO_TARGET_DIR, undefined);
  assert.equal(env.CARGO_BUILD_TARGET_DIR, undefined);
});

test("build and package commands match the workflow's macOS build", () => {
  const { command, args } = buildCommand();
  assert.equal(command, "cargo");
  assert.equal(
    args.join(" "),
    "build --locked --manifest-path crates/monocode-gpui/Cargo.toml -p monocode-gpui --target-dir crates/monocode-gpui/target --target aarch64-apple-darwin --release",
  );
  assert.deepEqual(packageCommand(V, "/tmp/out").args, ["scripts/package-native-monoptah.mjs", BINARY, "/tmp/out", V]);
  assert.equal(BINARY, "crates/monocode-gpui/target/aarch64-apple-darwin/release/monocode-gpui");
});

test("exactly the three macOS files are selected, ignoring the metadata json", () => {
  const names = assetNames(V);
  const listing = ["artifacts-macos-aarch64.json", names.sha256, names.zip, names.updater];
  assert.deepEqual(macosReleaseFiles(V, listing), [names.updater, names.zip, names.sha256]);
  assert.throws(() => macosReleaseFiles(V, listing.slice(0, 3)), /exactly one file matching/);
  assert.throws(() => macosReleaseFiles(V, []), /found 0/);
  // Files of another version do not count.
  assert.throws(() => macosReleaseFiles("0.40.6", listing), /found 0/);
});

test("the macOS SDK must be 26 or newer", () => {
  assert.equal(sdkProblem("26.2\n"), null);
  assert.equal(sdkProblem("27.0"), null);
  assert.match(sdkProblem("15.5"), /pre-macOS 26/);
  assert.match(sdkProblem(""), /unknown/);
});

const good = {
  fetchOk: true,
  branch: "main",
  head: SHA,
  originMain: SHA,
  dirty: "",
  notesFirstLine: `# Monoptah Native ${V}`,
  localTag: false,
  remoteTag: false,
  remoteTagCheckFailed: null,
  ghAuthed: true,
  release: "absent",
  ciCheckFailed: null,
  ciRuns: 1,
};

test("preconditions pass for a clean, current main with green CI", () => {
  assert.deepEqual(evaluatePreconditions(good, V), []);
});

test("every precondition failure is reported with an actionable message", () => {
  const cases = [
    [{ fetchOk: false, fetchError: "no network" }, /git fetch origin failed: no network/],
    [{ branch: "feat/x" }, /not main.*git switch main/],
    [{ branch: "" }, /\(detached\)/],
    [{ head: "b".repeat(40) }, /not origin\/main.*git pull --ff-only/],
    [{ dirty: " M package.json" }, /tracked changes.*package\.json/s],
    [{ notesFirstLine: "# Monoptah Native 0.40.4" }, /must be exactly "# Monoptah Native 0\.40\.5".*0\.40\.4/],
    [{ localTag: true }, /already exists locally.*git tag -d native-v0\.40\.5/],
    [{ remoteTag: true }, /already exists on origin/],
    [{ remoteTagCheckFailed: "denied" }, /Could not check the tag on origin: denied/],
    [{ ghAuthed: false }, /gh auth login/],
    [{ tauriCli: false }, /node_modules\/@tauri-apps\/cli is missing.*npm ci/],
    [{ release: "exists" }, /release native-v0\.40\.5 already exists/],
    [{ release: "error", releaseError: "HTTP 500" }, /only HTTP 404 means absent\): HTTP 500/],
    [{ ciRuns: 0 }, /no successful push run on main for commit aaaaaaaaaaaa/],
    [{ ciRuns: null, ciCheckFailed: "HTTP 403" }, /Could not read the native.yml runs: HTTP 403/],
  ];
  for (const [change, pattern] of cases) {
    const problems = evaluatePreconditions({ ...good, ...change }, V);
    assert.ok(problems.some((p) => pattern.test(p)), `${JSON.stringify(change)} -> ${problems.join(" | ")}`);
  }
  // Several problems are all listed.
  assert.equal(evaluatePreconditions({ ...good, branch: "x", ghAuthed: false }, V).length, 2);
});

function fakeRun(overrides = {}) {
  const table = {
    "git fetch origin": { status: 0 },
    "git branch --show-current": { status: 0, stdout: "main\n" },
    "git rev-parse HEAD": { status: 0, stdout: `${SHA}\n` },
    "git rev-parse refs/remotes/origin/main": { status: 0, stdout: `${SHA}\n` },
    "git status --porcelain --untracked-files=no": { status: 0, stdout: "" },
    [`git rev-parse -q --verify refs/tags/native-v${V}`]: { status: 1 },
    [`git ls-remote --tags origin refs/tags/native-v${V}`]: { status: 0, stdout: "" },
    "gh auth status": { status: 0 },
    [`gh api repos/hhoangg/monoptah/releases/tags/native-v${V}`]: { status: 1, stderr: "gh: Not Found (HTTP 404)" },
    [`gh api ${ciRunsPath(SHA)} --jq .total_count`]: { status: 0, stdout: "1\n" },
    ...overrides,
  };
  return (command, args) => {
    const key = [command, ...args].join(" ");
    if (!(key in table)) throw new Error(`unexpected command: ${key}`);
    return { stdout: "", stderr: "", ...table[key] };
  };
}
const notesText = `# Monoptah Native ${V}\n\nBody\n`;

test("gatherState reads the repository state through the injected runner", () => {
  const state = gatherState(fakeRun(), { version: V, notesText });
  assert.deepEqual(evaluatePreconditions(state, V), []);
  assert.equal(state.release, "absent");
  assert.equal(state.ciRuns, 1);
  assert.equal(state.notesFirstLine, `# Monoptah Native ${V}`);
});

test("gatherState: only HTTP 404 means no release, other gh errors fail", () => {
  const key = `gh api repos/hhoangg/monoptah/releases/tags/native-v${V}`;
  const exists = gatherState(fakeRun({ [key]: { status: 0, stdout: "{}" } }), { version: V, notesText });
  assert.equal(exists.release, "exists");
  const broken = gatherState(fakeRun({ [key]: { status: 1, stderr: "gh: Bad credentials (HTTP 401)" } }), { version: V, notesText });
  assert.equal(broken.release, "error");
  assert.ok(evaluatePreconditions(broken, V).some((p) => /HTTP 401/.test(p)));
});

test("gatherState: tags and CI are detected", () => {
  const state = gatherState(
    fakeRun({
      [`git rev-parse -q --verify refs/tags/native-v${V}`]: { status: 0, stdout: SHA },
      [`git ls-remote --tags origin refs/tags/native-v${V}`]: { status: 0, stdout: `${SHA}\trefs/tags/native-v${V}\n` },
      [`gh api ${ciRunsPath(SHA)} --jq .total_count`]: { status: 0, stdout: "0\n" },
    }),
    { version: V, notesText },
  );
  assert.equal(state.localTag, true);
  assert.equal(state.remoteTag, true);
  assert.equal(state.ciRuns, 0);
  assert.equal(evaluatePreconditions(state, V).length, 3);
  const failedGh = gatherState(
    fakeRun({ [`gh api ${ciRunsPath(SHA)} --jq .total_count`]: { status: 1, stderr: "boom" } }),
    { version: V, notesText },
  );
  assert.equal(failedGh.ciCheckFailed, "boom");
});

test("the CI query is the one the workflow gate uses", () => {
  const workflow = read(".github/workflows/native-release-monoptah.yml");
  assert.ok(
    workflow.includes("actions/workflows/native.yml/runs?head_sha=$SHA&event=push&branch=main&status=success&per_page=1"),
  );
  assert.equal(
    ciRunsPath(SHA),
    `repos/hhoangg/monoptah/actions/workflows/native.yml/runs?head_sha=${SHA}&event=push&branch=main&status=success&per_page=1`,
  );
});

const names = assetNames(V);
const params = { version: V, sha: SHA, dir: "/tmp/out", files: [names.updater, names.zip, names.sha256] };

test("publish commands: tag at the checked commit, push only that tag, release with the three files, dispatch", () => {
  const commands = publishCommands(params);
  assert.deepEqual(commands.map((c) => c.step), ["tag", "push-tag", "release", "dispatch"]);
  assert.deepEqual(commands[0].args, ["tag", "native-v0.40.5", SHA]);
  assert.deepEqual(commands[1].args, ["push", "origin", "refs/tags/native-v0.40.5"]);
  const release = commands[2].args;
  assert.deepEqual(release.slice(0, 2), ["release", "create"]);
  assert.equal(release[2], "native-v0.40.5");
  assert.deepEqual(release.slice(3, 6), params.files.map((n) => join("/tmp/out", n)));
  for (const flag of ["--verify-tag", "--latest=false"]) assert.ok(release.includes(flag));
  assert.equal(release[release.indexOf("--title") + 1], "Monoptah Native 0.40.5");
  assert.equal(release[release.indexOf("--notes-file") + 1], NOTES_FILE);
  assert.deepEqual(commands[3].args.slice(0, 3), ["workflow", "run", "native-release-monoptah.yml"]);
  assert.deepEqual(commands[3].args.slice(-4), ["--ref", "main", "-f", "tag=native-v0.40.5"]);
  assert.ok(!JSON.stringify(commands).includes("SIGNING"));
});

test("the workflow is dispatched by a file name that exists and accepts the tag input", () => {
  const dispatch = publishCommands(params)[3].args[2];
  const workflow = read(`.github/workflows/${dispatch}`);
  assert.ok(workflow.includes("workflow_dispatch"));
  assert.match(workflow, /inputs:\r?\n\s+tag:/);
});

test("each failed publish step prints the state left behind and a recovery", () => {
  assert.match(recoveryMessage("tag", params), /Nothing was pushed/);
  const push = recoveryMessage("push-tag", params);
  assert.match(push, /exists locally.*git tag -d native-v0\.40\.5/s);
  assert.match(push, /gh release create native-v0\.40\.5/);
  const release = recoveryMessage("release", params);
  assert.match(release, /is on origin.*gh release view native-v0\.40\.5/s);
  assert.match(release, /gh release create native-v0\.40\.5/);
  assert.match(release, /never replace an uploaded file/);
  const dispatch = recoveryMessage("dispatch", params);
  assert.match(dispatch, /gh workflow run native-release-monoptah\.yml .*tag=native-v0\.40\.5/);
  assert.match(dispatch, /safe to repeat/);
  assert.equal(formatCommand({ command: "gh", args: ["a b", "c"] }), 'gh "a b" c');
});

test("the CLI refuses bad input before touching anything", () => {
  const script = `${root}scripts/native-release-macos-local.mjs`;
  const bad = spawnSync(process.execPath, [script, "1.2"], { encoding: "utf8" });
  assert.notEqual(bad.status, 0);
  assert.match(bad.stderr, /plain native release version/);
  const none = spawnSync(process.execPath, [script], { encoding: "utf8" });
  assert.notEqual(none.status, 0);
  assert.match(none.stderr, /usage/);
});

test("the check stamp matches only the same commit and rustc", () => {
  const text = stampContent(SHA, "rustc 1.90.0 (abc 2026-01-01)\n");
  assert.ok(stampMatches(text, SHA, "rustc 1.90.0 (abc 2026-01-01)"));
  assert.ok(!stampMatches(text, "b".repeat(40), "rustc 1.90.0 (abc 2026-01-01)"));
  assert.ok(!stampMatches(text, SHA, "rustc 1.91.0 (def 2026-02-01)"));
  assert.ok(!stampMatches("not json", SHA, "x"));
  assert.ok(!stampMatches("", SHA, ""));
  assert.match(STAMP_FILE, /^crates\/monocode-gpui\/target\//);
});

test("an interrupt message names the step and how to continue", () => {
  assert.match(interruptedMessage(null, params), /Nothing was tagged/);
  const m = interruptedMessage("push-tag", params);
  assert.match(m, /Interrupted during the push-tag step/);
  assert.match(m, /git tag -d native-v0\.40\.5/);
});

// A fake io that records every command, so the tests can check the order.
function fakeIo({ files = true, headAfter = SHA, dirtyAfter = "", stamp = null, failOn = null } = {}) {
  const events = [];
  const written = [];
  let revParses = 0;
  let statuses = 0;
  const table = {
    "xcrun --sdk macosx --show-sdk-version": { status: 0, stdout: "26.2\n" },
    "xcrun metal --version": { status: 0 },
    "rustc -V": { status: 0, stdout: "rustc 1.90.0\n" },
  };
  const base = fakeRun();
  const names = assetNames(V);
  const io = {
    platform: "darwin",
    arch: "arm64",
    env: { PATH: "/bin" },
    capture(command, args) {
      const key = [command, ...args].join(" ");
      events.push(`capture ${key}`);
      if (key in table) return { stdout: "", stderr: "", ...table[key] };
      if (key === "git rev-parse HEAD") {
        revParses += 1;
        // The first read is the precondition; later ones are rechecks.
        return { status: 0, stdout: `${revParses === 1 ? SHA : headAfter}\n`, stderr: "" };
      }
      if (key === "git status --porcelain --untracked-files=no") {
        statuses += 1;
        return { status: 0, stdout: statuses === 1 ? "" : dirtyAfter, stderr: "" };
      }
      return base(command, args);
    },
    visible(command, args) {
      const key = [command, ...args].join(" ");
      events.push(`visible ${key}`);
      if (failOn && key.startsWith(failOn)) {
        if (failOn === "interrupt") throw new InterruptedError("SIGINT");
        throw new Error(`${key} failed (exit 1)`);
      }
    },
    readText: (path) => (path === NOTES_FILE ? notesText : read(path)),
    exists: (path) => (path === STAMP_FILE ? stamp !== null : true),
    listDir: () => (files ? ["artifacts-macos-aarch64.json", names.updater, names.zip, names.sha256] : []),
    makeTempDir: () => "/tmp/out",
    removeDir: (dir) => events.push(`remove ${dir}`),
    writeStamp: (text) => {
      written.push(text);
      events.push("writeStamp");
    },
    log: () => {},
  };
  if (stamp !== null) {
    const readText = io.readText;
    io.readText = (path) => (path === STAMP_FILE ? stamp : readText(path));
  }
  return { io, events, written };
}
const visible = (events) => events.filter((e) => e.startsWith("visible ")).map((e) => e.slice(8));

test("runRelease: preconditions, toolchain, check, build, package, recheck, then publish in order", async () => {
  const { io, events, written } = fakeIo();
  const result = await runRelease([V], io);
  assert.equal(result.dryRun, false);
  const at = (prefix) => events.findIndex((e) => e.startsWith(prefix));
  const order = [
    "capture git fetch origin",
    "capture xcrun --sdk",
    "capture xcrun metal",
    "capture rustc -V",
    "visible npm run native:check",
    "writeStamp",
    "visible cargo build",
    "visible " + process.execPath,
  ].map(at);
  assert.ok(order.every((i) => i >= 0), JSON.stringify(order));
  assert.deepEqual([...order].sort((a, b) => a - b), order);
  // After packaging: a fresh HEAD and tree read, then the four publish commands.
  const packaged = at("visible " + process.execPath);
  const publish = visible(events).slice(-4);
  assert.deepEqual(publish.map((c) => c.split(" ").slice(0, 2).join(" ")), ["git tag", "git push", "gh release", "gh workflow"]);
  const tagAt = at("visible git tag");
  const lastHeadRead = events.map((e, i) => (e === "capture git rev-parse HEAD" ? i : -1)).filter((i) => i >= 0).pop();
  assert.ok(packaged < lastHeadRead && lastHeadRead < tagAt);
  // The tag uses the commit that was verified.
  assert.equal(publish[0], `git tag native-v${V} ${SHA}`);
  assert.equal(written.length, 1);
  assert.ok(events.includes("remove /tmp/out"));
  // Nothing before the check ran a publishing command.
  assert.ok(!visible(events).slice(0, -4).some((c) => /^(git tag|git push|gh )/.test(c)));
});

test("runRelease --dry-run performs no publish command and leaves the files", async () => {
  const { io, events } = fakeIo();
  const result = await runRelease([V, "--dry-run"], io);
  assert.equal(result.dryRun, true);
  assert.ok(visible(events).every((c) => !/^(git tag|git push|gh )/.test(c)));
  assert.ok(!events.some((e) => e.startsWith("remove ")));
  assert.equal(visible(events).length, 3); // native:check, cargo build, package
});

test("runRelease skips native:check when the stamp matches, and --recheck forces it", async () => {
  const stamp = stampContent(SHA, "rustc 1.90.0");
  const skipped = fakeIo({ stamp });
  await runRelease([V, "--dry-run"], skipped.io);
  assert.ok(!visible(skipped.events).some((c) => c.startsWith("npm run native:check")));
  assert.equal(skipped.written.length, 0);
  const forced = fakeIo({ stamp });
  await runRelease([V, "--dry-run", "--recheck"], forced.io);
  assert.ok(visible(forced.events).some((c) => c.startsWith("npm run native:check")));
  // A stamp for another commit or rustc is ignored.
  const other = fakeIo({ stamp: stampContent("c".repeat(40), "rustc 1.90.0") });
  await runRelease([V, "--dry-run"], other.io);
  assert.ok(visible(other.events).some((c) => c.startsWith("npm run native:check")));
});

test("runRelease never records a stamp for a failed check or a dirty tree", async () => {
  const failed = fakeIo({ failOn: "npm run native:check" });
  await assert.rejects(runRelease([V], failed.io), /native:check failed/);
  assert.equal(failed.written.length, 0);
  assert.ok(!visible(failed.events).some((c) => c.startsWith("cargo")));
  const dirty = fakeIo({ dirtyAfter: " M src/x.ts" });
  await runRelease([V, "--dry-run"], dirty.io);
  assert.equal(dirty.written.length, 0);
});

test("runRelease refuses to tag when HEAD moved or the tree got dirty during the build", async () => {
  const moved = fakeIo({ headAfter: "d".repeat(40) });
  await assert.rejects(runRelease([V], moved.io), /HEAD moved from aaaaaaaaaaaa to dddddddddddd.*Nothing was tagged/s);
  assert.ok(!visible(moved.events).some((c) => /^(git tag|git push|gh )/.test(c)));
  const dirty = fakeIo({ dirtyAfter: " M src/x.ts" });
  await assert.rejects(runRelease([V], dirty.io), /tracked changes after the build.*src\/x\.ts/s);
  assert.ok(!visible(dirty.events).some((c) => /^(git tag|git push|gh )/.test(c)));
  assert.equal(dirty.written.length, 0);
});

test("runRelease: off Apple silicon, bad files and failures stop early", async () => {
  const intel = fakeIo();
  intel.io.arch = "x64";
  await assert.rejects(runRelease([V], intel.io), /Apple silicon/);
  assert.equal(intel.events.length, 0);
  await assert.rejects(runRelease([V], fakeIo({ files: false }).io), /exactly one file matching/);
  const sdk = fakeIo();
  const capture = sdk.io.capture;
  sdk.io.capture = (c, a) => (a[0] === "--sdk" ? { status: 0, stdout: "15.5\n", stderr: "" } : capture(c, a));
  await assert.rejects(runRelease([V], sdk.io), /pre-macOS 26/);
  // The toolchain checks run before native:check.
  assert.ok(!visible(sdk.events).some((c) => c.startsWith("npm")));
});

test("runRelease: a failure or Ctrl-C in the publish phase prints the recovery for that step", async () => {
  const pushFail = fakeIo({ failOn: "git push" });
  await assert.rejects(runRelease([V], pushFail.io), /git push .* failed.*exists locally.*git tag -d native-v0\.40\.5/s);
  assert.ok(!visible(pushFail.events).some((c) => c.startsWith("gh ")));
  const cancelled = fakeIo();
  const visibleFn = cancelled.io.visible;
  cancelled.io.visible = (c, a, e) => {
    if (c === "gh" && a[0] === "release") throw new InterruptedError("SIGINT");
    return visibleFn(c, a, e);
  };
  await assert.rejects(
    runRelease([V], cancelled.io),
    (error) => error.interrupted === true && /Interrupted \(SIGINT\).*Interrupted during the release step.*gh release view native-v0\.40\.5/s.test(error.message),
  );
  // Ctrl-C before publishing says nothing was changed (the error is not wrapped).
  const early = fakeIo({ failOn: "interrupt" });
  early.io.visible = () => {
    throw new InterruptedError("SIGINT");
  };
  await assert.rejects(runRelease([V], early.io), (error) => error.interrupted === true && !/recover/i.test(error.message));
});

test("a child killed by a signal is an interrupt, a plain failure is not", () => {
  const cmd = { command: "git", args: ["push"] };
  assert.equal(failureOf({ status: 0 }, cmd), null);
  const killed = failureOf({ status: null, signal: "SIGINT" }, cmd);
  assert.equal(killed.interrupted, true);
  assert.match(killed.message, /Interrupted \(SIGINT\)/);
  const failed = failureOf({ status: 2 }, cmd);
  assert.equal(failed.interrupted, undefined);
  assert.match(failed.message, /git push failed \(exit 2\)/);
  assert.match(failureOf({ error: new Error("ENOENT") }, cmd).message, /Could not run git: ENOENT/);
});
