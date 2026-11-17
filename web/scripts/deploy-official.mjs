/* global console, process */
// Deploys the official Monoptah web host. Maintainer only, from their Mac (docs/deploy-runbook.md).
//
//   pnpm deploy:official [--dry-run]
//
// Preflight, in this order, each refusal exits non-zero with a message:
//   1. CI is not set (never from CI)           2. macOS (a real run only; --dry-run works anywhere)
//   3. clean tree (untracked files count)      4. branch is main
//   5. main equals origin/main (after fetch)   6. wrangler.jsonc has no placeholder database_id or VAPID key
//   7. `pnpm check` passes in web/             8. scripts/check-fork-branding.sh passes
// Then: build the app, apply D1 migrations (FIRST), wrangler deploy, record the deploy in audit_log, and print
// the line to append to the deploy log of the runbook. --dry-run runs every preflight check for real (git and
// checks) and prints the plan instead of running any remote command.
//
// The script never reads or prints a Cloudflare token: wrangler uses the OAuth login of this machine.
import { spawnSync } from "node:child_process";
import { randomUUID } from "node:crypto";
import { existsSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
export const DATABASE_NAME = "monoptah";

export class Refusal extends Error {}

/** Default command runner. `stream` shows the output live (long steps); otherwise it is captured. */
export function defaultRun(cmd, args, { cwd, stream = false } = {}) {
  const r = spawnSync(cmd, args, { cwd, encoding: "utf8", stdio: stream ? "inherit" : "pipe" });
  return { status: r.status ?? 1, stdout: r.stdout ?? "", stderr: r.stderr ?? "" };
}

export function parseArgs(argv) {
  const out = { dryRun: false };
  for (const a of argv) {
    if (a === "--dry-run") out.dryRun = true;
    else throw new Error(`unknown argument: ${a}\nusage: pnpm deploy:official [--dry-run]`);
  }
  return out;
}

function git(run, root, args) {
  const r = run("git", args, { cwd: root });
  if (r.status !== 0) throw new Refusal(`git ${args.join(" ")} failed: ${r.stderr.trim() || r.stdout.trim()}`);
  return r.stdout.trim();
}

/** Throws Refusal on the first failed check; returns `{ commit }` when all pass. */
export function preflight({ root, env, platform, dryRun, run, log = () => {} }) {
  const webDir = join(root, "web");
  if (env.CI) throw new Refusal("CI is set: the official host is never deployed from CI");
  if (!dryRun && platform !== "darwin") throw new Refusal(`not macOS (${platform}): deploy from the maintainer Mac`);

  const dirty = git(run, root, ["status", "--porcelain"]);
  if (dirty) throw new Refusal(`working tree is not clean (untracked files count):\n${dirty}`);

  const branch = git(run, root, ["rev-parse", "--abbrev-ref", "HEAD"]);
  if (branch !== "main") throw new Refusal(`on branch "${branch}", not main`);

  git(run, root, ["fetch", "origin", "main"]);
  const head = git(run, root, ["rev-parse", "HEAD"]);
  const remote = git(run, root, ["rev-parse", "origin/main"]);
  if (head !== remote) {
    const [ahead, behind] = git(run, root, ["rev-list", "--left-right", "--count", "HEAD...origin/main"]).split(/\s+/);
    throw new Refusal(`main differs from origin/main (${ahead} ahead, ${behind} behind)`);
  }

  const cfgPath = join(webDir, "wrangler.jsonc");
  const cfg = existsSync(cfgPath) ? readFileSync(cfgPath, "utf8") : "";
  if (/"database_id"\s*:\s*"0{8}-0{4}-0{4}-0{4}-0{12}"/.test(cfg))
    throw new Refusal("web/wrangler.jsonc still has the placeholder database_id (all zeros)");
  if (/REPLACE_WITH/.test(cfg)) throw new Refusal("web/wrangler.jsonc still has a REPLACE_WITH placeholder (VAPID_PUBLIC_KEY)");

  log("preflight: running pnpm check in web/ ...");
  if (run("pnpm", ["check"], { cwd: webDir, stream: true }).status !== 0) throw new Refusal("`pnpm check` failed");

  log("preflight: running scripts/check-fork-branding.sh ...");
  if (run("./scripts/check-fork-branding.sh", [], { cwd: root, stream: true }).status !== 0)
    throw new Refusal("scripts/check-fork-branding.sh failed");

  return { commit: head };
}

/** The ordered remote steps. Migrations come before the deploy. */
export function buildPlan({ webDir, commit, version, now = Math.floor(Date.now() / 1000), id = randomUUID() }) {
  const wrangler = join(webDir, "worker", "node_modules", ".bin", "wrangler");
  const cfg = join(webDir, "wrangler.jsonc");
  const meta = JSON.stringify({ commit, version });
  const sql =
    "INSERT INTO audit_log (id, ts, actor_user_id, actor_kind, action, target_type, target_id, meta) " +
    `VALUES ('${id}', ${now}, NULL, 'system', 'deploy', NULL, NULL, '${meta}');`;
  return [
    { name: "build", cmd: "pnpm", args: ["--filter", "@monoptah/app", "build"] },
    { name: "migrate", cmd: wrangler, args: ["d1", "migrations", "apply", DATABASE_NAME, "--remote", "-c", cfg] },
    { name: "deploy", cmd: wrangler, args: ["deploy", "-c", cfg] },
    { name: "audit", cmd: wrangler, args: ["d1", "execute", DATABASE_NAME, "--remote", "-c", cfg, "--command", sql] },
  ];
}

export function runbookLine({ date, commit, operator, version }) {
  return `| ${date} | ${commit} | ${version} | ${operator} |`;
}

export function main(argv, { root = join(here, "..", ".."), env = process.env, platform = process.platform, run = defaultRun, log = console.log } = {}) {
  let opts;
  try {
    opts = parseArgs(argv);
  } catch (e) {
    console.error(`deploy:official: ${e.message}`);
    return 2;
  }
  let commit;
  try {
    ({ commit } = preflight({ root, env, platform, dryRun: opts.dryRun, run, log }));
  } catch (e) {
    if (!(e instanceof Refusal)) throw e;
    console.error(`deploy:official: refused: ${e.message}`);
    return 1;
  }

  const webDir = join(root, "web");
  const version = JSON.parse(readFileSync(join(webDir, "app", "package.json"), "utf8")).version;
  const plan = buildPlan({ webDir, commit, version });

  if (opts.dryRun) {
    log("deploy:official: preflight passed. Dry run: nothing is executed. Plan, in this order:");
    plan.forEach((s, i) => log(`  ${i + 1}. ${s.name}: ${s.cmd.replace(webDir + "/", "")} ${s.args.map((a) => (a.length > 60 ? "<sql>" : a)).join(" ")}`));
    return 0;
  }

  for (const step of plan) {
    log(`deploy:official: ${step.name} ...`);
    const r = run(step.cmd, step.args, { cwd: webDir, stream: true });
    if (r.status !== 0) {
      console.error(`deploy:official: step "${step.name}" failed (exit ${r.status}); stopping.`);
      if (step.name === "audit")
        console.error("The deploy itself succeeded. Record it by hand in audit_log and the runbook.");
      return 1;
    }
  }
  const who = run("whoami", [], { cwd: root }).stdout.trim() || "unknown";
  log("deploy:official: done. Append this line to the deploy log in web/docs/deploy-runbook.md:");
  log(runbookLine({ date: new Date().toISOString().slice(0, 10), commit, version, operator: who }));
  return 0;
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  process.exitCode = main(process.argv.slice(2));
}
