import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { App } from "./App";
import type { Fetcher } from "./auth/api";
import type { Session } from "./auth/session";

const user = { id: 7, email: "person@example.test", display_name: "Person", is_superadmin: false };
function makeSession(overrides?: Partial<Session>) {
  return { user, csrf_token: "test-csrf", created_at: 1, last_seen_at: 2, expires_at: 3, ...overrides };
}
const session = makeSession();

function renderAt(path: string, fetcher: Fetcher) {
  window.history.replaceState({}, "", path);
  return render(<App fetcher={fetcher} />);
}

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
  window.history.replaceState({}, "", "/");
});

describe("authentication pages", () => {
  it("preserves a consent continuation in the emailed login request", async () => {
    const fetcher = vi.fn().mockResolvedValueOnce(new Response(null, { status: 401 })).mockResolvedValueOnce(new Response("{}", { status: 200 }));
    renderAt("/login?redirect=%2Fconsent%3Fhandoff%3Dabc", fetcher);
    fireEvent.change(await screen.findByLabelText(/Email address/), { target: { value: "person@example.test" } });
    fireEvent.click(screen.getByRole("button", { name: "Email me a login link" }));
    await screen.findByRole("status");
    expect(fetcher).toHaveBeenCalledWith("/api/v1/auth/login-links", expect.objectContaining({ body: JSON.stringify({ email: "person@example.test", redirect: "/consent?handoff=abc" }) }));
  });

  it("ignores an external continuation in the emailed login request", async () => {
    const fetcher = vi.fn().mockResolvedValueOnce(new Response(null, { status: 401 })).mockResolvedValueOnce(new Response("{}", { status: 200 }));
    renderAt("/login?redirect=https%3A%2F%2Fevil.example", fetcher);
    fireEvent.change(await screen.findByLabelText(/Email address/), { target: { value: "person@example.test" } });
    fireEvent.click(screen.getByRole("button", { name: "Email me a login link" }));
    await screen.findByRole("status");
    expect(fetcher).toHaveBeenCalledWith("/api/v1/auth/login-links", expect.objectContaining({ body: JSON.stringify({ email: "person@example.test" }) }));
  });

  it("password login exposes the exact password label without its decorative icon", async () => {
    renderAt("/login", vi.fn().mockResolvedValue(new Response(null, { status: 401 })));
    await screen.findByRole("heading", { name: "Sign in" });
    fireEvent.click(screen.getByRole("button", { name: "Password" }));
    const password = screen.getByLabelText("Password", { exact: true });
    expect(password).toHaveAccessibleName("Password");
  });

  it("exposes stable application and card styling hooks on the login page", async () => {
    const { container } = renderAt("/login", vi.fn().mockResolvedValue(new Response(null, { status: 401 })));
    await screen.findByRole("heading", { name: "Sign in" });

    expect(screen.getByRole("main")).toHaveClass("app-page", "app-page--auth");
    expect(container.querySelector(".auth-card")).toBeInTheDocument();
    expect(container.querySelector("form")).toHaveClass("auth-form");
  });

  it("renders the authenticated shell with the current user and logs out", async () => {
    const fetcher = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = String(input);
      if (url === "/api/v1/auth/session" && init?.method === "DELETE") {
        return new Response(null, { status: 204 });
      }
      if (url === "/api/v1/auth/session") {
        return new Response(JSON.stringify(session), { status: 200 });
      }
      return new Response(JSON.stringify([]), { status: 200 });
    });
    renderAt("/calendars", fetcher);

    expect(await screen.findByRole("heading", { name: "CommonCal" })).toBeInTheDocument();
    expect(screen.getByRole("main")).toHaveClass("app-shell");
    expect(screen.getByRole("navigation", { name: "Primary navigation" })).toHaveClass("app-nav");
    expect(screen.getByText("Person")).toBeInTheDocument();
    expect(screen.getByText("person@example.test")).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Sign out" }));

    expect(await screen.findByRole("heading", { name: "Sign in" })).toBeInTheDocument();
    expect(fetcher).toHaveBeenCalledWith("/api/v1/auth/session", expect.objectContaining({ method: "DELETE" }));
  });

  it("returns an expired session to login with its safe relative destination", async () => {
    renderAt("/calendars?view=week#today", vi.fn().mockResolvedValue(new Response(null, { status: 401 })));

    expect(await screen.findByRole("heading", { name: "Sign in" })).toBeInTheDocument();
    expect(window.location.pathname).toBe("/login");
    expect(new URLSearchParams(window.location.search).get("redirect")).toBe("/calendars?view=week#today");
  });

  it("does not honor an unsafe redirect target after authentication", async () => {
    const fetcher = vi.fn()
      .mockResolvedValueOnce(new Response(JSON.stringify({ user, csrf_token: "csrf" }), { status: 200 }))
      .mockResolvedValueOnce(new Response(JSON.stringify(session), { status: 200 }));
    renderAt("/login/consume?token=secret&redirect=https%3A%2F%2Fevil.example", fetcher);

    expect(await screen.findByRole("heading", { name: "CommonCal" })).toBeInTheDocument();
    expect(window.location.pathname).toBe("/");
    expect(new URLSearchParams(window.location.search).get("view")).toBe("month");
    expect(new URLSearchParams(window.location.search).has("redirect")).toBe(false);
    expect(new URLSearchParams(window.location.search).has("token")).toBe(false);
  });

  it("resumes a safe redirect after login-link authentication", async () => {
    const fetcher = vi.fn()
      .mockResolvedValueOnce(new Response(JSON.stringify({ user, csrf_token: "csrf" }), { status: 200 }))
      .mockResolvedValueOnce(new Response(JSON.stringify(session), { status: 200 }));
    renderAt("/login/consume?token=secret&redirect=%2Fcalendars%3Fview%3Dweek", fetcher);

    expect(await screen.findByRole("heading", { name: "CommonCal" })).toBeInTheDocument();
    expect(`${window.location.pathname}${window.location.search}`).toBe("/calendars?view=week");
  });

  it("shows an accessible session-loading state and recoverable session error", async () => {
    const fetcher = vi.fn().mockResolvedValue(new Response(null, { status: 500 }));
    renderAt("/", fetcher);

    expect(screen.getByRole("status")).toHaveTextContent("Loading your session…");
    expect(screen.getByRole("main")).toHaveClass("app-page", "app-page--state");
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("We could not load your session.");
    expect(alert).toHaveClass("app-message", "app-message--error");
    expect(screen.getByRole("button", { name: "Retry" })).toBeInTheDocument();
  });

  it("confirms every login-link request generically", async () => {
    const fetcher = vi.fn().mockResolvedValueOnce(new Response(null, { status: 401 })).mockResolvedValueOnce(
      new Response(JSON.stringify({ message: "If the account is eligible, a login link will be sent" }), { status: 202 }),
    );
    renderAt("/login", fetcher);

    fireEvent.change(await screen.findByRole("textbox", { name: /Email address/ }), { target: { value: "unknown@example.test" } });
    fireEvent.click(screen.getByRole("button", { name: "Email me a login link" }));

    expect(await screen.findByRole("status")).toHaveTextContent("Check your email for a login link if the account is eligible.");
    expect(screen.queryByText("unknown@example.test")).not.toBeInTheDocument();
    expect(fetcher).toHaveBeenLastCalledWith("/api/v1/auth/login-links", expect.objectContaining({
      method: "POST", body: JSON.stringify({ email: "unknown@example.test" }),
    }));
  });

  it("previews an invitation without accepting it or establishing a session", async () => {
    const fetcher = vi.fn().mockResolvedValueOnce(new Response(JSON.stringify({ email: "invitee@example.test" }), { status: 200 }));
    renderAt("/invitations/consume?token=secret", fetcher);
    expect(await screen.findByText("invitee@example.test")).toBeInTheDocument();
    expect(fetcher).toHaveBeenCalledTimes(1);
    expect(fetcher).toHaveBeenCalledWith("/api/v1/auth/invitations/preview?token=secret", expect.anything());
    expect(window.location.search).toBe("");
    expect(screen.getByRole("button", { name: "Create account" })).toBeInTheDocument();
  });

  it("shows an invitation preview failure without consuming it", async () => {
    const fetcher = vi.fn().mockResolvedValueOnce(new Response(null, { status: 401 }));
    renderAt("/invitations/consume?token=bad", fetcher);

    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("Invitation is invalid or expired.");
    expect(alert).toHaveClass("app-message", "app-message--error");
    expect(fetcher).toHaveBeenCalledTimes(1);
    expect(window.location.search).toBe("");
  });

  it("consumes a login link and establishes the session without storing the token", async () => {
    const storage = vi.spyOn(Storage.prototype, "setItem");
    const fetcher = vi.fn()
      .mockResolvedValueOnce(new Response(JSON.stringify({ user, csrf_token: "csrf" }), { status: 200 }))
      .mockResolvedValueOnce(new Response(JSON.stringify(session), { status: 200 }));
    renderAt("/login/consume?token=secret", fetcher);

    expect(await screen.findByText("You are signed in.")).toBeInTheDocument();
    expect(fetcher).toHaveBeenCalledWith("/api/v1/auth/login-links/consume", expect.objectContaining({ body: JSON.stringify({ token: "secret" }) }));
    expect(window.location.search).toBe("");
    expect(storage).not.toHaveBeenCalled();
  });

  it("shows a login-link failure after consuming the token once", async () => {
    const fetcher = vi.fn().mockResolvedValueOnce(new Response(null, { status: 401 }));
    renderAt("/login/consume?token=bad", fetcher);

    expect(await screen.findByRole("alert")).toHaveTextContent("Login link is invalid or expired.");
    expect(fetcher).toHaveBeenCalledTimes(1);
    expect(window.location.search).toBe("");
  });
});

