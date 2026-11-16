// Fork-local (Monoptah). Builds and packages the macOS arm64 app of a native
// release on the maintainer's Mac, then hands over to CI. GitHub-hosted macOS
// runners are not used: this script creates the tag and the release with exactly
// the three unsigned macOS files. The workflow native-release-monoptah.yml builds
// Linux, Windows and rpm, signs everything (the updater signing key exists on CI
// only; this script never reads it), publishes the feed and bumps the Homebrew
// cask. To save time the script dispatches that workflow early, before
// native:check, with the commit (-f sha=...): CI builds while this Mac checks and
// builds, and the workflow's await-release job waits for the tag and the macOS
// files. If the script fails or stops before it pushes the tag it cancels that
// early run; after the release is created the early run just continues (or a
// fresh run is dispatched when there was none or it has finished).
//
// usage: npm run native:release:mac -- X.Y.Z [--dry-run] [--recheck]
//          [--since REF] [--notes FILE] [--ci-timeout MINUTES]
//   --dry-run is optional. A real run is safe on its own: every check, the
//   native:check, the build, the package step and the wait for native.yml all
//   happen before anything is tagged, so a failure leaves nothing behind. The
//   dry run stays for testing changes to this script: it runs the checks, builds
//   and packages, then prints the files, the generated release notes, the CI
//   state and the commands it would run. It never dispatches an early CI run,
//   waits for CI, tags, pushes or publishes.
//   --recheck runs npm run native:check even when a stamp says it already passed
//   for this commit and this rustc (see checkStamp).
//   --since REF starts the generated release notes at REF (a tag or commit)
//   instead of the previous native-vX.Y.Z tag.
//   --notes FILE adds the text of FILE (without a leading "# ..." line) as hand
//   written highlights on top of the generated release notes.
//   --ci-timeout MINUTES bounds the wait for native.yml (default 60).
//
// native:check and the build do not wait for native.yml. The script waits for a
// successful native.yml push run on main for HEAD only after building and
// packaging, right before it tags, so CI and the local work run in parallel. If
// CI fails or times out, nothing was tagged and the script stops; run it again
// (the native:check stamp makes that fast).
//
// The release notes are generated from the titles of the pull requests merged
// since the previous native tag (scripts/native-release-notes.mjs), so a
// release needs no separate notes PR. If docs/native/monoptah-release-notes.md
// starts with "# Monoptah Native X.Y.Z" for this version, its text is used as
// highlights; a file for another version is ignored.
//
// The logic lives in exported pure functions and in runRelease, which takes its
// side effects as an injected `io`, so the tests can run it without cargo, git
// pushes or gh.
import { spawnSync } from "node:child_process";
import { randomBytes } from "node:crypto";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { REPOSITORY } from "./native-release-monoptah.mjs";
import { generateNotes } from "./native-release-notes.mjs";
import { PLATFORMS, releaseVersion } from "./package-native-monoptah.mjs";

const root = fileURLToPath(new URL("../", import.meta.url));

export const WORKFLOW_FILE = "native-release-monoptah.yml";
export const NOTES_FILE = "docs/native/monoptah-release-notes.md";
export const TARGET = "aarch64-apple-darwin";
export const BINARY = `crates/monocode-gpui/target/${TARGET}/release/monocode-gpui`;
// Must equal NATIVE_FEED_URL in the release workflow (a test asserts it).
export const FEED_URL = `https://github.com/${REPOSITORY}/releases/download/native-latest/latest.json`;

export function tagFor(version) {
  return `native-v${releaseVersion(version)}`;
}

export const USAGE =
  "usage: npm run native:release:mac -- X.Y.Z [--dry-run] [--recheck] [--since REF] [--notes FILE] [--ci-timeout MINUTES]";
export const DEFAULT_CI_TIMEOUT_MINUTES = 60;
export const CI_POLL_MS = 30_000;

