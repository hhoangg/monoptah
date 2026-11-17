// Manual run of the mock desktop against a relay (for example `wrangler dev`):
//
//   pnpm --filter @monoptah/mock-desktop start --relay ws://localhost:8787 --token <desktop token> --desktop-id <uuid>
//
// It serves canned data only. Never point it at a real desktop token of a real account.
import { parseArgs } from "node:util";
import { bytesToHex } from "@noble/hashes/utils.js";
import { MockDesktop } from "./desktop.ts";
import { connectRelay } from "./transport.ts";

const { values } = parseArgs({
  options: {
    relay: { type: "string" },
    token: { type: "string" },
    "desktop-id": { type: "string" },
    "auto-advance-ms": { type: "string" },
    "confirm-device": { type: "string", multiple: true },
  },
});

if (!values.relay || !values.token) {
  console.error("usage: start --relay <ws(s)://host> --token <desktop token> [--desktop-id <uuid>] [--auto-advance-ms N] [--confirm-device <fingerprint>]...");
  process.exit(2);
}

const desktopId = values["desktop-id"] ?? crypto.randomUUID();
const desktop = new MockDesktop({
  desktopId,
  ...(values["auto-advance-ms"] ? { autoAdvanceMs: Number(values["auto-advance-ms"]) } : {}),
  devices: (values["confirm-device"] ?? []).map((fingerprint) => ({ fingerprint, status: "confirmed" as const, permission: "control" as const })),
});

const url = `${values.relay.replace(/\/$/, "")}/relay/desktop/${desktopId}`;
const socket = await connectRelay(url, { Authorization: `Bearer ${values.token}` });
desktop.attach(socket);
const previous = socket.onclose;
socket.onclose = (code) => {
  previous?.(code);
  console.log(`relay closed with ${code}`);
  process.exit(code === 1000 ? 0 : 1);
};
console.log(`mock desktop ${desktopId} connected to ${url}`);
console.log(`public key  ${bytesToHex(desktop.publicKey)}`);
console.log(`fingerprint ${desktop.fingerprint}`);
