// Fork-local (Monoptah). Signs the Monoptah Native updater archive and builds the
// signed update feed (latest.json) for the macOS arm64 build. Upstream's
// native-release.mjs needs all six platform targets and an R2 base URL; this fork
// ships one target from GitHub Releases. Kept in its own file so upstream merges
// do not conflict. The feed format matches crates/monocode-core/src/app_update:
// {"payload": base64(manifest JSON), "signature": <tauri signer signature>}.
//
// usage:
//   node scripts/native-release-monoptah.mjs sign <dir> <notes-file>
//       env: MONOCODE_NATIVE_VERSION, TAURI_SIGNING_PRIVATE_KEY (+ _PASSWORD)
//   node scripts/native-release-monoptah.mjs records <dir>
//       writes artifacts-macos-aarch64.json for assets already in <dir>
//       (used when a re-run signs the assets a release already published)
//   node scripts/native-release-monoptah.mjs feed-supersedes <new> [existing]
//       prints "true" when <new> should replace <existing> as the rolling feed
import { spawnSync } from "node:child_process";
import { existsSync } from "node:fs";
import { readFile, writeFile } from "node:fs/promises";
import { basename, join, resolve } from "node:path";
import { pathToFileURL } from "node:url";
import {
  artifactRecords,
  assetNames,
  releaseVersion,
} from "./package-native-monoptah.mjs";

export const REPOSITORY = "hhoangg/monoptah";
export const APP_ID = "com.monoptah.native";
// Must equal CHANNEL in crates/monocode-core/src/app_update/mod.rs.
export const CHANNEL = "native-preview";
export const TARGET = "darwin-aarch64-app";

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
    if (artifact.target !== TARGET || platforms[artifact.target])
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
  if (!platforms[TARGET])
    throw new Error(`Missing native package: ${TARGET}`);
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
  const names = assetNames(version);
  const records = JSON.parse(
    await readFile(join(directory, "artifacts-macos-aarch64.json"), "utf8"),
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
      if (record.filename !== names.updater)
        throw new Error(`Unexpected updater archive: ${record.filename}`);
      record.signature = await sign(join(directory, record.filename), env);
      files.push(`${record.filename}.sig`);
    }
  }
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
  } else if (command === "records" && first) {
    await writeFile(
      join(resolve(first), "artifacts-macos-aarch64.json"),
      JSON.stringify(
        artifactRecords(releaseVersion(process.env.MONOCODE_NATIVE_VERSION)),
        null,
        2,
      ),
    );
  } else if (command === "feed-supersedes" && first) {
    const existing = second && existsSync(second) ? await readFile(second, "utf8") : null;
    console.log(String(feedSupersedes(await readFile(first, "utf8"), existing)));
  } else {
    console.error(
      "usage: native-release-monoptah.mjs sign <dir> <notes-file> | records <dir> | feed-supersedes <new> [existing]",
    );
    process.exit(2);
  }
}
