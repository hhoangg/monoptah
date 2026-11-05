// Fork-local (Monoptah). Packaging runs against fake macOS tools; nothing is
// signed for real and no app is launched.
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { chmod, mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { delimiter, join } from "node:path";
import { test } from "node:test";
import {
  PLATFORMS,
  TARGETS,
  packageMonoptahNative,
  recordsFileName,
} from "./package-native-monoptah.mjs";
import { packageNative } from "./package-native.mjs";

const fakeTool = `#!/usr/bin/env node
const fs = require('node:fs');
const path = require('node:path');
const name = path.basename(process.argv[1]);
const args = process.argv.slice(2);
fs.appendFileSync(process.env.FAKE_TOOL_LOG, JSON.stringify([name, ...args]) + '\\n');
if (name === 'uname') console.log('Darwin');
if (name === 'lipo') console.log(process.env.FAKE_ARCH || 'arm64');
if (name === 'ditto') fs.writeFileSync(args.at(-1), 'zip bytes');
if (name === 'tar') fs.writeFileSync(args[1], 'archive bytes');
if (name === 'codesign' && args[0] === '-dv') {
  process.stderr.write(process.env.FAKE_SIGNATURE || 'Signature=adhoc\\n');
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
  for (const name of ["uname", "lipo", "ditto", "tar", "codesign"]) {
    await writeFile(join(bin, name), fakeTool);
    await chmod(join(bin, name), 0o755);
  }
  const log = join(directory, "tools.log");
  await writeFile(log, "");
  const saved = { ...process.env };
  process.env.PATH = `${bin}${delimiter}${process.env.PATH}`;
  process.env.FAKE_TOOL_LOG = log;
  t.after(() => {
    for (const key of ["PATH", "FAKE_TOOL_LOG", "FAKE_ARCH", "FAKE_SIGNATURE"]) {
      if (saved[key] === undefined) delete process.env[key];
      else process.env[key] = saved[key];
    }
  });
  return { directory, binary, out: join(directory, "dist"), log };
}
const calls = async (log) =>
  (await readFile(log, "utf8")).trim().split("\n").filter(Boolean).map((l) => JSON.parse(l));

test(
  "packaging signs ad-hoc and emits the updater archive, zip and checksum",
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
    // Ad-hoc: the bundle script signs with "-" when no identity is given.
    assert.ok(made.some(([n, ...a]) => n === "codesign" && a.join(" ").startsWith("--force --sign - ")));
    assert.ok(made.some(([n, ...a]) => n === "codesign" && a[0] === "--verify" && a.includes("--deep") && a.includes("--strict")));
    assert.ok(!made.some(([n]) => n === "xcrun" || n === "spctl"), "no notarization");
    const tar = made.find(([n]) => n === "tar");
    assert.equal(tar.at(-1), "Monoptah Native.app");
    assert.ok(made.some(([n, ...a]) => n === "ditto" && a.includes("--keepParent")));
    const plist = await readFile(join(binary, "../Monoptah Native.app/Contents/Info.plist"), "utf8");
    assert.match(plist, /<string>com\.monoptah\.native<\/string>/);
    assert.match(plist, /<key>MonocodeNativeVersion<\/key><string>0\.34\.0<\/string>/);
  },
);

test(
  "packaging refuses preview versions, a mismatched binary, other architectures and non-adhoc signatures",
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
    process.env.FAKE_ARCH = "arm64";
    process.env.FAKE_SIGNATURE = "Authority=Developer ID Application: Example\n";
    await assert.rejects(
      packageMonoptahNative({ binary, out, version: "0.34.0" }),
      /not ad-hoc signed/,
    );
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