export function parseArgs(argv) {
  let version;
  let dryRun = false;
  let recheck = false;
  let since;
  let notesFile;
  let ciTimeoutMinutes = DEFAULT_CI_TIMEOUT_MINUTES;
  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i];
    // Options with a value: "--name VALUE" or "--name=VALUE".
    const valued = /^(--since|--notes|--ci-timeout)(?:=(.*))?$/.exec(arg);
    if (valued) {
      let value = valued[2];
      if (value === undefined) {
        value = argv[i + 1];
        i += 1;
      }
      if (value === undefined || value === "" || value.startsWith("--"))
        throw new Error(`${valued[1]} needs a value. ${USAGE}`);
      if (valued[1] === "--since") since = value;
      else if (valued[1] === "--notes") notesFile = value;
      else {
        ciTimeoutMinutes = Number(value);
        if (!(ciTimeoutMinutes > 0) || !Number.isFinite(ciTimeoutMinutes))
          throw new Error(`--ci-timeout needs a number of minutes above 0, got: ${value}`);
      }
    } else if (arg === "--dry-run") dryRun = true;
    else if (arg === "--recheck") recheck = true;
    else if (arg.startsWith("-")) throw new Error(`Unknown option: ${arg}`);
    else if (version === undefined) version = arg;
    else throw new Error(`Unexpected extra argument: ${arg}`);
  }
  if (version === undefined) throw new Error(USAGE);
  return { version: releaseVersion(version), dryRun, recheck, since, notesFile, ciTimeoutMinutes };
}

// The updater public key is the one the Tauri app trusts, from tauri.conf.json.
export function readUpdaterPubkey(confText) {
  const key = JSON.parse(confText)?.plugins?.updater?.pubkey;
  if (typeof key !== "string" || !key)
    throw new Error("src-tauri/tauri.conf.json has no plugins.updater.pubkey");
  return key;
}

export function buildEnv(version, pubkey, baseEnv = process.env) {
  const env = { ...baseEnv };
  // The updater signing key stays on CI. Never hand it to a child process.
  for (const name of Object.keys(env))
    if (name.startsWith("TAURI_SIGNING_PRIVATE_KEY")) delete env[name];
  // The binary is read from crates/monocode-gpui/target/..., so cargo must not
  // build elsewhere.
  delete env.CARGO_TARGET_DIR;
  delete env.CARGO_BUILD_TARGET_DIR;
  env.MONOCODE_NATIVE_VERSION = version;
  env.MONOCODE_NATIVE_UPDATER_ENDPOINT = FEED_URL;
  env.MONOCODE_NATIVE_UPDATER_PUBKEY = pubkey;
  return env;
}

export const buildCommand = () => ({
  command: "cargo",
  args: [
    "build",
    "--locked",
    "--manifest-path",
    "crates/monocode-gpui/Cargo.toml",
    "-p",
    "monocode-gpui",
    "--target-dir",
    "crates/monocode-gpui/target",
    "--target",
    TARGET,
    "--release",
  ],
});

export const packageCommand = (version, outDir) => ({
  command: process.execPath,
  args: ["scripts/package-native-monoptah.mjs", BINARY, outDir, version],
});

// The three macOS release files, found by the patterns of the platform table.
// Exactly one match per pattern; other files in the directory (the metadata
// json) are not release files.
export function macosReleaseFiles(version, names) {
  const platform = PLATFORMS.find((p) => p.kind === "macos");
  const files = [];
  for (const { pattern } of platform.assets(version)) {
    const found = names.filter((name) => pattern.test(name));
    if (found.length !== 1)
      throw new Error(
        `Expected exactly one file matching ${pattern} after packaging, found ${found.length}`,
      );
    files.push(found[0]);
  }
  return files;
}

export function sdkProblem(sdkVersion) {
  const major = Number.parseInt(String(sdkVersion).trim(), 10);
  if (!Number.isInteger(major) || major < 26)
    return `macOS SDK ${String(sdkVersion).trim() || "unknown"} would draw pre-macOS 26 window controls. Install Xcode 26 or newer and select it with xcode-select.`;
  return null;
}

// The native.yml push runs on main for a commit, in any state.
export function ciRunsPath(sha) {
  return `repos/${REPOSITORY}/actions/workflows/native.yml/runs?head_sha=${sha}&event=push&branch=main&per_page=20`;
}
export const CI_RUNS_JQ = "[.workflow_runs[] | {id: .id, status: .status, conclusion: .conclusion, url: .html_url}]";

// "success" when any run succeeded, "pending" while a run is not completed or
// none exists yet (it may not have been created), "failed" when every run
// completed and none succeeded.
export function ciVerdict(runs) {
  if (runs.some((run) => run.conclusion === "success")) return "success";
  if (runs.length === 0 || runs.some((run) => run.status !== "completed")) return "pending";
  return "failed";
}

const describeRun = (run) => `run ${run.id} (${run.status === "completed" ? run.conclusion : run.status}) ${run.url}`;
export const describeCi = (runs) =>
  runs.length === 0 ? "no native.yml run found yet" : runs.map(describeRun).join("; ");

