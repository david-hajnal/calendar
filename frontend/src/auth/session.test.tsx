import { render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { AuthProvider, useAuth } from "./session";

function SessionStatus() {
  const { state } = useAuth();
  return <output>{state.status === "authenticated" ? state.session.user.email : state.status}</output>;
}

describe("AuthProvider", () => {
  it("loads the current session and exposes its user", async () => {
    const fetcher = vi.fn().mockResolvedValue(
      new Response(
        JSON.stringify({
          user: { id: 7, email: "person@example.test", display_name: "Person", is_superadmin: false },
          created_at: 1,
          last_seen_at: 2,
          expires_at: 3,
        }),
        { status: 200 },
      ),
    );

    render(
      <AuthProvider fetcher={fetcher}>
        <SessionStatus />
      </AuthProvider>,
    );

    await waitFor(() => expect(screen.getByText("person@example.test")).toBeInTheDocument());
    expect(fetcher).toHaveBeenCalledWith(
      "/api/v1/auth/session",
      expect.objectContaining({ credentials: "same-origin" }),
    );
  });

  it("represents an expired or missing session as unauthenticated", async () => {
    render(
      <AuthProvider fetcher={vi.fn().mockResolvedValue(new Response(null, { status: 401 }))}>
        <SessionStatus />
      </AuthProvider>,
    );

    await waitFor(() => expect(screen.getByText("unauthenticated")).toBeInTheDocument());
  });

  it("does not persist session or CSRF data in web storage", async () => {
    const localSetItem = vi.spyOn(Storage.prototype, "setItem");
    const sessionSetItem = vi.spyOn(Storage.prototype, "setItem");

    render(
      <AuthProvider
        fetcher={vi.fn().mockResolvedValue(
          new Response(JSON.stringify({ user: { id: 7, email: "person@example.test", display_name: null, is_superadmin: false }, created_at: 1, last_seen_at: 2, expires_at: 3 }), { status: 200 }),
        )}
      >
        <SessionStatus />
      </AuthProvider>,
    );

    await waitFor(() => expect(screen.getByText("person@example.test")).toBeInTheDocument());
    expect(localSetItem).not.toHaveBeenCalled();
    expect(sessionSetItem).not.toHaveBeenCalled();
  });
});
