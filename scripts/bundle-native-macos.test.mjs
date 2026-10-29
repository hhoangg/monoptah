import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { access, chmod, mkdir, mkdtemp, readdir, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { delimiter, join } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("../", import.meta.url));

test("Finder launches use development data for dev bundles and installed data for release bundles", {
  skip: process.platform === "win32",
}, async (t) => {
  const directory = await mkdtemp(join(tmpdir(), "monocode-bundle-data-"));
  t.after(() => rm(directory, { recursive: true, force: true }));
  const bin = join(directory, "bin");
  const target = join(directory, "target with spaces");
  await mkdir(bin);
  // Exercise packaging without signing or opening an app.
  for (const [name, script] of [
    ["uname", "#!/bin/sh\nprintf 'Darwin\\n'\n"],
    ["codesign", "#!/bin/sh\nexit 0\n"],
  ]) {
    await writeFile(join(bin, name), script);
    await chmod(join(bin, name), 0o755);
  }
  for (const [profile, override, mode] of [
    ["native-dev", undefined, "dev-data"],
    ["release", undefined, "monocode-data"],
    ["release", "dev-data", "dev-data"],
    ["native-dev", "monocode-data", "monocode-data"],
  ]) {
    await t.test(`${profile}: ${override ?? "default"}`, async () => {
      const output = join(target, profile);
      const devDir = join(directory, "dev apps", profile, String(override));
      const dev = profile !== "release";
      await mkdir(output, { recursive: true });
      const executable = join(output, "monocode-gpui");
      await writeFile(executable, "#!/bin/sh\nprintf '%s\\n' \"$@\"\n");
      await chmod(executable, 0o755);
      const args = ["scripts/bundle-native-macos.sh", profile, target, "0.16.0"];
      if (override) args.push(override);
      const packaged = spawnSync("bash", args, {
        cwd: root,
        env: { ...process.env, MONOCODE_DEV_APP_DIR: devDir, PATH: `${bin}${delimiter}${process.env.PATH}` },
        encoding: "utf8",
      });
      assert.ifError(packaged.error);
      assert.equal(packaged.status, 0, packaged.stdout + packaged.stderr);
      const bundle = dev
        ? join(devDir, "Monoptah Native Dev.app/Contents")
        : join(output, "Monoptah Native.app/Contents");
      const name = dev ? "Monoptah Native Dev" : "Monoptah Native";
      const id = dev ? "com.monoptah.native.dev" : "com.monoptah.native";
      const plist = await readFile(join(bundle, "Info.plist"), "utf8");
      assert.ok(plist.includes(`<key>MonocodeDataMode</key><string>${mode}</string>`));
      assert.ok(plist.includes(`<key>CFBundleIdentifier</key><string>${id}</string>`));
      assert.ok(plist.includes(`<key>CFBundleName</key><string>${name}</string>`));
      assert.ok(plist.includes(`<key>CFBundleDisplayName</key><string>${name}</string>`));
      // macOS rejects menu bar items from a process that exec'd after launch.
      assert.ok(plist.includes("<key>CFBundleExecutable</key><string>monocode-gpui</string>"));
      await assert.rejects(access(join(bundle, "MacOS/monocode-launcher")));
      const launched = spawnSync(join(bundle, "MacOS/monocode-gpui"), ["--cwd", "a project with spaces"], {
        encoding: "utf8",
      });
      assert.ifError(launched.error);
      assert.equal(launched.status, 0, launched.stderr);
      assert.deepEqual(launched.stdout.trim().split("\n"), ["--cwd", "a project with spaces"]);
    });
  }
});

async function setup(t, codesign = "#!/bin/sh\nexit 0\n") {
  const directory = await mkdtemp(join(tmpdir(), "monocode-bundle-dev-"));
  t.after(() => rm(directory, { recursive: true, force: true }));
  const bin = join(directory, "bin");
  const target = join(directory, "target");
  await mkdir(bin);
  for (const [name, script] of [
    ["uname", "#!/bin/sh\nprintf 'Darwin\\n'\n"],
    ["codesign", codesign],
  ]) {
    await writeFile(join(bin, name), script);
    await chmod(join(bin, name), 0o755);
  }
  for (const profile of ["native-dev", "release"]) {
    await mkdir(join(target, profile), { recursive: true });
    await writeFile(join(target, profile, "monocode-gpui"), "#!/bin/sh\n");
    await chmod(join(target, profile, "monocode-gpui"), 0o755);
  }
  const devDir = join(directory, "Applications");
  const run = (profile, env = {}) => spawnSync("bash", ["scripts/bundle-native-macos.sh", profile, target, "0.16.0"], {
    cwd: root,
    env: Object.fromEntries(Object.entries({
      ...process.env, MONOCODE_DEV_APP_DIR: devDir, PATH: `${bin}${delimiter}${process.env.PATH}`, ...env,
    }).filter(([, value]) => value !== undefined)),
    encoding: "utf8",
  });
  return { directory, target, devDir, run };
}

