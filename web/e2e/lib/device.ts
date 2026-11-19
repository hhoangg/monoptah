// The device (browser) half of the pairing spec, played from the Playwright test process with the TypeScript device
// client of web/mock-desktop (`@monoptah/mock-desktop/test-client`). The UI flows (tests/w14-flows.spec.ts) use the
// app's own link client in the page; this client serves the protocol-level specs that need raw frames and tokens. The
// e2e package has no workspace link to mock-desktop, so the sources are imported by path.
import type { FrameSocket } from "../../mock-desktop/src/transport.ts";
import { TestClient } from "../../mock-desktop/src/client.ts";
import { generateKeyPair, type KeyPair } from "../../mock-desktop/src/noise.ts";
import { connectRelay } from "../../mock-desktop/src/transport.ts";
import { base64urlDecode } from "./pure.ts";

export { generateKeyPair, type KeyPair };

export interface DeviceLinkOptions {
  /** `http://localhost:PORT`. */
  origin: string;
  desktopId: string;
  deviceId: string;
  deviceKey: KeyPair;
  /** The desktop key from the directory (`GET /api/desktops/:id`), base64url. */
  desktopPublicKey: string;
  /** `name=value` of the session cookie. Never logged. */
  cookie: string;
  deviceName: string;
  /** Noise channel (default 1). Two live clients of one device need two channels. */
  channel?: number;
  /** Wraps the socket before the handshake (a recorder that captures the frames sent). */
  wrapSocket?: (socket: FrameSocket) => FrameSocket;
}

/** Opens `/relay/device/:desktopId?device=:id` with the session cookie and handshakes (Noise IK). */
export async function connectDevice(options: DeviceLinkOptions): Promise<TestClient> {
  const wsOrigin = options.origin.replace(/^http/, "ws");
  const socket = await connectRelay(`${wsOrigin}/relay/device/${options.desktopId}?device=${options.deviceId}`, {
    Cookie: options.cookie,
    Origin: options.origin,
  });
  return TestClient.connect({
    socket: options.wrapSocket ? options.wrapSocket(socket) : socket,
    ...(options.channel === undefined ? {} : { channel: options.channel }),
    desktopId: options.desktopId,
    deviceId: options.deviceId,
    desktopPublicKey: base64urlDecode(options.desktopPublicKey),
    deviceKey: options.deviceKey,
    deviceName: options.deviceName,
  });
}
