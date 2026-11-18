// Live interop client for the ignored Rust test `live_tests`. It plays the browser:
// the real device client of web/mock-desktop (`TestClient`, a @noble Noise initiator)
// over a real WebSocket to a local relay stub, against the real Rust desktop web link.
//   node --experimental-strip-types live-ts-client.ts --url ws://127.0.0.1:PORT/relay/device/ID?device=UUID \
//     --desktop-key HEX --device-private HEX --desktop-id UUID --device-id UUID [--expect-bye]
// Prints one JSON line and exits 0, or prints the error to stderr and exits 1.
import { TestClient } from "../../../../../../web/mock-desktop/src/client.ts";
import { generateKeyPair } from "../../../../../../web/mock-desktop/src/noise.ts";
import { connectRelay } from "../../../../../../web/mock-desktop/src/transport.ts";

const args = new Map<string, string>();
const argv = process.argv.slice(2);
for (let i = 0; i < argv.length; i++) {
  const key = argv[i] as string;
  if (!key.startsWith("--")) throw new Error(`unexpected argument ${key}`);
  const next = argv[i + 1];
  if (next === undefined || next.startsWith("--")) args.set(key.slice(2), "true");
  else {
    args.set(key.slice(2), next);
    i++;
  }
}
const need = (name: string): string => {
  const value = args.get(name);
  if (!value) throw new Error(`missing --${name}`);
  return value;
};
const fromHex = (hex: string): Uint8Array => Uint8Array.from(Buffer.from(hex, "hex"));

function within<T>(what: string, promise: Promise<T>, ms = 20_000): Promise<T> {
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error(`timed out waiting for ${what}`)), ms);
    promise.then(
      (value) => {
        clearTimeout(timer);
        resolve(value);
      },
      (error) => {
        clearTimeout(timer);
        reject(error);
      },
    );
  });
}

async function main(): Promise<Record<string, unknown>> {
  const url = need("url");
  const socket = await within("relay connect", connectRelay(url));
  const options = {
    socket,
    desktopId: need("desktop-id"),
    deviceId: need("device-id"),
    desktopPublicKey: fromHex(need("desktop-key")),
    deviceKey: generateKeyPair(fromHex(need("device-private"))),
    deviceName: "Live smoke browser",
  };
  if (args.has("expect-bye")) {
    try {
      await within("BYE", TestClient.connect(options));
    } catch (error) {
      const code = (error as { code?: string }).code;
      if (code !== "bye") throw error;
      socket.close();
      return { byeReceived: true, state: null };
    }
    throw new Error("expected BYE but the desktop answered with a state");
  }

  const client = await within("handshake and state", TestClient.connect(options));
  const state = client.state as unknown as { state?: string; proto?: number };
  if (state.state !== "confirmed" || state.proto !== 1) throw new Error(`unexpected state ${JSON.stringify(state)}`);

  const list = await within("workspace.list", client.request("workspace.list"));
  if (!list.ok) throw new Error(`workspace.list failed: ${JSON.stringify(list)}`);

  const read = await within("conversation.read", client.request("conversation.read", { conversationId: "conv-live" }));
  if (!read.ok) throw new Error(`conversation.read failed: ${JSON.stringify(read)}`);
  const readResult = (read as { result: { conversationId?: string } }).result;
  if (readResult.conversationId !== "conv-live") throw new Error(`unexpected read result ${JSON.stringify(readResult)}`);

  const watch = await within("watch ack", client.openWatch({ conversationId: "conv-live" }));
  await within(
    "3 watch frames",
    new Promise<void>((resolve) => {
      const check = () => {
        if (watch.frames.length >= 3) resolve();
      };
      watch.onFrame = check;
      check();
    }),
  );
  const seqs = watch.frames.map((frame) => (frame as unknown as { seq: number }).seq);
  watch.cancel();

  const big = "x".repeat(200 * 1024);
  const echo = await within("chunked request", client.request("conversation.read", { conversationId: "conv-live", blob: big }));
  if (!echo.ok) throw new Error(`chunked request failed: ${JSON.stringify(echo)}`);
  const echoed = (echo as { result: { blobLength?: number } }).result.blobLength;
  if (echoed !== big.length) throw new Error(`desktop saw ${String(echoed)} bytes, sent ${big.length}`);

  client.bye();
  socket.close();
  return {
    byeReceived: false,
    state: state.state,
    proto: state.proto,
    workspaceListOk: true,
    readOk: true,
    watchFrames: watch.frames.length,
    watchSeqs: seqs,
    chunkedBytes: echoed,
  };
}

main().then(
  (result) => {
    console.log(JSON.stringify(result));
    process.exit(0);
  },
  (error) => {
    console.error(error instanceof Error ? `${error.name}: ${error.message}` : String(error));
    process.exit(1);
  },
);
