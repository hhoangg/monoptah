// Builds the latest.json that the in-app updater downloads, and refuses to
// write it unless every signature really verifies against the public key
// shipped in tauri.conf.json. A manifest with bad signatures would only fail
// later on a user's machine, so the check happens here, in the release job.
//
// Usage:
//   node scripts/make-update-manifest.mjs <version> <tag> <notes-file> <out-file> \
//     <platform[,platform...]>=<artifact-path>...
// Each artifact needs a "<artifact-path>.sig" next to it.
import { createHash, createPublicKey, verify } from "node:crypto";
import { readFileSync, writeFileSync } from "node:fs";
import { basename } from "node:path";
import { fileURLToPath } from "node:url";

// The only place updates may come from. tauri.conf.json must point at the
// same repository, which is asserted below.
export const REPO = "hhoangg/monoptah";
const CONF_PATH = fileURLToPath(new URL("../src-tauri/tauri.conf.json", import.meta.url));

// Wraps a raw 32-byte Ed25519 public key into the DER form node expects.
const ED25519_SPKI_PREFIX = Buffer.from("302a300506032b6570032100", "hex");

function fail(message) {
  throw new Error(message);
}

// Tauri stores minisign files base64-encoded as a whole, so decode once to get
// the two-line text and then the payload line.
function decodeMinisignFile(base64Text) {
  const text = Buffer.from(base64Text.trim(), "base64").toString("utf8");
  return { lines: text.split("\n").map((line) => line.replace(/\r$/, "")) };
}

export function parsePublicKey(base64Text) {
  const { lines } = decodeMinisignFile(base64Text);
  const raw = Buffer.from(lines[1] ?? "", "base64");
  if (raw.length !== 42) fail(`public key payload is ${raw.length} bytes, expected 42`);
  if (raw.subarray(0, 2).toString("latin1") !== "Ed") fail("public key has an unsupported algorithm");
  return { keyId: raw.subarray(2, 10), key: raw.subarray(10, 42) };
}

export function parseSignature(base64Text) {
  const { lines } = decodeMinisignFile(base64Text);
  const first = Buffer.from(lines[1] ?? "", "base64");
  const global = Buffer.from(lines[3] ?? "", "base64");
  const trustedLine = lines[2] ?? "";
  if (first.length !== 74 || global.length !== 64 || !trustedLine.startsWith("trusted comment: ")) {
    fail("signature file is not a valid minisign signature");
  }
  // The updater plugin only accepts pre-hashed ("ED") signatures.
  if (first.subarray(0, 2).toString("latin1") !== "ED") fail("signature is not pre-hashed (ED)");
  return {
    keyId: first.subarray(2, 10),
    signature: first.subarray(10, 74),
    trustedComment: Buffer.from(trustedLine.slice("trusted comment: ".length), "utf8"),
    globalSignature: global,
  };
}

function ed25519Verify(publicKey, message, signature) {
  const key = createPublicKey({
    key: Buffer.concat([ED25519_SPKI_PREFIX, publicKey.key]),
    format: "der",
    type: "spki",
  });
  return verify(null, message, key, signature);
}

// Mirrors what the updater plugin does at install time (minisign-verify).
export function verifyArtifact(publicKey, signatureText, artifactBytes, name) {
  const sig = parseSignature(signatureText);
  if (!sig.keyId.equals(publicKey.keyId)) {
    fail(`${name}: signed with a different key than the public key in tauri.conf.json`);
  }
  const digest = createHash("blake2b512").update(artifactBytes).digest();
  if (!ed25519Verify(publicKey, digest, sig.signature)) fail(`${name}: file signature does not verify`);
  const global = Buffer.concat([sig.signature, sig.trustedComment]);
  if (!ed25519Verify(publicKey, global, sig.globalSignature)) {
    fail(`${name}: trusted comment signature does not verify`);
  }
}

export function buildManifest({ version, tag, notes, pubDate, conf, entries }) {
  if (!/^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$/.test(version)) fail(`invalid version: ${version}`);
  if (tag !== `v${version}`) fail(`tag ${tag} does not match version ${version}`);
  if (conf.version !== version) fail(`tauri.conf.json version ${conf.version} does not match ${version}`);

  const updater = conf.plugins?.updater ?? {};
  const expectedEndpoint = `https://github.com/${REPO}/releases/latest/download/latest.json`;
  if (JSON.stringify(updater.endpoints) !== JSON.stringify([expectedEndpoint])) {
    fail(`tauri.conf.json updater endpoints must be exactly [${expectedEndpoint}]`);
  }
  const publicKey = parsePublicKey(updater.pubkey ?? "");

  const base = `https://github.com/${REPO}/releases/download/${tag}`;
  const platforms = {};
  for (const { platforms: keys, name, bytes, signatureText } of entries) {
    verifyArtifact(publicKey, signatureText, bytes, name);
    const asset = {
      signature: signatureText.trim(),
      url: `${base}/${encodeURIComponent(name)}`,
    };
    for (const key of keys) {
      if (platforms[key]) fail(`duplicate platform key: ${key}`);
      platforms[key] = asset;
    }
  }
  return { version, notes, pub_date: pubDate, platforms };
}

function main(argv) {
  const [version, tag, notesFile, outFile, ...specs] = argv;
  if (!version || !tag || !notesFile || !outFile || specs.length === 0) {
    fail("usage: make-update-manifest.mjs <version> <tag> <notes-file> <out-file> <platforms>=<artifact>...");
  }
  const entries = specs.map((spec) => {
    const split = spec.indexOf("=");
    if (split < 1) fail(`bad artifact spec: ${spec}`);
    const path = spec.slice(split + 1);
    return {
      platforms: spec.slice(0, split).split(","),
      name: basename(path),
      bytes: readFileSync(path),
      signatureText: readFileSync(`${path}.sig`, "utf8"),
    };
  });
  const manifest = buildManifest({
    version,
    tag,
    notes: readFileSync(notesFile, "utf8").trim(),
    pubDate: new Date().toISOString().replace(/\.\d{3}Z$/, "Z"),
    conf: JSON.parse(readFileSync(CONF_PATH, "utf8")),
    entries,
  });
  writeFileSync(outFile, `${JSON.stringify(manifest, null, 2)}\n`);
  console.log(`Wrote ${outFile} for ${Object.keys(manifest.platforms).join(", ")}`);
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  try {
    main(process.argv.slice(2));
  } catch (error) {
    console.error(error instanceof Error ? error.message : error);
    process.exit(1);
  }
}