describe("routing", () => {
  it("shows the default calendar view at /dashboard", async () => {
    let calendarsResolved = false;
    const fetcher = vi.fn().mockImplementation(async (input: RequestInfo | URL) => {
      const url = String(input);
      if (url === "/api/v1/auth/session") {
        return new Response(JSON.stringify(makeSession()), { status: 200 });
      }
      if (url === "/api/v1/calendars") {
        // Delay calendar resolution so the loading state is visible
        if (!calendarsResolved) {
          calendarsResolved = true;
          await new Promise((r) => setTimeout(r, 50));
        }
      }
      return new Response(JSON.stringify([]), { status: 200 });
    });
    renderAt("/dashboard", fetcher);

    expect(await screen.findByText("Loading calendars…")).toBeInTheDocument();
  });

  it("shows the composite view management page at /shared", async () => {
    const fetcher = vi.fn(async (input: RequestInfo | URL) => {
      const url = String(input);
      if (url === "/api/v1/auth/session") {
        return new Response(JSON.stringify(session), { status: 200 });
      }
      return new Response(JSON.stringify([]), { status: 200 });
    });
    renderAt("/shared", fetcher);

    expect(await screen.findByText("Composite views")).toBeInTheDocument();
  });

  it("navigates to /shared when clicking Composite views button", async () => {
    const fetcher = vi.fn().mockImplementation(async (input: RequestInfo | URL) => {
      const url = String(input);
      if (url === "/api/v1/auth/session") {
        return new Response(JSON.stringify(makeSession()), { status: 200 });
      }
      return new Response(JSON.stringify([]), { status: 200 });
    });
    renderAt("/calendars", fetcher);

    await screen.findByText("CommonCal");
    fireEvent.click(screen.getByRole("button", { name: "Composite views" }));

    expect(window.location.pathname).toBe("/shared");
  });

  it("redirects unauthenticated users to /dashboard", async () => {
    renderAt("/", vi.fn().mockResolvedValue(new Response(null, { status: 401 })));

    expect(await screen.findByRole("heading", { name: "Sign in" })).toBeInTheDocument();
    expect(new URLSearchParams(window.location.search).get("redirect")).toBe("/");
  });

  it("settings opens Account and retains navigation to calendar connections", async () => {
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
      return new Response(JSON.stringify([]), { status: 200 });
    });
    renderAt("/calendars", fetcher);

    await screen.findByText("CommonCal");
    fireEvent.click(screen.getByRole("button", { name: "Settings" }));

    expect(window.location.pathname).toBe("/settings/account");
    expect(await screen.findByRole("heading", { name: "Account" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Users" })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Calendar connections" }));
    expect(window.location.pathname).toBe("/settings/calendar-connections");
    expect(await screen.findByRole("heading", { name: "Apple Calendar" })).toBeInTheDocument();
  });
});

