/* global process */
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { dirname, join } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";
import { generateVapidKeys } from "./vapid-generate.mjs";

const script = join(dirname(fileURLToPath(import.meta.url)), "vapid-generate.mjs");
const BASE64URL = /^[A-Za-z0-9_-]+$/;

test("generateVapidKeys returns a 65-byte public point and a 32-byte scalar", async () => {
  const { publicKey, privateKey } = await generateVapidKeys();
  assert.match(publicKey, BASE64URL);
  assert.match(privateKey, BASE64URL);
  const point = Buffer.from(publicKey, "base64url");
  assert.equal(point.length, 65);
  assert.equal(point[0], 4);
  assert.equal(Buffer.from(privateKey, "base64url").length, 32);
});

test("two runs produce different pairs", async () => {
  const a = await generateVapidKeys();
  const b = await generateVapidKeys();
  assert.notEqual(a.privateKey, b.privateKey);
});

test("the script prints exactly the two assignments on stdout, with valid shapes", () => {
  const run = spawnSync(process.execPath, [script], { encoding: "utf8" });
  assert.equal(run.status, 0);
  const lines = run.stdout.trim().split("\n");
  assert.equal(lines.length, 2);
  const publicMatch = /^VAPID_PUBLIC_KEY=(\S+)$/.exec(lines[0] ?? "");
  const privateMatch = /^VAPID_PRIVATE_KEY=(\S+)$/.exec(lines[1] ?? "");
  assert.ok(publicMatch && privateMatch);
  assert.equal(Buffer.from(publicMatch[1], "base64url").length, 65);
  assert.equal(Buffer.from(privateMatch[1], "base64url").length, 32);
});
