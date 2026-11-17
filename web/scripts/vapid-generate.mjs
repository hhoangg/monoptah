/* global process, console */
// Prints a VAPID key pair (P-256, base64url): the public key is the 65-byte uncompressed point (a Worker var),
// the private key is the 32-byte scalar `d` (a Worker secret). Nothing is written to disk.
import { webcrypto } from "node:crypto";
import { fileURLToPath } from "node:url";

/** Generates a pair: `{ publicKey, privateKey }`, both unpadded base64url. */
export async function generateVapidKeys() {
  const pair = await webcrypto.subtle.generateKey({ name: "ECDSA", namedCurve: "P-256" }, true, ["sign", "verify"]);
  const publicKey = Buffer.from(await webcrypto.subtle.exportKey("raw", pair.publicKey)).toString("base64url");
  const jwk = await webcrypto.subtle.exportKey("jwk", pair.privateKey);
  return { publicKey, privateKey: jwk.d };
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const { publicKey, privateKey } = await generateVapidKeys();
  console.log(`VAPID_PUBLIC_KEY=${publicKey}`);
  console.log(`VAPID_PRIVATE_KEY=${privateKey}`);
  console.error("Put the public key in wrangler.jsonc vars and the private key in `wrangler secret put VAPID_PRIVATE_KEY`. Never commit the private key.");
}