it("password login links to recovery and reset pages do not load an existing session", async () => {
  const fetcher = vi.fn(async () => new Response(null, { status: 401 }));
  renderAt("/login", fetcher);
  await screen.findByRole("heading", { name: "Sign in" });
  fireEvent.click(screen.getByRole("button", { name: /^Password$/ }));
  expect(screen.getByRole("link", { name: "Forgot password?" })).toHaveAttribute("href", "/forgot-password");
  cleanup(); fetcher.mockClear();
  renderAt("/password-reset?token=secret", fetcher);
  expect(await screen.findByRole("heading", { name: "Set a new password" })).toBeInTheDocument();
  expect(window.location.search).toBe("");
  expect(fetcher).not.toHaveBeenCalled();
});

describe("live mobile Agenda", () => {
  it("defaults to readable live events without sample content", async () => {
    vi.stubGlobal("innerWidth", 390);
    const fetcher = vi.fn(async (input: RequestInfo | URL) => new Response(JSON.stringify(
      String(input) === "/api/v1/auth/session" ? session
      : String(input) === "/api/v1/calendars" ? [{ id: 1, name: "Work", color: "#2563eb", role: "owner", access: "details" }]
      : String(input).includes("/events?") ? [{ id: 10, calendar_id: 1, access: "details", status: "confirmed", event_kind: "timed", title: "Live planning with a readable long title", start_utc: Date.parse("2026-10-09T09:00:00Z") / 1000, end_utc: Date.parse("2026-10-09T10:00:00Z") / 1000, version: 1 }] : []
    ), { status: 200 }));
    renderAt("/dashboard?date=2026-10-09", fetcher);
    expect(await screen.findByRole("region", { name: "Agenda" })).toBeInTheDocument();
    expect(await screen.findByRole("button", { name: /Live planning with a readable long title/ })).toBeInTheDocument();
    expect(screen.queryByText("Sample Agenda")).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Preview mobile Agenda" })).not.toBeInTheDocument();
    expect(new URLSearchParams(window.location.search).get("view")).toBe("agenda");
  });
});

