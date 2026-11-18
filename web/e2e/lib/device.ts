// The device (browser) half of the pairing spec, played from the Playwright test process with the TypeScript device
// client of web/mock-desktop (`@monoptah/mock-desktop/test-client`). The app has no route that renders a catalog yet
// (WS-M desktop pages are stubs), so the app's own link client cannot be driven from a page. The e2e package has no
// workspace link to mock-desktop (and `pnpm add` is not allowed for this task), so the sources are imported by path.
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
}

/** Opens `/relay/device/:desktopId?device=:id` with the session cookie and handshakes (Noise IK). */
export async function connectDevice(options: DeviceLinkOptions): Promise<TestClient> {
  const wsOrigin = options.origin.replace(/^http/, "ws");
  const socket = await connectRelay(`${wsOrigin}/relay/device/${options.desktopId}?device=${options.deviceId}`, {
    Cookie: options.cookie,
    Origin: options.origin,
  });
  return TestClient.connect({
    socket,
    desktopId: options.desktopId,
    deviceId: options.deviceId,
    desktopPublicKey: base64urlDecode(options.desktopPublicKey),
    deviceKey: options.deviceKey,
    deviceName: options.deviceName,
  });
}
