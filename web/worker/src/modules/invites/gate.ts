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

let invalidCapLogged = false;

/**
 * `MAX_USERS` as the cap: 0 means no cap, a positive integer is the cap, and `null` means the value is invalid
 * (anything that is not a non-negative integer string, such as `""`, `"abc"`, `"-1"` or `"1.5"`). An invalid
 * value fails closed, see `gateParams` and `registrationOpen`: a typo must not turn a capped instance into an
 * unlimited one. Logs one metadata-only error line per isolate.
 */
export function userCap(env: Pick<Env, "MAX_USERS">): number | null {
  const text = String(env.MAX_USERS);
  if (/^\d{1,9}$/.test(text)) return Number(text);
  if (!invalidCapLogged) {
    invalidCapLogged = true;
    console.error(`MAX_USERS is not a non-negative integer (length ${text.length}): registration and invites are closed`);
  }
  return null;
}

/** Lets a test see the error line of `userCap` again. */
export function resetInvalidCapLog(): void {
  invalidCapLogged = false;
}

/**
 * The cap as shown to an admin. With an invalid `MAX_USERS` (the gate is closed) it is the current user count, at
 * least 1: "n of n users", never 0, which would read as "no cap".
 */
export function maxUsers(env: Pick<Env, "MAX_USERS">, userCount = 0): number {
  return userCap(env) ?? Math.max(1, userCount);
}

/** Bind values of `GATE_SQL`. An invalid cap binds (1, 0): `? = 0` is false and `count < 0` is false, so the gate is shut. */
export function gateParams(env: Pick<Env, "MAX_USERS">): [number, number] {
  const cap = userCap(env);
  return cap === null ? [1, 0] : [cap, cap];
}

/** True when invites may currently be used to register: not paused and below the user cap. An invalid cap is closed. */
export async function registrationOpen(env: Pick<Env, "DB" | "MAX_USERS">): Promise<boolean> {
  const cap = userCap(env);
  if (cap === null) return false;
  const [paused, count] = await Promise.all([
    env.DB.prepare(INVITES_PAUSED_SQL).first<{ value: string }>(),
    cap > 0 ? env.DB.prepare(USER_COUNT_SQL).first<{ n: number }>() : Promise.resolve(null),
  ]);
  if (paused?.value === "1") return false;
  return cap === 0 || (count?.n ?? 0) < cap;
}
