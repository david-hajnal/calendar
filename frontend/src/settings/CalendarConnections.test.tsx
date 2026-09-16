import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { createApiClient, type Fetcher } from "../auth/api";
import { AuthProvider } from "../auth/session";
import { ThemeProvider } from "../theme/themeContext";
import { CalendarConnections } from "./CalendarConnections";

const user = { id: 7, email: "person@example.test", display_name: "Person", is_superadmin: false };
const session = { user, csrf_token: "test-csrf", created_at: 1, last_seen_at: 2, expires_at: 3 };

function renderConnections(fetcher: Fetcher) {
  const api = createApiClient(fetcher);
  return render(
    <ThemeProvider>
      <AuthProvider fetcher={fetcher}>
        <CalendarConnections api={api} />
      </AuthProvider>
    </ThemeProvider>,
  );
}

afterEach(() => {
  cleanup();
});

describe("CalendarConnections", () => {
  it("renders the server url, username, and generate form", async () => {
    const fetcher = vi.fn(async (input: RequestInfo | URL) => {
      const url = String(input);
      if (url === "/api/v1/auth/session") {
        return new Response(JSON.stringify(session), { status: 200 });
      }
      if (url === "/api/v1/calendar-connections/apple") {
        return new Response(
          JSON.stringify({
            server_url: "http://127.0.0.1:3100/dav/",
            principal_id: "abc",
            credentials: [],
            last_successful_access_at: null,
          }),
          { status: 200 },
        );
      }
      return new Response("{}", { status: 200 });
    });
    renderConnections(fetcher);

    expect(await screen.findByRole("heading", { name: "Apple Calendar" })).toBeInTheDocument();
    expect(screen.getByText("http://127.0.0.1:3100/dav/")).toBeInTheDocument();
    expect(screen.getByText("person@example.test")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Generate password" })).toBeInTheDocument();
    expect(screen.getByText("No connected devices yet.")).toBeInTheDocument();
  });

  it("shows the generated secret once with a copy affordance and a do-not-store warning", async () => {
    const fetcher = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = String(input);
      if (url === "/api/v1/auth/session") {
        return new Response(JSON.stringify(session), { status: 200 });
      }
      if (url === "/api/v1/calendar-connections/apple/passwords" && init?.method === "POST") {
        return new Response(
          JSON.stringify({
            password: { id: 1, label: "My iPhone", created_at: 1000, last_used_at: null },
            clear_password: "super-secret-password",
            server_url: "http://127.0.0.1:3100/dav/",
            username: "person@example.test",
          }),
          { status: 201 },
        );
      }
      if (url === "/api/v1/calendar-connections/apple") {
        return new Response(
          JSON.stringify({
            server_url: "http://127.0.0.1:3100/dav/",
            principal_id: "abc",
            credentials: [{ id: 1, label: "My iPhone", created_at: 1000, last_used_at: null }],
            last_successful_access_at: null,
          }),
          { status: 200 },
        );
      }
      return new Response("{}", { status: 200 });
    });
    renderConnections(fetcher);

    await screen.findByRole("heading", { name: "Apple Calendar" });
    fireEvent.change(screen.getByRole("textbox", { name: /Device label/ }), {
      target: { value: "My iPhone" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Generate password" }));

    expect(await screen.findByText("super-secret-password")).toBeInTheDocument();
    expect(screen.getByText(/shown only once/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Copy" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Revoke" })).toBeInTheDocument();
  });

  it("lists existing credentials with revoke actions", async () => {
    const fetcher = vi.fn(async (input: RequestInfo | URL) => {
      const url = String(input);
      if (url === "/api/v1/auth/session") {
        return new Response(JSON.stringify(session), { status: 200 });
      }
      if (url === "/api/v1/calendar-connections/apple") {
        return new Response(
          JSON.stringify({
            server_url: "http://127.0.0.1:3100/dav/",
            principal_id: "abc",
            credentials: [
              { id: 1, label: "My iPhone", created_at: 1000, last_used_at: 2000 },
              { id: 2, label: "Work Mac", created_at: 1000, last_used_at: null },
            ],
            last_successful_access_at: 2000,
          }),
          { status: 200 },
        );
      }
      return new Response("{}", { status: 200 });
    });
    renderConnections(fetcher);

    expect(await screen.findByText("My iPhone")).toBeInTheDocument();
    expect(screen.getByText("Work Mac")).toBeInTheDocument();
    expect(screen.getAllByRole("button", { name: "Revoke" })).toHaveLength(2);
  });
});
