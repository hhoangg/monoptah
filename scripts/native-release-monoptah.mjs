// Fork-local (Monoptah). Plans, signs and describes the Monoptah Native release:
// the signed update feed (latest.json) covers macOS arm64, Linux x86_64 (AppImage,
// deb, rpm) and Windows x86_64 (NSIS). Upstream's native-release.mjs needs all six
// upstream targets and an R2 base URL; this fork ships five targets from GitHub
// Releases. Kept in its own file so upstream merges do not conflict. The feed
// format matches crates/monocode-core/src/app_update:
// {"payload": base64(manifest JSON), "signature": <tauri signer signature>}.
//
// usage (MONOCODE_NATIVE_VERSION must be set for all but feed-supersedes):
//   node scripts/native-release-monoptah.mjs plan [--defer-macos] [--check-planned] [published-names-file]
//       prints step output lines: matrix (JSON, the platforms still to build
//       except rpm), rpm (true/false), build_any (true/false) and reuse (the
//       published filenames to download instead of building, space separated).
//       Fails when macOS would have to be built: its files are built on the
//       maintainer's Mac (npm run native:release:mac) and must already be on
//       the release.
//       --defer-macos is for a run that starts before the release exists: the
//       macOS files are not all published yet, so macOS is left out of the
//       matrix without failing (the await-release job waits for them).
//       --check-planned (env PLANNED_MATRIX, PLANNED_RPM) compares the result
//       with an earlier plan and fails when they build different platforms.
//   node scripts/native-release-monoptah.mjs macos-ready <assets-json-file>
//       prints "true" when the release assets (the JSON list from the GitHub
//       API) hold all macOS files in state "uploaded", else "false"
//   node scripts/native-release-monoptah.mjs records <dir>
//       writes artifacts-<kind>-<arch>.json for every platform whose assets are
//       in <dir> but that has no metadata file yet (used when a re-run signs the
//       assets a release already published)
//   node scripts/native-release-monoptah.mjs sign <dir> <notes-file>
//       env: TAURI_SIGNING_PRIVATE_KEY (+ _PASSWORD)
//   node scripts/native-release-monoptah.mjs file-count
//       prints how many files release-files.json must list
//   node scripts/native-release-monoptah.mjs feed-supersedes <new> [existing]
//       prints "true" when <new> should replace <existing> as the rolling feed
//   node scripts/native-release-monoptah.mjs ci-gate <sha>
//       env: GH_TOKEN, GITHUB_REPOSITORY (MONOCODE_NATIVE_VERSION is not needed)
//       requires a native.yml push run on main with conclusion "success" for
//       <sha>. Reads the unfiltered run list (never status=success: GitHub's
//       filtered index can miss successful runs for hours) and retries up to 6
//       reads, 20 s apart, while nothing is visible yet or a run is pending.
import { spawnSync } from "node:child_process";
import { existsSync } from "node:fs";
import { readFile, readdir, writeFile } from "node:fs/promises";
import { basename, join, resolve } from "node:path";
import { pathToFileURL } from "node:url";
import {
  PLATFORMS,
  RELEASE_FILE_COUNT,
  TARGETS,
  recordsFileName,
  releaseVersion,
} from "./package-native-monoptah.mjs";

export const REPOSITORY = "hhoangg/monoptah";
export const APP_ID = "com.monoptah.native";
// Must equal CHANNEL in crates/monocode-core/src/app_update/mod.rs.
export const CHANNEL = "native-preview";
export { RELEASE_FILE_COUNT, TARGETS };

export function downloadBase(version) {
  releaseVersion(version);
  return `https://github.com/${REPOSITORY}/releases/download/native-v${version}`;
}

