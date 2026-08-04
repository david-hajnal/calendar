export type Fetcher = (input: RequestInfo | URL, init?: RequestInit) => Promise<Response>;

const unsafeMethods = new Set(["POST", "PUT", "PATCH", "DELETE"]);

export class ApiError extends Error {
  constructor(public readonly response: Response) {
    super(`API request failed with ${response.status}`);
  }
}

export interface ApiClient {
  readonly csrfToken: string | null;
  setCsrfToken(token: string | null): void;
  request(input: RequestInfo | URL, init?: RequestInit): Promise<Response>;
  logout(): Promise<void>;
}

export function createApiClient(fetcher: Fetcher = fetch): ApiClient {
  let csrfToken: string | null = null;

  const request = async (input: RequestInfo | URL, init: RequestInit = {}) => {
    const method = (init.method ?? (input instanceof Request ? input.method : "GET")).toUpperCase();
    const url = new URL(input instanceof Request ? input.url : input.toString(), window.location.origin);
    const headers = new Headers(init.headers);

    if (csrfToken !== null && unsafeMethods.has(method) && url.origin === window.location.origin) {
      headers.set("x-csrf-token", csrfToken);
    }

    return fetcher(input, { ...init, credentials: "same-origin", headers });
  };

  return {
    get csrfToken() {
      return csrfToken;
    },
    setCsrfToken(token) {
      csrfToken = token;
    },
    request,
    async logout() {
      const response = await request("/api/v1/auth/session", { method: "DELETE" });
      if (!response.ok && response.status !== 401) {
        throw new ApiError(response);
      }
      csrfToken = null;
    },
  };
}
