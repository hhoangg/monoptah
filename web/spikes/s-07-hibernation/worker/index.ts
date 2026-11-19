import { DurableObject } from "cloudflare:workers";
import { TokenBucket, type BucketState } from "../src/bucket.ts";
import {
  TYPE_STATS,
  checkFrameSize,
  readDeviceId,
  writeDeviceId,
} from "../src/frame.ts";

// S-07 spike relay. NOT the product DesktopRoom: a minimal prototype of the hibernation rules of
// architecture.md 4.4 (no timer, no alarm, no outbound socket; state only in attachments and storage).

interface Env {
  ROOM: DurableObjectNamespace<SpikeRoom>;
  LIMITER?: { limit(options: { key: string }): Promise<{ success: boolean }> };
}

interface Attachment {
  role: "desktop" | "device";
  deviceId: string;
  joinedAt: number;
  bucket: BucketState;
  /** Refill rate of this socket's bucket (tokens/s); a query param so tests can make it clock independent. */
  refill: number;
}

const BUCKET_CAPACITY = 100;
const BUCKET_REFILL_PER_SEC = 40;

export default {
  async fetch(request: Request, env: Env): Promise<Response> {
    const url = new URL(request.url);
    if (url.pathname === "/rl") {
      // S-01 item 2: does the Rate Limiting binding work (locally)?
      if (!env.LIMITER) return new Response("no binding", { status: 501 });
      const { success } = await env.LIMITER.limit({ key: url.searchParams.get("key") ?? "k" });
      return new Response(success ? "ok" : "limited", { status: success ? 200 : 429 });
    }
    const room = env.ROOM.get(env.ROOM.idFromName(url.searchParams.get("room") ?? "r"));
    if (url.pathname === "/stats") return room.fetch(request);
    if (request.headers.get("Upgrade") !== "websocket") return new Response("expected websocket", { status: 426 });
    const role = url.pathname === "/desktop" ? "desktop" : url.pathname === "/device" ? "device" : null;
    if (!role) return new Response("not found", { status: 404 });
    const headers = new Headers(request.headers);
    headers.set("x-role", role);
    headers.set("x-device-id", role === "device" ? (url.searchParams.get("device") ?? "dev") : "");
    headers.set("x-refill", url.searchParams.get("refill") ?? String(BUCKET_REFILL_PER_SEC));
    return room.fetch(new Request(request, { headers }));
  },
} satisfies ExportedHandler<Env>;

export class SpikeRoom extends DurableObject<Env> {
  /** In-memory buckets. Lost on eviction and rebuilt lazily from the attachment. */
  private buckets = new Map<WebSocket, TokenBucket>();
  /** Buckets built in this instance from an attachment, and the tokens of the last one (stats frame). */
  private built = 0;
  private lastBuiltTokens = -1;
  private readonly constructedAt = Date.now();

  constructor(ctx: DurableObjectState, env: Env) {
    super(ctx, env);
    // Constructor counter, in storage: how many times the object was (re)constructed.
    const n = (ctx.storage.kv.get<number>("ctor") ?? 0) + 1;
    ctx.storage.kv.put("ctor", n);
    ctx.setWebSocketAutoResponse(new WebSocketRequestResponsePair("ping", "pong"));
  }

  override async fetch(request: Request): Promise<Response> {
    const url = new URL(request.url);
    if (url.pathname === "/stats") return Response.json(this.stats());
    const role = request.headers.get("x-role") as "desktop" | "device";
    const deviceId = request.headers.get("x-device-id") ?? "";
    const pair = new WebSocketPair();
    const [client, server] = [pair[0], pair[1]];
    const tags = role === "desktop" ? ["desktop"] : ["device", `device:${deviceId}`];
    if (role === "desktop") {
      for (const old of this.ctx.getWebSockets("desktop")) old.close(4009, "replaced");
    }
    this.ctx.acceptWebSocket(server, tags);
    const refill = Number(request.headers.get("x-refill") ?? BUCKET_REFILL_PER_SEC);
    const attachment: Attachment = {
      refill: Number.isFinite(refill) && refill >= 0 ? refill : BUCKET_REFILL_PER_SEC,
      role,
      deviceId,
      joinedAt: Date.now(),
      bucket: new TokenBucket(BUCKET_CAPACITY, refill, Date.now()).snapshot(),
    };
    server.serializeAttachment(attachment);
    return new Response(null, { status: 101, webSocket: client });
  }

  private stats(): Record<string, unknown> {
    return {
      ctor: this.ctx.storage.kv.get<number>("ctor") ?? 0,
      msgs: this.ctx.storage.kv.get<number>("msgs") ?? 0,
      constructedAt: this.constructedAt,
      builtBuckets: this.built,
      lastBuiltTokens: this.lastBuiltTokens,
      sockets: this.ctx.getWebSockets().length,
    };
  }

  private bucketFor(ws: WebSocket, att: Attachment): TokenBucket {
    let bucket = this.buckets.get(ws);
    if (!bucket) {
      bucket = new TokenBucket(BUCKET_CAPACITY, att.refill, Date.now(), att.bucket);
      this.buckets.set(ws, bucket);
      this.built++;
      this.lastBuiltTokens = bucket.snapshot().tokens;
    }
    return bucket;
  }

  override async webSocketMessage(ws: WebSocket, message: string | ArrayBuffer): Promise<void> {
    this.ctx.storage.kv.put("msgs", (this.ctx.storage.kv.get<number>("msgs") ?? 0) + 1);
    const att = ws.deserializeAttachment() as Attachment | null;
    if (!att) {
      ws.close(4012, "no attachment");
      return;
    }
    if (typeof message === "string") {
      ws.close(4012, "text frames are only the ping auto-response");
      return;
    }
    const size = checkFrameSize(message.byteLength);
    if (size === "too_big") {
      ws.close(1009, "message too big");
      return;
    }
    if (size === "too_small") {
      ws.close(4012, "frame too small");
      return;
    }
    const frame = new Uint8Array(message);
    if (frame[1] === TYPE_STATS) {
      const body = new TextEncoder().encode(JSON.stringify(this.stats()));
      const out = new Uint8Array(20 + body.length);
      out[0] = 1;
      out[1] = TYPE_STATS;
      out.set(body, 20);
      ws.send(out);
      return;
    }
    const bucket = this.bucketFor(ws, att);
    if (!bucket.take(1, Date.now())) {
      ws.close(4008, "rate limited");
      return;
    }
    att.bucket = bucket.snapshot();
    ws.serializeAttachment(att);

    if (att.role === "device") {
      writeDeviceId(frame, att.deviceId); // overwrite with the authenticated id
      const desktop = this.ctx.getWebSockets("desktop")[0];
      if (desktop) desktop.send(frame);
      return;
    }
    const target = readDeviceId(frame);
    for (const sock of this.ctx.getWebSockets(`device:${target}`)) sock.send(frame);
  }

  override async webSocketClose(ws: WebSocket, code: number): Promise<void> {
    this.buckets.delete(ws);
    // Echo a valid close code back (1005/1006 cannot be sent).
    try {
      ws.close(code >= 1000 && code !== 1005 && code !== 1006 ? code : 1000, "bye");
    } catch {
      /* already closed */
    }
  }
}
