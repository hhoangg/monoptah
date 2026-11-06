// Fork-local (Monoptah). Builds and packages the macOS arm64 app of a native
// release on the maintainer's Mac, then hands over to CI. GitHub-hosted macOS
// runners are not used: this script creates the tag and the release with exactly
// the three unsigned macOS files and dispatches native-release-monoptah.yml,
// which builds Linux, Windows and rpm, signs everything (the updater signing key
// exists on CI only; this script never reads it), publishes the feed and bumps
// the Homebrew cask.
//
// usage: npm run native:release:mac -- X.Y.Z [--dry-run] [--recheck]
//   --dry-run runs every check, builds and packages, then prints the files and
//   the commands it would run, without tagging, pushing or publishing.
//   --recheck runs npm run native:check even when a stamp says it already passed
//   for this commit and this rustc (see checkStamp).
//
// The logic lives in exported pure functions and in runRelease, which takes its
// side effects as an injected `io`, so the tests can run it without cargo, git
// pushes or gh.
import { spawnSync } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { REPOSITORY } from "./native-release-monoptah.mjs";
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

export function parseArgs(argv) {
  let version;
  let dryRun = false;
  let recheck = false;
  for (const arg of argv) {
    if (arg === "--dry-run") dryRun = true;
    else if (arg === "--recheck") recheck = true;
    else if (arg.startsWith("-")) throw new Error(`Unknown option: ${arg}`);
    else if (version === undefined) version = arg;
    else throw new Error(`Unexpected extra argument: ${arg}`);
  }
  if (version === undefined)
    throw new Error("usage: npm run native:release:mac -- X.Y.Z [--dry-run] [--recheck]");
  return { version: releaseVersion(version), dryRun, recheck };
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

export function ciRunsPath(sha) {
  return `repos/${REPOSITORY}/actions/workflows/native.yml/runs?head_sha=${sha}&event=push&branch=main&status=success&per_page=1`;
}

// Reads the state the preconditions look at. `run(command, args)` returns
// { status, stdout, stderr } and never throws.
export function gatherState(run, { version, notesText, tauriCli = true }) {
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
  state.notesFirstLine = notesText.split("\n", 1)[0];
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
  const ci = run("gh", ["api", ciRunsPath(state.head), "--jq", ".total_count"]);
  state.ciCheckFailed = ci.status !== 0 ? errorOf(ci) : null;
  state.ciRuns = ci.status === 0 ? Number.parseInt(out(ci), 10) : null;
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
      `HEAD (${(state.head ?? "").slice(0, 8)}) is not origin/main (${(state.originMain ?? "").slice(0, 8)}). Merge the release notes PR, then run: git pull --ff-only origin main (and push or drop any local commits).`,
    );
  if (state.dirty)
    problems.push(`There are tracked changes. Commit, stash or discard them:\n${state.dirty}`);
  if (state.notesFirstLine !== `# Monoptah Native ${version}`)
    problems.push(
      `The first line of ${NOTES_FILE} must be exactly "# Monoptah Native ${version}", found: "${state.notesFirstLine}". Edit the notes and merge them into main first.`,
    );
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
  if (state.ciCheckFailed !== null && state.ciCheckFailed !== undefined)
    problems.push(`Could not read the native.yml runs: ${state.ciCheckFailed}`);
  else if (!(state.ciRuns >= 1))
    problems.push(
      `native.yml has no successful push run on main for commit ${(state.head ?? "").slice(0, 12)}. Wait for it to pass (gh run list --workflow native.yml --branch main), then run this again.`,
    );
  return problems;
}

// The commands that publish, in order. `sha` pins the tag to the commit that
// was checked.
export function publishCommands({ version, sha, dir, files }) {
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
        NOTES_FILE,
      ],
    },
    {
      step: "dispatch",
      command: "gh",
      args: ["workflow", "run", WORKFLOW_FILE, "--repo", REPOSITORY, "--ref", "main", "-f", `tag=${tag}`],
    },
  ];
}

export const formatCommand = ({ command, args }) =>
  [command, ...args].map((a) => (/^[\w@%+=:,./-]+$/.test(a) ? a : JSON.stringify(a))).join(" ");

