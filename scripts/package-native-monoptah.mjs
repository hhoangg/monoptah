// Fork-local (Monoptah). Packages a prebuilt macOS arm64 native executable as an
// ad-hoc signed app. Upstream's package-native.mjs requires a Developer ID and
// notarization and only accepts X.Y.Z-preview.N versions; this fork has no Apple
// Developer account and releases plain X.Y.Z. Kept in its own file so upstream
// merges do not conflict. Never builds or launches the app.
//
// usage: node scripts/package-native-monoptah.mjs <binary> <out-dir> [version]
// The version defaults to MONOCODE_NATIVE_VERSION.
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import { basename, dirname, join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const root = fileURLToPath(new URL("../", import.meta.url));
const APP_NAME = "Monoptah Native.app";

// Plain release versions only. Pre-release forms such as 0.34.0-preview.1 are
// rejected on purpose: the cask, the sha256 asset and the rolling feed all
// assume a final X.Y.Z, and the cask bump script refuses anything else.
export function releaseVersion(version) {
  const match = /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/.exec(version ?? "");
  if (!match || match.slice(1).some((n) => Number(n) > 65535))
    throw new Error(
      `Expected a plain native release version such as 0.34.0, got: ${version}`,
    );
  return version;
}

export function assetNames(version) {
  releaseVersion(version);
  const stem = `MonoptahNative_${version}_aarch64`;
  return {
    updater: `${stem}.app.tar.gz`,
    zip: `${stem}.zip`,
    sha256: `${stem}.zip.sha256`,
  };
}

// The metadata the signing step reads. Only the updater archive has a target,
// because only it is signed and listed in the feed.
export function artifactRecords(version) {
  const names = assetNames(version);
  return [
    { filename: names.updater, target: "darwin-aarch64-app" },
    { filename: names.zip, target: null },
    { filename: names.sha256, target: null },
  ];
}

function run(command, args, options = {}) {
  const result = spawnSync(command, args, {
    cwd: root,
    stdio: "inherit",
    ...options,
  });
  if (result.error) throw result.error;
  if (result.status !== 0)
    throw new Error(`${command} ${args[0] ?? ""} failed (${result.status})`);
  return result;
}
function capture(command, args, options = {}) {
  return run(command, args, {
    encoding: "utf8",
    stdio: ["ignore", "pipe", "inherit"],
    ...options,
  }).stdout.trim();
}

export async function packageMonoptahNative({ binary, out, version }) {
  releaseVersion(version);
  binary = resolve(binary);
  out = resolve(out);
  const embedded = capture(binary, ["--version"]);
  if (embedded !== `Monoptah Native ${version}`)
    throw new Error(`Native executable version does not match ${version}`);
  const arch = capture("lipo", ["-archs", binary]);
  if (arch !== "arm64") throw new Error(`Expected arm64, found ${arch}`);

  // bundle-native-macos.sh writes the app next to the binary's profile directory
  // (<target>/release/Monoptah Native.app). An empty signing identity makes it
  // sign ad-hoc (`codesign --sign -`).
  run(
    "bash",
    [
      "scripts/bundle-native-macos.sh",
      "release",
      dirname(dirname(binary)),
      version,
      "monocode-data",
    ],
    { env: { ...process.env, MONOCODE_NATIVE_SIGNING_IDENTITY: "" } },
  );
  const app = join(dirname(binary), APP_NAME);
  run("codesign", ["--verify", "--deep", "--strict", "--verbose=2", app]);
  // codesign -dv writes to stderr.
  const details = spawnSync("codesign", ["-dv", app], { encoding: "utf8" });
  if (!/^Signature=adhoc$/m.test(details.stderr ?? ""))
    throw new Error("The packaged app is not ad-hoc signed");

  await mkdir(out, { recursive: true });
  const names = assetNames(version);
  // The updater archive must hold only files and directories under
  // "Monoptah Native.app/": the client refuses links and other roots.
  // COPYFILE_DISABLE keeps AppleDouble (._*) entries out of it.
  run("tar", ["-czf", join(out, names.updater), "-C", dirname(app), APP_NAME], {
    env: { ...process.env, COPYFILE_DISABLE: "1" },
  });
  // ditto keeps symlinks and extended attributes that plain zip can break
  // inside an .app bundle. The sha256 file is what the cask pins.
  run("ditto", [
    "-c",
    "-k",
    "--sequesterRsrc",
    "--keepParent",
    app,
    join(out, names.zip),
  ]);
  const digest = createHash("sha256")
    .update(await readFile(join(out, names.zip)))
    .digest("hex");
  await writeFile(join(out, names.sha256), `${digest}  ${names.zip}\n`);
  // Same metadata shape as upstream's artifacts-*.json.
  const artifacts = artifactRecords(version);
  await writeFile(
    join(out, "artifacts-macos-aarch64.json"),
    JSON.stringify(artifacts, null, 2),
  );
  return { app, artifacts, sha256: digest };
}

if (
  process.argv[1] &&
  import.meta.url === pathToFileURL(resolve(process.argv[1])).href
) {
  const [binary, out, version = process.env.MONOCODE_NATIVE_VERSION] =
    process.argv.slice(2);
  if (!binary || !out)
    throw new Error(
      `usage: ${basename(process.argv[1])} <binary> <out-dir> [version]`,
    );
  await packageMonoptahNative({ binary, out, version });
}
