/* global console, process */
// Creates the first-admin `bootstrap` invite (architecture.md section 9 step 11).
//
//   pnpm bootstrap -- --local [--persist-to DIR] [--origin URL]
//   pnpm bootstrap -- --remote [--origin URL]
//   pnpm --filter @monoptah/worker bootstrap -- --remote
//
// The invite is one use, valid for 24 hours, kind 'bootstrap' (created_by NULL, which the schema allows only
// for this kind). The code is 16 random bytes, base64url (128 bit). Only its hash is stored:
//
//   code_hash = lowercase hex SHA-256 of the UTF-8 bytes of the code string
//
// The registration code path hashes the submitted invite code the same way. The code is printed once, with the
// link <RP_ORIGIN>/register#invite=CODE, and is never written to a file, to the SQL or to a log.
// An explicit --local or --remote is required (no default). --remote is refused when CI is set.
import { createHash, randomBytes, randomUUID } from "node:crypto";
import { spawnSync } from "node:child_process";
import { existsSync, readFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const webDir = join(here, "..");
const WRANGLER_CONFIG = join(webDir, "wrangler.jsonc");
export const INVITE_TTL_SECONDS = 24 * 60 * 60;
export const DATABASE_NAME = "monoptah";

export function usage() {
  return "usage: pnpm bootstrap -- (--local [--persist-to DIR] | --remote) [--origin URL]";
}

/** Parses argv (without node and script). Throws an Error with a user-facing message. */
export function parseArgs(argv) {
  const out = { mode: null, persistTo: null, origin: null };
  const args = argv[0] === "--" ? argv.slice(1) : argv;
  for (let i = 0; i < args.length; i++) {
    const a = args[i];
    if (a === "--local" || a === "--remote") {
      const mode = a.slice(2);
      if (out.mode && out.mode !== mode) throw new Error("pass only one of --local and --remote");
      out.mode = mode;
    } else if (a === "--persist-to" || a === "--origin") {
      const v = args[++i];
      if (!v || v.startsWith("--")) throw new Error(`${a} needs a value`);
      if (a === "--persist-to") out.persistTo = v;
      else out.origin = v;
    } else {
      throw new Error(`unknown argument: ${a}`);
    }
  }
  if (!out.mode) throw new Error("pass --local or --remote explicitly (there is no default)");
  if (out.persistTo && out.mode !== "local") throw new Error("--persist-to only applies to --local");
  return out;
}

/** Throws when a remote run is attempted from CI. */
export function assertNotCi(opts, env) {
  if (opts.mode === "remote" && env.CI) throw new Error("refusing --remote when CI is set: bootstrap runs from an operator machine");
}

export function newInviteCode() {
  return randomBytes(16).toString("base64url");
}

export function hashInviteCode(code) {
  return createHash("sha256").update(code, "utf8").digest("hex");
}

/** SQL for the invite row. Contains the hash only, never the code. `nowSeconds` is unix seconds. */
export function buildInviteSql(codeHash, nowSeconds, id = randomUUID()) {
  if (!/^[0-9a-f]{64}$/.test(codeHash)) throw new Error("code hash must be 64 lowercase hex characters");
  if (!/^[0-9a-f-]{36}$/.test(id)) throw new Error("invite id must be a UUID");
  const expires = nowSeconds + INVITE_TTL_SECONDS;
  return (
    "INSERT INTO invites (id, code_hash, created_by, kind, expires_at, created_at) " +
    `VALUES ('${id}', '${codeHash}', NULL, 'bootstrap', ${expires}, ${nowSeconds});`
  );
}

/** Reads RP_ORIGIN from the `vars` of wrangler.jsonc (JSONC: comments and trailing commas allowed). */
export function readRpOrigin(jsonc) {
  const m = /"RP_ORIGIN"\s*:\s*"([^"]+)"/.exec(jsonc);
  if (!m) throw new Error("RP_ORIGIN not found in wrangler.jsonc vars; pass --origin");
  return m[1];
}

export function registerLink(origin, code) {
  return `${origin.replace(/\/+$/, "")}/register#invite=${code}`;
}

function wranglerBin() {
  const bin = join(webDir, "worker", "node_modules", ".bin", process.platform === "win32" ? "wrangler.cmd" : "wrangler");
  if (!existsSync(bin)) throw new Error("wrangler not found: run `pnpm install` in web/ first");
  return bin;
}

/** Arguments for `wrangler d1 execute`. */
export function wranglerArgs(opts, sql) {
  const args = ["d1", "execute", DATABASE_NAME, `--${opts.mode}`, "-c", WRANGLER_CONFIG, "--command", sql];
  if (opts.persistTo) args.push("--persist-to", resolve(opts.persistTo));
  return args;
}

export function main(argv, env = process.env) {
  let opts;
  try {
    opts = parseArgs(argv);
    assertNotCi(opts, env);
  } catch (e) {
    console.error(`bootstrap: ${e.message}\n${usage()}`);
    return 2;
  }
  const origin = opts.origin ?? readRpOrigin(readFileSync(WRANGLER_CONFIG, "utf8"));
  const code = newInviteCode();
  const sql = buildInviteSql(hashInviteCode(code), Math.floor(Date.now() / 1000));
  const r = spawnSync(wranglerBin(), wranglerArgs(opts, sql), { cwd: webDir, encoding: "utf8", env });
  if (r.status !== 0) {
    // wrangler output holds no code (only the hash is in the SQL), so it is safe to show.
    console.error(r.stdout, r.stderr);
    console.error("bootstrap: wrangler failed; no invite code is shown because the row may not exist");
    return 1;
  }
  console.log("Bootstrap invite created (one use, valid for 24 hours). Shown once, it is not stored anywhere:");
  console.log(`  ${registerLink(origin, code)}`);
  return 0;
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  process.exitCode = main(process.argv.slice(2));
}
