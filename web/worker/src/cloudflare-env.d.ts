// Bindings, vars and secrets of the Worker, as declared in web/wrangler.jsonc. Hand-written instead
// of `wrangler types` output so that no generated file has to be kept in sync. When a binding or var is
// added to wrangler.jsonc, add it here too (`wrangler types -c ../wrangler.jsonc` prints the expected names).
declare namespace Cloudflare {
  interface Env {
    // Bindings
    ASSETS: Fetcher;
    DB: D1Database;
    /** R2 bucket of the encrypted file transfers (objects under `transfers/`, deleted after a day by a lifecycle rule). */
    TRANSFERS: R2Bucket;
    // eslint-disable-next-line @typescript-eslint/consistent-type-imports -- a global .d.ts cannot use a top-level import
    ROOM: DurableObjectNamespace<import("./relay/DesktopRoom.ts").DesktopRoom>;
    // Vars (wrangler.jsonc `vars`)
    RP_ID: string;
    RP_ORIGIN: string;
    VAPID_PUBLIC_KEY: string;
    VAPID_KEY_ID: string;
    VAPID_SUBJECT: string;
    /** Rotation only (deploy-runbook.md "VAPID key rotation"): the previous public key and its key id. Unset otherwise. */
    VAPID_PREVIOUS_PUBLIC_KEY?: string;
    VAPID_PREVIOUS_KEY_ID?: string;
    /** Decimal string. "0" means unlimited. */
    MAX_USERS: string;
    MAX_DESKTOPS_PER_USER: string;
    MAX_DEVICES_PER_USER: string;
    // Secrets (`wrangler secret put`, web/.dev.vars locally): never in `vars`
    VAPID_PRIVATE_KEY: string;
    /** Rotation only: the private half of `VAPID_PREVIOUS_PUBLIC_KEY`. Remove it when the rotation ends. */
    VAPID_PREVIOUS_PRIVATE_KEY?: string;
    RECOVERY_PEPPER: string;
    AUDIT_IP_SALT: string;
  }
}
