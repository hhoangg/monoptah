// Per-account quotas from the environment (architecture.md 4.4). A value that is not a positive integer falls
// back to the protocol default instead of disabling the quota.
import { ACCOUNT_QUOTAS } from "@monoptah/protocol";
import type { Env } from "../../env.ts";

function positiveInt(value: unknown, fallback: number): number {
  const text = String(value ?? "");
  if (!/^\d{1,6}$/.test(text)) return fallback;
  const n = Number(text);
  return n > 0 ? n : fallback;
}

/** `MAX_DESKTOPS_PER_USER` (default 5) counts desktops that are not revoked. */
export function maxDesktopsPerUser(env: Pick<Env, "MAX_DESKTOPS_PER_USER">): number {
  return positiveInt(env.MAX_DESKTOPS_PER_USER, ACCOUNT_QUOTAS.maxDesktopsPerUser);
}

/** `MAX_DEVICES_PER_USER` (default 10) counts devices that are not revoked. */
export function maxDevicesPerUser(env: Pick<Env, "MAX_DEVICES_PER_USER">): number {
  return positiveInt(env.MAX_DEVICES_PER_USER, ACCOUNT_QUOTAS.maxDevicesPerUser);
}
