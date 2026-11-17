import { mkdirSync } from "node:fs";
import { join } from "node:path";
import { outputDir } from "./paths.ts";
import { startServer } from "./server.ts";

/**
 * Starts the Worker once for the whole run and hands the origin and the bootstrap invite to the test worker through
 * the environment (inherited by the worker process; never logged). The returned function is the teardown.
 */
export default async function globalSetup(): Promise<() => Promise<void>> {
  const out = outputDir();
  mkdirSync(out, { recursive: true });
  const server = await startServer(join(out, "wrangler.log"));
  process.env["E2E_ORIGIN"] = server.origin;
  process.env["E2E_INVITE"] = server.invite;
  process.env["E2E_INVITE_PAIRING"] = server.extraInvites.pairing;
  process.env["E2E_INVITE_DEVICES"] = server.extraInvites.devices;
  process.env["E2E_INVITE_UI"] = server.extraInvites.ui;
  return () => server.stop();
}
