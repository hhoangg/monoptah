// Dev and test adapter: a DesktopLink over the mock desktop (MockDesktop + InMemoryRelay +
// TestClient). It is browser safe (the mock desktop uses only @noble and WebCrypto), so the dev
// build with VITE_MOCK_API=1 uses it as well. It never ships: main.tsx imports this file
// dynamically behind `import.meta.env.DEV`, and scripts/check-dist.mjs fails on its marker.
import { fingerprint, generateKeyPair, InMemoryRelay, MockDesktop, type KeyPair } from "@monoptah/mock-desktop";
import { ClientError, TestClient } from "@monoptah/mock-desktop/test-client";
import type { MethodName, ParamsOf, ResultOf } from "@monoptah/protocol";
import { DesktopLinkError, provideDesktopLink, type DesktopLink, type DesktopLinkStatus } from "./desktopLink";

/** Unique string the dist guard looks for. */
export const MOCK_DESKTOP_MARKER = "__MONOPTAH_MOCK_DESKTOP__";

export interface MockDesktopLinkOptions {
  /** False leaves the desktop side of the relay empty, so the link goes offline (`desktop_offline`). Default true. */
  online?: boolean;
  name?: string;
}

/** A link plus the pieces tests inspect. */
export interface MockDesktopLink extends DesktopLink {
  readonly desktop: MockDesktop;
  readonly relay: InMemoryRelay;
  /** Every method name sent through `call`, in order. */
  readonly calls: Array<{ method: string; params: unknown }>;
  /** Number of completed handshakes (HS1 sent) so far. */
  readonly handshakes: number;
  close(): void;
}

function toLinkError(error: unknown): DesktopLinkError {
  if (error instanceof DesktopLinkError) return error;
  if (error instanceof ClientError) return new DesktopLinkError(error.message, error.code);
  return new DesktopLinkError(error instanceof Error ? error.message : "Link error");
}

export function createMockDesktopLink(options: MockDesktopLinkOptions = {}): MockDesktopLink {
  const desktopId = crypto.randomUUID();
  const deviceId = crypto.randomUUID();
  const deviceKey: KeyPair = generateKeyPair();
  const relay = new InMemoryRelay();
  const desktop = new MockDesktop({ desktopId, name: options.name ?? "Mock desktop" });
  desktop.setDevice(fingerprint(deviceKey.publicKey), { status: "confirmed", permission: "control" });
  if (options.online !== false) desktop.attach(relay.connectDesktop());

  const listeners = new Set<() => void>();
  const calls: Array<{ method: string; params: unknown }> = [];
  let status: DesktopLinkStatus = "connecting";
  let client: TestClient | null = null;
  let connecting: Promise<void> | null = null;
  let handshakes = 0;
  let channel = 0;

  function setStatus(next: DesktopLinkStatus): void {
    if (status === next) return;
    status = next;
    for (const listener of [...listeners]) listener();
  }

  function connect(): Promise<void> {
    channel += 1;
    handshakes += 1;
    const attempt = TestClient.connect({
      socket: relay.connectDevice(deviceId),
      desktopId,
      deviceId,
      channel,
      desktopPublicKey: desktop.publicKey,
      deviceKey,
      deviceName: "This browser",
    }).then(
      (next) => {
        client = next;
        setStatus("connected");
      },
      (error: unknown) => {
        client = null;
        const failure = toLinkError(error);
        setStatus(failure.code === "desktop_offline" ? "offline" : "failed");
        throw failure;
      },
    );
    connecting = attempt;
    return attempt;
  }

  const first = connect();
  // The first attempt is awaited by `call`; keep an unhandled rejection from reaching the console.
  first.catch(() => undefined);

  return {
    desktop,
    relay,
    calls,
    get handshakes() {
      return handshakes;
    },
    get status() {
      return status;
    },
    subscribe(listener) {
      listeners.add(listener);
      return () => {
        listeners.delete(listener);
      };
    },
    async call<M extends MethodName>(method: M, params?: ParamsOf<M>): Promise<ResultOf<M>> {
      calls.push({ method, params });
      if (connecting) await connecting.catch(() => undefined);
      if (!client) throw new DesktopLinkError("The desktop is not connected", status === "offline" ? "desktop_offline" : "closed");
      try {
        return await client.call(method, params);
      } catch (error) {
        throw toLinkError(error);
      }
    },
    async reconnect() {
      setStatus("reconnecting");
      const old = client;
      client = null;
      old?.bye();
      await relay.settled();
      await connect();
    },
    close() {
      client?.bye();
      client = null;
      desktop.close();
    },
  };
}

/** Dev only: every desktop id gets its own mock link; the "Fake Linux box" (offline in the mock account) is offline here too. */
export function installMockDesktopLink(): void {
  Object.defineProperty(window, MOCK_DESKTOP_MARKER, { value: true, configurable: true });
  provideDesktopLink((desktopId) => createMockDesktopLink({ online: desktopId !== "desktop-dev-2", name: desktopId }));
}
