import { env } from "cloudflare:workers";
import worker from "../src/index.ts";

export const ORIGIN = env.RP_ORIGIN;

/** Sends a request through the Worker's `fetch` handler with the test bindings. */
export function call(path: string, init: RequestInit = {}): Promise<Response> {
  return callWithEnv(env, path, init);
}

/**
 * Like `call`, with an environment override. Use it to swap a binding for a test double, for example
 * `ROOM` (see `fakeRoomNamespace` in support/accounts.ts), without touching the real bindings.
 */
export function callWithEnv(testEnv: typeof env, path: string, init: RequestInit = {}): Promise<Response> {
  return Promise.resolve(worker.fetch(new Request(`${ORIGIN}${path}`, init), testEnv, {
    waitUntil() {},
    passThroughOnException() {},
  } as unknown as ExecutionContext));
}

/** Headers a legitimate browser call carries on a non-GET cookie route. */
export const BROWSER_HEADERS = { Origin: ORIGIN, "X-Monoptah-CSRF": "1", "Content-Type": "application/json" } as const;
