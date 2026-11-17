// The registration gate of architecture.md 4.4 (A-06): the "Pause invites" switch (`settings.invites_paused`) and
// the `MAX_USERS` cap. `registrationOpen` is the early check of `register/options`; the same two conditions are
// repeated inside the registration batch (`GATE_SQL`), which is what makes two parallel registrations unable to
// pass the cap together.
import type { Env } from "../../env.ts";

export const INVITES_PAUSED_SQL = "SELECT value FROM settings WHERE key = 'invites_paused'";
/** Counts every user, blocked ones included. `users` is capped by `MAX_USERS`, so the covering index scan is bounded. */
export const USER_COUNT_SQL = "SELECT count(*) AS n FROM users";

/**
 * SQL condition for `WHERE` of the registration batch: not paused, and `MAX_USERS` is 0 (no cap) or above the user
 * count. Binds `maxUsers` twice, see `gateParams`.
 */
export const GATE_SQL =
  "COALESCE((SELECT value FROM settings WHERE key = 'invites_paused'), '0') != '1' AND (? = 0 OR (SELECT count(*) FROM users) < ?)";

/** `MAX_USERS` as a number; anything that is not a non-negative integer counts as 0 (no cap). */
export function maxUsers(env: Pick<Env, "MAX_USERS">): number {
  const value = Number(env.MAX_USERS);
  return Number.isInteger(value) && value > 0 ? value : 0;
}

export function gateParams(env: Pick<Env, "MAX_USERS">): [number, number] {
  const max = maxUsers(env);
  return [max, max];
}

/** True when invites may currently be used to register: not paused and below the user cap. */
export async function registrationOpen(env: Pick<Env, "DB" | "MAX_USERS">): Promise<boolean> {
  const max = maxUsers(env);
  const [paused, count] = await Promise.all([
    env.DB.prepare(INVITES_PAUSED_SQL).first<{ value: string }>(),
    max > 0 ? env.DB.prepare(USER_COUNT_SQL).first<{ n: number }>() : Promise.resolve(null),
  ]);
  if (paused?.value === "1") return false;
  return max === 0 || (count?.n ?? 0) < max;
}
