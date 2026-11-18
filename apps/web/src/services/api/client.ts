/** Typed error for every non-2xx API response (shape in docs/api.md). */
export class ApiError extends Error {
  readonly status: number;
  readonly code: string;

  constructor(status: number, code: string, message: string) {
    super(message);
    this.name = 'ApiError';
    this.status = status;
    this.code = code;
  }
}

// Used only when the body is not the contract shape (for example a gateway page). A bare 429 or
// 503 does not say which limit was hit, so it gets a neutral code instead of a misleading one.
const FALLBACK_CODES: Record<number, string> = {
  400: 'invalid_request',
  401: 'unauthenticated',
  403: 'forbidden',
  404: 'not_found',
  409: 'conflict',
  413: 'too_large',
  429: 'rate_limited',
  503: 'unavailable',
};

/** Build an ApiError from a failed response. Tolerates bodies that are not the contract shape. */
export async function parseApiError(response: Response): Promise<ApiError> {
  let code: string | undefined;
  let message: string | undefined;
  try {
    const body: unknown = await response.json();
    if (body && typeof body === 'object' && 'error' in body) {
      const err = (body as { error: unknown }).error;
      if (err && typeof err === 'object') {
        const e = err as { code?: unknown; message?: unknown };
        if (typeof e.code === 'string') code = e.code;
        if (typeof e.message === 'string') message = e.message;
      }
    }
  } catch {
    // Not JSON (for example a gateway error page). Use the fallbacks below.
  }
  return new ApiError(
    response.status,
    code ?? FALLBACK_CODES[response.status] ?? 'internal',
    message ?? `Request failed with status ${response.status}`,
  );
}

/** Lowercase hex SHA-256 of the exact bytes. */
export async function sha256Hex(bytes: BufferSource): Promise<string> {
  if (!globalThis.crypto?.subtle) {
    throw new Error('This page needs a secure connection (HTTPS, or localhost) to sign its requests.');
  }
  const digest = await crypto.subtle.digest('SHA-256', bytes);
  return Array.from(new Uint8Array(digest), (b) => b.toString(16).padStart(2, '0')).join('');
}

type TokenProvider = () => Promise<string | null>;
type UnauthenticatedHandler = () => void;

let tokenProvider: TokenProvider = async () => null;
let unauthenticatedHandler: UnauthenticatedHandler = () => {};

/** The auth layer registers how to get a fresh ID token. */
export function setTokenProvider(provider: TokenProvider): void {
  tokenProvider = provider;
}

/** The auth layer registers what to do when the API says the session is not valid. */
export function setUnauthenticatedHandler(handler: UnauthenticatedHandler): void {
  unauthenticatedHandler = handler;
}

export interface RequestOptions {
  method?: string;
  /** JSON-serializable body. Omit for an empty body. */
  body?: unknown;
  signal?: AbortSignal;
  headers?: Record<string, string>;
}

const encoder = new TextEncoder();

/**
 * The one fetch wrapper. It adds `x-auth-token`, the JSON content type, and, for every method other
 * than GET and HEAD, `x-amz-content-sha256` of the exact bytes it sends. It returns the raw
 * response for 2xx and throws ApiError otherwise.
 */
export async function apiRequest(path: string, options: RequestOptions = {}): Promise<Response> {
  const method = (options.method ?? 'GET').toUpperCase();
  const headers = new Headers(options.headers);

  const token = await tokenProvider();
  if (token) headers.set('x-auth-token', token);

  let body: Uint8Array<ArrayBuffer> | undefined;
  if (options.body !== undefined) {
    body = encoder.encode(JSON.stringify(options.body));
    headers.set('content-type', 'application/json');
  }
  if (method !== 'GET' && method !== 'HEAD') {
    headers.set('x-amz-content-sha256', await sha256Hex(body ?? new Uint8Array()));
  }

  const response = await fetch(path, { method, headers, body, signal: options.signal });
  if (!response.ok) {
    const error = await parseApiError(response);
    // Every route except health needs a token, so a 401 always means the session is gone.
    if (error.status === 401 && !path.startsWith('/api/health')) unauthenticatedHandler();
    throw error;
  }
  return response;
}

/** JSON in, JSON out. 204 responses return undefined. */
export async function apiJson<T>(path: string, options: RequestOptions = {}): Promise<T> {
  const response = await apiRequest(path, options);
  if (response.status === 204) return undefined as T;
  return (await response.json()) as T;
}