// Reads the runs through `capture`; { runs } or { error }.
export function readCiRuns(capture, sha) {
  const result = capture("gh", ["api", ciRunsPath(sha), "--jq", CI_RUNS_JQ]);
  if (result.status !== 0) return { error: (result.stderr || result.error?.message || "gh failed").trim() };
  try {
    const runs = JSON.parse(result.stdout);
    if (!Array.isArray(runs)) throw new Error("not a list");
    return { runs };
  } catch (error) {
    return { error: `unreadable answer (${error.message})` };
  }
}

// Waits until native.yml succeeded for `sha`. Throws, so that nothing is tagged,
// when a run failed or the time ran out. A few failed reads in a row are
// tolerated (a flaky network must not waste a finished build).
export async function waitForCi(io, sha, timeoutMinutes) {
  const started = io.now();
  const limit = timeoutMinutes * 60_000;
  let readErrors = 0;
  const short = sha.slice(0, 12);
  for (;;) {
    const read = readCiRuns(io.capture, sha);
    const elapsed = Math.round((io.now() - started) / 60_000);
    if (read.error !== undefined) {
      readErrors += 1;
      if (readErrors >= 5) throw new Error(`Could not read the native.yml runs five times in a row: ${read.error}. Nothing was tagged. Run this script again.`);
      io.log(`CI: could not read the runs (${read.error}), retrying.`);
    } else {
      readErrors = 0;
      const verdict = ciVerdict(read.runs);
      if (verdict === "success") {
        io.log(`CI: native.yml passed for ${short} (${describeCi(read.runs.filter((r) => r.conclusion === "success"))}).`);
        return;
      }
      if (verdict === "failed")
        throw new Error(
          `native.yml did not succeed for commit ${short}: ${describeCi(read.runs)}. Nothing was tagged. Fix CI (re-run the failed jobs, or merge a fix and start the release from the new main), then run this script again; the native:check stamp makes the rerun fast.`,
        );
      io.log(`CI: waiting for native.yml, ${elapsed} min elapsed: ${describeCi(read.runs)}`);
    }
    if (io.now() - started >= limit)
      throw new Error(
        `native.yml for commit ${short} did not finish within ${timeoutMinutes} minutes. Nothing was tagged. Run this script again when CI is done (the native:check stamp makes the rerun fast), or use --ci-timeout MINUTES to wait longer.`,
      );
    await io.sleep(CI_POLL_MS);
  }
}

// Reads the state the preconditions look at. `run(command, args)` returns
// { status, stdout, stderr } and never throws.
export function gatherState(run, { version, tauriCli = true }) {
  const tag = tagFor(version);
  const out = (r) => (r.stdout ?? "").trim();
  const errorOf = (r) => (r.stderr || r.error?.message || "").trim();
  const state = { errors: [] };
  const fetch = run("git", ["fetch", "origin"]);
  state.fetchOk = fetch.status === 0;
  if (!state.fetchOk) state.fetchError = errorOf(fetch);
  state.branch = out(run("git", ["branch", "--show-current"]));
  state.head = out(run("git", ["rev-parse", "HEAD"]));
  state.originMain = out(run("git", ["rev-parse", "refs/remotes/origin/main"]));
  state.dirty = (run("git", ["status", "--porcelain", "--untracked-files=no"]).stdout ?? "").trimEnd();
  state.localTag = run("git", ["rev-parse", "-q", "--verify", `refs/tags/${tag}`]).status === 0;
  const remote = run("git", ["ls-remote", "--tags", "origin", `refs/tags/${tag}`]);
  state.remoteTagCheckFailed = remote.status !== 0 ? errorOf(remote) : null;
  state.remoteTag = remote.status === 0 && out(remote) !== "";
  const auth = run("gh", ["auth", "status"]);
  state.ghAuthed = auth.status === 0;
  state.tauriCli = tauriCli;
  const rel = run("gh", ["api", `repos/${REPOSITORY}/releases/tags/${tag}`]);
  if (rel.status === 0) state.release = "exists";
  else if (/HTTP 404/.test(`${rel.stderr ?? ""}${rel.stdout ?? ""}`)) state.release = "absent";
  else {
    state.release = "error";
    state.releaseError = errorOf(rel);
  }
  return state;
}

