import { env } from "cloudflare:workers";
import worker from "../src/index.ts";

export const ORIGIN = env.RP_ORIGIN;

/** Sends a request through the Worker's `fetch` handler with the test bindings. */
export function call(path: string, init: RequestInit = {}): Promise<Response> {
  return Promise.resolve(worker.fetch(new Request(`${ORIGIN}${path}`, init), env, {
    waitUntil() {},
    passThroughOnException() {},
  } as unknown as ExecutionContext));
}

/** Headers a legitimate browser call carries on a non-GET cookie route. */
export const BROWSER_HEADERS = { Origin: ORIGIN, "X-Monoptah-CSRF": "1" } as const;