export function monoptahManifest({ version, baseUrl, artifacts, notes = "" }) {
  releaseVersion(version);
  const base = new URL(baseUrl);
  if (
    base.protocol !== "https:" ||
    base.username ||
    base.password ||
    base.search ||
    base.hash
  )
    throw new Error(
      "The native download base must be an HTTPS URL without credentials, query or fragment",
    );
  const platforms = {};
  for (const artifact of artifacts) {
    if (
      basename(artifact.filename) !== artifact.filename ||
      artifact.filename.includes("\\")
    )
      throw new Error("An artifact must be a plain filename");
    if (!artifact.target) continue;
    if (!TARGETS.includes(artifact.target) || platforms[artifact.target])
      throw new Error(
        `Duplicate or unexpected native target: ${artifact.target}`,
      );
    if (!artifact.signature?.trim())
      throw new Error(`Missing signature for ${artifact.target}`);
    platforms[artifact.target] = {
      url: `${base.href.replace(/\/$/, "")}/${encodeURIComponent(artifact.filename)}`,
      signature: artifact.signature.trim(),
    };
  }
  for (const target of TARGETS)
    if (!platforms[target])
      throw new Error(`Missing native package: ${target}`);
  return { app_id: APP_ID, channel: CHANNEL, version, notes, platforms };
}

// The notes shipped in the feed and on the release must be about this version,
// so a forgotten notes file cannot publish the previous release's text.
export function checkNotes(notes, version) {
  const heading = notes.split("\n", 1)[0].trim();
  if (heading !== `# Monoptah Native ${version}`)
    throw new Error(
      `The release notes must start with "# Monoptah Native ${version}", found: ${heading}`,
    );
  return notes;
}

function sign(filename, env) {
  const result = spawnSync(
    process.execPath,
    ["node_modules/@tauri-apps/cli/tauri.js", "signer", "sign", filename],
    {
      env: { ...env, CI: "true" },
      encoding: "utf8",
      stdio: ["ignore", "pipe", "pipe"],
    },
  );
  if (result.error || result.status !== 0)
    throw new Error(
      `Could not sign ${basename(filename)}: ${(result.stderr ?? "").trim()}`,
    );
  return readFile(`${filename}.sig`, "utf8").then((s) => s.trim());
}

export async function prepareMonoptahRelease(
  directory,
  notesFile,
  env = process.env,
) {
  const version = env.MONOCODE_NATIVE_VERSION;
  releaseVersion(version);
  if (!env.TAURI_SIGNING_PRIVATE_KEY)
    throw new Error("TAURI_SIGNING_PRIVATE_KEY is required");
  directory = resolve(directory);
  // Which filename each updater target must have, so a package from another
  // version or platform cannot be signed under the wrong target.
  const expected = {};
  for (const platform of PLATFORMS)
    for (const { pattern, target } of platform.assets(version))
      if (target) expected[target] = pattern;
  const records = [];
  for (const metadata of (await readdir(directory))
    .filter((name) => /^artifacts-.*\.json$/.test(name))
    .sort())
    records.push(
      ...JSON.parse(await readFile(join(directory, metadata), "utf8")),
    );
  const files = [];
  for (const record of records) {
    if (
      basename(record.filename) !== record.filename ||
      record.filename.includes("\\")
    )
      throw new Error("Invalid artifact filename");
    if (files.includes(record.filename))
      throw new Error(`Duplicate native artifact: ${record.filename}`);
    if (!existsSync(join(directory, record.filename)))
      throw new Error(`Missing native artifact: ${record.filename}`);
    files.push(record.filename);
    if (record.target) {
      if (!expected[record.target]?.test(record.filename))
        throw new Error(
          `Unexpected updater package for ${record.target}: ${record.filename}`,
        );
      record.signature = await sign(join(directory, record.filename), env);
      files.push(`${record.filename}.sig`);
    }
  }
  if (files.length !== RELEASE_FILE_COUNT)
    throw new Error(
      `Expected ${RELEASE_FILE_COUNT} release files, found ${files.length}`,
    );
  const notes = checkNotes(await readFile(notesFile, "utf8"), version);
  const manifest = monoptahManifest({
    version,
    baseUrl: downloadBase(version),
    artifacts: records,
    notes,
  });
  const payload = Buffer.from(JSON.stringify(manifest));
  const payloadPath = join(directory, "native-manifest.payload");
  await writeFile(payloadPath, payload);
  const signature = await sign(payloadPath, env);
  await writeFile(
    join(directory, "latest.json"),
    JSON.stringify({ payload: payload.toString("base64"), signature }),
  );
  // What the versioned GitHub release publishes. latest.json goes to the
  // rolling native-latest release instead.
  await writeFile(join(directory, "release-files.json"), JSON.stringify(files));
  return files;
}

