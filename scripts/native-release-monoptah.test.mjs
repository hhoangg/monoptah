// Fork-local (Monoptah). Tests the build plan and the signed feed for the macOS
// arm64, Linux x86_64 and Windows x86_64 native builds.
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
  planBuilds,
  prepareMonoptahRelease,
  requireReused,
  writeMissingRecords,
} from "./native-release-monoptah.mjs";
import {
  PLATFORMS,
  RELEASE_FILE_COUNT,
  TARGETS,
  artifactRecords,
  assetNames,
  releaseVersion,
} from "./package-native-monoptah.mjs";

// What each package job writes, as upstream's package-native.mjs and
// packageMonoptahNative produce them (the rpm dist tag comes from the container).
const RPM = "monoptah-native-0.34.0-0.preview.0.el10.x86_64.rpm";
const records = {
  "artifacts-macos-aarch64.json": artifactRecords("0.34.0"),
  "artifacts-linux-x86_64.json": [
    { filename: "monoptah-native_0.34.0_amd64.deb", target: "linux-x86_64-deb" },
    { filename: "MonoptahNative_0.34.0_amd64.AppImage", target: "linux-x86_64-appimage" },
  ],
  "artifacts-rpm-x86_64.json": [{ filename: RPM, target: "linux-x86_64-rpm" }],
  "artifacts-windows-x86_64.json": [
    { filename: "MonoptahNative_0.34.0_x64-setup.exe", target: "windows-x86_64-nsis" },
  ],
};
const artifacts = Object.values(records)
  .flat()
  .map((record) => (record.target ? { ...record, signature: `signed-${record.target}` } : record));
const macArtifact = artifacts[0];
function assetFiles() {
  return Object.values(records)
    .flat()
    .map((r) => r.filename);
}
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

test("the manifest lists every target on the versioned release URL", () => {
  const result = manifest();
  assert.equal(result.app_id, "com.monoptah.native");
  assert.equal(result.channel, "native-preview");
  assert.equal(result.version, "0.34.0");
  assert.deepEqual(Object.keys(result.platforms).sort(), [...TARGETS].sort());
  const base = "https://github.com/hhoangg/monoptah/releases/download/native-v0.34.0/";
  assert.deepEqual(result.platforms["darwin-aarch64-app"], {
    url: `${base}MonoptahNative_0.34.0_aarch64.app.tar.gz`,
    signature: "signed-darwin-aarch64-app",
  });
  assert.equal(result.platforms["linux-x86_64-appimage"].url, `${base}MonoptahNative_0.34.0_amd64.AppImage`);
  assert.equal(result.platforms["linux-x86_64-deb"].url, `${base}monoptah-native_0.34.0_amd64.deb`);
  assert.equal(result.platforms["linux-x86_64-rpm"].url, `${base}${RPM}`);
  assert.equal(result.platforms["windows-x86_64-nsis"].url, `${base}MonoptahNative_0.34.0_x64-setup.exe`);
});

test("the platform table covers exactly the feed targets and the release file count", () => {
  const targets = PLATFORMS.flatMap((p) => p.assets("0.34.0").map((a) => a.target)).filter(Boolean);
  assert.deepEqual([...targets].sort(), [...TARGETS].sort());
  // Each updater package plus its .sig, plus the files that are not signed.
  const unsigned = PLATFORMS.flatMap((p) => p.assets("0.34.0")).filter((a) => !a.target).length;
  assert.equal(targets.length * 2 + unsigned, RELEASE_FILE_COUNT);
  // The sample records above name every asset the patterns expect.
  for (const platform of PLATFORMS) {
    const file = `artifacts-${platform.kind}-${platform.arch}.json`;
    for (const { pattern } of platform.assets("0.34.0"))
      assert.ok(records[file].some((r) => pattern.test(r.filename)), `${file}: ${pattern}`);
  }
});

test("the build plan skips platforms whose files are all published", () => {
  const all = [...assetFiles(), "MonoptahNative_0.34.0_aarch64.app.tar.gz.sig"];
  const kinds = (plan) => plan.matrix.map((e) => e.kind);
  // Fresh tag: everything is built, the rpm in its own job.
  const fresh = planBuilds("0.34.0", []);
  assert.deepEqual(kinds(fresh), ["macos", "linux", "windows"]);
  assert.deepEqual(fresh.matrix[0], {
    kind: "macos",
    os: "macos-latest",
    target: "aarch64-apple-darwin",
    arch: "aarch64",
  });
  assert.equal(fresh.rpm, true);
  assert.equal(fresh.buildAny, true);
  assert.deepEqual(fresh.reuse, []);
  // Release published by the previous macOS-only workflow: only the new
  // platforms are built and the mac files are reused as they are.
  const macOnly = planBuilds("0.34.0", all.filter((n) => n.startsWith("MonoptahNative_0.34.0_aarch64")));
  assert.deepEqual(kinds(macOnly), ["linux", "windows"]);
  assert.equal(macOnly.rpm, true);
  assert.deepEqual(macOnly.reuse, [
    "MonoptahNative_0.34.0_aarch64.app.tar.gz",
    "MonoptahNative_0.34.0_aarch64.zip",
    "MonoptahNative_0.34.0_aarch64.zip.sha256",
  ]);
  // A fully published release builds nothing and reuses the real rpm name.
  const done = planBuilds("0.34.0", all);
  assert.deepEqual(done.matrix, []);
  assert.equal(done.rpm, false);
  assert.equal(done.buildAny, false);
  assert.ok(done.reuse.includes(RPM));
  assert.equal(done.reuse.length, 7);
  // A platform with only some of its files published is refused, naming it.
  assert.throws(
    () => planBuilds("0.34.0", ["MonoptahNative_0.34.0_aarch64.zip"]),
    /macos files on release native-v0\.34\.0 are incomplete.*Delete every published macos asset/s,
  );
  assert.throws(
    () => planBuilds("0.34.0", ["MonoptahNative_0.34.0_amd64.AppImage", ...done.reuse.filter((n) => n.endsWith(".exe") || n.endsWith(".rpm") || n.startsWith("MonoptahNative_0.34.0_aarch64"))]),
    /linux files/,
  );
  // Files of another version never count.
  assert.deepEqual(planBuilds("0.34.1", all).reuse, []);
});

