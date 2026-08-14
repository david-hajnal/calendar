import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { App } from "./App";
import type { Fetcher } from "./auth/api";

const user = { id: 7, email: "person@example.test", display_name: "Person", is_superadmin: false };
const session = { user, created_at: 1, last_seen_at: 2, expires_at: 3 };

function renderAt(path: string, fetcher: Fetcher) {
  window.history.replaceState({}, "", path);
  return render(<App fetcher={fetcher} />);
}

afterEach(() => {
  cleanup();
  window.history.replaceState({}, "", "/");
});

describe("authentication pages", () => {
  it("exposes stable application and card styling hooks on the login page", async () => {
    const { container } = renderAt("/login", vi.fn());

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
    expect(window.location.search).toBe("");
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

    fireEvent.change(screen.getByRole("textbox", { name: /Email address/ }), { target: { value: "unknown@example.test" } });
    fireEvent.click(screen.getByRole("button", { name: "Email me a login link" }));

    expect(await screen.findByRole("status")).toHaveTextContent("Check your email for a login link if the account is eligible.");
    expect(screen.queryByText("unknown@example.test")).not.toBeInTheDocument();
    expect(fetcher).toHaveBeenLastCalledWith("/api/v1/auth/login-links", expect.objectContaining({
      method: "POST", body: JSON.stringify({ email: "unknown@example.test" }),
    }));
  });

  it("consumes an invitation, establishes the session, and removes its token from the URL", async () => {
    const replaceState = vi.spyOn(window.history, "replaceState");
    const fetcher = vi.fn()
      .mockResolvedValueOnce(new Response(JSON.stringify({ user, csrf_token: "csrf" }), { status: 200 }))
      .mockResolvedValueOnce(new Response(JSON.stringify(session), { status: 200 }));
    renderAt("/invitations/consume?token=secret", fetcher);

    const success = await screen.findByText("Invitation accepted. You are signed in.");
    expect(success).toHaveClass("app-message", "app-message--success");
    expect(screen.getByRole("main")).toHaveClass("app-page", "app-page--auth");
    expect(fetcher).toHaveBeenCalledWith("/api/v1/auth/invitations/consume", expect.objectContaining({ body: JSON.stringify({ token: "secret" }) }));
    expect(replaceState).toHaveBeenLastCalledWith({}, "", "/invitations/consume");
  });

  it("shows an invitation failure after consuming the token once", async () => {
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
    const fetcher = vi.fn().mockResolvedValue(new Response(JSON.stringify(session), { status: 200 }));
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
    const fetcher = vi.fn().mockResolvedValue(new Response(JSON.stringify(session), { status: 200 }));
    renderAt("/calendars", fetcher);

    await screen.findByRole("heading", { name: "CommonCal" });
    fireEvent.click(screen.getByRole("button", { name: "Composite views" }));

    expect(window.location.pathname).toBe("/shared");
  });

  it("redirects unauthenticated users to /dashboard", async () => {
    renderAt("/", vi.fn().mockResolvedValue(new Response(null, { status: 401 })));

    expect(await screen.findByRole("heading", { name: "Sign in" })).toBeInTheDocument();
    expect(new URLSearchParams(window.location.search).get("redirect")).toBe("/");
  });


});