// Which platforms still need a build. A platform counts as published when every
// one of its files is already on the release; those files are reused (never
// rebuilt or replaced) so the cask pin and the feed keep describing published
// bytes. A platform with only some of its files published is an error. `published` is the list of asset names on the existing release.
export function planBuilds(version, published = [], { deferMacos = false } = {}) {
  releaseVersion(version);
  const matrix = [];
  let rpm = false;
  const reuse = [];
  for (const platform of PLATFORMS) {
    const found = platform
      .assets(version)
      .map(({ pattern }) => published.filter((name) => pattern.test(name)));
    if (found.every((names) => names.length === 1)) {
      reuse.push(...found.flat());
      continue;
    }
    // The macOS files come from the maintainer's Mac while the run is already
    // going; they are not built here and the await-release job waits for them.
    if (deferMacos && platform.kind === "macos") continue;
    // Some files published, some not: rebuilding would produce bytes that differ
    // from the published ones, and only missing names are uploaded, so the feed
    // and the cask checksum would describe files the release does not serve.
    // The operator must clear the platform by hand.
    if (found.some((names) => names.length > 0)) {
      const present = found.flat();
      throw new Error(
        `The ${platform.kind} files on release native-v${version} are incomplete or ambiguous (found: ${present.join(", ") || "none"}). ` +
          `Delete every published ${platform.kind} asset of this release (the files and their .sig files) by hand, then re-run the workflow so the platform is rebuilt as a whole.`,
      );
    }
    if (platform.kind === "rpm") rpm = true;
    else
      matrix.push({
        kind: platform.kind,
        os: platform.os,
        target: platform.rust,
        arch: platform.arch,
      });
  }
  return { matrix, rpm, buildAny: matrix.length > 0 || rpm, reuse };
}

// True when the release carries every macOS file, each one fully uploaded.
// `assets` is the list from the GitHub API: [{ name, state }].
export function macosFilesReady(version, assets) {
  const platform = PLATFORMS.find((p) => p.kind === "macos");
  return platform.assets(version).every(({ pattern }) => {
    const found = assets.filter((asset) => pattern.test(asset.name));
    return found.length === 1 && found[0].state === "uploaded";
  });
}

// The await-release job plans again after the macOS files arrived. It must
// build exactly what the first plan built: a platform that became published in
// between (another run) would be built here and also downloaded, and one that
// is newly missing would never be built.
export function checkPlanned(planned, now) {
  const kinds = (plan) => [...plan.matrix.map((e) => e.kind), ...(plan.rpm ? ["rpm"] : [])].sort().join(",");
  if (kinds(planned) !== kinds(now))
    throw new Error(
      `The platforms to build changed since the plan job ran (planned: ${kinds(planned) || "none"}, now: ${kinds(now) || "none"}). Another run probably published a platform. Nothing was published; check the release assets and start the workflow again.`,
    );
}

// The workflow never builds macOS: the release must already carry its files.
// Kept apart from planBuilds so that stays generic.
export function requireReused(plan, kind, version) {
  if (plan.matrix.some((entry) => entry.kind === kind))
    throw new Error(
      `The ${kind} files are missing from release native-v${version}. They are built on the maintainer's Mac, not on CI: run "npm run native:release:mac -- ${version}" from a clean main checkout (it creates the release with those files and dispatches this workflow).`,
    );
  return plan;
}

