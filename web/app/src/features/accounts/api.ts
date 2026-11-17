// Typed client for the Worker `/api/*` routes (architecture.md 4.2, security.md 3.4).
//
// Every call names a route of the `@monoptah/protocol` registry (`endpoint`), so a typo in a method
// or path fails at startup of the call, and the response is validated with the registry schema.
// Non-GET calls always carry `X-Monoptah-CSRF: 1` and `Content-Type: application/json`, even
// without a body. The transport defaults to `fetch`; the dev mock plugs in through `setApiTransport`.
import { API_ERROR_CODES, ApiErrorSchema, findRoute, type ApiErrorCode, type ApiMethod, type ApiRoute } from "@monoptah/protocol";

export type ApiTransport = (url: string, init: RequestInit) => Promise<Response>;

/** Codes the client adds itself: no response at all, or a body that does not match the contract. */
export type ClientErrorCode = "network" | "bad_response";

/** A failed call. `code` is the server's stable key; `message` is the server's human text. */
export class ApiError extends Error {
  readonly status: number;
  readonly code: ApiErrorCode | ClientErrorCode;
  /** Seconds from `Retry-After` (429), else null. */
  readonly retryAfter: number | null;

  constructor(init: { status: number; code: ApiErrorCode | ClientErrorCode; message: string; retryAfter?: number | null }) {
    super(init.message);
    this.name = "ApiError";
    this.status = init.status;
    this.code = init.code;
    this.retryAfter = init.retryAfter ?? null;
  }
}

export function isApiError(value: unknown, code?: ApiErrorCode | ClientErrorCode): value is ApiError {
  return value instanceof ApiError && (code === undefined || value.code === code);
}

const defaultTransport: ApiTransport = (url, init) => fetch(url, init);
let transport: ApiTransport = defaultTransport;

/** Replaces the transport (the dev mock, tests). Returns a function that restores the previous one. */
export function setApiTransport(next: ApiTransport | null): () => void {
  const previous = transport;
  transport = next ?? defaultTransport;
  return () => {
    transport = previous;
  };
}

interface Parseable<T> {
  parse(data: unknown): T;
}

/** A registry route plus the type of its response (and optionally its body). */
export interface Endpoint<Res, Body = undefined> {
  readonly route: ApiRoute;
  readonly response: Parseable<Res>;
  readonly request: Parseable<Body> | undefined;
}

/**
 * Looks the route up in `API_ROUTES` and binds the schemas that give `apiFetch` its types.
 * Pass the same schema the registry uses (`MeResponse` for `GET /api/me`).
 */
export function endpoint<Res, Body = undefined>(
  method: ApiMethod,
  path: string,
  response: Parseable<Res>,
  request?: Parseable<Body>,
): Endpoint<Res, Body> {
  const route = findRoute(method, path);
  if (!route) throw new Error(`Unknown API route ${method} ${path}`);
  return { route, response, request };
}

export interface ApiFetchOptions<Body> {
  params?: Record<string, string>;
  query?: Record<string, string | number | undefined>;
  body?: Body;
  signal?: AbortSignal;
}

function buildPath(path: string, params: Record<string, string> = {}): string {
  return path.replace(/:([A-Za-z0-9_]+)/g, (_match, name: string) => {
    const value = params[name];
    if (value === undefined) throw new Error(`Missing path parameter "${name}" for ${path}`);
    return encodeURIComponent(value);
  });
}

function buildQuery(query: Record<string, string | number | undefined> = {}): string {
  const search = new URLSearchParams();
  for (const [key, value] of Object.entries(query)) {
    if (value !== undefined) search.set(key, String(value));
  }
  const text = search.toString();
  return text ? `?${text}` : "";
}

function parseRetryAfter(header: string | null): number | null {
  if (!header) return null;
  const seconds = Number(header);
  if (Number.isFinite(seconds) && seconds >= 0) return Math.ceil(seconds);
  const date = Date.parse(header);
  if (Number.isNaN(date)) return null;
  return Math.max(0, Math.ceil((date - Date.now()) / 1000));
}

const KNOWN_CODES: ReadonlySet<string> = new Set(API_ERROR_CODES);

async function toApiError(response: Response): Promise<ApiError> {
  const retryAfter = parseRetryAfter(response.headers.get("Retry-After"));
  let body: unknown = null;
  try {
    body = await response.json();
  } catch {
    // Not JSON (a proxy error page): fall through to the generic error below.
  }
  const parsed = ApiErrorSchema.safeParse(body);
  if (parsed.success) {
    return new ApiError({ status: response.status, code: parsed.data.code, message: parsed.data.error, retryAfter });
  }
  // An unknown code from a newer server keeps its text but maps to the generic `internal` key.
  const loose = body as { error?: unknown; code?: unknown } | null;
  const message = typeof loose?.error === "string" ? loose.error : `Request failed (${response.status})`;
  const code: ApiErrorCode =
    typeof loose?.code === "string" && KNOWN_CODES.has(loose.code) ? (loose.code as ApiErrorCode) : "internal";
  return new ApiError({ status: response.status, code, message, retryAfter });
}

/** Calls one registry route. Throws `ApiError` for every failure. */
export async function apiFetch<Res, Body = undefined>(
  ep: Endpoint<Res, Body>,
  options: ApiFetchOptions<Body> = {},
): Promise<Res> {
  const { route } = ep;
  const url = buildPath(route.path, options.params) + buildQuery(options.query);
  const headers: Record<string, string> = { Accept: "application/json" };
  const init: RequestInit = { method: route.method, headers, credentials: "same-origin", signal: options.signal };
  if (route.method !== "GET") {
    headers["Content-Type"] = "application/json";
    headers["X-Monoptah-CSRF"] = "1";
    if (options.body !== undefined) {
      const body = ep.request ? ep.request.parse(options.body) : options.body;
      init.body = JSON.stringify(body);
    }
  }

  let response: Response;
  try {
    response = await transport(url, init);
  } catch (error) {
    if (error instanceof DOMException && error.name === "AbortError") throw error;
    throw new ApiError({ status: 0, code: "network", message: "Could not reach the server" });
  }
  if (!response.ok) throw await toApiError(response);

  let json: unknown;
  try {
    json = await response.json();
  } catch {
    throw new ApiError({ status: response.status, code: "bad_response", message: "The server sent an unreadable response" });
  }
  try {
    return ep.response.parse(json);
  } catch {
    throw new ApiError({ status: response.status, code: "bad_response", message: "The server response did not match the contract" });
  }
}
