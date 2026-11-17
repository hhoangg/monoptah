import assert from "node:assert/strict";
import { test } from "node:test";
import { findSecretHits, parseSessionCookie, redact } from "./api-smoke-lib.mjs";

test("parseSessionCookie reads a set cookie", () => {
  const lines = ["other=1; Path=/", "__Host-mc_session=abc123; HttpOnly; Secure; Path=/; Max-Age=100"];
  assert.deepEqual(parseSessionCookie(lines), { token: "abc123" });
});

test("parseSessionCookie reads a cleared cookie", () => {
  assert.deepEqual(parseSessionCookie(["__Host-mc_session=; Max-Age=0; Path=/"]), { token: null });
  assert.deepEqual(parseSessionCookie(["__Host-mc_session=x; Expires=Thu, 01 Jan 1970 00:00:00 GMT"]), { token: null });
});

test("parseSessionCookie ignores other cookies", () => {
  assert.equal(parseSessionCookie(["a=b"]), undefined);
  assert.equal(parseSessionCookie([]), undefined);
});

test("findSecretHits reports labels only and skips short values", () => {
  const secrets = [["invite", "0123456789abcdef"], ["short", "abc"], ["absent", "zzzzzzzzzzzzzzzz"]];
  assert.deepEqual(findSecretHits("log 0123456789abcdef abc", secrets), ["invite"]);
});

test("redact removes secrets", () => {
  assert.equal(redact("x 0123456789abcdef y", [["a", "0123456789abcdef"]]), "x [redacted] y");
});