// Writes the metadata file for each platform whose files are in `directory`
// but whose packaging step did not run (a reused platform).
export async function writeMissingRecords(directory, version) {
  releaseVersion(version);
  directory = resolve(directory);
  const present = await readdir(directory);
  const written = [];
  for (const platform of PLATFORMS) {
    const file = recordsFileName(platform);
    if (present.includes(file)) continue;
    const assets = platform.assets(version);
    const records = [];
    for (const { pattern, target } of assets) {
      const matches = present.filter((name) => pattern.test(name));
      if (matches.length !== 1) break;
      records.push({ filename: matches[0], target });
    }
    if (records.length !== assets.length) continue;
    await writeFile(join(directory, file), JSON.stringify(records, null, 2));
    written.push(file);
  }
  return written;
}

export function feedVersion(feedText) {
  const envelope = JSON.parse(feedText);
  const manifest = JSON.parse(
    Buffer.from(envelope.payload, "base64").toString("utf8"),
  );
  return releaseVersion(manifest.version);
}
function compare(a, b) {
  const [x, y] = [a, b].map((v) => v.split(".").map(Number));
  for (let i = 0; i < 3; i++) if (x[i] !== y[i]) return x[i] - y[i];
  return 0;
}
// The rolling feed never goes backwards: a re-run of an older tag must not
// replace a newer advertised version. The same version may be republished.
// An existing feed that cannot be read is replaced.
export function feedSupersedes(newFeed, existingFeed = null) {
  if (existingFeed === null) return true;
  let existing;
  try {
    existing = feedVersion(existingFeed);
  } catch {
    return true;
  }
  return compare(feedVersion(newFeed), existing) >= 0;
}

// ---- native.yml gate (shared by the workflow and native-release-macos-local.mjs) ----

// The native.yml push runs on main for a commit, in any state. No status filter:
// GitHub's `status=success` filtered index returned no runs for a run that had
// succeeded, for hours. The conclusion is judged from the unfiltered list.
export function ciRunsPath(sha, repository = REPOSITORY) {
  return `repos/${repository}/actions/workflows/native.yml/runs?head_sha=${sha}&event=push&branch=main&per_page=100`;
}
export const CI_RUNS_JQ =
  "{total: .total_count, runs: [.workflow_runs[] | {id: .id, status: .status, conclusion: .conclusion, url: .html_url}]}";

// "success" when any run succeeded, "pending" while a run is not completed or
// none exists yet (it may not have been created), "failed" when every run
// completed and none succeeded. The runs API returns each run's latest attempt:
// a rerun that failed after an earlier success reads as a failure, and a rerun
// in progress reads as pending.
export function ciVerdict(runs) {
  if (runs.some((run) => run.conclusion === "success")) return "success";
  if (runs.length === 0 || runs.some((run) => run.status !== "completed")) return "pending";
  return "failed";
}

const describeRun = (run) => `run ${run.id} (${run.status === "completed" ? run.conclusion : run.status}) ${run.url}`;
export const describeCi = (runs) =>
  runs.length === 0 ? "no native.yml run found yet" : runs.map(describeRun).join("; ");

// Reads the runs through `capture`; { runs } or { error }. A list that holds
// fewer runs than the total is an error, never judged, unless it already holds
// a successful run (enough to decide, whatever the missing runs are).
export function readCiRuns(capture, sha, repository = REPOSITORY) {
  const result = capture("gh", ["api", ciRunsPath(sha, repository), "--jq", CI_RUNS_JQ]);
  if (result.status !== 0) return { error: (result.stderr || result.error?.message || "gh failed").trim() };
  try {
    const answer = JSON.parse(result.stdout);
    if (!Array.isArray(answer?.runs) || !Number.isInteger(answer.total)) throw new Error("not a run list");
    if (answer.total > answer.runs.length && !answer.runs.some((run) => run.conclusion === "success"))
      throw new Error(`truncated list: ${answer.runs.length} of ${answer.total} runs`);
    return { runs: answer.runs };
  } catch (error) {
    return { error: `unreadable answer (${error.message})` };
  }
}

export const CI_GATE_ATTEMPTS = 6;
export const CI_GATE_SLEEP_MS = 20_000;

