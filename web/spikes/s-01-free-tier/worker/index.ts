// S-01 CPU probe: a plain fetch handler running exactly ONE indexed D1 query per request.
// Why not Hono: `hono` is not a dependency of web/spikes (no new dependency allowed) and the product routes in
// web/worker need a session, so none of them can be called without auth. Hono's router costs a few
// microseconds; the number that matters for the 10 ms budget is the D1 round trip plus JSON, which this keeps.
// The measured value is therefore a lower bound of a real Hono route by the router cost (state this when recording).

interface Env {
  DB: D1Database;
}

export default {
  async fetch(request: Request, env: Env): Promise<Response> {
    const url = new URL(request.url);
    if (url.pathname !== "/d1") return new Response("not found", { status: 404 });
    // One query, primary key lookup (about 1 row read).
    const row = await env.DB.prepare("SELECT id, name FROM probe WHERE id = ?").bind(1).first<{ id: number; name: string }>();
    return Response.json({ row });
  },
} satisfies ExportedHandler<Env>;
