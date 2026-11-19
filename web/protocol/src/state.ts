// The `state` message: first encrypted message from the desktop on a channel,
// re-sent on a permission, selection or capability change
// (architecture.md 3.3 step 4, 5.5).
import "./zod-config.ts";
import { z } from "zod";

/** Capability names a desktop advertises today. Unknown names are accepted (the web ignores them). */
export const KNOWN_CAPABILITIES = [
  "approvals",
  "cloud",
  "cloudStart",
  "cloudMove",
  "gitPush",
  "monoTools",
  "monoFiles",
  "mentionedFiles",
  "mentionedFiles2",
] as const;
export type KnownCapability = (typeof KNOWN_CAPABILITIES)[number];

export const DeviceStateSchema = z.enum(["pending", "confirmed", "revoked"]);
export type DeviceState = z.infer<typeof DeviceStateSchema>;

export const PermissionSchema = z.enum(["read", "control"]);
export type Permission = z.infer<typeof PermissionSchema>;

/**
 * `permission`, `proto`, `capabilities` and `desktop` are optional because the
 * pending and revoked messages of architecture.md 6.3 carry only `state`. A
 * `confirmed` message always carries all of them. Unknown keys are kept.
 */
export const StateMessageSchema = z.looseObject({
  type: z.literal("state"),
  state: DeviceStateSchema,
  permission: PermissionSchema.optional(),
  proto: z.number().int().nonnegative().optional(),
  /** Free-form on purpose: a newer desktop may advertise names this client does not know. */
  capabilities: z.array(z.string()).optional(),
  desktop: z.looseObject({ name: z.string(), version: z.string() }).optional(),
});
export type StateMessage = z.infer<typeof StateMessageSchema>;

export function hasCapability(state: Pick<StateMessage, "capabilities">, name: KnownCapability | (string & {})): boolean {
  return state.capabilities?.includes(name) ?? false;
}
