// Fork-local (Monoptah). Packaging runs against fake macOS tools; nothing is
// signed for real and no app is launched.
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { existsSync, lstatSync, symlinkSync } from "node:fs";
import { chmod, mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { delimiter, join } from "node:path";
import { test } from "node:test";
import {
  PLATFORMS,
  TARGETS,
  assertOutsideBundle,
  markNeverIndex,
  packageMonoptahNative,
  removeBuiltAppBundle,
  recordsFileName,
} from "./package-native-monoptah.mjs";
import { packageNative } from "./package-native.mjs";

const SHA = "836CA8AA6AB393A7E0356223551C62BED7D95E45";
const fakeTool = `#!/usr/bin/env node
const fs = require('node:fs');
const path = require('node:path');
const name = path.basename(process.argv[1]);
const args = process.argv.slice(2);
fs.appendFileSync(process.env.FAKE_TOOL_LOG, JSON.stringify([name, ...args]) + '\\n');
if (name === 'uname') console.log('Darwin');
if (name === 'lipo') console.log(process.env.FAKE_ARCH || 'arm64');
if (name === 'ditto') fs.writeFileSync(args.at(-1), 'zip bytes');
if (name === 'tar') {
  const plist = path.join(args[3], args[4], 'Contents/Info.plist');
  fs.writeFileSync(args[1], fs.existsSync(plist) ? fs.readFileSync(plist) : 'archive bytes');
}
if (name === 'codesign' && args[0] === '-dv') {
  process.stderr.write(process.env.FAKE_SIGNATURE || 'Identifier=com.monoptah.native\\n');
}
if (name === 'codesign' && args[0] === '-d' && args[1] === '-r-') {
  process.stderr.write(process.env.FAKE_REQUIREMENT ?? ('designated => identifier "com.monoptah.native" and certificate leaf = H"' + '${SHA.toLowerCase()}' + '"\\n'));
}
if (name === 'security') {
  process.stdout.write(process.env.FAKE_IDENTITIES ?? ('  1) ${SHA} "Monoptah Native Release Signing" (CSSMERR_TP_NOT_TRUSTED)\\n     1 identities found\\n'));
}
`;

async function setup(t, { binaryVersion = "0.34.0" } = {}) {
  const directory = await mkdtemp(join(tmpdir(), "monoptah packaging with spaces-"));
  t.after(() => rm(directory, { recursive: true, force: true }));
  const bin = join(directory, "tools");
  const release = join(directory, "target/aarch64-apple-darwin/release");
  await mkdir(bin);
  await mkdir(release, { recursive: true });
  const binary = join(release, "monocode-gpui");
  await writeFile(
    binary,
    `#!/bin/sh\nprintf 'Monoptah Native ${binaryVersion}\\n'\n`,
  );
  await chmod(binary, 0o755);
  for (const name of ["uname", "lipo", "ditto", "tar", "codesign", "security"]) {
    await writeFile(join(bin, name), fakeTool);
    await chmod(join(bin, name), 0o755);
  }
  const log = join(directory, "tools.log");
  await writeFile(log, "");
  const saved = { ...process.env };
  process.env.PATH = `${bin}${delimiter}${process.env.PATH}`;
  process.env.FAKE_TOOL_LOG = log;
  process.env.MONOPTAH_NATIVE_SIGNING_IDENTITY = SHA;
  delete process.env.MONOPTAH_NATIVE_SIGNING_KEYCHAIN;
  delete process.env.FAKE_IDENTITIES;
  delete process.env.FAKE_REQUIREMENT;
  t.after(() => {
    for (const key of ["PATH", "FAKE_TOOL_LOG", "FAKE_ARCH", "FAKE_SIGNATURE", "FAKE_IDENTITIES", "FAKE_REQUIREMENT", "MONOPTAH_NATIVE_SIGNING_IDENTITY", "MONOPTAH_NATIVE_SIGNING_KEYCHAIN"]) {
      if (saved[key] === undefined) delete process.env[key];
      else process.env[key] = saved[key];
    }
  });
  return { directory, binary, out: join(directory, "dist"), log };
}
const calls = async (log) =>
  (await readFile(log, "utf8")).trim().split("\n").filter(Boolean).map((l) => JSON.parse(l));

test(
  "packaging signs with the stable self-signed identity and emits the updater archive, zip and checksum",
  { skip: process.platform !== "darwin" },
  async (t) => {
    const { binary, out, log } = await setup(t);
    const result = await packageMonoptahNative({ binary, out, version: "0.34.0" });
    const zip = await readFile(join(out, "MonoptahNative_0.34.0_aarch64.zip"));
    const expected = createHash("sha256").update(zip).digest("hex");
    assert.equal(result.sha256, expected);
    assert.equal(
      await readFile(join(out, "MonoptahNative_0.34.0_aarch64.zip.sha256"), "utf8"),
      `${expected}  MonoptahNative_0.34.0_aarch64.zip\n`,
    );
    assert.deepEqual(
      JSON.parse(await readFile(join(out, "artifacts-macos-aarch64.json"), "utf8")),
      [
        { filename: "MonoptahNative_0.34.0_aarch64.app.tar.gz", target: "darwin-aarch64-app" },
        { filename: "MonoptahNative_0.34.0_aarch64.zip", target: null },
        { filename: "MonoptahNative_0.34.0_aarch64.zip.sha256", target: null },
      ],
    );
    const made = await calls(log);
    // Self-signed: the identity, no timestamp, no hardened runtime, no entitlements.
    const signed = made.find(([n, ...a]) => n === "codesign" && a[0] === "--force");
    assert.ok(signed, "bundle was signed");
    assert.deepEqual(signed.slice(1, 5), ["--force", "--sign", SHA, "--timestamp=none"]);
    assert.ok(!signed.includes("--options") && !signed.includes("--entitlements"));
    assert.ok(!signed.includes("--keychain"));
    assert.ok(made.some(([n, ...a]) => n === "codesign" && a[0] === "-d" && a[1] === "-r-"));
    assert.ok(made.some(([n, ...a]) => n === "codesign" && a[0] === "--verify" && a.includes("--deep") && a.includes("--strict")));
    assert.ok(!made.some(([n]) => n === "xcrun" || n === "spctl"), "no notarization");
    const tar = made.find(([n]) => n === "tar");
    assert.equal(tar.at(-1), "Monoptah Native.app");
    assert.ok(made.some(([n, ...a]) => n === "ditto" && a.includes("--keepParent")));
    // The fake tar copies Info.plist into the archive, so the bundle content is checked there.
    const plist = await readFile(join(out, "MonoptahNative_0.34.0_aarch64.app.tar.gz"), "utf8");
    assert.match(plist, /<string>com\.monoptah\.native<\/string>/);
    assert.match(plist, /<key>MonocodeNativeVersion<\/key><string>0\.34\.0<\/string>/);
    // The built bundle is removed after packaging; the binary and the marker stay.
    assert.ok(!existsSync(join(binary, "../Monoptah Native.app")));
    assert.ok(existsSync(binary));
    assert.ok(existsSync(join(binary, "../.metadata_never_index")));
  },
);

test(
  "packaging refuses preview versions, a mismatched binary and other architectures",
  { skip: process.platform !== "darwin" },
  async (t) => {
    const { binary, out } = await setup(t);
    await assert.rejects(
      packageMonoptahNative({ binary, out, version: "0.34.0-preview.1" }),
      /plain native release version/,
    );
    await assert.rejects(
      packageMonoptahNative({ binary, out, version: "0.34.1" }),
      /does not match 0\.34\.1/,
    );
    process.env.FAKE_ARCH = "x86_64";
    await assert.rejects(
      packageMonoptahNative({ binary, out, version: "0.34.0" }),
      /Expected arm64/,
    );
  },
);

test(
  "packaging fails without a signing identity and never falls back to ad-hoc",
  { skip: process.platform !== "darwin" },
  async (t) => {
    const { binary, out, log } = await setup(t);
    delete process.env.MONOPTAH_NATIVE_SIGNING_IDENTITY;
    await assert.rejects(
      packageMonoptahNative({ binary, out, version: "0.34.0" }),
      /MONOPTAH_NATIVE_SIGNING_IDENTITY is not set.*setup-native-signing-identity\.sh/s,
    );
    process.env.MONOPTAH_NATIVE_SIGNING_IDENTITY = SHA;
    process.env.FAKE_IDENTITIES = "     0 identities found\n";
    await assert.rejects(
      packageMonoptahNative({ binary, out, version: "0.34.0" }),
      /is not in the keychain search list.*setup-native-signing-identity\.sh/s,
    );
    const made = await calls(log);
    assert.ok(!made.some(([n]) => n === "codesign"), "nothing was signed");
    assert.ok(!existsSync(join(binary, "../Monoptah Native.app")));
  },
);

test(
  "packaging resolves the common name to its SHA-1 and passes a keychain to codesign",
  { skip: process.platform !== "darwin" },
  async (t) => {
    const { binary, out, log } = await setup(t);
    process.env.MONOPTAH_NATIVE_SIGNING_IDENTITY = "Monoptah Native Release Signing";
    process.env.MONOPTAH_NATIVE_SIGNING_KEYCHAIN = "/tmp/test.keychain-db";
    await packageMonoptahNative({ binary, out, version: "0.34.0" });
    const made = await calls(log);
    const signed = made.find(([n, ...a]) => n === "codesign" && a[0] === "--force");
    assert.deepEqual(signed.slice(1), [
      "--force", "--sign", SHA, "--timestamp=none",
      "--keychain", "/tmp/test.keychain-db", signed.at(-1),
    ]);
    const find = made.find(([n]) => n === "security");
    assert.deepEqual(find.slice(1), ["find-identity", "-p", "codesigning", "/tmp/test.keychain-db"]);
  },
);

test(
  "packaging fails when the common name matches more than one identity",
  { skip: process.platform !== "darwin" },
  async (t) => {
    const { binary, out, log } = await setup(t);
    process.env.MONOPTAH_NATIVE_SIGNING_IDENTITY = "Monoptah Native Release Signing";
    process.env.FAKE_IDENTITIES =
      `  1) ${SHA} "Monoptah Native Release Signing" (CSSMERR_TP_NOT_TRUSTED)\n  2) ${"2".repeat(40)} "Monoptah Native Release Signing" (CSSMERR_TP_NOT_TRUSTED)\n`;
    await assert.rejects(packageMonoptahNative({ binary, out, version: "0.34.0" }), /matches 2 identities/);
    assert.ok(!(await calls(log)).some(([n]) => n === "codesign"));
  },
);

test(
  "packaging fails when the result is ad-hoc or the requirement is not the stable certificate form",
  { skip: process.platform !== "darwin" },
  async (t) => {
    const { binary, out } = await setup(t);
    process.env.FAKE_SIGNATURE = "Signature=adhoc\n";
    await assert.rejects(packageMonoptahNative({ binary, out, version: "0.34.0" }), /ad-hoc signed/);
    assert.ok(!existsSync(join(binary, "../Monoptah Native.app")), "bundle removed after failure");
    delete process.env.FAKE_SIGNATURE;
    process.env.FAKE_REQUIREMENT = 'designated => cdhash H"8875c4604ec0c782ee2321079a2a9ea0af3c2a9a"\n';
    await assert.rejects(
      packageMonoptahNative({ binary, out, version: "0.34.0" }),
      /not exactly the expected stable form.*designated => cdhash/,
    );
    const other = "1".repeat(40);
    process.env.FAKE_REQUIREMENT = `designated => identifier "com.monoptah.native" and certificate leaf = H"${other}"\n`;
    await assert.rejects(packageMonoptahNative({ binary, out, version: "0.34.0" }), /not exactly the expected stable form/);
    process.env.FAKE_REQUIREMENT = `designated => identifier "com.monoptah.native" and certificate leaf = H"${SHA.toLowerCase()}" or cdhash H"${other}"\n`;
    await assert.rejects(packageMonoptahNative({ binary, out, version: "0.34.0" }), /not exactly the expected stable form/);
    process.env.FAKE_REQUIREMENT = 'designated => identifier "com.other" and certificate leaf = H"' + SHA.toLowerCase() + '"\n';
    await assert.rejects(packageMonoptahNative({ binary, out, version: "0.34.0" }), /not exactly the expected stable form/);
    assert.ok(!existsSync(join(binary, "../Monoptah Native.app")), "bundle removed after failure");
  },
);

// Linux and Windows are packaged by upstream's package-native.mjs. Run it for
// real (only the external packagers are faked) with a plain X.Y.Z version and
// check that what it writes is what PLATFORMS and the release workflow expect.
const fakePackager = `#!/usr/bin/env node
const fs = require('node:fs');
const path = require('node:path');
const name = path.basename(process.argv[1]);
const args = process.argv.slice(2);
if (name === 'dpkg-deb') fs.writeFileSync(args.at(-1), 'deb bytes');
if (name === 'appimagetool') fs.writeFileSync(args[1], 'appimage bytes');
if (name === 'makensis') {
  const script = fs.readFileSync(args[0], 'utf8');
  fs.writeFileSync(/^OutFile "(.*)"$/m.exec(script)[1], 'installer bytes');
}
if (name === 'rpmbuild') {
  const top = args.find((a) => a.startsWith('_topdir ')).slice(8);
  fs.mkdirSync(path.join(top, 'RPMS/x86_64'), { recursive: true });
  fs.writeFileSync(path.join(top, 'RPMS/x86_64/monoptah-native-0.34.0-0.preview.0.el10.x86_64.rpm'), 'rpm bytes');
}
`;

test(
  "upstream packaging of linux, rpm and windows matches the platform table",
  { skip: process.platform === "win32" },
  async (t) => {
    const directory = await mkdtemp(join(tmpdir(), "monoptah packaging linux-"));
    t.after(() => rm(directory, { recursive: true, force: true }));
    const bin = join(directory, "tools");
    await mkdir(bin);
    for (const name of ["dpkg-deb", "appimagetool", "makensis", "rpmbuild"]) {
      await writeFile(join(bin, name), fakePackager);
      await chmod(join(bin, name), 0o755);
    }
    const binary = join(directory, "monocode-gpui");
    await writeFile(binary, "#!/bin/sh\nprintf 'Monoptah Native 0.34.0\\n'\n");
    await chmod(binary, 0o755);
    const saved = { PATH: process.env.PATH, APPIMAGETOOL: process.env.APPIMAGETOOL };
    process.env.PATH = `${bin}${delimiter}${process.env.PATH}`;
    process.env.APPIMAGETOOL = join(bin, "appimagetool");
    t.after(() => {
      process.env.PATH = saved.PATH;
      if (saved.APPIMAGETOOL === undefined) delete process.env.APPIMAGETOOL;
      else process.env.APPIMAGETOOL = saved.APPIMAGETOOL;
    });
    const out = join(directory, "dist");
    for (const kind of ["linux", "rpm", "windows"])
      await packageNative({ kind, binary, out, version: "0.34.0", arch: "x86_64" });

    for (const platform of PLATFORMS.filter((p) => p.kind !== "macos")) {
      const records = JSON.parse(await readFile(join(out, recordsFileName(platform)), "utf8"));
      const expected = platform.assets("0.34.0");
      assert.equal(records.length, expected.length, platform.kind);
      for (const { pattern, target } of expected) {
        const record = records.find((r) => pattern.test(r.filename));
        assert.ok(record, `${platform.kind}: ${pattern}`);
        assert.equal(record.target, target);
        assert.ok((await readFile(join(out, record.filename), "utf8")).length > 0);
      }
    }
    // The installed layout and marker are Monoptah's, and the client accepts them.
    const nsi = await readFile(join(out, "work/installer.nsi"), "utf8");
    assert.match(nsi, /Name "Monoptah Native"/);
    assert.match(nsi, /VIProductVersion "0\.34\.0\.0"/);
    assert.match(
      await readFile(join(out, "work/windows/native-install.json"), "utf8"),
      /"app_id":"com\.monoptah\.native","kind":"nsis"/,
    );
    assert.match(
      await readFile(join(out, "work/deb/usr/share/applications/monoptah-native.desktop"), "utf8"),
      /^Name=Monoptah Native$/m,
    );
  },
);

test("PLATFORMS names every package job once", () => {
  assert.deepEqual(PLATFORMS.map((p) => p.kind), ["macos", "linux", "rpm", "windows"]);
  assert.deepEqual(PLATFORMS.map((p) => recordsFileName(p)), [
    "artifacts-macos-aarch64.json",
    "artifacts-linux-x86_64.json",
    "artifacts-rpm-x86_64.json",
    "artifacts-windows-x86_64.json",
  ]);
  assert.deepEqual(
    PLATFORMS.flatMap((p) => p.assets("1.2.3").map((a) => a.target)).filter(Boolean),
    TARGETS,
  );
});

async function releaseDir(t) {
  const directory = await mkdtemp(join(tmpdir(), "monoptah-bundle-removal-"));
  t.after(() => rm(directory, { recursive: true, force: true }));
  const release = join(directory, "target/aarch64-apple-darwin/release");
  await mkdir(join(release, "deps"), { recursive: true });
  await writeFile(join(release, "monocode-gpui"), "bin");
  return { directory, release };
}

test("removeBuiltAppBundle removes only the bundle and logs it", async (t) => {
  const { release } = await releaseDir(t);
  const app = join(release, "Monoptah Native.app");
  await mkdir(join(app, "Contents"), { recursive: true });
  const logs = [];
  assert.equal(removeBuiltAppBundle(app, (l) => logs.push(l)), true);
  assert.ok(!existsSync(app));
  assert.ok(existsSync(join(release, "monocode-gpui")) && existsSync(join(release, "deps")));
  assert.deepEqual(logs, [`Removed built app bundle: ${app}`]);
  assert.equal(removeBuiltAppBundle(app, (l) => logs.push(l)), false);
  assert.equal(logs.length, 1);
});

test("removeBuiltAppBundle refuses other names, other parents and symlinks", async (t) => {
  const { directory, release } = await releaseDir(t);
  const other = join(release, "Other.app");
  await mkdir(other);
  assert.throws(() => removeBuiltAppBundle(other, () => {}), /not named/);
  assert.ok(existsSync(other));
  const wrongParent = join(directory, "target/debug/Monoptah Native.app");
  await mkdir(wrongParent, { recursive: true });
  assert.throws(() => removeBuiltAppBundle(wrongParent, () => {}), /release directory/);
  assert.ok(existsSync(wrongParent));
  const precious = join(directory, "precious");
  await mkdir(precious);
  await writeFile(join(precious, "keep"), "x");
  const link = join(release, "Monoptah Native.app");
  symlinkSync(precious, link);
  assert.throws(() => removeBuiltAppBundle(link, () => {}), /not a real directory/);
  assert.ok(lstatSync(link).isSymbolicLink());
  assert.ok(existsSync(join(precious, "keep")));
});

test("markNeverIndex creates the marker once and is idempotent", async (t) => {
  const { release } = await releaseDir(t);
  const dir = join(release, "fresh");
  markNeverIndex(dir);
  markNeverIndex(dir);
  assert.equal(await readFile(join(dir, ".metadata_never_index"), "utf8"), "");
});

test("the output directory may not be inside the built bundle", () => {
  const binary = "/x/target/aarch64-apple-darwin/release/monocode-gpui";
  const app = "/x/target/aarch64-apple-darwin/release/Monoptah Native.app";
  assert.throws(() => assertOutsideBundle(app, binary), /must not be inside/);
  assert.throws(() => assertOutsideBundle(`${app}/Contents/out`, binary), /must not be inside/);
  assert.doesNotThrow(() => assertOutsideBundle(`${app}-out`, binary));
  assert.doesNotThrow(() => assertOutsideBundle("/tmp/out", binary));
});
