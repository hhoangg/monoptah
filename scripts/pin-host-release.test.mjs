// Fork-local (Monoptah). Tests scripts/pin-host-release.mjs with `gh` mocked: no network.
import assert from "node:assert/strict";
import { mkdirSync, readdirSync, writeFileSync } from "node:fs";
import { mkdir, mkdtemp, readFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import {
  TARGETS,
  crossCheck,
  compareCore,
  parseTag,
  pinnedVersion,
  pinHostRelease,
  renderHostRelease,
  validateManifest,
} from "./pin-host-release.mjs";

const hash = (n) => String(n).repeat(64).slice(0, 64);
const assets = Object.fromEntries(TARGETS.map((t, i) => [t, hash(i + 1)]));
const manifest = { version: "2.3.4+monoptah", assets };
const ext = (t) => (t.startsWith("win32") ? "zip" : "tar.gz");
const sums = (overrides = {}) =>
  Object.fromEntries(
    TARGETS.map((t) => [
      `monocode-host-${t}.${ext(t)}.sha256`,
      `${overrides[t] ?? assets[t]}  monocode-host-${t}.${ext(t)}\n`,
    ]),
  );

// A fake gh: serves the manifest from a run artifact (or the release) and the sums.
function fakeGh({ runs = [{ databaseId: 77 }], artifact = true, files = sums(), attest = 0, manifestJson = manifest } = {}) {
  const calls = [];
  const gh = (args) => {
    calls.push(args.join(" "));
    const out = (status = 0, stdout = "", stderr = "") => ({ status, stdout, stderr });
    const dir = args[args.indexOf("-D") + 1];
    if (args[0] === "run" && args[1] === "list") return out(0, JSON.stringify(runs));
    if (args[0] === "run" && args[1] === "download") {
      if (!artifact) return out(1, "", "no artifact");
      return syncPut(dir, "host-release.json", JSON.stringify(manifestJson));
    }
    if (args[0] === "attestation") return out(attest, "", attest ? "no attestation" : "");
    if (args[0] === "release" && args[1] === "download") {
      if (args.includes("host-release.json"))
        return syncPut(dir, "host-release.json", JSON.stringify(manifestJson));
      for (const [name, text] of Object.entries(files)) syncPut(dir, name, text);
      return out();
    }
    return out(1, "", `unexpected ${args.join(" ")}`);
  };
  gh.calls = calls;
  return gh;
}
function syncPut(dir, name, text) {
  mkdirSync(dir, { recursive: true });
  writeFileSync(join(dir, name), text);
  return { status: 0, stdout: "", stderr: "" };
}

async function root() {
  const dir = await mkdtemp(join(tmpdir(), "pin-host-test-"));
  await mkdir(join(dir, "crates/monocode-core/src/remote"), { recursive: true });
  return dir;
}
const target = (dir) => join(dir, "crates/monocode-core/src/remote/host_release.rs");

test("parses the tag", () => {
  assert.equal(parseTag("host-v1.2.3"), "1.2.3");
  for (const bad of ["v1.2.3", "host-v1.2", "host-v1.2.3+monoptah", "host-v1.2.3-rc.1", undefined])
    assert.throws(() => parseTag(bad), /Invalid Host tag/);
});

test("validates the manifest against the tag", () => {
  assert.ok(validateManifest(manifest, "host-v2.3.4"));
  assert.throws(() => validateManifest(manifest, "host-v2.3.5"), /does not match/);
  assert.throws(
    () => validateManifest({ ...manifest, assets: { ...assets, "linux-x64": "zz" } }, "host-v2.3.4"),
    /bad sha256/,
  );
  const { "win32-x64": _drop, ...fewer } = assets;
  assert.throws(() => validateManifest({ ...manifest, assets: fewer }, "host-v2.3.4"), /targets are/);
});

test("cross-check accepts matching hashes and rejects a different one", () => {
  crossCheck(assets, sums());
  assert.throws(() => crossCheck(assets, sums({ "linux-x64": hash(9) })), /Hash mismatch for linux-x64/);
  const missing = sums();
  delete missing["monocode-host-win32-x64.zip.sha256"];
  assert.throws(() => crossCheck(assets, missing), /win32-x64/);
});

test("renders the Rust file with the version, base URL and every hash", () => {
  const text = renderHostRelease("host-v2.3.4", manifest);
  assert.match(text, /HOST_VERSION: &str = "2\.3\.4\+monoptah"/);
  assert.match(text, /hhoangg\/monoptah\/releases\/download\/host-v2\.3\.4"/);
  for (const t of TARGETS) assert.ok(text.includes(`"${t}",\n        "${assets[t]}"`));
});

test("pins from the run artifact after verifying the attestation", async () => {
  const dir = await root();
  const gh = fakeGh();
  try {
    const result = await pinHostRelease("host-v2.3.4", { root: dir, gh });
    assert.equal(result.source, "run 77");
    assert.match(await readFile(target(dir), "utf8"), /2\.3\.4\+monoptah/);
    const order = gh.calls.map((c) => c.split(" ").slice(0, 2).join(" "));
    assert.ok(order.indexOf("attestation verify") > order.indexOf("run download"));
    assert.ok(order.indexOf("attestation verify") < order.lastIndexOf("release download"));
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});

test("falls back to the release asset when there is no run artifact", async () => {
  const dir = await root();
  try {
    const result = await pinHostRelease("host-v2.3.4", {
      root: dir,
      gh: fakeGh({ runs: [], artifact: false }),
    });
    assert.equal(result.source, "release host-v2.3.4");
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});

test("refuses and writes nothing on attestation failure or a hash mismatch", async () => {
  for (const [options, message] of [
    [{ attest: 1 }, /attestation verify/],
    [{ files: sums({ "darwin-x64": hash(9) }) }, /Hash mismatch/],
  ]) {
    const dir = await root();
    try {
      await assert.rejects(pinHostRelease("host-v2.3.4", { root: dir, gh: fakeGh(options) }), message);
      await assert.rejects(readFile(target(dir), "utf8"), /ENOENT/);
    } finally {
      await rm(dir, { recursive: true, force: true });
    }
  }
});

test("verifies the attestation against the signer workflow, tag and runner kind", async () => {
  const dir = await root();
  const gh = fakeGh();
  try {
    await pinHostRelease("host-v2.3.4", { root: dir, gh });
    const verify = gh.calls.find((c) => c.startsWith("attestation verify"));
    assert.match(verify, / -R hhoangg\/monoptah /);
    assert.ok(
      verify.includes(
        "--signer-workflow hhoangg/monoptah/.github/workflows/host-release-monoptah.yml",
      ),
    );
    assert.ok(verify.includes("--source-ref refs/tags/host-v2.3.4"));
    assert.ok(verify.endsWith("--deny-self-hosted-runners"));
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});

test("refuses a downgrade, allows a re-pin and leaves no temporary file", async () => {
  const dir = await root();
  try {
    await pinHostRelease("host-v2.3.4", { root: dir, gh: fakeGh() });
    assert.deepEqual(pinnedVersion(await readFile(target(dir), "utf8")), [2, 3, 4]);
    const before = await readFile(target(dir), "utf8");
    // A lower version is refused before any network call.
    const gh = fakeGh({ manifestJson: { version: "2.3.3+monoptah", assets } });
    await assert.rejects(pinHostRelease("host-v2.3.3", { root: dir, gh }), /downgrade/);
    assert.deepEqual(gh.calls, []);
    assert.equal(await readFile(target(dir), "utf8"), before);
    // The same version is a re-pin.
    const again = await pinHostRelease("host-v2.3.4", { root: dir, gh: fakeGh() });
    assert.equal(again.repinned, true);
    // A higher one moves the pin.
    const higher = { version: "2.4.0+monoptah", assets };
    const up = await pinHostRelease("host-v2.4.0", { root: dir, gh: fakeGh({ manifestJson: higher }) });
    assert.equal(up.repinned, false);
    assert.deepEqual(pinnedVersion(await readFile(target(dir), "utf8")), [2, 4, 0]);
    assert.deepEqual(readdirSync(join(dir, "crates/monocode-core/src/remote")), ["host_release.rs"]);
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});

test("compares the pinned core with a plain version", () => {
  assert.equal(compareCore([1, 1, 0], "1.1.0"), 0);
  assert.equal(compareCore([1, 10, 0], "1.9.0"), 1);
  assert.equal(compareCore([1, 1, 0], "1.1.1"), -1);
  assert.equal(pinnedVersion("nothing"), null);
});