// Pure: turns the state into a list of actionable problems (empty means go).
export function evaluatePreconditions(state, version) {
  const tag = tagFor(version);
  const problems = [];
  if (!state.fetchOk)
    problems.push(`git fetch origin failed: ${state.fetchError ?? ""}. Check the network and the remote.`);
  if (state.branch !== "main")
    problems.push(`The current branch is "${state.branch || "(detached)"}", not main. Run: git switch main`);
  if (!state.head || !state.originMain || state.head !== state.originMain)
    problems.push(
      `HEAD (${(state.head ?? "").slice(0, 8)}) is not origin/main (${(state.originMain ?? "").slice(0, 8)}). Merge the last PR, then run: git pull --ff-only origin main (and push or drop any local commits).`,
    );
  if (state.dirty)
    problems.push(`There are tracked changes. Commit, stash or discard them:\n${state.dirty}`);
  if (state.localTag)
    problems.push(`The tag ${tag} already exists locally. If it is a leftover, delete it: git tag -d ${tag}`);
  if (state.remoteTagCheckFailed !== null && state.remoteTagCheckFailed !== undefined)
    problems.push(`Could not check the tag on origin: ${state.remoteTagCheckFailed}`);
  else if (state.remoteTag)
    problems.push(`The tag ${tag} already exists on origin. This script only starts new releases; see CLAUDE.md for re-running a partial one.`);
  if (!state.ghAuthed) problems.push("gh is not authenticated. Run: gh auth login");
  if (state.tauriCli === false)
    problems.push(
      "node_modules/@tauri-apps/cli is missing, so the signing tests in native:check would be skipped silently. Run: npm ci",
    );
  if (state.release === "exists")
    problems.push(`A GitHub release ${tag} already exists. This script only starts new releases.`);
  else if (state.release === "error")
    problems.push(`Could not check for release ${tag} (only HTTP 404 means absent): ${state.releaseError ?? ""}`);
  return problems;
}

// The commands that publish, in order. `sha` pins the tag to the commit that
// was checked.
export function publishCommands({ version, sha, dir, files, notesFile }) {
  const tag = tagFor(version);
  return [
    { step: "tag", command: "git", args: ["tag", tag, sha] },
    { step: "push-tag", command: "git", args: ["push", "origin", `refs/tags/${tag}`] },
    {
      step: "release",
      command: "gh",
      args: [
        "release",
        "create",
        tag,
        ...files.map((file) => join(dir, file)),
        "--repo",
        REPOSITORY,
        "--verify-tag",
        "--latest=false",
        "--title",
        `Monoptah Native ${version}`,
        "--notes-file",
        notesFile,
      ],
    },
  ];
}

// ---- The early workflow run ------------------------------------------------
// Before native:check the script dispatches native-release-monoptah.yml with the
// commit, so Linux, Windows and rpm build while this Mac checks and builds. The
// run's await-release job waits for the tag and the macOS files.

// gh workflow run arguments. `requestId` lands in the run name, so the run can be
// found again. Without `sha` this is the tag-only form of the workflow.
export function dispatchArgs({ version, sha, requestId }) {
  const args = ["workflow", "run", WORKFLOW_FILE, "--repo", REPOSITORY, "--ref", "main", "-f", `tag=${tagFor(version)}`];
  if (sha) args.push("-f", `sha=${sha}`);
  if (requestId) args.push("-f", `request_id=${requestId}`);
  return args;
}
// What to run by hand to start the workflow for a tag that is on origin.
export const manualDispatch = (version) => formatCommand({ command: "gh", args: dispatchArgs({ version }) });
export const cancelCommand = (id) => formatCommand({ command: "gh", args: ["run", "cancel", String(id), "--repo", REPOSITORY] });

export const RUN_LIST_FIELDS = "databaseId,displayTitle,status,conclusion,createdAt,url";
export const runListArgs = () => [
  "run", "list", "--repo", REPOSITORY, "--workflow", WORKFLOW_FILE, "--event", "workflow_dispatch", "--limit", "30", "--json", RUN_LIST_FIELDS,
];
// The run whose name carries the request id (run-name: "... <tag> <request_id>").
export function pickRun(runs, requestId) {
  return runs.find((run) => typeof run.displayTitle === "string" && run.displayTitle.split(/\s+/).includes(requestId)) ?? null;
}

const FIND_ATTEMPTS = 10;
const FIND_WAIT_MS = 3000;