it("preserves an explicit mobile view, date, extra query, and empty filters through Settings", async () => {
  vi.stubGlobal("innerWidth", 390);
  const fetcher = vi.fn(async (input: RequestInfo | URL) => new Response(JSON.stringify(
    String(input) === "/api/v1/auth/session" ? session
    : String(input) === "/api/v1/calendars" ? [{ id: 1, name: "Work", color: "#2563eb", role: "owner", access: "details" }] : []
  ), { status: 200 }));
  renderAt("/dashboard?view=day&date=2026-10-15&source=bookmark", fetcher);
  await screen.findByRole("region", { name: "Day calendar" });
  // The mobile filter supplements the desktop sidebar, which CSS hides on phones.
  fireEvent.click(screen.getAllByRole("checkbox", { name: "Work" }).at(-1)!);
  fireEvent.click(screen.getByRole("button", { name: "Settings" }));
  fireEvent.click(await screen.findByRole("button", { name: "Return to calendar" }));
  await screen.findByRole("region", { name: "Day calendar" });
  expect(new URLSearchParams(window.location.search).get("date")).toBe("2026-10-15");
  expect(new URLSearchParams(window.location.search).get("source")).toBe("bookmark");
  expect(screen.getAllByRole("checkbox", { name: "Work" }).at(-1)).not.toBeChecked();
  expect(window.history.state.commoncalCalendar.snapshot.visibleCalendarIds).toEqual([]);
});

it("waits for session resolution at login and replaces signed-in login with its continuation", async () => {
  let resolveSession!: (response: Response) => void;
  const fetcher = vi.fn((input: RequestInfo | URL) => String(input) === "/api/v1/auth/session"
    ? new Promise<Response>(resolve => { resolveSession = resolve; })
    : Promise.resolve(new Response("[]", { status: 200 })));
  renderAt("/login?redirect=%2Fsettings%2Faccount", fetcher);
  expect(screen.queryByRole("heading", { name: "Sign in" })).not.toBeInTheDocument();
  expect(screen.getByRole("status")).toHaveTextContent("Loading your session");
  resolveSession(new Response(JSON.stringify(session), { status: 200 }));
  expect(await screen.findByRole("heading", { name: "Account" })).toBeInTheDocument();
  expect(window.location.pathname).toBe("/settings/account");
  expect(screen.queryByRole("heading", { name: "Sign in" })).not.toBeInTheDocument();
});

it("keeps the authenticated shell and its notification polling stable across section navigation", async () => {
  const fetcher = vi.fn(async (input: RequestInfo | URL) => new Response(JSON.stringify(String(input) === "/api/v1/auth/session" ? session : []), { status: 200 }));
  renderAt("/calendars", fetcher);
  await screen.findByRole("heading", { name: "CommonCal" });
  const shell = screen.getByRole("main");
  const notifications = fetcher.mock.calls.filter(([url]) => String(url).includes("/notifications")).length;
  fireEvent.click(screen.getByRole("button", { name: "Settings" }));
  await screen.findByRole("heading", { name: "Account" });
  expect(screen.getByRole("main")).toBe(shell);
  expect(fetcher.mock.calls.filter(([url]) => String(url).includes("/notifications")).length).toBe(notifications);
});

it("offers session retry at login instead of showing credentials after a session-read error", async () => {
  const fetcher = vi.fn().mockResolvedValueOnce(new Response(null, { status: 500 })).mockResolvedValueOnce(new Response(null, { status: 401 }));
  renderAt("/login", fetcher);
  expect(await screen.findByRole("alert")).toHaveTextContent("We could not load your session");
  expect(screen.queryByLabelText("Email address")).not.toBeInTheDocument();
  fireEvent.click(screen.getByRole("button", { name: "Retry" }));
  expect(await screen.findByRole("heading", { name: "Sign in" })).toBeInTheDocument();
});
