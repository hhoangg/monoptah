// Daily cleanup trigger (wrangler.jsonc `triggers.crons`): runs `cleanup(env, now)` of every module.
import type { Env } from "./env.ts";
import { MODULES } from "./modules/index.ts";

/** Runs every module cleanup. One failing module does not stop the others; failures are rethrown together. */
export async function runCleanups(env: Env, now: number): Promise<void> {
  const failures: { module: string; error: unknown }[] = [];
  for (const [module, entry] of Object.entries(MODULES)) {
    try {
      await entry.cleanup(env, now);
    } catch (error) {
      // Metadata only (security.md 10): the module name and the error class, never a row.
      console.error(`cleanup failed: module=${module} error=${error instanceof Error ? error.name : "unknown"}`);
      failures.push({ module, error });
    }
  }
  if (failures.length > 0) {
    throw new AggregateError(
      failures.map((f) => f.error),
      `cleanup failed for: ${failures.map((f) => f.module).join(", ")}`,
    );
  }
}

/** The `scheduled` handler. `now` is the scheduled time in unix seconds. */
export const scheduled: ExportedHandlerScheduledHandler<Env> = async (controller, env, ctx) => {
  ctx.waitUntil(runCleanups(env, Math.floor(controller.scheduledTime / 1000)));
};
