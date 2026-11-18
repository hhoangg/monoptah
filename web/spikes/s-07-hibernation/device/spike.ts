// S-07 PENDING-DEVICE page: connects to a relay URL and logs every open, close, pong, tick and visibility
// change with timestamps, on screen and in localStorage (so the log survives a killed page). No inline
// script and no innerHTML: it runs under the CSP of web/worker/src/csp.ts (Trusted Types, script-src 'self').

const LOG_KEY = "s07.log";
const urlInput = document.getElementById("url") as HTMLInputElement;
const logEl = document.getElementById("log") as HTMLElement;
const envEl = document.getElementById("env") as HTMLElement;

let socket: WebSocket | null = null;
let lastPongAt = 0;
let lastTickAt = Date.now();

const stamp = (t = Date.now()) => new Date(t).toISOString().slice(11, 23);

function log(line: string): void {
  const text = `${stamp()} ${line}`;
  const lines = (localStorage.getItem(LOG_KEY) ?? "").split("\n").filter(Boolean);
  lines.push(text);
  localStorage.setItem(LOG_KEY, lines.slice(-400).join("\n"));
  render();
}

function render(): void {
  logEl.textContent = (localStorage.getItem(LOG_KEY) ?? "").split("\n").slice(-60).join("\n");
}

function connect(): void {
  socket?.close();
  const ws = new WebSocket(urlInput.value);
  socket = ws;
  ws.addEventListener("open", () => log("ws open"));
  ws.addEventListener("close", (e) => log(`ws close code=${e.code} clean=${e.wasClean}`));
  ws.addEventListener("error", () => log("ws error"));
  ws.addEventListener("message", (e) => {
    if (e.data === "pong") lastPongAt = Date.now();
  });
}

// Every second: note the time. When the page is suspended in the background this stops, so the gap
// between two ticks is the time the page was frozen. Every 5 s also send a ping.
let tickCount = 0;
setInterval(() => {
  const now = Date.now();
  const gap = now - lastTickAt;
  lastTickAt = now;
  tickCount++;
  if (gap > 3000) log(`tick gap ${Math.round(gap / 1000)} s (page was frozen)`);
  if (tickCount % 5 === 0 && socket?.readyState === WebSocket.OPEN) socket.send("ping");
}, 1000);

document.addEventListener("visibilitychange", () => {
  const pongAge = lastPongAt ? `${Math.round((Date.now() - lastPongAt) / 1000)} s` : "never";
  log(`visibility ${document.visibilityState} readyState=${socket?.readyState ?? "none"} lastPongAge=${pongAge}`);
});
window.addEventListener("pagehide", () => log("pagehide"));
window.addEventListener("pageshow", () => log("pageshow"));

const standalone =
  (navigator as Navigator & { standalone?: boolean }).standalone === true ||
  window.matchMedia("(display-mode: standalone)").matches;
envEl.textContent = `standalone=${standalone} ${navigator.userAgent}`;
urlInput.value = new URLSearchParams(location.search).get("url") ?? `wss://${location.host}/device?room=ios&device=ios1`;
document.getElementById("connect")?.addEventListener("click", connect);
document.getElementById("clear")?.addEventListener("click", () => {
  localStorage.removeItem(LOG_KEY);
  render();
});
log("page loaded");