test("the manifest refuses other targets, missing or duplicate packages, unsigned packages and traversal", () => {
  assert.throws(() => manifest({ artifacts: [] }), /Missing native package/);
  // Any single missing target is refused, not just an empty list.
  for (const target of TARGETS)
    assert.throws(
      () => manifest({ artifacts: artifacts.filter((a) => a.target !== target) }),
      new RegExp(`Missing native package: ${target}`),
    );
  assert.throws(
    () =>
      manifest({
        artifacts: [...artifacts, { ...macArtifact, target: "darwin-x86_64-app" }],
      }),
    /unexpected native target/,
  );
  assert.throws(
    () => manifest({ artifacts: [...artifacts, macArtifact] }),
    /Duplicate/,
  );
  assert.throws(
    () => manifest({ artifacts: [{ ...macArtifact, signature: " " }, ...artifacts.slice(1)] }),
    /Missing signature/,
  );
  for (const filename of ["../latest.json", "directory\\latest.json"])
    assert.throws(
      () => manifest({ artifacts: [{ ...macArtifact, filename }, ...artifacts.slice(1)] }),
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

test("the plan command prints step output lines", async (t) => {
  const directory = await mkdtemp(join(tmpdir(), "monoptah-native-plan-"));
  t.after(() => rm(directory, { recursive: true, force: true }));
  const published = join(directory, "published.txt");
  const macFiles = assetFiles().filter((n) => n.startsWith("MonoptahNative_0.34.0_aarch64"));
  await writeFile(published, `${[...macFiles, ...assetFiles().filter((n) => n.endsWith(".exe"))].join("\n")}\n`);
  const script = join(root, "scripts/native-release-monoptah.mjs");
  const run = (args) =>
    spawnSync(process.execPath, [script, ...args], {
      encoding: "utf8",
      env: { ...process.env, MONOCODE_NATIVE_VERSION: "0.34.0" },
    });
  const partial = run(["plan", published]);
  assert.equal(partial.status, 0, partial.stderr);
  const lines = Object.fromEntries(
    partial.stdout.trim().split("\n").map((l) => [l.slice(0, l.indexOf("=")), l.slice(l.indexOf("=") + 1)]),
  );
  assert.deepEqual(JSON.parse(lines.matrix).map((e) => e.kind), ["linux"]);
  assert.equal(lines.rpm, "true");
  assert.equal(lines.build_any, "true");
  assert.equal(
    lines.reuse,
    [...macFiles, "MonoptahNative_0.34.0_x64-setup.exe"].join(" "),
  );
  // The macOS files are not on the release: macOS is never built on CI.
  await writeFile(published, "MonoptahNative_0.34.0_x64-setup.exe\n");
  const noMac = run(["plan", published]);
  assert.notEqual(noMac.status, 0);
  assert.equal(noMac.stdout, "");
  assert.match(noMac.stderr, /macos files are missing from release native-v0\.34\.0.*npm run native:release:mac -- 0\.34\.0/s);
  // No release listing at all fails the same way.
  const none = run(["plan", join(directory, "none.txt")]);
  assert.notEqual(none.status, 0);
  assert.match(none.stderr, /native:release:mac/);
  // A partially published platform makes the plan command fail.
  await writeFile(published, "MonoptahNative_0.34.0_aarch64.zip\n");
  const bad = run(["plan", published]);
  assert.notEqual(bad.status, 0);
  assert.match(bad.stderr, /incomplete or ambiguous/);
  assert.equal(run(["file-count"]).stdout.trim(), String(RELEASE_FILE_COUNT));
});

test("the records command writes metadata only for reused platforms", async (t) => {
  const directory = await mkdtemp(join(tmpdir(), "monoptah-native-records-"));
  t.after(() => rm(directory, { recursive: true, force: true }));
  const script = join(root, "scripts/native-release-monoptah.mjs");
  const run = (version) =>
    spawnSync(process.execPath, [script, "records", directory], {
      encoding: "utf8",
      env: { ...process.env, MONOCODE_NATIVE_VERSION: version },
    });
  // Only the mac and the rpm files are in the directory (reused).
  for (const name of [...artifactRecords("0.34.0").map((r) => r.filename), RPM])
    await writeFile(join(directory, name), "bytes");
  // The Windows build ran: its metadata exists and must not be rewritten.
  await writeFile(join(directory, "artifacts-windows-x86_64.json"), "[]");
  const result = run("0.34.0");
  assert.equal(result.status, 0, result.stderr);
  assert.deepEqual(
    JSON.parse(await readFile(join(directory, "artifacts-macos-aarch64.json"), "utf8")),
    artifactRecords("0.34.0"),
  );
  assert.deepEqual(
    JSON.parse(await readFile(join(directory, "artifacts-rpm-x86_64.json"), "utf8")),
    records["artifacts-rpm-x86_64.json"],
  );
  assert.equal(await readFile(join(directory, "artifacts-windows-x86_64.json"), "utf8"), "[]");
  // Linux files are absent, so nothing is invented for it.
  assert.ok(!existsSync(join(directory, "artifacts-linux-x86_64.json")));
  assert.deepEqual(await writeMissingRecords(directory, "0.34.0"), []);
  assert.notEqual(run("0.34.0-preview.1").status, 0);
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
  for (const [file, list] of Object.entries(records)) {
    await writeFile(join(dist, file), JSON.stringify(list));
    for (const record of list)
      await writeFile(
        join(dist, record.filename),
        record.filename === names.updater ? archive : `bytes of ${record.filename}`,
      );
  }
  const notes = join(directory, "notes.md");
  await writeFile(notes, "# Monoptah Native 0.34.0\n\nFirst release.\n");
  const env = {
    ...process.env,
    MONOCODE_NATIVE_VERSION: "0.34.0",
    TAURI_SIGNING_PRIVATE_KEY: await readFile(keyFile, "utf8"),
    TAURI_SIGNING_PRIVATE_KEY_PASSWORD: "",
  };
  const files = await prepareMonoptahRelease(dist, notes, env);
  assert.equal(files.length, RELEASE_FILE_COUNT);
  assert.deepEqual(files, [
    "monoptah-native_0.34.0_amd64.deb",
    "monoptah-native_0.34.0_amd64.deb.sig",
    "MonoptahNative_0.34.0_amd64.AppImage",
    "MonoptahNative_0.34.0_amd64.AppImage.sig",
    names.updater,
    `${names.updater}.sig`,
    names.zip,
    names.sha256,
    RPM,
    `${RPM}.sig`,
    "MonoptahNative_0.34.0_x64-setup.exe",
    "MonoptahNative_0.34.0_x64-setup.exe.sig",
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
  assert.deepEqual(Object.keys(parsed.platforms).sort(), [...TARGETS].sort());
  for (const [target, entry] of Object.entries(parsed.platforms)) {
    const filename = decodeURIComponent(entry.url.split("/").at(-1));
    assert.ok(
      minisignVerify(publicKey, entry.signature, await readFile(join(dist, filename))),
      target,
    );
  }
  const platform = parsed.platforms["darwin-aarch64-app"];
  assert.equal(
    platform.signature,
    (await readFile(join(dist, `${names.updater}.sig`), "utf8")).trim(),
  );
  assert.ok(minisignVerify(publicKey, platform.signature, archive));
  assert.ok(!minisignVerify(publicKey, platform.signature, Buffer.from("tampered")));

  // A package under the wrong target or another version is never signed.
  await writeFile(
    join(dist, "artifacts-windows-x86_64.json"),
    JSON.stringify([{ filename: "MonoptahNative_0.33.0_x64-setup.exe", target: "windows-x86_64-nsis" }]),
  );
  await writeFile(join(dist, "MonoptahNative_0.33.0_x64-setup.exe"), "old");
  await assert.rejects(
    prepareMonoptahRelease(dist, notes, env),
    /Unexpected updater package for windows-x86_64-nsis/,
  );
  await writeFile(
    join(dist, "artifacts-windows-x86_64.json"),
    JSON.stringify(records["artifacts-windows-x86_64.json"]),
  );
  // A platform that never ran leaves the release incomplete.
  await rm(join(dist, "artifacts-rpm-x86_64.json"));
  await assert.rejects(
    prepareMonoptahRelease(dist, notes, env),
    /Expected 12 release files, found 10/,
  );
  await writeFile(
    join(dist, "artifacts-rpm-x86_64.json"),
    JSON.stringify(records["artifacts-rpm-x86_64.json"]),
  );
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

test("requireReused refuses a plan that would build the platform", () => {
  const withMac = planBuilds("0.34.0", []);
  assert.throws(() => requireReused(withMac, "macos", "0.34.0"), /npm run native:release:mac -- 0\.34\.0/);
  const macPublished = planBuilds("0.34.0", assetFiles().filter((n) => n.startsWith("MonoptahNative_0.34.0_aarch64")));
  assert.equal(requireReused(macPublished, "macos", "0.34.0"), macPublished);
  // Other platforms may still be built.
  assert.ok(macPublished.buildAny);
});
