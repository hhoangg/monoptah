// Fork-local (Monoptah). Tests the signed feed for the macOS arm64 native build.
import assert from "node:assert/strict";
import { createHash, createPublicKey, verify } from "node:crypto";
import { spawnSync } from "node:child_process";
import { existsSync } from "node:fs";
import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";
import {
  checkNotes,
  downloadBase,
  feedSupersedes,
  feedVersion,
  monoptahManifest,
  prepareMonoptahRelease,
} from "./native-release-monoptah.mjs";
import { artifactRecords, assetNames, releaseVersion } from "./package-native-monoptah.mjs";

const artifacts = [
  {
    target: "darwin-aarch64-app",
    filename: "MonoptahNative_0.34.0_aarch64.app.tar.gz",
    signature: "signed-package",
  },
  { target: null, filename: "MonoptahNative_0.34.0_aarch64.zip" },
];
function manifest(changes = {}) {
  return monoptahManifest({
    version: "0.34.0",
    baseUrl: downloadBase("0.34.0"),
    artifacts,
    notes: "# Monoptah Native 0.34.0",
    ...changes,
  });
}

test("only plain X.Y.Z versions are accepted", () => {
  assert.equal(releaseVersion("0.34.0"), "0.34.0");
  for (const version of [
    "0.34.0-preview.1",
    "0.34.0-beta.1",
    "v0.34.0",
    "native-v0.34.0",
    "0.34",
    "0.34.0/../x",
    "01.2.3",
    "70000.0.0",
    "",
    undefined,
  ])
    assert.throws(() => releaseVersion(version), /plain native release version/);
});

test("asset names follow the cask and release layout", () => {
  assert.deepEqual(assetNames("0.34.0"), {
    updater: "MonoptahNative_0.34.0_aarch64.app.tar.gz",
    zip: "MonoptahNative_0.34.0_aarch64.zip",
    sha256: "MonoptahNative_0.34.0_aarch64.zip.sha256",
  });
});

test("the manifest lists only the Apple silicon app on the versioned release URL", () => {
  const result = manifest();
  assert.equal(result.app_id, "com.monoptah.native");
  assert.equal(result.channel, "native-preview");
  assert.equal(result.version, "0.34.0");
  assert.deepEqual(Object.keys(result.platforms), ["darwin-aarch64-app"]);
  assert.deepEqual(result.platforms["darwin-aarch64-app"], {
    url: "https://github.com/hhoangg/monoptah/releases/download/native-v0.34.0/MonoptahNative_0.34.0_aarch64.app.tar.gz",
    signature: "signed-package",
  });
});

test("the manifest refuses other targets, missing or duplicate packages, unsigned packages and traversal", () => {
  assert.throws(() => manifest({ artifacts: [] }), /Missing native package/);
  assert.throws(
    () =>
      manifest({
        artifacts: [{ ...artifacts[0], target: "darwin-x86_64-app" }],
      }),
    /unexpected native target/,
  );
  assert.throws(
    () => manifest({ artifacts: [...artifacts, artifacts[0]] }),
    /Duplicate/,
  );
  assert.throws(
    () => manifest({ artifacts: [{ ...artifacts[0], signature: " " }] }),
    /Missing signature/,
  );
  for (const filename of ["../latest.json", "directory\\latest.json"])
    assert.throws(
      () => manifest({ artifacts: [{ ...artifacts[0], filename }] }),
      /plain filename/,
    );
  for (const baseUrl of [
    "http://github.com/x",
    "https://user:secret@github.com/x",
    "https://github.com/x?a=b",
    "https://github.com/x#frag",
  ])
    assert.throws(() => manifest({ baseUrl }), /HTTPS URL/);
  assert.throws(() => manifest({ version: "0.34.0-preview.1" }), /plain native/);
});

test("release notes must be about the version being released", () => {
  assert.equal(
    checkNotes("# Monoptah Native 0.34.0\n\nbody", "0.34.0"),
    "# Monoptah Native 0.34.0\n\nbody",
  );
  assert.throws(
    () => checkNotes("# Monoptah Native 0.33.0\n", "0.34.0"),
    /must start with/,
  );
  assert.throws(() => checkNotes("", "0.34.0"), /must start with/);
});

function feed(version) {
  const payload = Buffer.from(JSON.stringify({ version })).toString("base64");
  return JSON.stringify({ payload, signature: "x" });
}
test("the rolling feed never moves to an older version", () => {
  assert.equal(feedSupersedes(feed("0.34.0")), true);
  assert.equal(feedSupersedes(feed("0.34.1"), feed("0.34.0")), true);
  assert.equal(feedSupersedes(feed("0.34.0"), feed("0.34.0")), true);
  assert.equal(feedSupersedes(feed("0.34.0"), feed("0.34.1")), false);
  assert.equal(feedSupersedes(feed("0.9.0"), feed("0.10.0")), false);
  assert.equal(feedSupersedes(feed("1.0.0"), feed("0.99.99")), true);
  assert.equal(feedSupersedes(feed("0.34.0"), "not json"), true);
  assert.equal(feedVersion(feed("0.34.0")), "0.34.0");
});

// Minisign verification, enough to check what `tauri signer sign` writes
// (prehashed Ed25519 over BLAKE2b-512) without the real key or the Rust client.
function minisignVerify(publicKeyBase64, signatureBase64, message) {
  const text = (value) =>
    Buffer.from(value.trim(), "base64").toString("utf8").split("\n");
  const key = Buffer.from(text(publicKeyBase64)[1], "base64");
  const sig = Buffer.from(text(signatureBase64)[1], "base64");
  assert.equal(key.subarray(0, 2).toString(), "Ed");
  assert.equal(sig.subarray(0, 2).toString(), "ED", "expected a prehashed signature");
  assert.deepEqual(sig.subarray(2, 10), key.subarray(2, 10), "key id");
  const publicKey = createPublicKey({
    key: Buffer.concat([
      Buffer.from("302a300506032b6570032100", "hex"),
      key.subarray(10),
    ]),
    format: "der",
    type: "spki",
  });
  const digest = createHash("blake2b512").update(message).digest();
  return verify(null, digest, publicKey, sig.subarray(10));
}

