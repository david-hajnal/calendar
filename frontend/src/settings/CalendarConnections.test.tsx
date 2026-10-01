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

  it("shows an accessible loading state before the connection resolves", async () => {
    let resolveConnection: (value: Response) => void = () => {};
    const connectionPromise = new Promise<Response>((resolve) => {
      resolveConnection = resolve;
    });
    const fetcher = vi.fn(async (input: RequestInfo | URL) => {
      const url = String(input);
      if (url === "/api/v1/auth/session") {
        return new Response(JSON.stringify(session), { status: 200 });
      }
      if (url === "/api/v1/calendar-connections/apple") {
        return connectionPromise;
      }
      return new Response("{}", { status: 200 });
    });
    const { container } = renderConnections(fetcher);

    const status = await screen.findByRole("status");
    expect(status).toHaveTextContent("Loading your Apple Calendar connection…");
    expect(container.querySelector(".calendar-connections")).toHaveAttribute("aria-busy", "true");

    resolveConnection(
      new Response(
        JSON.stringify({
          server_url: "http://127.0.0.1:3100/dav/",
          principal_id: "abc",
          credentials: [],
          last_successful_access_at: null,
        }),
        { status: 200 },
      ),
    );

    expect(await screen.findByText("No connected devices yet.")).toBeInTheDocument();
  });

  it("shows a recoverable attention state when the connection status fails to load", async () => {
    const fetcher = vi.fn(async (input: RequestInfo | URL) => {
      const url = String(input);
      if (url === "/api/v1/auth/session") {
        return new Response(JSON.stringify(session), { status: 200 });
      }
      if (url === "/api/v1/calendar-connections/apple") {
        return new Response(null, { status: 500 });
      }
      return new Response("{}", { status: 200 });
    });
    renderConnections(fetcher);

    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("We could not load your Apple Calendar connection.");
    expect(alert).toHaveClass("app-message", "app-message--error");
    expect(screen.getByRole("button", { name: "Retry" })).toBeInTheDocument();
  });

  it("shows a recoverable attention state when a mutation fails", async () => {
    const fetcher = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = String(input);
      if (url === "/api/v1/auth/session") {
        return new Response(JSON.stringify(session), { status: 200 });
      }
      if (url === "/api/v1/calendar-connections/apple/passwords/1" && init?.method === "DELETE") {
        return new Response(null, { status: 500 });
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

    await screen.findByText("My iPhone");
    fireEvent.click(screen.getByRole("button", { name: "Revoke" }));

    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("We could not revoke that password. Please try again.");
    expect(alert).toHaveClass("app-message", "app-message--error");
  });

  it("revoking one device preserves the other connected devices", async () => {
    let revoked = false;
    const fetcher = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = String(input);
      if (url === "/api/v1/auth/session") {
        return new Response(JSON.stringify(session), { status: 200 });
      }
      if (url === "/api/v1/calendar-connections/apple/passwords/1" && init?.method === "DELETE") {
        revoked = true;
        return new Response(null, { status: 204 });
      }
      if (url === "/api/v1/calendar-connections/apple") {
        const credentials = revoked
          ? [{ id: 2, label: "Work Mac", created_at: 1000, last_used_at: null }]
          : [
              { id: 1, label: "My iPhone", created_at: 1000, last_used_at: null },
              { id: 2, label: "Work Mac", created_at: 1000, last_used_at: null },
            ];
        return new Response(
          JSON.stringify({
            server_url: "http://127.0.0.1:3100/dav/",
            principal_id: "abc",
            credentials,
            last_successful_access_at: null,
          }),
          { status: 200 },
        );
      }
      return new Response("{}", { status: 200 });
    });
    renderConnections(fetcher);

    await screen.findByText("My iPhone");
    expect(screen.getByText("Work Mac")).toBeInTheDocument();
    expect(screen.getAllByRole("button", { name: "Revoke" })).toHaveLength(2);

    fireEvent.click(screen.getAllByRole("button", { name: "Revoke" })[0]);

    expect(await screen.findByText("Work Mac")).toBeInTheDocument();
    expect(screen.queryByText("My iPhone")).not.toBeInTheDocument();
    expect(screen.getAllByRole("button", { name: "Revoke" })).toHaveLength(1);
    expect(fetcher).toHaveBeenCalledWith(
      "/api/v1/calendar-connections/apple/passwords/1",
      expect.objectContaining({ method: "DELETE" }),
    );
  });

  it("requires confirmation and clears the connected state on disconnect-all", async () => {
    const confirmSpy = vi.spyOn(window, "confirm").mockReturnValue(true);
    let disconnected = false;
    const fetcher = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = String(input);
      if (url === "/api/v1/auth/session") {
        return new Response(JSON.stringify(session), { status: 200 });
      }
      if (url === "/api/v1/calendar-connections/apple" && init?.method === "DELETE") {
        disconnected = true;
        return new Response(null, { status: 204 });
      }
      if (url === "/api/v1/calendar-connections/apple") {
        const credentials = disconnected
          ? []
          : [{ id: 1, label: "My iPhone", created_at: 1000, last_used_at: null }];
        return new Response(
          JSON.stringify({
            server_url: "http://127.0.0.1:3100/dav/",
            principal_id: "abc",
            credentials,
            last_successful_access_at: null,
          }),
          { status: 200 },
        );
      }
      return new Response("{}", { status: 200 });
    });
    renderConnections(fetcher);

    await screen.findByText("My iPhone");
    fireEvent.click(screen.getByRole("button", { name: "Disconnect Apple Calendar" }));

    expect(confirmSpy).toHaveBeenCalled();
    expect(await screen.findByText("No connected devices yet.")).toBeInTheDocument();
    expect(screen.queryAllByRole("button", { name: "Revoke" })).toHaveLength(0);
    expect(fetcher).toHaveBeenCalledWith(
      "/api/v1/calendar-connections/apple",
      expect.objectContaining({ method: "DELETE" }),
    );
    confirmSpy.mockRestore();
  });
});