// Dispatches the workflow and finds the run it created. Returns
// { dispatched: false, error } when gh refused, otherwise { dispatched: true, run }
// where run is { id, url, requestId, dispatchedAt } or null when it was not found.
export async function dispatchWorkflow(io, { version, sha }) {
  const requestId = io.randomId();
  const result = io.capture("gh", dispatchArgs({ version, sha, requestId }));
  if (result.status !== 0) return { dispatched: false, error: (result.stderr || result.error?.message || "gh failed").trim() };
  const dispatchedAt = io.now();
  for (let attempt = 0; attempt < FIND_ATTEMPTS; attempt++) {
    const list = io.capture("gh", runListArgs());
    if (list.status === 0) {
      try {
        const found = pickRun(JSON.parse(list.stdout), requestId);
        if (found) return { dispatched: true, run: { id: found.databaseId, url: found.url, requestId, dispatchedAt } };
      } catch {
        // Unreadable listing: try again.
      }
    }
    await io.sleep(FIND_WAIT_MS);
  }
  return { dispatched: true, run: null };
}

// { status, conclusion, url } of a run, or null when it cannot be read.
export function viewRun(io, id) {
  const result = io.capture("gh", ["run", "view", String(id), "--repo", REPOSITORY, "--json", "status,conclusion,url"]);
  if (result.status !== 0) return null;
  try {
    return JSON.parse(result.stdout);
  } catch {
    return null;
  }
}

// Cancels the early run, best effort. Returns the sentence to append to an error.
export function cancelRun(io, run) {
  const result = io.capture("gh", ["run", "cancel", String(run.id), "--repo", REPOSITORY]);
  if (result.status === 0) return `The early CI run ${run.url} was cancelled.`;
  return `The early CI run ${run.url} could not be cancelled (${(result.stderr || "gh failed").trim()}). Cancel it: ${cancelCommand(run.id)}. If you leave it, it fails by itself after its 120 minute wait for the release; nothing is published.`;
}

export const formatCommand = ({ command, args }) =>
  [command, ...args].map((a) => (/^[\w@%+=:,./-]+$/.test(a) ? a : JSON.stringify(a))).join(" ");

// What was left behind when a publish step failed, and how to continue.
// `early` ({ id, url }) is the early workflow run, when there is one.
export function recoveryMessage(step, { version, sha, dir, files, notesFile, early = null }) {
  const tag = tagFor(version);
  const commands = publishCommands({ version, sha, dir, files, notesFile });
  const cmd = (name) => formatCommand(commands.find((c) => c.step === name));
  const kept = `The packaged files are kept in ${dir}.`;
  // The early run waits up to two hours for the tag, the release and its files,
  // then publishes by itself; it does not need a new dispatch while it waits.
  const earlyWaiting = early
    ? `The early CI run ${early.url} is still waiting for the release and the macOS files; once they are on origin and uploaded it publishes by itself (it gives up after 120 minutes). If it has finished or you cancelled it, start a new one with: ${manualDispatch(version)}`
    : `Then start the workflow: ${manualDispatch(version)}`;
  switch (step) {
    case "tag":
      return [
        `Nothing was pushed. ${kept} Check git status, then run this script again.`,
        early ? `If the early run was not cancelled above, cancel it: ${cancelCommand(early.id)}` : "",
      ].filter(Boolean).join("\n");
    case "push-tag":
      return [
        `The tag ${tag} exists locally and may or may not be on origin (check: git ls-remote --tags origin ${tag}). No release exists. ${kept}`,
        `If it is not on origin, delete the local tag and run this script again: git tag -d ${tag}`,
        early ? `Before running the script again, cancel the early run: ${cancelCommand(early.id)}` : "",
        `If it is on origin, continue by hand:`,
        `  ${cmd("release")}`,
        earlyWaiting,
      ].filter(Boolean).join("\n");
    case "release":
      return [
        `The tag ${tag} is on origin. The release may be missing or hold only some of the macOS files (check: gh release view ${tag}). ${kept}`,
        `If it is missing, create it with the same command:`,
        `  ${cmd("release")}`,
        `If some files were uploaded, upload the missing ones from ${dir} with gh release upload ${tag} <file> (never replace an uploaded file), so the release ends with all ${files.length} files.`,
        earlyWaiting,
      ].join("\n");
    case "dispatch":
      return [
        `The tag ${tag} is on origin and the release has the macOS files, but the workflow did not start. Starting it is safe to repeat (the workflow reuses the published macOS files, and runs of one tag never overlap):`,
        `  ${manualDispatch(version)}`,
        early ? `The early run ${early.url} may still be going; a second run is harmless. To stop it: ${cancelCommand(early.id)}` : "",
      ].filter(Boolean).join("\n");
    default:
      return `Step ${step} failed.`;
  }
}

