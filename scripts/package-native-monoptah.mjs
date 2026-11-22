// Fork-local (Monoptah). Packages a prebuilt macOS arm64 native executable as an
// app signed with the stable self-signed identity (see native-signing.mjs). Upstream's package-native.mjs requires a Developer ID and
// notarization for macOS; this fork has no Apple Developer account. Kept in its
// own file so upstream merges do not conflict. Never builds or launches the app.
//
// It also owns the asset naming for every platform the fork ships (the PLATFORMS
// table below). Linux and Windows are packaged by upstream's package-native.mjs
// (kinds linux, rpm, windows), which already accepts plain X.Y.Z versions and
// brands its files Monoptah; their names are only described here so the release
// script and the workflow can plan, sign and verify them.
//
// The built bundle is deleted after packaging; the returned `app` path no longer exists.
//
// usage: node scripts/package-native-monoptah.mjs <binary> <out-dir> [version]
// The version defaults to MONOCODE_NATIVE_VERSION.
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { lstatSync, mkdirSync, rmSync, writeFileSync } from "node:fs";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import { basename, dirname, join, resolve, sep } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import {
  SIGNING_IDENTITY_ENV,
  SIGNING_KEYCHAIN_ENV,
  signatureProblem,
  resolveSigningIdentity,
} from "./native-signing.mjs";

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

const escapeRegExp = (value) => value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
const exact = (name) => new RegExp(`^${escapeRegExp(name)}$`);

// Every feed target the fork publishes. The client looks the target up as
// <os>-<arch>-<install kind> (crates/monocode-core/src/app_update/mod.rs).
export const TARGETS = [
  "darwin-aarch64-app",
  "linux-x86_64-appimage",
  "linux-x86_64-deb",
  "linux-x86_64-rpm",
  "windows-x86_64-nsis",
];

// One entry per package job. `assets(version)` lists every file the job
// produces as a RegExp that must match the whole filename, plus the feed target
// of the updater package (null for files that are not signed or in the feed).
// The rpm name is only known after rpmbuild ran (the dist tag, such as el10,
// comes from the build container), hence the pattern.
export const PLATFORMS = [
  {
    kind: "macos",
    os: "macos-latest",
    rust: "aarch64-apple-darwin",
    arch: "aarch64",
    assets: (version) => {
      const names = assetNames(version);
      return [
        { pattern: exact(names.updater), target: "darwin-aarch64-app" },
        { pattern: exact(names.zip), target: null },
        { pattern: exact(names.sha256), target: null },
      ];
    },
  },
  {
    kind: "linux",
    os: "ubuntu-22.04",
    rust: "x86_64-unknown-linux-gnu",
    arch: "x86_64",
    assets: (version) => [
      {
        pattern: exact(`MonoptahNative_${version}_amd64.AppImage`),
        target: "linux-x86_64-appimage",
      },
      {
        pattern: exact(`monoptah-native_${version}_amd64.deb`),
        target: "linux-x86_64-deb",
      },
    ],
  },
  {
    kind: "rpm",
    os: "ubuntu-latest",
    rust: "x86_64-unknown-linux-gnu",
    arch: "x86_64",
    assets: (version) => [
      {
        pattern: new RegExp(
          `^monoptah-native-${escapeRegExp(version)}-[0-9A-Za-z._~+]+\\.x86_64\\.rpm$`,
        ),
        target: "linux-x86_64-rpm",
      },
    ],
  },
  {
    kind: "windows",
    os: "windows-latest",
    rust: "x86_64-pc-windows-msvc",
    arch: "x86_64",
    assets: (version) => [
      {
        pattern: exact(`MonoptahNative_${version}_x64-setup.exe`),
        target: "windows-x86_64-nsis",
      },
    ],
  },
];

// The metadata file package-native.mjs (and packageMonoptahNative) writes.
export const recordsFileName = (platform) =>
  `artifacts-${platform.kind}-${platform.arch}.json`;

// Every file a release carries: each platform's assets plus a .sig next to each
// updater package. Used to check the published file list.
export const RELEASE_FILE_COUNT = 12;

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

