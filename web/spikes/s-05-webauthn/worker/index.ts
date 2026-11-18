// Throwaway spike Worker for S-05, never deployed by the agent. It exists so the maintainer can read the production CPU
// time of one WebAuthn verification (FINDINGS.md, PENDING-MAC).
//
//   GET /verify?alg=ES256|RS256|Ed25519&op=auth|reg
//
// The first request of an isolate builds a fixture (key generation, a registration and an assertion from a software
// authenticator; expensive, discard it). Every later request only runs `verifyAuthenticationResponse` (op=auth) or
// `verifyRegistrationResponse` (op=reg) on the pre-built response, so its CPU time is the library's verification cost.
// The stored counter stays 0, so the same assertion can be verified again and again.
import type { Algorithm } from "../src/authenticator.ts";
import { buildFixture, verifyFixture, type Fixture } from "../src/ceremony.ts";

const fixtures = new Map<Algorithm, Promise<Fixture>>();

export default {
  async fetch(request: Request): Promise<Response> {
    const url = new URL(request.url);
    if (url.pathname === "/probe") {
      const g = globalThis as Record<string, unknown>;
      return Response.json({ Buffer: typeof g["Buffer"], process: typeof g["process"] });
    }
    const alg = url.searchParams.get("alg") ?? "ES256";
    if (alg !== "ES256" && alg !== "RS256" && alg !== "Ed25519") return new Response("bad alg", { status: 400 });
    const op = url.searchParams.get("op") === "reg" ? "reg" : "auth";
    const cached = fixtures.get(alg);
    const first = cached === undefined;
    const fixture = cached ?? buildFixture(alg);
    if (first) fixtures.set(alg, fixture);
    const verified = await verifyFixture(await fixture, op);
    return Response.json({ alg, op, verified, fixtureBuiltInThisRequest: first });
  },
} satisfies ExportedHandler;
