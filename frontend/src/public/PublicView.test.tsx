import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { PublicViewPage } from "./PublicView";

afterEach(cleanup);

describe("PublicViewPage", () => {
  it("renders only its public allowlist, even when a response contains private fields", async () => {
    const fetcher = vi.fn()
      .mockResolvedValueOnce(new Response(JSON.stringify({ name: "Published view", projection: "full_details", display_timezone: "UTC", expires_at: 2_000, owner_user_id: 7 }), { status: 200 }))
      .mockResolvedValueOnce(new Response(JSON.stringify([{ title: "Planning", description: "Visible detail", location: "Room 1", event_kind: "timed", start_utc: 100, end_utc: 200, timezone: "UTC", calendar_id: 4, created_by_user_id: 7 }]), { status: 200 }));

    render(<PublicViewPage token="public-token" fetcher={fetcher} now={() => 100} />);

    expect(await screen.findByRole("heading", { name: "Published view" })).toBeInTheDocument();
    expect(screen.getByText("Planning")).toBeInTheDocument();
    expect(screen.getByText("Visible detail")).toBeInTheDocument();
    expect(screen.queryByText(/owner_user_id|calendar_id|created_by_user_id/)).not.toBeInTheDocument();
    expect(fetcher).toHaveBeenCalledWith("/api/v1/public/views/public-token", { credentials: "omit" });
  });

  it("offers unauthenticated month and mobile-friendly agenda views without widening each projection", async () => {
    const fetcher = vi.fn()
      .mockResolvedValueOnce(new Response(JSON.stringify({ name: "Busy schedule", projection: "free_busy", display_timezone: "UTC", expires_at: 2_000 }), { status: 200 }))
      .mockResolvedValueOnce(new Response(JSON.stringify([{ title: "Private meeting", description: "Secret", location: "Hidden", event_kind: "timed", start_utc: 100, end_utc: 200, busy: true }]), { status: 200 }));

    render(<PublicViewPage token="public-token" fetcher={fetcher} now={() => 100} />);

    expect(await screen.findByRole("region", { name: "Public month" })).toBeInTheDocument();
    expect(screen.getByText("Busy")).toBeInTheDocument();
    expect(screen.queryByText("Private meeting")).not.toBeInTheDocument();
    expect(screen.queryByText("Secret")).not.toBeInTheDocument();
    expect(screen.queryByText("Hidden")).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Agenda view" }));
    expect(await screen.findByRole("region", { name: "Public agenda" })).toBeInTheDocument();
    expect(screen.getByRole("list", { name: "Public agenda events" })).toBeInTheDocument();
    for (const [, options] of fetcher.mock.calls) expect(options).toEqual({ credentials: "omit" });
  });

  it("renders title-and-time events without full-detail fields or authenticated mutations", async () => {
    const fetcher = vi.fn()
      .mockResolvedValueOnce(new Response(JSON.stringify({ name: "Team schedule", projection: "title_and_time", display_timezone: "UTC", expires_at: 2_000 }), { status: 200 }))
      .mockResolvedValueOnce(new Response(JSON.stringify([{ title: "Standup", description: "Internal notes", location: "Private room", status: "confirmed", event_kind: "timed", start_utc: 100, end_utc: 200, timezone: "UTC", calendar_id: 4, recurrence_rule: "FREQ=DAILY" }]), { status: 200 }));

    render(<PublicViewPage token="public-token" fetcher={fetcher} now={() => 100} />);

    expect(await screen.findByText("Standup")).toBeInTheDocument();
    expect(screen.queryByText("Internal notes")).not.toBeInTheDocument();
    expect(screen.queryByText("Private room")).not.toBeInTheDocument();
    await waitFor(() => expect(fetcher).toHaveBeenCalledTimes(2));
    for (const [path, options] of fetcher.mock.calls) {
      expect(String(path)).toMatch(/^\/api\/v1\/public\/views\/public-token/);
      expect(options).toEqual({ credentials: "omit" });
    }
  });

  it("uses the same generic message for an expired, revoked, or otherwise unavailable link", async () => {
    const fetcher = vi.fn().mockResolvedValue(new Response(JSON.stringify({ code: "not_found" }), { status: 404 }));
    render(<PublicViewPage token="expired-token" fetcher={fetcher} now={() => 100} />);

    expect(await screen.findByRole("alert")).toHaveTextContent("This public link is unavailable.");
  });

  it("loads the public data once when callers use the default clock", async () => {
    const fetcher = vi.fn((input: RequestInfo | URL) => Promise.resolve(new Response(JSON.stringify(String(input).endsWith("public-token")
      ? { name: "Published view", projection: "title_and_time", display_timezone: "UTC", expires_at: 2_000 }
      : []), { status: 200 })));
    render(<PublicViewPage token="public-token" fetcher={fetcher} />);

    await screen.findByRole("heading", { name: "Published view" });
    await waitFor(() => expect(fetcher).toHaveBeenCalledTimes(2));
  });
});
