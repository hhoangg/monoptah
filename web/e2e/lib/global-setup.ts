import { mkdirSync } from "node:fs";
import { createServer } from "node:http";
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
  process.env["E2E_INVITE_W14"] = server.extraInvites.w14;
  process.env["E2E_INVITE_W14B"] = server.extraInvites.w14b;
  process.env["E2E_INVITE_R12"] = server.extraInvites.r12;
  process.env["E2E_INVITE_FILEPREVIEW"] = server.extraInvites.filepreview;
  process.env["E2E_INVITE_MLAYOUT"] = server.extraInvites.mlayout;
  process.env["E2E_INVITE_SELECTION"] = server.extraInvites.selection;
  // The tests run in another process: a loopback control port lets the R-12 spec restart the relay (the Worker) and the specs that pair a headless desktop start a fresh rate limit window.
  const control = createServer((req, res) => {
    const clear = req.method === "POST" && req.url === "/clear-rate-limits";
    if (!clear && (req.method !== "POST" || req.url !== "/restart")) {
      res.writeHead(404).end();
      return;
    }
    server.restart({ clearRateLimits: clear }).then(
      () => res.writeHead(200).end("ok"),
      (err: unknown) => res.writeHead(500).end(String(err)),
    );
  });
  await new Promise<void>((resolve) => control.listen(0, "127.0.0.1", resolve));
  const address = control.address();
  process.env["E2E_CONTROL"] = `http://127.0.0.1:${typeof address === "object" && address !== null ? address.port : 0}`;
  return async () => {
    control.close();
    await server.stop();
  };
}