test("dev bundling twice leaves exactly one dev bundle and prints its path", { skip: process.platform === "win32" }, async (t) => {
  const f = await setup(t);
  for (let i = 0; i < 2; i++) {
    if (i === 1) await writeFile(join(f.devDir, "Monoptah Native Dev.app/stale"), "");
    const result = f.run("native-dev");
    assert.equal(result.status, 0, result.stdout + result.stderr);
    assert.ok(result.stdout.includes(`App: ${join(f.devDir, "Monoptah Native Dev.app")}\n`));
  }
  assert.deepEqual(await readdir(f.devDir), ["Monoptah Native Dev.app"]);
  await assert.rejects(access(join(f.devDir, "Monoptah Native Dev.app/stale")));
  await assert.rejects(access(join(f.target, "native-dev/Monoptah Native.app")));
});

test("dev bundling removes a stale legacy bundle from the target directory", { skip: process.platform === "win32" }, async (t) => {
  const f = await setup(t);
  const legacy = join(f.target, "native-dev/Monoptah Native.app/Contents");
  await mkdir(legacy, { recursive: true });
  const result = f.run("native-dev");
  assert.equal(result.status, 0, result.stdout + result.stderr);
  await assert.rejects(access(join(f.target, "native-dev/Monoptah Native.app")));
});

test("release bundling keeps the production name and location and never touches the dev dir", { skip: process.platform === "win32" }, async (t) => {
  const f = await setup(t);
  const result = f.run("release");
  assert.equal(result.status, 0, result.stdout + result.stderr);
  assert.ok(result.stdout.includes(`App: ${join(f.target, "release/Monoptah Native.app")}\n`));
  await assert.rejects(access(f.devDir));
});

test("dev bundling refuses to run without a safe destination", { skip: process.platform === "win32" }, async (t) => {
  const f = await setup(t);
  const result = f.run("native-dev", { MONOCODE_DEV_APP_DIR: "relative/dir" });
  assert.notEqual(result.status, 0);
  assert.ok(result.stderr.includes("unsafe"));
});

test("a failing signing step keeps the previous dev bundle and leaves no temp dir", { skip: process.platform === "win32" }, async (t) => {
  const f = await setup(t, "#!/bin/sh\nexit 1\n");
  const old = join(f.devDir, "Monoptah Native Dev.app");
  await mkdir(old, { recursive: true });
  await writeFile(join(old, "previous"), "keep");
  const result = f.run("native-dev");
  assert.notEqual(result.status, 0);
  assert.equal(await readFile(join(old, "previous"), "utf8"), "keep");
  assert.deepEqual(await readdir(f.devDir), ["Monoptah Native Dev.app"]);
});

test("a failing copy step keeps the previous dev bundle and leaves no temp dir", { skip: process.platform === "win32" }, async (t) => {
  const f = await setup(t);
  const old = join(f.devDir, "Monoptah Native Dev.app");
  await mkdir(old, { recursive: true });
  await writeFile(join(old, "previous"), "keep");
  // A directory where the executable should be makes cp fail after staging began.
  await rm(join(f.target, "native-dev/monocode-gpui"));
  await mkdir(join(f.target, "native-dev/monocode-gpui"));
  const result = f.run("native-dev");
  assert.notEqual(result.status, 0);
  assert.equal(await readFile(join(old, "previous"), "utf8"), "keep");
  assert.deepEqual(await readdir(f.devDir), ["Monoptah Native Dev.app"]);
});

test("the dev bundle falls back to $HOME/Applications", { skip: process.platform === "win32" }, async (t) => {
  const f = await setup(t);
  const home = join(f.directory, "home");
  const result = f.run("native-dev", { MONOCODE_DEV_APP_DIR: undefined, HOME: home });
  assert.equal(result.status, 0, result.stdout + result.stderr);
  assert.deepEqual(await readdir(join(home, "Applications")), ["Monoptah Native Dev.app"]);
  assert.ok(result.stdout.includes(`App: ${join(home, "Applications/Monoptah Native Dev.app")}\n`));
});

test("an unset or empty HOME with no override fails and removes nothing", { skip: process.platform === "win32" }, async (t) => {
  const f = await setup(t);
  const legacy = join(f.target, "native-dev/Monoptah Native.app");
  await mkdir(legacy, { recursive: true });
  for (const home of [undefined, ""]) {
    const result = f.run("native-dev", { MONOCODE_DEV_APP_DIR: undefined, HOME: home });
    assert.notEqual(result.status, 0);
    await access(legacy);
  }
});

test("a release run leaves an unrelated existing release bundle contents alone except for refreshing it", { skip: process.platform === "win32" }, async (t) => {
  const f = await setup(t);
  const dev = join(f.devDir, "Monoptah Native Dev.app");
  await mkdir(dev, { recursive: true });
  const legacyLike = join(f.target, "native-dev/Monoptah Native.app");
  await mkdir(legacyLike, { recursive: true });
  const result = f.run("release");
  assert.equal(result.status, 0, result.stdout + result.stderr);
  await access(dev);
  await access(legacyLike);
  await access(join(f.target, "release/Monoptah Native.app/Contents/Info.plist"));
});
