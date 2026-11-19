// Static server for the S-08 stub app and the S-03 page. Sends the exact `buildCsp(host)` of
// web/worker/src/csp.ts and COMPANION_HEADERS on every response (the real Worker does the same).
// `startServer(dir, port, variant)` with variant none (control, no CSP) | exact | no-style-attr | no-uir | no-style-attr-no-uir (the no-uir ones exist because WebKit upgrades
// http://localhost subresources to https). Started by serve-all.ts.
import { createServer } from "node:http";
import { createServer as createHttpsServer } from "node:https";
import { readFile, stat } from "node:fs/promises";
import { extname, join, normalize, resolve } from "node:path";
import { buildCsp, COMPANION_HEADERS } from "../../worker/src/csp.ts";

const MIME: Record<string, string> = {
  ".html": "text/html; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".css": "text/css; charset=utf-8",
  ".json": "application/json",
  ".svg": "image/svg+xml",
  ".webmanifest": "application/manifest+json",
};

// Page script (served, so the page CSP applies; page.evaluate is not subject to the eval block) that tries
// the code-string sinks and records the outcome in window.__probe.
const PROBE_JS = `window.__probe = {};
for (const [name, fn] of [
  ["eval", () => eval("1+1")],
  ["new Function", () => new Function("return 1")()],
  ["setTimeout string", () => setTimeout("1", 0)],
  ["innerHTML", () => { document.createElement("div").innerHTML = "<b>x</b>"; }],
  ["insertAdjacentHTML", () => document.createElement("div").insertAdjacentHTML("beforeend", "<b>x</b>")],
  ["createContextualFragment", () => document.createRange().createContextualFragment("<b>x</b>")],
  ["DOMParser.parseFromString", () => new DOMParser().parseFromString("<b>x</b>", "text/html")],
  ["script.textContent", () => { document.createElement("script").textContent = "1"; }],
  ["script.src offsite", () => { document.createElement("script").src = "https://example.com/x.js"; }],
  ["iframe.srcdoc", () => { document.createElement("iframe").srcdoc = "<b>x</b>"; }],
  ["new Worker offsite", () => new Worker("https://example.com/w.js")],
]) {
  try { fn(); window.__probe[name] = "allowed"; } catch (e) { window.__probe[name] = e.name + ": " + e.message.slice(0, 80); }
}`;

/** `replace` that throws when nothing changed, so a reformatted `buildCsp` cannot turn a variant into a no-op. */
function mustChange(csp: string, from: string, to: string): string {
  const out = csp.replace(from, to);
  if (out === csp) throw new Error(`CSP variant: "${from}" not found in buildCsp output`);
  return out;
}

/** The CSP for a request host, with the variant tweaks the spike needs. */
export function cspFor(host: string, v: string): string {
  let csp = buildCsp(host);
  if (v.includes("no-style-attr")) csp = mustChange(csp, "style-src-attr 'unsafe-inline'; ", "");
  if (v.endsWith("no-uir")) csp = mustChange(csp, "; upgrade-insecure-requests", "");
  return csp;
}

export function startServer(dirArg: string, port: number, variant: string, tls?: { key: Buffer; cert: Buffer }): Promise<void> {
  const dir = resolve(dirArg);
  const handler: Parameters<typeof createServer>[1] = async (req, res) => {
  const url = new URL(req.url ?? "/", "http://x");
  const host = req.headers.host ?? `localhost:${port}`;
  const headers: Record<string, string> = { ...COMPANION_HEADERS, "Cache-Control": "no-store" };
  if (variant !== "none") headers["Content-Security-Policy"] = cspFor(host, variant);
  if (url.pathname === "/api/hello") {
    res.writeHead(200, { ...headers, "Content-Type": "application/json" });
    res.end(JSON.stringify({ hello: "world" }));
    return;
  }
  if (url.pathname === "/__probe.html") {
    res.writeHead(200, { ...headers, "Content-Type": MIME[".html"]! });
    res.end('<!doctype html><title>probe</title><script src="/__probe.js"></script>');
    return;
  }
  if (url.pathname === "/__probe-style.html") {
    // Positive control for style-src-attr: an HTML style attribute and a script-set style attribute.
    res.writeHead(200, { ...headers, "Content-Type": MIME[".html"]! });
    res.end('<!doctype html><title>probe</title><div id="parsed" style="color: rgb(255, 0, 0)">parsed</div><div id="scripted">scripted</div><script src="/__probe-style.js"></script>');
    return;
  }
  if (url.pathname === "/__probe-style.js") {
    res.writeHead(200, { ...headers, "Content-Type": MIME[".js"]! });
    res.end('document.getElementById("scripted").setAttribute("style", "color: rgb(255, 0, 0)");');
    return;
  }
  if (url.pathname === "/__probe.js") {
    res.writeHead(200, { ...headers, "Content-Type": MIME[".js"]! });
    res.end(PROBE_JS);
    return;
  }
  let file = normalize(join(dir, decodeURIComponent(url.pathname)));
  if (!file.startsWith(dir)) { res.writeHead(403, headers); res.end(); return; }
  try {
    if ((await stat(file)).isDirectory()) file = join(file, "index.html");
  } catch {
    // SPA fallback for client routes (no extension), 404 otherwise.
    if (extname(file) === "") file = join(dir, "index.html");
  }
  try {
    const body = await readFile(file);
    const extra: Record<string, string> = {};
    if (file.endsWith("sw.js")) extra["Service-Worker-Allowed"] = "/";
    res.writeHead(200, { ...headers, ...extra, "Content-Type": MIME[extname(file)] ?? "application/octet-stream" });
    res.end(body);
  } catch {
    res.writeHead(404, headers);
    res.end("not found");
  }
};
  const server = tls ? createHttpsServer(tls, handler) : createServer(handler);
  return new Promise((ok) => server.listen(port, "127.0.0.1", () => { console.log(`serving ${dir} on ${tls ? "https" : "http"}://localhost:${port} (${variant})`); ok(); }));
}
