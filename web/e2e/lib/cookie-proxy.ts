// A local reverse proxy for the WebKit project of tests/html-preview.spec.ts. WebKit on Linux does not send a Secure
// cookie on the `ws://localhost` relay handshake (it does on http://localhost fetches), and the relay authenticates a
// device by that cookie alone, so the page would sit on "Reconnecting..." forever. Production is wss, where the cookie
// is sent. The proxy forwards HTTP and WebSocket upgrades to the local Worker unchanged, and adds the test's own
// session cookie to a `/relay/device/` upgrade that arrives without one. `Origin` is rewritten to the Worker's, which
// is the one it checks against (RP_ORIGIN); `Host` stays the proxy's, so the CSP `connect-src wss://HOST` still names
// the address the page was loaded from.
import { createServer, request, type IncomingMessage } from "node:http";
import { connect, type Socket } from "node:net";

export interface CookieProxy {
  /** `http://localhost:<port>`: where the browser goes. */
  origin: string;
  close: () => Promise<void>;
}

export async function startCookieProxy(upstream: string, cookie: string): Promise<CookieProxy> {
  const target = new URL(upstream);
  const upstreamPort = Number(target.port || 80);
  const rewrite = (name: string, value: string): string => (name.toLowerCase() === "origin" ? target.origin : value);
  const sockets = new Set<Socket>();

  const server = createServer((req, res) => {
    const headers: Record<string, string | string[] | undefined> = { ...req.headers };
    if (typeof headers["origin"] === "string") headers["origin"] = target.origin;
    const forwarded = request({ host: "127.0.0.1", port: upstreamPort, method: req.method, path: req.url, headers }, (answer) => {
      res.writeHead(answer.statusCode ?? 502, answer.headers);
      answer.pipe(res);
    });
    forwarded.on("error", () => res.destroy());
    req.pipe(forwarded);
  });

  server.on("upgrade", (req: IncomingMessage, client: Socket, head: Buffer) => {
    sockets.add(client);
    const upstreamSocket = connect(upstreamPort, "127.0.0.1");
    sockets.add(upstreamSocket);
    const lines: string[] = [`${req.method ?? "GET"} ${req.url ?? "/"} HTTP/1.1`];
    let hasCookie = false;
    for (let i = 0; i + 1 < req.rawHeaders.length; i += 2) {
      const name = req.rawHeaders[i] as string;
      if (name.toLowerCase() === "cookie") hasCookie = true;
      lines.push(`${name}: ${rewrite(name, req.rawHeaders[i + 1] as string)}`);
    }
    if (!hasCookie && (req.url ?? "").startsWith("/relay/device/")) lines.push(`Cookie: ${cookie}`);
    upstreamSocket.write(`${lines.join("\r\n")}\r\n\r\n`);
    if (head.length > 0) upstreamSocket.write(head);
    upstreamSocket.pipe(client);
    client.pipe(upstreamSocket);
    const drop = (): void => {
      client.destroy();
      upstreamSocket.destroy();
      sockets.delete(client);
      sockets.delete(upstreamSocket);
    };
    client.on("error", drop).on("close", drop);
    upstreamSocket.on("error", drop).on("close", drop);
  });

  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
  const address = server.address();
  const port = typeof address === "object" && address !== null ? address.port : 0;
  return {
    origin: `http://localhost:${port}`,
    close: () =>
      new Promise<void>((resolve) => {
        for (const socket of sockets) socket.destroy();
        server.close(() => resolve());
        server.closeAllConnections();
      }),
  };
}