// native.yml runs the native node tests without `npm ci`, so the Tauri CLI that
// does the signing may be missing there. The release workflow runs this test
// after `npm ci`, where it is present.
const root = fileURLToPath(new URL("../", import.meta.url));
const tauriCli = join(root, "node_modules/@tauri-apps/cli/tauri.js");
const signingTest = existsSync(tauriCli)
  ? test
  : (name, fn) =>
      test(name, { skip: "node_modules/@tauri-apps/cli is not installed" }, fn);

test("the records command writes the same metadata packaging does", async (t) => {
  const directory = await mkdtemp(join(tmpdir(), "monoptah-native-records-"));
  t.after(() => rm(directory, { recursive: true, force: true }));
  const result = spawnSync(
    process.execPath,
    [join(root, "scripts/native-release-monoptah.mjs"), "records", directory],
    { encoding: "utf8", env: { ...process.env, MONOCODE_NATIVE_VERSION: "0.34.0" } },
  );
  assert.equal(result.status, 0, result.stderr);
  assert.deepEqual(
    JSON.parse(await readFile(join(directory, "artifacts-macos-aarch64.json"), "utf8")),
    artifactRecords("0.34.0"),
  );
  const bad = spawnSync(
    process.execPath,
    [join(root, "scripts/native-release-monoptah.mjs"), "records", directory],
    { encoding: "utf8", env: { ...process.env, MONOCODE_NATIVE_VERSION: "0.34.0-preview.1" } },
  );
  assert.notEqual(bad.status, 0);
});

signingTest("signing produces a feed and package signature that verify with the public key", async (t) => {
  const directory = await mkdtemp(join(tmpdir(), "monoptah-native-release-"));
  t.after(() => rm(directory, { recursive: true, force: true }));
  // A throwaway key; the real updater key is never used by tests.
  const keyFile = join(directory, "throwaway.key");
  const generated = spawnSync(
    process.execPath,
    [
      tauriCli,
      "signer",
      "generate",
      "-w",
      keyFile,
      "--ci",
      "-p",
      "",
    ],
    { encoding: "utf8" },
  );
  assert.equal(generated.status, 0, generated.stderr);
  const publicKey = (await readFile(`${keyFile}.pub`, "utf8")).trim();

  const dist = join(directory, "dist");
  await mkdir(dist);
  const names = assetNames("0.34.0");
  const archive = Buffer.from("updater archive bytes");
  await writeFile(join(dist, names.updater), archive);
  await writeFile(join(dist, names.zip), "zip");
  await writeFile(join(dist, names.sha256), "sha");
  await writeFile(
    join(dist, "artifacts-macos-aarch64.json"),
    JSON.stringify([
      { filename: names.updater, target: "darwin-aarch64-app" },
      { filename: names.zip, target: null },
      { filename: names.sha256, target: null },
    ]),
  );
  const notes = join(directory, "notes.md");
  await writeFile(notes, "# Monoptah Native 0.34.0\n\nFirst release.\n");
  const env = {
    ...process.env,
    MONOCODE_NATIVE_VERSION: "0.34.0",
    TAURI_SIGNING_PRIVATE_KEY: await readFile(keyFile, "utf8"),
    TAURI_SIGNING_PRIVATE_KEY_PASSWORD: "",
  };
  const files = await prepareMonoptahRelease(dist, notes, env);
  assert.deepEqual(files, [
    names.updater,
    `${names.updater}.sig`,
    names.zip,
    names.sha256,
  ]);
  assert.deepEqual(
    JSON.parse(await readFile(join(dist, "release-files.json"), "utf8")),
    files,
  );

  const envelope = JSON.parse(await readFile(join(dist, "latest.json"), "utf8"));
  const payload = Buffer.from(envelope.payload, "base64");
  assert.ok(minisignVerify(publicKey, envelope.signature, payload));
  assert.ok(!minisignVerify(publicKey, envelope.signature, Buffer.from("other")));
  const parsed = JSON.parse(payload.toString("utf8"));
  assert.equal(parsed.version, "0.34.0");
  assert.equal(parsed.notes, "# Monoptah Native 0.34.0\n\nFirst release.\n");
  const platform = parsed.platforms["darwin-aarch64-app"];
  assert.equal(
    platform.signature,
    (await readFile(join(dist, `${names.updater}.sig`), "utf8")).trim(),
  );
  assert.ok(minisignVerify(publicKey, platform.signature, archive));
  assert.ok(!minisignVerify(publicKey, platform.signature, Buffer.from("tampered")));

  // Stale notes and a missing key stop the release before anything is published.
  await writeFile(notes, "# Monoptah Native 0.33.0\n");
  await assert.rejects(prepareMonoptahRelease(dist, notes, env), /must start with/);
  await assert.rejects(
    prepareMonoptahRelease(dist, notes, { ...env, TAURI_SIGNING_PRIVATE_KEY: "" }),
    /TAURI_SIGNING_PRIVATE_KEY is required/,
  );
  await assert.rejects(
    prepareMonoptahRelease(dist, notes, {
      ...env,
      MONOCODE_NATIVE_VERSION: "0.34.0-preview.1",
    }),
    /plain native release version/,
  );
});
