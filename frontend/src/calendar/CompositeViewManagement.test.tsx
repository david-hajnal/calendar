import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { CompositeViewManagement } from "./CompositeViewManagement";
import type { ApiClient } from "../auth/api";

const work = { id: 1, access: "details", role: "owner", name: "Work", color: "#123456" } as const;
const family = { id: 2, access: "details", role: "viewer", name: "Family", color: "#654321" } as const;
const busyOnly = { id: 3, access: "free_busy", role: "free_busy_viewer", name: "Private", color: "#111111" } as const;
const view = { id: 8, owner_user_id: 7, name: "Week", version: 1, created_at: 1, updated_at: 1, calendars: [{ calendar_id: 1, position: 0, color: "#123456" }] };

function response(value: unknown, status = 200) {
  return new Response(JSON.stringify(value), { status, headers: { "content-type": "application/json" } });
}

function renderManager(request = vi.fn().mockResolvedValue(response([]))) {
  const api = { request, csrfToken: "csrf", setCsrfToken: vi.fn(), logout: vi.fn() } as unknown as ApiClient;
  render(<CompositeViewManagement api={api} />);
  return request;
}

afterEach(cleanup);

describe("CompositeViewManagement", () => {
  it("creates a view and then saves only calendars with detail access", async () => {
    const created = { ...view, id: 9, name: "New view", calendars: [] };
    const request = vi.fn()
      .mockResolvedValueOnce(response([]))
      .mockResolvedValueOnce(response([work, family, busyOnly]))
      .mockResolvedValueOnce(response(created, 201))
      .mockResolvedValueOnce(response({ ...created, calendars: [{ calendar_id: 2, position: 0, color: "#654321" }] }));
    renderManager(request);
    await screen.findByText("No composite views yet.");

    fireEvent.click(screen.getByRole("button", { name: "New composite view" }));
    fireEvent.change(screen.getByLabelText("View name"), { target: { value: "New view" } });
    fireEvent.click(screen.getByRole("button", { name: "Create view" }));
    await waitFor(() => expect(request).toHaveBeenCalledWith("/api/v1/views", expect.objectContaining({ method: "POST", body: JSON.stringify({ name: "New view" }) })));

    expect(screen.getByRole("option", { name: "Work" })).toBeInTheDocument();
    expect(screen.queryByRole("option", { name: "Private" })).not.toBeInTheDocument();
    fireEvent.change(screen.getByLabelText("Add calendar"), { target: { value: "2" } });
    fireEvent.click(screen.getByRole("button", { name: "Add calendar to view" }));
    fireEvent.click(screen.getByRole("button", { name: "Save view calendars" }));
    await waitFor(() => expect(request).toHaveBeenCalledWith("/api/v1/views/9/calendars", expect.objectContaining({ method: "PUT", body: JSON.stringify({ calendars: [{ calendar_id: 2, position: 0, color: "#654321" }] }) })));
  });

  it("edits a view name, reorders its calendars, and saves color overrides", async () => {
    const current = { ...view, calendars: [{ calendar_id: 1, position: 0, color: "#123456" }, { calendar_id: 2, position: 1, color: "#654321" }] };
    const request = vi.fn()
      .mockResolvedValueOnce(response([current]))
      .mockResolvedValueOnce(response([work, family, busyOnly]))
      .mockResolvedValueOnce(response({ ...current, name: "Updated", version: 2 }))
      .mockResolvedValueOnce(response({ ...current, name: "Updated", version: 3, calendars: [{ calendar_id: 2, position: 0, color: "#abcdef" }, { calendar_id: 1, position: 1, color: "#123456" }] }));
    renderManager(request);
    fireEvent.click(await screen.findByRole("button", { name: "Edit Week" }));

    fireEvent.change(screen.getByLabelText("View name"), { target: { value: "Updated" } });
    fireEvent.click(screen.getByRole("button", { name: "Save view name" }));
    await waitFor(() => expect(request).toHaveBeenCalledWith("/api/v1/views/8", expect.objectContaining({ method: "PATCH", body: JSON.stringify({ name: "Updated" }) })));

    fireEvent.change(screen.getByLabelText("Color for Family"), { target: { value: "#abcdef" } });
    fireEvent.click(screen.getByRole("button", { name: "Move Family up" }));
    fireEvent.click(screen.getByRole("button", { name: "Save view calendars" }));
    await waitFor(() => expect(request).toHaveBeenCalledWith("/api/v1/views/8/calendars", expect.objectContaining({ method: "PUT", body: JSON.stringify({ calendars: [{ calendar_id: 2, position: 0, color: "#abcdef" }, { calendar_id: 1, position: 1, color: "#123456" }] }) })));
  });

  it("publishes a view with its chosen detail level and expiration, then displays and rotates its public link", async () => {
    const expiresAt = Math.floor(new Date("2027-01-15T08:00").getTime() / 1_000);
    const request = vi.fn()
      .mockResolvedValueOnce(response([view]))
      .mockResolvedValueOnce(response([work]))
      .mockResolvedValueOnce(response({ token: "first-token", projection: "title_and_time", display_timezone: "UTC", expires_at: expiresAt, revoked: false, version: 1 }, 201))
      .mockResolvedValueOnce(response({ projection: "free_busy", display_timezone: "UTC", expires_at: expiresAt, revoked: false, version: 2 }))
      .mockResolvedValueOnce(response({ token: "second-token", projection: "title_and_time", display_timezone: "UTC", expires_at: 1_800_000_000, revoked: false, version: 2 }));
    renderManager(request);
    fireEvent.click(await screen.findByRole("button", { name: "Edit Week" }));

    fireEvent.change(screen.getByLabelText("Public detail level"), { target: { value: "title_and_time" } });
    fireEvent.change(screen.getByLabelText("Public link expires at"), { target: { value: "2027-01-15T08:00" } });
    fireEvent.click(screen.getByRole("button", { name: "Publish view" }));
    await waitFor(() => expect(request).toHaveBeenCalledWith("/api/v1/views/8/publication", expect.objectContaining({ method: "POST", body: JSON.stringify({ projection: "title_and_time", display_timezone: "UTC", expires_at: expiresAt }) })));
    expect(await screen.findByRole("link", { name: "Current public link" })).toHaveAttribute("href", "/public/views/first-token");

    fireEvent.change(screen.getByLabelText("Public detail level"), { target: { value: "free_busy" } });
    fireEvent.click(screen.getByRole("button", { name: "Save publication" }));
    await waitFor(() => expect(request).toHaveBeenCalledWith("/api/v1/views/8/publication", expect.objectContaining({ method: "PATCH", body: JSON.stringify({ projection: "free_busy", display_timezone: "UTC", expires_at: expiresAt }) })));
    expect(screen.getByRole("link", { name: "Current public link" })).toHaveAttribute("href", "/public/views/first-token");

    fireEvent.click(screen.getByRole("button", { name: "Rotate public link" }));
    await waitFor(() => expect(request).toHaveBeenCalledWith("/api/v1/views/8/publication/rotate", expect.objectContaining({ method: "POST" })));
    expect(await screen.findByRole("link", { name: "Current public link" })).toHaveAttribute("href", "/public/views/second-token");
  });
});
