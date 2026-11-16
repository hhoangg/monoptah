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
  WORKFLOW_FILE,
  buildCommand,
  buildEnv,
  cancelCommand,
  dispatchArgs,
  manualDispatch,
  pickRun,
  CI_POLL_MS,
  CI_RUNS_JQ,
  ciRunsPath,
  ciVerdict,
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
  waitForCi,
} from "./native-release-macos-local.mjs";
import { assetNames } from "./package-native-monoptah.mjs";

const root = fileURLToPath(new URL("../", import.meta.url));
const read = (path) => readFileSync(new URL(`../${path}`, import.meta.url), "utf8");
const V = "0.40.5";
const SHA = "a".repeat(40);

test("arguments: a plain version, optional --dry-run, nothing else", () => {
  const base = { version: V, dryRun: false, recheck: false, since: undefined, notesFile: undefined, ciTimeoutMinutes: 60 };
  assert.deepEqual(parseArgs([V]), base);
  assert.deepEqual(parseArgs([V, "--dry-run"]), { ...base, dryRun: true });
  assert.deepEqual(parseArgs(["--dry-run", V, "--recheck"]), { ...base, dryRun: true, recheck: true });
  assert.deepEqual(parseArgs([V, "--since", "native-v0.40.0", "--notes=hl.md"]), {
    ...base,
    since: "native-v0.40.0",
    notesFile: "hl.md",
  });
  assert.equal(parseArgs([V, "--ci-timeout", "90"]).ciTimeoutMinutes, 90);
  assert.equal(parseArgs([V, "--ci-timeout=0.5"]).ciTimeoutMinutes, 0.5);
  assert.throws(() => parseArgs([V, "--ci-timeout", "0"]), /number of minutes above 0/);
  assert.throws(() => parseArgs([V, "--ci-timeout", "abc"]), /number of minutes above 0/);
  assert.throws(() => parseArgs([V, "--since"]), /--since needs a value/);
  assert.throws(() => parseArgs([V, "--notes", "--dry-run"]), /--notes needs a value/);
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
  localTag: false,
  remoteTag: false,
  remoteTagCheckFailed: null,
  ghAuthed: true,
  release: "absent",
};

test("preconditions pass for a clean, current main and do not look at CI", () => {
  assert.deepEqual(evaluatePreconditions(good, V), []);
});

test("every precondition failure is reported with an actionable message", () => {
  const cases = [
    [{ fetchOk: false, fetchError: "no network" }, /git fetch origin failed: no network/],
    [{ branch: "feat/x" }, /not main.*git switch main/],
    [{ branch: "" }, /\(detached\)/],
    [{ head: "b".repeat(40) }, /not origin\/main.*git pull --ff-only/],
    [{ dirty: " M package.json" }, /tracked changes.*package\.json/s],
    [{ localTag: true }, /already exists locally.*git tag -d native-v0\.40\.5/],
    [{ remoteTag: true }, /already exists on origin/],
    [{ remoteTagCheckFailed: "denied" }, /Could not check the tag on origin: denied/],
    [{ ghAuthed: false }, /gh auth login/],
    [{ tauriCli: false }, /node_modules\/@tauri-apps\/cli is missing.*npm ci/],
    [{ release: "exists" }, /release native-v0\.40\.5 already exists/],
    [{ release: "error", releaseError: "HTTP 500" }, /only HTTP 404 means absent\): HTTP 500/],
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
    "git tag --list native-v*": { status: 0, stdout: "native-v0.40.4\nnative-v0.40.5-preview.1\n" },
    "git merge-base --is-ancestor native-v0.40.4 HEAD": { status: 0 },
    "git log --first-parent --format=%H%x1f%s%x1f%b%x1e native-v0.40.4..HEAD": {
      status: 0,
      stdout:
        "c1\x1ffeat(ui): new panel (#7)\x1f\x1e\nc2\x1fMerge pull request #8 from hhoangg/fix/y\x1ffix: y\x1f\x1e\n",
    },
    "gh api repos/hhoangg/monoptah/pulls/7 --jq {title: .title, labels: [.labels[].name]}": {
      status: 0,
      stdout: JSON.stringify({ title: "feat(ui): new panel", labels: [] }),
    },
    "gh api repos/hhoangg/monoptah/pulls/8 --jq {title: .title, labels: [.labels[].name]}": {
      status: 0,
      stdout: JSON.stringify({ title: "fix(core): y is fixed", labels: [] }),
    },
    ...overrides,
  };
  return (command, args) => {
    const key = [command, ...args].join(" ");
    if (!(key in table)) throw new Error(`unexpected command: ${key}`);
    return { stdout: "", stderr: "", ...table[key] };
  };
}
const docsNotes = `# Monoptah Native ${V}\n\nBody\n`;

test("gatherState reads the repository state through the injected runner", () => {
  const state = gatherState(fakeRun(), { version: V });
  assert.deepEqual(evaluatePreconditions(state, V), []);
  assert.equal(state.release, "absent");
});

test("gatherState: only HTTP 404 means no release, other gh errors fail", () => {
  const key = `gh api repos/hhoangg/monoptah/releases/tags/native-v${V}`;
  const exists = gatherState(fakeRun({ [key]: { status: 0, stdout: "{}" } }), { version: V });
  assert.equal(exists.release, "exists");
  const broken = gatherState(fakeRun({ [key]: { status: 1, stderr: "gh: Bad credentials (HTTP 401)" } }), { version: V });
  assert.equal(broken.release, "error");
  assert.ok(evaluatePreconditions(broken, V).some((p) => /HTTP 401/.test(p)));
});

test("gatherState: tags are detected", () => {
  const state = gatherState(
    fakeRun({
      [`git rev-parse -q --verify refs/tags/native-v${V}`]: { status: 0, stdout: SHA },
      [`git ls-remote --tags origin refs/tags/native-v${V}`]: { status: 0, stdout: `${SHA}\trefs/tags/native-v${V}\n` },
    }),
    { version: V },
  );
  assert.equal(state.localTag, true);
  assert.equal(state.remoteTag, true);
  assert.equal(evaluatePreconditions(state, V).length, 2);
});

test("the CI query lists the native.yml push runs on main for the commit, in any state", () => {
  assert.equal(
    ciRunsPath(SHA),
    `repos/hhoangg/monoptah/actions/workflows/native.yml/runs?head_sha=${SHA}&event=push&branch=main&per_page=20`,
  );
});

const run = (status, conclusion = null, id = 1) => ({ id, status, conclusion, url: `https://example.test/runs/${id}` });

test("ciVerdict: success wins, pending while unfinished or absent, failed when all finished without success", () => {
  assert.equal(ciVerdict([run("completed", "success")]), "success");
  assert.equal(ciVerdict([run("completed", "failure", 1), run("completed", "success", 2)]), "success");
  assert.equal(ciVerdict([run("in_progress"), run("completed", "failure", 2)]), "pending");
  assert.equal(ciVerdict([run("queued")]), "pending");
  assert.equal(ciVerdict([]), "pending");
  assert.equal(ciVerdict([run("completed", "failure")]), "failed");
  assert.equal(ciVerdict([run("completed", "cancelled", 1), run("completed", "skipped", 2)]), "failed");
});

const names = assetNames(V);
const params = {
  version: V,
  sha: SHA,
  dir: "/tmp/out",
  files: [names.updater, names.zip, names.sha256],
  notesFile: "/tmp/out/release-notes.md",
};

test("publish commands: tag at the checked commit, push only that tag, release with the three files", () => {
  const commands = publishCommands(params);
  assert.deepEqual(commands.map((c) => c.step), ["tag", "push-tag", "release"]);
  assert.deepEqual(commands[0].args, ["tag", "native-v0.40.5", SHA]);
  assert.deepEqual(commands[1].args, ["push", "origin", "refs/tags/native-v0.40.5"]);
  const release = commands[2].args;
  assert.deepEqual(release.slice(0, 2), ["release", "create"]);
  assert.equal(release[2], "native-v0.40.5");
  assert.deepEqual(release.slice(3, 6), params.files.map((n) => join("/tmp/out", n)));
  for (const flag of ["--verify-tag", "--latest=false"]) assert.ok(release.includes(flag));
  assert.equal(release[release.indexOf("--title") + 1], "Monoptah Native 0.40.5");
  assert.equal(release[release.indexOf("--notes-file") + 1], "/tmp/out/release-notes.md");
  assert.ok(!JSON.stringify(commands).includes("SIGNING"));
});

test("the workflow is dispatched by a file name that exists and accepts the tag, sha and request_id inputs", () => {
  const workflow = read(`.github/workflows/${WORKFLOW_FILE}`);
  assert.ok(workflow.includes("workflow_dispatch"));
  for (const input of ["tag", "sha", "request_id"]) assert.match(workflow, new RegExp(`inputs:[\\s\\S]*\\n      ${input}:`));
  // The run is found again by the request id in its name.
  assert.match(workflow, /^run-name: .*inputs\.tag.*inputs\.request_id/m);
  const args = dispatchArgs({ version: V, sha: SHA, requestId: "abc123" });
  assert.deepEqual(args.slice(0, 3), ["workflow", "run", "native-release-monoptah.yml"]);
  assert.deepEqual(args.slice(-6), ["-f", "tag=native-v0.40.5", "-f", `sha=${SHA}`, "-f", "request_id=abc123"]);
  assert.equal(manualDispatch(V), "gh workflow run native-release-monoptah.yml --repo hhoangg/monoptah --ref main -f tag=native-v0.40.5");
  assert.equal(cancelCommand(4242), "gh run cancel 4242 --repo hhoangg/monoptah");
  const runs = [
    { databaseId: 1, displayTitle: "Native release native-v0.40.5 other", url: "u1" },
    { databaseId: 2, displayTitle: "Native release native-v0.40.5 abc123", url: "u2" },
  ];
  assert.equal(pickRun(runs, "abc123").databaseId, 2);
  assert.equal(pickRun(runs, "zzz"), null);
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
  // With an early run every step names it and the exact commands.
  const withEarly = { ...params, early: { id: 4242, url: "https://example.test/runs/4242" } };
  assert.match(recoveryMessage("tag", withEarly), /gh run cancel 4242 --repo hhoangg\/monoptah/);
  const pushE = recoveryMessage("push-tag", withEarly);
  assert.match(pushE, /cancel the early run: gh run cancel 4242/);
  assert.match(pushE, /early CI run https:\/\/example\.test\/runs\/4242 is still waiting.*120 minutes.*gh workflow run native-release-monoptah\.yml --repo hhoangg\/monoptah --ref main -f tag=native-v0\.40\.5/s);
  assert.match(recoveryMessage("release", withEarly), /still waiting for the release and the macOS files/);
  assert.match(recoveryMessage("dispatch", withEarly), /early run .*4242.*gh run cancel 4242/s);
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
function fakeIo({ files = true, headAfter = SHA, dirtyAfter = "", stamp = null, failOn = null, docs = docsNotes, ciRuns = [[run("completed", "success")]], early = {} } = {}) {
  // Options of the early workflow run: dispatchFails, findable, cancelFails, statusAfterRelease.
  let { dispatchFails = 0, findable = true, cancelFails = false, statusAfterRelease = "in_progress" } = early;
  const dispatched = [];
  let ids = 0;
  const events = [];
  const written = [];
  const logs = [];
  const texts = {};
  let clock = 0;
  let ciReads = 0;
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
      if (command === "gh" && args[0] === "workflow") {
        if (dispatchFails > 0) {
          dispatchFails -= 1;
          return { status: 1, stdout: "", stderr: "HTTP 422 workflow not found" };
        }
        dispatched.push(args.find((a) => a.startsWith("request_id=")).slice("request_id=".length));
        return { status: 0, stdout: "", stderr: "" };
      }
      if (command === "gh" && args[0] === "run" && args[1] === "list") {
        const list = findable
          ? dispatched.map((id, i) => ({
              databaseId: 4242 + i,
              displayTitle: `Native release native-v${V} ${id}`,
              status: "queued",
              url: `https://example.test/runs/${4242 + i}`,
            }))
          : [];
        return { status: 0, stdout: JSON.stringify(list), stderr: "" };
      }
      if (command === "gh" && args[0] === "run" && args[1] === "view")
        return { status: 0, stdout: JSON.stringify({ status: statusAfterRelease, conclusion: statusAfterRelease === "completed" ? "failure" : null }), stderr: "" };
      if (command === "gh" && args[0] === "run" && args[1] === "cancel")
        return cancelFails ? { status: 1, stdout: "", stderr: "HTTP 403" } : { status: 0, stdout: "", stderr: "" };
      if (command === "gh" && args[0] === "api" && args[1] === ciRunsPath(SHA)) {
        const answer = ciRuns[Math.min(ciReads, ciRuns.length - 1)];
        ciReads += 1;
        return answer.error !== undefined
          ? { status: 1, stdout: "", stderr: answer.error }
          : { status: 0, stdout: JSON.stringify(answer), stderr: "" };
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
    readText: (path) => (path === NOTES_FILE ? docs : read(path)),
    exists: (path) => (path === STAMP_FILE ? stamp !== null : true),
    listDir: () => (files ? ["artifacts-macos-aarch64.json", names.updater, names.zip, names.sha256] : []),
    makeTempDir: () => "/tmp/out",
    removeDir: (dir) => events.push(`remove ${dir}`),
    writeStamp: (text) => {
      written.push(text);
      events.push("writeStamp");
    },
    writeText: (path, text) => {
      texts[path] = text;
      events.push(`writeText ${path}`);
    },
    randomId: () => `req-${++ids}`,
    now: () => clock,
    sleep: async (ms) => {
      clock += ms;
      events.push("sleep");
    },
    log: (text) => logs.push(text),
  };
  if (stamp !== null) {
    const readText = io.readText;
    io.readText = (path) => (path === STAMP_FILE ? stamp : readText(path));
  }
  return { io, events, written, logs, texts, dispatched };
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
  const publish = visible(events).slice(-3);
  assert.deepEqual(publish.map((c) => c.split(" ").slice(0, 2).join(" ")), ["git tag", "git push", "gh release"]);
  const tagAt = at("visible git tag");
  const lastHeadRead = events.map((e, i) => (e === "capture git rev-parse HEAD" ? i : -1)).filter((i) => i >= 0).pop();
  assert.ok(packaged < lastHeadRead && lastHeadRead < tagAt);
  // The tag uses the commit that was verified.
  assert.equal(publish[0], `git tag native-v${V} ${SHA}`);
  assert.equal(written.length, 1);
  assert.ok(events.includes("remove /tmp/out"));
  // Nothing before the check ran a publishing command.
  assert.ok(!visible(events).slice(0, -3).some((c) => /^(git tag|git push|gh )/.test(c)));
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

test("runRelease generates the notes, prints them, writes them to the release dir and uses that file", async () => {
  const { io, events, logs, texts } = fakeIo();
  await runRelease([V], io);
  const text = texts["/tmp/out/release-notes.md"];
  assert.ok(text.startsWith(`# Monoptah Native ${V}\n\nBody\n\n## Changes since 0.40.4`));
  assert.match(text, /### Features\n\n- New panel \(#7\)/);
  assert.match(text, /### Fixes\n\n- Y is fixed \(#8\)/);
  assert.ok(logs.join("\n").includes("- New panel (#7)"));
  const release = visible(events).find((c) => c.startsWith("gh release create"));
  assert.ok(release.endsWith("--notes-file /tmp/out/release-notes.md"));
  assert.ok(events.indexOf("writeText /tmp/out/release-notes.md") < events.findIndex((e) => e.startsWith("visible git tag")));
});

test("runRelease --dry-run prints the notes and no longer needs a notes heading for this version", async () => {
  const { io, logs, texts } = fakeIo({ docs: "# Monoptah Native 0.40.4\n\nOld notes\n" });
  await runRelease([V, "--dry-run"], io);
  const out = logs.join("\n");
  assert.ok(out.includes(`# Monoptah Native ${V}`) && out.includes("- New panel (#7)"));
  assert.match(out, /Ignoring docs\/native\/monoptah-release-notes\.md.*0\.40\.4/);
  assert.ok(!texts["/tmp/out/release-notes.md"].includes("Old notes"));
});

test("runRelease --notes adds highlights and --since overrides the start", async () => {
  const { io, texts } = fakeIo();
  const readText = io.readText;
  io.readText = (path) => (path === "hl.md" ? "# x\n\nHand written\n" : readText(path));
  const capture = io.capture;
  io.capture = (c, a) =>
    a[0] === "rev-parse" && a[3] === "native-v0.40.4^{commit}"
      ? { status: 0, stdout: "", stderr: "" }
      : capture(c, a);
  await runRelease([V, "--dry-run", "--notes", "hl.md", "--since", "native-v0.40.4"], io);
  assert.match(texts["/tmp/out/release-notes.md"], /^# Monoptah Native 0\.40\.5\n\nHand written\n\n## Changes since 0\.40\.4/);
});

test("runRelease stops before the long checks when a PR cannot be read", async () => {
  const { io, events } = fakeIo();
  const capture = io.capture;
  io.capture = (c, a) => (c === "gh" && a[1]?.includes("/pulls/7") ? { status: 1, stdout: "", stderr: "HTTP 401" } : capture(c, a));
  await assert.rejects(runRelease([V], io), /Could not read pull request #7/);
  assert.equal(visible(events).length, 0);
  assert.ok(!events.some((e) => e.startsWith("capture xcrun")));
});

test("runRelease starts native:check and the build without waiting for CI, then waits right before tagging", async () => {
  const { io, events, logs } = fakeIo({
    ciRuns: [[], [run("queued")], [run("in_progress")], [run("completed", "success")]],
  });
  await runRelease([V], io);
  const ciReads = events.map((e, i) => (e === `capture gh api ${ciRunsPath(SHA)} --jq ${CI_RUNS_JQ}` ? i : -1)).filter((i) => i >= 0);
  assert.equal(ciReads.length, 4);
  const at = (prefix) => events.findIndex((e) => e.startsWith(prefix));
  assert.ok(Math.min(...ciReads) > at("visible " + process.execPath), "CI is read only after packaging");
  assert.ok(Math.max(...ciReads) < at("visible git tag"));
  assert.equal(events.filter((e) => e === "sleep").length, 3);
  assert.ok(logs.some((l) => /CI: waiting for native\.yml, 0 min elapsed: no native\.yml run found yet/.test(l)));
  assert.ok(logs.some((l) => /CI: waiting.*run 1 \(queued\) https:\/\/example\.test\/runs\/1/.test(l)));
  assert.ok(logs.some((l) => /CI: native\.yml passed/.test(l)));
});

test("runRelease does not tag when native.yml failed and names the run", async () => {
  const { io, events } = fakeIo({ ciRuns: [[run("completed", "failure", 77)]] });
  await assert.rejects(runRelease([V], io), /native\.yml did not succeed.*run 77 \(failure\) https:\/\/example\.test\/runs\/77.*Nothing was tagged/s);
  assert.ok(!visible(events).some((c) => /^(git tag|git push|gh )/.test(c)));
});

test("runRelease does not tag on a CI timeout and says how to continue", async () => {
  const { io, events } = fakeIo({ ciRuns: [[run("in_progress")]] });
  await assert.rejects(
    runRelease([V, "--ci-timeout", "2"], io),
    /did not finish within 2 minutes.*Nothing was tagged.*Run this script again.*stamp/s,
  );
  assert.ok(!visible(events).some((c) => /^(git tag|git push|gh )/.test(c)));
  assert.equal(events.filter((e) => e === "sleep").length, 4); // 2 min at 30 s
});

test("waitForCi tolerates a few failed reads and gives up after five in a row", async () => {
  const flaky = fakeIo({ ciRuns: [{ error: "HTTP 502" }, { error: "HTTP 502" }, [run("completed", "success")]] });
  await waitForCi(flaky.io, SHA, 10);
  assert.ok(flaky.logs.some((l) => /could not read the runs \(HTTP 502\)/.test(l)));
  const broken = fakeIo({ ciRuns: [{ error: "HTTP 401" }] });
  await assert.rejects(waitForCi(broken.io, SHA, 10), /five times in a row: HTTP 401.*Nothing was tagged/s);
  assert.equal(CI_POLL_MS, 30_000);
});

test("runRelease --dry-run prints the CI state once and never waits", async () => {
  const { io, events, logs } = fakeIo({ ciRuns: [[run("in_progress")]] });
  await runRelease([V, "--dry-run"], io);
  assert.equal(events.filter((e) => e === "sleep").length, 0);
  assert.equal(events.filter((e) => e.startsWith("capture gh api") && e.includes("/workflows/native.yml/runs")).length, 1);
  assert.ok(logs.some((l) => /CI state of native\.yml for aaaaaaaaaaaa: pending, run 1 \(in_progress\)/.test(l)));
});

test("runRelease --dry-run never tags, pushes, creates a release, dispatches or cancels a run", async () => {
  const { io, events } = fakeIo();
  await runRelease([V, "--dry-run"], io);
  const all = events.filter((e) => e.startsWith("visible ") || e.startsWith("capture ")).map((e) => e.replace(/^\w+ /, ""));
  for (const forbidden of [/^git tag (?!--list)/, /^git push/, /^gh release/, /^gh workflow/, /^gh run cancel/])
    assert.ok(!all.some((c) => forbidden.test(c)), String(forbidden));
});

const indexOf = (events, prefix) => events.findIndex((e) => e.startsWith(prefix));
const cmds = (events) => events.filter((e) => e.startsWith("capture gh ") || e.startsWith("visible ")).map((e) => e.replace(/^\w+ /, ""));

test("the early run is dispatched after the toolchain checks and before native:check, with the commit and a request id", async () => {
  const { io, events, logs, dispatched } = fakeIo();
  await runRelease([V], io);
  const dispatch = indexOf(events, "capture gh workflow run");
  assert.ok(indexOf(events, "capture rustc -V") < dispatch && dispatch < indexOf(events, "visible npm run native:check"));
  assert.ok(events[dispatch].endsWith(`-f tag=native-v${V} -f sha=${SHA} -f request_id=req-1`));
  assert.deepEqual(dispatched, ["req-1"]);
  assert.ok(logs.some((l) => l.includes("Early CI run") && l.includes("https://example.test/runs/4242")));
});

test("after the release is created a running early run continues and no second run is dispatched", async () => {
  const { io, events, dispatched } = fakeIo({ early: { statusAfterRelease: "queued" } });
  await runRelease([V], io);
  assert.deepEqual(dispatched, ["req-1"]);
  assert.ok(indexOf(events, "capture gh run view 4242") > indexOf(events, "visible gh release create"));
  assert.ok(!cmds(events).some((c) => c.startsWith("gh run cancel")));
});

test("a finished early run is replaced by a fresh dispatch after the release is created", async () => {
  const { io, events, dispatched } = fakeIo({ early: { statusAfterRelease: "completed" } });
  await runRelease([V], io);
  assert.deepEqual(dispatched, ["req-1", "req-2"]);
  const fresh = events.findLastIndex((e) => e.startsWith("capture gh workflow run"));
  assert.ok(fresh > indexOf(events, "visible gh release create"));
  assert.ok(events[fresh].endsWith(`-f tag=native-v${V} -f sha=${SHA} -f request_id=req-2`));
});

test("without an early run (dispatch refused or run not found) the workflow is dispatched after the release", async () => {
  const refused = fakeIo({ early: { dispatchFails: 1 } });
  await runRelease([V], refused.io);
  assert.ok(refused.logs.some((l) => /could not start the early CI run \(HTTP 422/.test(l)));
  assert.ok(refused.events.every((e) => !e.startsWith("capture gh run view")));
  assert.deepEqual(refused.dispatched, ["req-2"]);
  // A refusal of the fresh dispatch too is an error with the manual command.
  const always = fakeIo({ early: { dispatchFails: 2 } });
  const err = await runRelease([V], always.io).catch((e) => e);
  assert.match(err.message, /Could not dispatch native-release-monoptah\.yml: HTTP 422.*safe to repeat.*gh workflow run native-release-monoptah\.yml --repo hhoangg\/monoptah --ref main -f tag=native-v0\.40\.5/s);
  assert.match(err.message, /safe to repeat/);
  const lost = fakeIo({ early: { findable: false } });
  await runRelease([V], lost.io);
  assert.ok(lost.logs.some((l) => /could not be found in the run list/.test(l)));
  assert.equal(lost.dispatched.length, 2); // early (not found) and the fresh one
});

test("a failure before the tag cancels the early run and says so", async () => {
  const failed = fakeIo({ failOn: "npm run native:check" });
  await assert.rejects(runRelease([V], failed.io), /native:check failed.*early CI run https:\/\/example\.test\/runs\/4242 was cancelled/s);
  assert.ok(cmds(failed.events).includes("gh run cancel 4242 --repo hhoangg/monoptah"));
  const build = fakeIo({ failOn: "cargo build" });
  await assert.rejects(runRelease([V], build.io), /was cancelled/);
  const moved = fakeIo({ headAfter: "d".repeat(40) });
  await assert.rejects(runRelease([V], moved.io), /HEAD moved.*was cancelled/s);
  const ci = fakeIo({ ciRuns: [[run("completed", "failure", 9)]] });
  await assert.rejects(runRelease([V], ci.io), /native\.yml did not succeed.*was cancelled/s);
  const timeout = fakeIo({ ciRuns: [[run("queued")]] });
  await assert.rejects(runRelease([V, "--ci-timeout", "1"], timeout.io), /did not finish.*was cancelled/s);
  for (const f of [build, moved, ci, timeout]) assert.ok(cmds(f.events).includes("gh run cancel 4242 --repo hhoangg/monoptah"));
  assert.ok(!cmds(failed.events).some((c) => /^(git tag|git push|gh release)/.test(c)));
});

test("when the early run cannot be cancelled the exact command is printed", async () => {
  const { io } = fakeIo({ failOn: "npm run native:check", early: { cancelFails: true } });
  await assert.rejects(runRelease([V], io), /could not be cancelled \(HTTP 403\)\. Cancel it: gh run cancel 4242 --repo hhoangg\/monoptah.*120 minute wait/s);
});

test("a failed git tag cancels the early run, a failed push keeps it and says how to continue", async () => {
  const tagFail = fakeIo({ failOn: "git tag" });
  await assert.rejects(runRelease([V], tagFail.io), /was cancelled.*Nothing was pushed/s);
  assert.ok(cmds(tagFail.events).includes("gh run cancel 4242 --repo hhoangg/monoptah"));
  const pushFail = fakeIo({ failOn: "git push" });
  await assert.rejects(
    runRelease([V], pushFail.io),
    /git tag -d native-v0\.40\.5.*cancel the early run: gh run cancel 4242.*still waiting.*gh workflow run native-release-monoptah\.yml/s,
  );
  assert.ok(!cmds(pushFail.events).some((c) => c.startsWith("gh run cancel")));
});

test("no step before the tag ever runs a publishing command or cancels in a successful run", async () => {
  const { io, events } = fakeIo({ early: { statusAfterRelease: "queued" } });
  await runRelease([V], io);
  const tagAt = indexOf(events, "visible git tag");
  assert.ok(!events.slice(0, tagAt).some((e) => /^(visible|capture) (git push|gh release|gh run cancel)/.test(e)));
});

test("the workflow builds from the planned commit and publishes only after await-release", () => {
  const workflow = read(`.github/workflows/${WORKFLOW_FILE}`);
  const job = (name) => {
    const start = workflow.indexOf(`\n  ${name}:\n`);
    assert.ok(start >= 0, name);
    const next = /\n  [a-z-]+:\n/.exec(workflow.slice(start + 1));
    return workflow.slice(start, next ? start + 1 + next.index : undefined);
  };
  for (const name of ["package", "package-rpm", "await-release"])
    assert.ok(job(name).includes("ref: ${{ needs.plan.outputs.sha }}"), name);
  const publish = job("publish");
  assert.match(publish, /needs: \[plan, package, package-rpm, await-release\]/);
  assert.match(publish, /needs\.await-release\.result == 'success'/);
  assert.ok(publish.includes("needs.await-release.outputs.reuse"));
  assert.ok(!publish.includes("needs.plan.outputs.reuse"));
  assert.match(publish, /git rev-parse HEAD[\s\S]*PLAN_SHA/);
  // Only publish sees the signing key.
  for (const name of ["plan", "package", "package-rpm", "await-release", "homebrew"]) assert.ok(!job(name).includes("TAURI_SIGNING"), name);
  assert.ok(!job("await-release").includes("contents: write"));
});