// Native check stamp: a passing native:check is remembered per commit and rustc
// so the next run at the same commit skips it. The tree must have been clean.
export const STAMP_FILE = "crates/monocode-gpui/target/.native-check-stamp.json";
export function stampContent(sha, rustc) {
  return JSON.stringify({ sha, rustc: String(rustc).trim() });
}
export function stampMatches(text, sha, rustc) {
  try {
    const stamp = JSON.parse(text);
    return Boolean(sha) && stamp.sha === sha && stamp.rustc === String(rustc).trim() && stamp.rustc !== "";
  } catch {
    return false;
  }
}

// Shown when the script is stopped (Ctrl-C) or a child dies from a signal.
export function interruptedMessage(step, params) {
  if (!step) return "Interrupted. Nothing was tagged, pushed or published.";
  return "Interrupted during the " + step + " step.\n\n" + recoveryMessage(step, params);
}

export class InterruptedError extends Error {
  constructor(signal) {
    super("Interrupted" + (signal ? " (" + signal + ")" : ""));
    this.interrupted = true;
  }
}

// Interrupts: no SIGINT handler is installed. A child killed by a signal raises
// InterruptedError (recovery text for the publish step, exit 130; before the tag
// the early run is cancelled). Ctrl-C landing between two children kills this
// process with the default action instead, and prints nothing: if that happens
// while publishing, check the tag and release state with gh release view. If it
// happens before the tag, the early run is not cancelled: it fails by itself
// after its 120 minute wait for the release, and nothing is published.
//
// The whole flow with its side effects injected:
//   io.platform, io.arch, io.env
//   io.capture(command, args) -> { status, stdout, stderr }   (never throws)
//   io.visible(command, args, env)                            (throws on failure)
//   io.readText(path), io.exists(path), io.listDir(dir)
//   io.makeTempDir(version), io.removeDir(dir), io.writeStamp(text), io.log(text)
//   io.writeText(path, text), io.now() (ms), io.sleep(ms) (awaitable)
//   io.randomId() -> a short unique string for the workflow run name
// The order is: preconditions, toolchain checks, native:check (or its stamp),
// build, package, the dry-run stop, a recheck of HEAD and the tree, then publish.
export async function runRelease(argv, io) {
  const { version, dryRun, recheck, since, notesFile, ciTimeoutMinutes } = parseArgs(argv);
  if (io.platform !== "darwin" || io.arch !== "arm64")
    throw new Error(
      "This builds the macOS arm64 app and must run on an Apple silicon Mac, not " + io.platform + "/" + io.arch + ".",
    );
  const tag = tagFor(version);
  const pubkey = readUpdaterPubkey(io.readText("src-tauri/tauri.conf.json"));
  const env = buildEnv(version, pubkey, io.env);

  io.log("Checking preconditions for " + tag + "...");
  const state = gatherState(io.capture, {
    version,
    tauriCli: io.exists("node_modules/@tauri-apps/cli"),
  });
  const problems = evaluatePreconditions(state, version);
  if (problems.length > 0) throw new Error("Cannot release " + tag + ":\n- " + problems.join("\n- "));
  const sha = state.head;

  // Generated now so that a gh problem or a missing previous tag stops the
  // release before the long checks and the build.
  const notes = generateNotes({
    run: io.capture,
    version,
    since,
    docsText: io.exists(NOTES_FILE) ? io.readText(NOTES_FILE) : "",
    notesFileText: notesFile === undefined ? undefined : io.readText(notesFile),
  });
  io.log(notes.note);
  io.log("Release notes (" + notes.commits + " commits since " + notes.since + "):\n\n" + notes.text);

  // Fail fast, before the long checks.
  const sdk = io.capture("xcrun", ["--sdk", "macosx", "--show-sdk-version"]);
  if (sdk.status !== 0)
    throw new Error("xcrun could not report the macOS SDK. Install Xcode 26 or newer. " + (sdk.stderr ?? "").trim());
  const sdkIssue = sdkProblem(sdk.stdout);
  if (sdkIssue) throw new Error(sdkIssue);
  if (io.capture("xcrun", ["metal", "--version"]).status !== 0)
    throw new Error(
      "The Metal compiler is missing. GPUI's shaders need it: run xcodebuild -downloadComponent MetalToolchain, then run this again.",
    );
  const rustc = io.capture("rustc", ["-V"]);
  if (rustc.status !== 0) throw new Error("rustc -V failed. Install the Rust toolchain: https://rustup.rs");
  const rustcVersion = rustc.stdout.trim();

  // Start Linux, Windows and rpm now, from the commit, while this Mac checks and
  // builds. Without it (dispatch refused, run not found) a run is dispatched
  // after the release is created, as before.
  let early = null;
  if (!dryRun) {
    const started = await dispatchWorkflow(io, { version, sha });
    if (!started.dispatched)
      io.log("Warning: could not start the early CI run (" + started.error + "). The workflow is dispatched after the release is created instead.");
    else if (started.run === null)
      io.log("Warning: the early CI run was dispatched but could not be found in the run list, so it cannot be cancelled or watched. It stops by itself if the release never comes; a fresh run is dispatched after the release is created.");
    else {
      early = started.run;
      io.log("Early CI run (builds Linux, Windows and rpm while this Mac works): " + early.url);
    }
  }

  // Everything up to the tag. A failure here leaves nothing on GitHub except the
  // early run, which is cancelled (best effort) before the error goes on.
  let dir;
  let files;
  let params;
  let commands;
  try {
    let stamp = "";
    if (io.exists(STAMP_FILE)) stamp = io.readText(STAMP_FILE);
    if (!recheck && stampMatches(stamp, sha, rustcVersion)) {
      io.log("Skipping npm run native:check: it already passed for commit " + sha.slice(0, 12) + " with " + rustcVersion + " (use --recheck to run it again).");
    } else {
      io.log("Running npm run native:check...");
      io.visible("npm", ["run", "native:check"], env);
      // Only a check that passed on a clean tree at this commit is remembered.
      const after = readTreeState(io.capture);
      if (after.head === sha && after.dirty === "") io.writeStamp(stampContent(sha, rustcVersion));
      else io.log("Not recording the check: the tree changed while it ran.");
    }

    io.log("Building " + version + " for " + TARGET + "...");
    const build = buildCommand();
    io.visible(build.command, build.args, env);

    dir = io.makeTempDir(version);
    io.log("Packaging into " + dir + "...");
    const pack = packageCommand(version, dir);
    io.visible(pack.command, pack.args, env);
    files = macosReleaseFiles(version, io.listDir(dir));
    const notesPath = join(dir, "release-notes.md");
    io.writeText(notesPath, notes.text);
    params = { version, sha, dir, files, notesFile: notesPath };
    commands = publishCommands(params);

    io.log("\nmacOS release files:");
    for (const file of files) io.log("  " + join(dir, file));
    if (dryRun) {
      const ci = readCiRuns(io.capture, sha);
      io.log(
        "\nCI state of native.yml for " + sha.slice(0, 12) + ": " +
          (ci.error !== undefined ? "unknown (" + ci.error + ")" : ciVerdict(ci.runs) + ", " + describeCi(ci.runs)) +
          ". A real run waits for it just before tagging.",
      );
      io.log("\nDry run: nothing was tagged, pushed or published, and no early CI run was started. Commit " + sha + " would be released with:");
      for (const c of commands) io.log("  " + formatCommand(c));
      io.log("\nThen the workflow runs (early run or a fresh dispatch): " + manualDispatch(version));
      io.log("\nThe files stay in " + dir + ".");
      return { dryRun: true, sha, dir, files };
    }

    // The build took a while; make sure the commit and the tree are still the
    // ones that were checked before anything is tagged.
    const now = readTreeState(io.capture);
    if (now.head !== sha)
      throw new Error("HEAD moved from " + sha.slice(0, 12) + " to " + now.head.slice(0, 12) + " during the build. Nothing was tagged. Run this again.");
    if (now.dirty !== "")
      throw new Error("The tree has tracked changes after the build; nothing was tagged. Commit, stash or discard them and run this again:\n" + now.dirty);

    // CI ran in parallel with the check and the build; it must have passed
    // before anything is tagged.
    io.log("\nWaiting for native.yml on " + sha.slice(0, 12) + "...");
    await waitForCi(io, sha, ciTimeoutMinutes);
  } catch (error) {
    if (early) error.message = error.message + "\n\n" + cancelRun(io, early);
    throw error;
  }

  // Past this point the repository changes; every failure says what is left.
  const recoveryParams = { ...params, early };
  for (const c of commands) {
    io.log("\n$ " + formatCommand(c));
    try {
      io.visible(c.command, c.args, env);
    } catch (error) {
      // The tag step failed before anything was pushed: the early run has nothing to wait for.
      if (c.step === "tag" && early) error.message = error.message + "\n\n" + cancelRun(io, early);
      error.message = error.message + "\n\n" + (error.interrupted ? interruptedMessage(c.step, recoveryParams) : recoveryMessage(c.step, recoveryParams));
      throw error;
    }
  }

  // The release now has the macOS files. A running early run picks them up by
  // itself; a finished one (or none) needs a new run.
  let run = early;
  try {
    const status = early ? viewRun(io, early.id) : null;
    if (early && status && status.status !== "completed") {
      io.log("\nThe early CI run continues and now sees the tag and the release: " + early.url);
    } else {
      io.log(
        early
          ? "\nThe early CI run is " + (status ? "finished (" + (status.conclusion ?? "no conclusion") + ")" : "not readable") + "; dispatching a new run."
          : "\nNo early CI run; dispatching the workflow.",
      );
      const fresh = await dispatchWorkflow(io, { version, sha });
      if (!fresh.dispatched) throw new Error("Could not dispatch " + WORKFLOW_FILE + ": " + fresh.error);
      run = fresh.run;
    }
  } catch (error) {
    error.message = error.message + "\n\n" + recoveryMessage("dispatch", recoveryParams);
    throw error;
  }
  io.removeDir(dir);
  io.log("\nRelease: https://github.com/" + REPOSITORY + "/releases/tag/" + tag);
  io.log(
    "It holds only the macOS files until the workflow finishes; the in-app feed and the Homebrew cask change at the end.\n" +
      (run ? "Follow the run: " + run.url + "\n" : "") +
      "Or: gh run list --repo " + REPOSITORY + " --workflow " + WORKFLOW_FILE + " --limit 3  (or https://github.com/" + REPOSITORY + "/actions/workflows/" + WORKFLOW_FILE + ")",
  );
  return { dryRun: false, sha, dir, files };
}