// Deletes a built bundle, and only that: the path must be a real directory named
// exactly "Monoptah Native.app" directly inside a "release" directory. A symlink
// is refused, never followed. Nothing else in target (the build cache) is touched.
// Returns true when something was removed, false when there was nothing to remove.
export function removeBuiltAppBundle(app, log = console.log) {
  const path = resolve(app);
  if (basename(path) !== APP_NAME)
    throw new Error(`Refusing to remove ${path}: not named ${APP_NAME}`);
  if (basename(dirname(path)) !== "release")
    throw new Error(`Refusing to remove ${path}: not inside a release directory`);
  let stat;
  try {
    stat = lstatSync(path);
  } catch (error) {
    if (error?.code === "ENOENT") return false;
    throw error;
  }
  if (stat.isSymbolicLink() || !stat.isDirectory())
    throw new Error(`Refusing to remove ${path}: not a real directory`);
  rmSync(path, { recursive: true, force: true });
  log(`Removed built app bundle: ${path}`);
  return true;
}

// Tells Spotlight not to index a build directory, so a bundle left there never
// shows up as an app. An empty marker file; harmless off macOS.
export function markNeverIndex(dir) {
  mkdirSync(dir, { recursive: true });
  const marker = join(dir, ".metadata_never_index");
  try {
    lstatSync(marker);
  } catch (error) {
    if (error?.code !== "ENOENT") throw error;
    writeFileSync(marker, "");
  }
}

// The artifacts must never live in the bundle that is deleted after packaging.
export function assertOutsideBundle(out, binary) {
  const app = join(dirname(resolve(binary)), APP_NAME);
  const dir = resolve(out);
  if (dir === app || dir.startsWith(app + sep))
    throw new Error(`The output directory must not be inside ${app}`);
}

export async function packageMonoptahNative({ binary, out, version }) {
  releaseVersion(version);
  binary = resolve(binary);
  out = resolve(out);
  assertOutsideBundle(out, binary);
  const embedded = capture(binary, ["--version"]);
  if (embedded !== `Monoptah Native ${version}`)
    throw new Error(`Native executable version does not match ${version}`);
  const arch = capture("lipo", ["-archs", binary]);
  if (arch !== "arm64") throw new Error(`Expected arm64, found ${arch}`);

  // Spotlight must not index anything under the release directory.
  markNeverIndex(dirname(binary));
  const app = join(dirname(binary), APP_NAME);
  // The built bundle is removed afterwards (also on failure): only the archives
  // in `out` are kept, and they are written completely before the removal.
  let result;
  try {
    // bundle-native-macos.sh writes the app next to the binary's profile directory
    // (<target>/release/Monoptah Native.app). It signs with the stable self-signed
    // identity (selfsigned mode); there is no ad-hoc fallback.
    const resolved = resolveSigningIdentity(process.env, (command, args) => {
      const r = spawnSync(command, args, { encoding: "utf8" });
      return { status: r.status ?? 1, stdout: r.stdout ?? "", stderr: r.stderr ?? "" };
    });
    if (resolved.problem) throw new Error(resolved.problem);
    const keychain = (process.env[SIGNING_KEYCHAIN_ENV] ?? "").trim();
    run(
      "bash",
      [
        "scripts/bundle-native-macos.sh",
        "release",
        dirname(dirname(binary)),
        version,
        "monocode-data",
      ],
      {
        env: {
          ...process.env,
          MONOCODE_NATIVE_SIGNING_MODE: "selfsigned",
          // The resolved SHA-1 pins the exact certificate, whatever name was configured.
          MONOCODE_NATIVE_SIGNING_IDENTITY: resolved.sha1,
          MONOCODE_NATIVE_SIGNING_KEYCHAIN: keychain,
        },
      },
    );
    run("codesign", ["--verify", "--deep", "--strict", "--verbose=2", app]);
    // codesign -d writes to stderr.
    const requirement = spawnSync("codesign", ["-d", "-r-", app], { encoding: "utf8" });
    if (requirement.status !== 0) throw new Error(`codesign -d -r- failed (${requirement.status})`);
    const requirementText = `${requirement.stdout ?? ""}${requirement.stderr ?? ""}`;
    console.log(`Designated requirement:\n${requirementText.trim()}`);
    const details = spawnSync("codesign", ["-dv", app], { encoding: "utf8" });
    const bad = signatureProblem(requirementText, details.stderr ?? "", resolved.sha1);
    if (bad) throw new Error(bad);

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
    result = { app, artifacts, sha256: digest };
  } catch (error) {
    // A cleanup problem must never hide the error that stopped the packaging.
    try {
      removeBuiltAppBundle(app);
    } catch (cleanup) {
      if (error instanceof Error)
        error.message += `\n(could not remove the built app bundle: ${cleanup.message})`;
    }
    throw error;
  }
  try {
    removeBuiltAppBundle(app);
  } catch (cleanup) {
    // The archives are complete and valid; only warn.
    console.warn(`Warning: could not remove the built app bundle: ${cleanup.message}`);
  }
  return result;
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
