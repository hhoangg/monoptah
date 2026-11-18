// S-06 device test sender (Node 22+). Sends ONE push with src/webpush.ts to a subscription copied from the test page.
//
//   node s-06-webpush/send.ts keygen [vapid.json]                       writes a throwaway VAPID key pair, prints the public key
//   node s-06-webpush/send.ts send <subscription.json> [vapid.json]     sends one push
//       [--title T] [--body B] [--ttl SECONDS] [--urgency very-low|low|normal|high] [--topic NAME] [--silent]
//
// The VAPID key is generated at run time and written to a file OUTSIDE the repository (default: the OS temp dir). Never
// commit it. Real delivery needs the test page on an https origin (a secure context); localhost is only enough on the
// same machine for desktop browsers.
import { readFile, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { buildPushRequest, classifyPushResponse, generateVapidKeys, importVapidKeys, type PushSubscriptionJson, type Urgency } from "./src/webpush.ts";

const [command, ...rest] = process.argv.slice(2);
const flags = new Map<string, string>();
const positional: string[] = [];
for (let i = 0; i < rest.length; i += 1) {
  const item = rest[i] ?? "";
  if (item.startsWith("--")) {
    const next = rest[i + 1];
    if (next === undefined || next.startsWith("--")) flags.set(item.slice(2), "true");
    else {
      flags.set(item.slice(2), next);
      i += 1;
    }
  } else positional.push(item);
}

if (command === "keygen") {
  const file = positional[0] ?? join(tmpdir(), "monoptah-s06-vapid.json");
  const keys = await generateVapidKeys();
  await writeFile(file, JSON.stringify(keys), { mode: 0o600 });
  console.log(`Key pair written to ${file} (keep it out of git).`);
  console.log(`VAPID public key (paste into the test page): ${keys.publicKey}`);
} else if (command === "send") {
  const subscriptionFile = positional[0];
  if (!subscriptionFile) throw new Error("usage: send <subscription.json> [vapid.json] [options]");
  const vapidFile = positional[1] ?? join(tmpdir(), "monoptah-s06-vapid.json");
  const subscription = JSON.parse(await readFile(subscriptionFile, "utf8")) as PushSubscriptionJson;
  const vapid = JSON.parse(await readFile(vapidFile, "utf8")) as { publicKey: string; privateKey: string };
  const silent = flags.has("silent");
  const payload = silent
    ? { silent: true }
    : { title: flags.get("title") ?? "Monoptah push spike", body: flags.get("body") ?? `Sent at ${new Date().toISOString()}`, tag: flags.get("topic") };
  const topic = flags.get("topic");
  const urgency = flags.get("urgency") as Urgency | undefined;
  const request = await buildPushRequest({
    subscription,
    payload: new TextEncoder().encode(JSON.stringify(payload)),
    ttl: Number(flags.get("ttl") ?? "60"),
    ...(urgency === undefined ? {} : { urgency }),
    ...(topic === undefined ? {} : { topic }),
    vapid: { publicKey: vapid.publicKey, keyPair: await importVapidKeys(vapid.publicKey, vapid.privateKey), subject: "mailto:spike@example.invalid" },
  });
  const started = performance.now();
  const response = await fetch(request.url, { method: request.init.method, headers: request.init.headers, ...(request.init.body === undefined ? {} : { body: request.init.body }) });
  const outcome = classifyPushResponse(response.status, response.headers.get("retry-after"));
  console.log(`${new URL(request.url).host} -> HTTP ${response.status} in ${Math.round(performance.now() - started)} ms, action: ${JSON.stringify(outcome)}`);
  const text = await response.text();
  if (text) console.log(`response body: ${text.slice(0, 300)}`);
  if (response.status !== 201) process.exitCode = 1;
} else {
  console.error("usage: send.ts keygen [file] | send <subscription.json> [vapid.json] [options]");
  process.exitCode = 2;
}