// Turns a spawnSync result into an error, or null on success. A child killed by
// a signal (Ctrl-C reaches the whole foreground process group) is an interrupt.
export function failureOf(result, { command, args }) {
  if (result.error) return new Error("Could not run " + command + ": " + result.error.message);
  if (result.signal) return new InterruptedError(result.signal);
  if (result.status !== 0) return new Error(formatCommand({ command, args }) + " failed (exit " + result.status + ")");
  return null;
}

function withoutSigningKey(base) {
  const env = { ...base };
  for (const name of Object.keys(env))
    if (name.startsWith("TAURI_SIGNING_PRIVATE_KEY")) delete env[name];
  return env;
}

function readTreeState(capture) {
  return {
    head: (capture("git", ["rev-parse", "HEAD"]).stdout ?? "").trim(),
    dirty: (capture("git", ["status", "--porcelain", "--untracked-files=no"]).stdout ?? "").trimEnd(),
  };
}

function realIo() {
  const abs = (path) => resolve(root, path);
  return {
    platform: process.platform,
    arch: process.arch,
    env: process.env,
    capture(command, args) {
      const result = spawnSync(command, args, { cwd: root, encoding: "utf8", env: withoutSigningKey(process.env) });
      return { status: result.status ?? 1, stdout: result.stdout ?? "", stderr: result.stderr ?? "", error: result.error };
    },
    visible(command, args, env) {
      const result = spawnSync(command, args, { cwd: root, stdio: "inherit", env });
      const failure = failureOf(result, { command, args });
      if (failure) throw failure;
    },
    readText: (path) => readFileSync(abs(path), "utf8"),
    writeText: (path, text) => writeFileSync(path, text),
    exists: (path) => existsSync(abs(path)),
    listDir: (dir) => readdirSync(dir),
    makeTempDir: (version) => mkdtempSync(join(tmpdir(), "monoptah-native-" + version + "-")),
    removeDir: (dir) => rmSync(dir, { recursive: true, force: true }),
    writeStamp(text) {
      mkdirSync(join(root, "crates/monocode-gpui/target"), { recursive: true });
      writeFileSync(abs(STAMP_FILE), text);
    },
    randomId: () => randomBytes(6).toString("hex"),
    now: () => Date.now(),
    sleep: (ms) => new Promise((done) => setTimeout(done, ms)),
    log: (text) => console.log(text),
  };
}

if (
  process.argv[1] &&
  import.meta.url === pathToFileURL(resolve(process.argv[1])).href
) {
  runRelease(process.argv.slice(2), realIo()).catch((error) => {
    console.error(error.message);
    process.exit(error.interrupted ? 130 : 1);
  });
}
