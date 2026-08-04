import { describe, expect, it, vi } from "vitest";

import { createApiClient } from "./api";

describe("API client", () => {
  it("includes the in-memory CSRF token on unsafe same-origin requests", async () => {
    const fetcher = vi.fn().mockResolvedValue(new Response(null, { status: 204 }));
    const api = createApiClient(fetcher);
    api.setCsrfToken("csrf-token");

    await api.request("/api/v1/calendars", { method: "POST" });

    expect(fetcher).toHaveBeenCalledWith("/api/v1/calendars", expect.objectContaining({ credentials: "same-origin" }));
    expect((fetcher.mock.calls[0][1]?.headers as Headers).get("x-csrf-token")).toBe("csrf-token");
  });

  it("logs out through the session endpoint and clears in-memory authentication", async () => {
    const fetcher = vi.fn().mockResolvedValue(new Response(null, { status: 204 }));
    const api = createApiClient(fetcher);
    api.setCsrfToken("csrf-token");

    await api.logout();

    expect(fetcher).toHaveBeenCalledWith("/api/v1/auth/session", expect.objectContaining({ method: "DELETE" }));
    expect((fetcher.mock.calls[0][1]?.headers as Headers).get("x-csrf-token")).toBe("csrf-token");
    expect(api.csrfToken).toBeNull();
  });
});