// What was left behind when a publish step failed, and how to continue.
export function recoveryMessage(step, { version, sha, dir, files }) {
  const tag = tagFor(version);
  const commands = publishCommands({ version, sha, dir, files });
  const cmd = (name) => formatCommand(commands.find((c) => c.step === name));
  const kept = `The packaged files are kept in ${dir}.`;
  switch (step) {
    case "tag":
      return `Nothing was pushed. ${kept} Check git status, then run this script again.`;
    case "push-tag":
      return [
        `The tag ${tag} exists locally and may or may not be on origin (check: git ls-remote --tags origin ${tag}). No release exists. ${kept}`,
        `If it is not on origin, delete the local tag and run this script again: git tag -d ${tag}`,
        `If it is on origin, continue by hand:`,
        `  ${cmd("release")}`,
        `  ${cmd("dispatch")}`,
      ].join("\n");
    case "release":
      return [
        `The tag ${tag} is on origin. The release may be missing or hold only some of the macOS files (check: gh release view ${tag}). ${kept}`,
        `If it is missing, create it with the same command:`,
        `  ${cmd("release")}`,
        `If some files were uploaded, upload the missing ones from ${dir} with gh release upload ${tag} <file> (never replace an uploaded file), so the release ends with all ${files.length} files.`,
        `Then dispatch the workflow:`,
        `  ${cmd("dispatch")}`,
      ].join("\n");
    case "dispatch":
      return [
        `The tag ${tag} is on origin and the release has the macOS files, but the workflow did not start. Starting it is safe to repeat (the workflow reuses the published macOS files):`,
        `  ${cmd("dispatch")}`,
      ].join("\n");
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

// Interrupts: no SIGINT handler is installed, because runRelease is synchronous
// and a handler would never run while it works. A child killed by a signal raises
// InterruptedError (recovery text for the publish step, exit 130). Ctrl-C landing
// between two children kills this process with the default action instead, and
// prints nothing: if that happens while publishing, check the tag and release
// state with gh release view.
//
// The whole flow with its side effects injected:
//   io.platform, io.arch, io.env
//   io.capture(command, args) -> { status, stdout, stderr }   (never throws)
//   io.visible(command, args, env)                            (throws on failure)
//   io.readText(path), io.exists(path), io.listDir(dir)
//   io.makeTempDir(version), io.removeDir(dir), io.writeStamp(text), io.log(text)
// The order is: preconditions, toolchain checks, native:check (or its stamp),
// build, package, the dry-run stop, a recheck of HEAD and the tree, then publish.
export async function runRelease(argv, io) {
  const { version, dryRun, recheck } = parseArgs(argv);
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
    notesText: io.readText(NOTES_FILE),
    tauriCli: io.exists("node_modules/@tauri-apps/cli"),
  });
  const problems = evaluatePreconditions(state, version);
  if (problems.length > 0) throw new Error("Cannot release " + tag + ":\n- " + problems.join("\n- "));
  const sha = state.head;

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

  const dir = io.makeTempDir(version);
  io.log("Packaging into " + dir + "...");
  const pack = packageCommand(version, dir);
  io.visible(pack.command, pack.args, env);
  const files = macosReleaseFiles(version, io.listDir(dir));
  const params = { version, sha, dir, files };
  const commands = publishCommands(params);

  io.log("\nmacOS release files:");
  for (const file of files) io.log("  " + join(dir, file));
  if (dryRun) {
    io.log("\nDry run: nothing was tagged, pushed or published. Commit " + sha + " would be released with:");
    for (const c of commands) io.log("  " + formatCommand(c));
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

  // Past this point the repository changes; every failure says what is left.
  for (const c of commands) {
    io.log("\n$ " + formatCommand(c));
    try {
      io.visible(c.command, c.args, env);
    } catch (error) {
      error.message = error.message + "\n\n" + (error.interrupted ? interruptedMessage(c.step, params) : recoveryMessage(c.step, params));
      throw error;
    }
  }
  io.removeDir(dir);
  io.log("\nRelease: https://github.com/" + REPOSITORY + "/releases/tag/" + tag);
  io.log(
    "It holds only the macOS files until the workflow finishes; the in-app feed and the Homebrew cask change at the end.\nFollow the run: gh run list --repo " + REPOSITORY + " --workflow " + WORKFLOW_FILE + " --limit 1  (or https://github.com/" + REPOSITORY + "/actions/workflows/" + WORKFLOW_FILE + ")",
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
  const abs = (path) => join(root, path);
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
    exists: (path) => existsSync(abs(path)),
    listDir: (dir) => readdirSync(dir),
    makeTempDir: (version) => mkdtempSync(join(tmpdir(), "monoptah-native-" + version + "-")),
    removeDir: (dir) => rmSync(dir, { recursive: true, force: true }),
    writeStamp(text) {
      mkdirSync(join(root, "crates/monocode-gpui/target"), { recursive: true });
      writeFileSync(abs(STAMP_FILE), text);
    },
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