// Requires a successful native.yml run for `sha`. Retries read errors and
// pending states (including no run yet) so that a short lag does not fail a
// release; fails at once when every run finished without success. io:
// { capture, sleep, log }.
export async function ciGate(io, sha, { repository = REPOSITORY, attempts = CI_GATE_ATTEMPTS, sleepMs = CI_GATE_SLEEP_MS } = {}) {
  let last = "no read made";
  for (let attempt = 1; attempt <= attempts; attempt += 1) {
    const read = readCiRuns(io.capture, sha, repository);
    if (read.error !== undefined) {
      last = `could not read the native.yml runs (${read.error})`;
    } else {
      const verdict = ciVerdict(read.runs);
      if (verdict === "success") {
        io.log(`native.yml passed for ${sha}: ${describeCi(read.runs.filter((r) => r.conclusion === "success"))}`);
        return;
      }
      if (verdict === "failed")
        throw new Error(`native.yml has no successful push run on main for commit ${sha}: ${describeCi(read.runs)}`);
      last = `native.yml has no successful push run on main for commit ${sha} yet: ${describeCi(read.runs)}`;
    }
    if (attempt < attempts) {
      io.log(`${last}; retrying in ${sleepMs / 1000} s (${attempt}/${attempts}).`);
      await io.sleep(sleepMs);
    }
  }
  throw new Error(`${last} (after ${attempts} reads)`);
}

if (
  process.argv[1] &&
  import.meta.url === pathToFileURL(resolve(process.argv[1])).href
) {
  const [command, first, second] = process.argv.slice(2);
  if (command === "sign" && first && second) {
    await prepareMonoptahRelease(first, second);
  } else if (command === "macos-ready" && first) {
    const assets = JSON.parse(await readFile(first, "utf8"));
    console.log(String(macosFilesReady(process.env.MONOCODE_NATIVE_VERSION, assets)));
  } else if (command === "plan") {
    const flags = process.argv.slice(3).filter((arg) => arg.startsWith("--"));
    const file = process.argv.slice(3).find((arg) => !arg.startsWith("--"));
    const version = process.env.MONOCODE_NATIVE_VERSION;
    const published =
      file && existsSync(file)
        ? (await readFile(file, "utf8"))
            .split("\n")
            .map((line) => line.trim())
            .filter(Boolean)
        : [];
    const deferMacos = flags.includes("--defer-macos");
    const planned = planBuilds(version, published, { deferMacos });
    const plan = deferMacos ? planned : requireReused(planned, "macos", version);
    if (flags.includes("--check-planned"))
      checkPlanned(
        { matrix: JSON.parse(process.env.PLANNED_MATRIX ?? "[]"), rpm: process.env.PLANNED_RPM === "true" },
        plan,
      );
    console.log(`matrix=${JSON.stringify(plan.matrix)}`);
    console.log(`rpm=${plan.rpm}`);
    console.log(`build_any=${plan.buildAny}`);
    console.log(`reuse=${plan.reuse.join(" ")}`);
  } else if (command === "records" && first) {
    await writeMissingRecords(first, process.env.MONOCODE_NATIVE_VERSION);
  } else if (command === "file-count") {
    console.log(String(RELEASE_FILE_COUNT));
  } else if (command === "feed-supersedes" && first) {
    const existing =
      second && existsSync(second) ? await readFile(second, "utf8") : null;
    console.log(String(feedSupersedes(await readFile(first, "utf8"), existing)));
  } else if (command === "ci-gate" && first) {
    try {
      await ciGate(
        {
          capture: (cmd, args) => spawnSync(cmd, args, { encoding: "utf8" }),
          sleep: (ms) => new Promise((done) => setTimeout(done, ms)),
          log: (line) => console.log(line),
        },
        first,
        { repository: process.env.GITHUB_REPOSITORY || REPOSITORY },
      );
    } catch (error) {
      console.error(error.message);
      process.exit(1);
    }
  } else {
    console.error(
      "usage: native-release-monoptah.mjs plan [--defer-macos] [--check-planned] [published-file] | macos-ready <assets-json> | records <dir> | sign <dir> <notes-file> | file-count | feed-supersedes <new> [existing] | ci-gate <sha>",
    );
    process.exit(2);
  }
}
