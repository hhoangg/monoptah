// Fork-local (Monoptah). Packaging runs against fake macOS tools; nothing is
// signed for real and no app is launched.
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { chmod, mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { delimiter, join } from "node:path";
import { test } from "node:test";
import { packageMonoptahNative } from "./package-native-monoptah.mjs";

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
