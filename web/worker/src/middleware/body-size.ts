// Request body size limit for every module route. The Zod bounds in `@monoptah/protocol` run inside the handlers,
// after the body is buffered and parsed, so they cannot bound the body itself: this does. A `Content-Length` over
// the limit is refused at once; without one (chunked) the body is read through a counter that stops at the limit.
import { bodyLimit } from "hono/body-limit";
import type { MiddlewareHandler } from "hono";
import { apiError, type AppEnv } from "../http.ts";

/**
 * The largest JSON body of any route. The biggest legitimate one is a WebAuthn attestation (credential ids up to
 * 1023 bytes twice, the attestation object and client data: a few KiB); 32 KiB leaves ample headroom.
 */
export const MAX_BODY_BYTES = 32 * 1024;

export const bodySize: MiddlewareHandler<AppEnv> = bodyLimit({
  maxSize: MAX_BODY_BYTES,
  onError: (c) => apiError(c, 413, "invalid", "Request body too large"),
});
