// Fork-local (Monoptah). Plans, signs and describes the Monoptah Native release:
// the signed update feed (latest.json) covers macOS arm64, Linux x86_64 (AppImage,
// deb, rpm) and Windows x86_64 (NSIS). Upstream's native-release.mjs needs all six
// upstream targets and an R2 base URL; this fork ships five targets from GitHub
// Releases. Kept in its own file so upstream merges do not conflict. The feed
// format matches crates/monocode-core/src/app_update:
// {"payload": base64(manifest JSON), "signature": <tauri signer signature>}.
//
// usage (MONOCODE_NATIVE_VERSION must be set for all but feed-supersedes):
//   node scripts/native-release-monoptah.mjs plan [published-names-file]
//       prints step output lines: matrix (JSON, the platforms still to build
//       except rpm), rpm (true/false), build_any (true/false) and reuse (the
//       published filenames to download instead of building, space separated)
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
export function planBuilds(version, published = []) {
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

if (
  process.argv[1] &&
  import.meta.url === pathToFileURL(resolve(process.argv[1])).href
) {
  const [command, first, second] = process.argv.slice(2);
  if (command === "sign" && first && second) {
    await prepareMonoptahRelease(first, second);
  } else if (command === "plan") {
    const published =
      first && existsSync(first)
        ? (await readFile(first, "utf8"))
            .split("\n")
            .map((line) => line.trim())
            .filter(Boolean)
        : [];
    const plan = planBuilds(process.env.MONOCODE_NATIVE_VERSION, published);
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
  } else {
    console.error(
      "usage: native-release-monoptah.mjs plan [published-file] | records <dir> | sign <dir> <notes-file> | file-count | feed-supersedes <new> [existing]",
    );
    process.exit(2);
  }
}
