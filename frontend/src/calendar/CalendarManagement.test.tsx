import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { CalendarManagement } from "./CalendarManagement";
import type { ApiClient } from "../auth/api";

const manager = {
  id: 2, access: "details", role: "manager", owner_user_id: 1, name: "Team", description: "Planning", color: "#123456",
  default_timezone: "UTC", default_event_visibility: "private", default_notification_rules_json: null, archived: false, version: 1,
};
const owner = { ...manager, id: 7, role: "owner", owner_user_id: 7 };
const viewer = { ...manager, role: "viewer" };

function response(value: unknown, status = 200) {
  return new Response(JSON.stringify(value), { status, headers: { "content-type": "application/json" } });
}

function renderManager(calendars: unknown[], request = vi.fn().mockResolvedValue(response(calendars))) {
  const api = { request, csrfToken: "csrf", setCsrfToken: vi.fn(), logout: vi.fn() } as unknown as ApiClient;
  render(<CalendarManagement api={api} />);
  return request;
}

afterEach(cleanup);

describe("CalendarManagement", () => {
  it("does not render management controls for a viewer", async () => {
    renderManager([viewer]);
    await screen.findByText("Team");
    expect(screen.queryByRole("button", { name: /edit team/i })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /archive team/i })).not.toBeInTheDocument();
  });

  it("renders only permitted management controls for a manager", async () => {
    renderManager([manager]);
    await screen.findByText("Team");
    expect(screen.getByRole("button", { name: /edit team/i })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /archive team/i })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /manage sharing for team/i })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /delete team/i })).not.toBeInTheDocument();
  });

  it("lets an owner open sharing and lists collaborators", async () => {
    const request = vi.fn()
      .mockResolvedValueOnce(response([owner]))
      .mockResolvedValueOnce(response([{ user_id: 7, role: "owner", created_at: 1, updated_at: 1 }, { user_id: 8, role: "viewer", created_at: 1, updated_at: 1 }]));
    renderManager([owner], request);
    await screen.findByText("Team");

    fireEvent.click(screen.getByRole("button", { name: /manage sharing for team/i }));

    expect(await screen.findByRole("dialog", { name: /share team/i })).toBeInTheDocument();
    expect(screen.getByText("User 8")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /transfer ownership/i })).toBeInTheDocument();
  });

  it("grants or updates a collaborator role and revokes non-owner access", async () => {
    const entry = { user_id: 8, role: "viewer", created_at: 1, updated_at: 1 };
    const request = vi.fn()
      .mockResolvedValueOnce(response([manager]))
      .mockResolvedValueOnce(response([entry]))
      .mockResolvedValueOnce(response({ ...entry, role: "editor" }))
      .mockResolvedValueOnce(new Response(null, { status: 204 }));
    renderManager([manager], request);
    await screen.findByText("Team");
    fireEvent.click(screen.getByRole("button", { name: /manage sharing for team/i }));
    await screen.findByRole("dialog");

    fireEvent.change(screen.getByLabelText("Role for user 8"), { target: { value: "editor" } });
    fireEvent.click(screen.getByRole("button", { name: "Save role for user 8" }));
    await waitFor(() => expect(request).toHaveBeenCalledWith("/api/v1/calendars/2/acl/8", expect.objectContaining({ method: "PUT", body: JSON.stringify({ role: "editor" }) })));

    fireEvent.click(screen.getByRole("button", { name: "Revoke access for user 8" }));
    await waitFor(() => expect(request).toHaveBeenCalledWith("/api/v1/calendars/2/acl/8", expect.objectContaining({ method: "DELETE" })));
  });

  it("does not transfer ownership until the owner explicitly confirms", async () => {
    const request = vi.fn()
      .mockResolvedValueOnce(response([owner]))
      .mockResolvedValueOnce(response([{ user_id: 7, role: "owner", created_at: 1, updated_at: 1 }, { user_id: 8, role: "manager", created_at: 1, updated_at: 1 }]))
      .mockResolvedValueOnce(response({ ...owner, owner_user_id: 8, role: "manager", version: 2 }));
    renderManager([owner], request);
    await screen.findByText("Team");
    fireEvent.click(screen.getByRole("button", { name: /manage sharing for team/i }));
    await screen.findByRole("dialog");

    fireEvent.click(screen.getByRole("button", { name: /transfer ownership/i }));
    expect(request).toHaveBeenCalledTimes(2);
    expect(screen.getByRole("dialog", { name: /confirm ownership transfer/i })).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Confirm transfer" }));
    await waitFor(() => expect(request).toHaveBeenCalledWith("/api/v1/calendars/7/transfer", expect.objectContaining({ method: "POST", body: JSON.stringify({ new_owner_user_id: 8, version: 1 }) })));
  });

  it("creates, updates, archives, and restores calendars through the authenticated API", async () => {
    const created = { ...owner, id: 3, name: "New calendar" };
    const updated = { ...manager, name: "Renamed", version: 2 };
    const archived = { ...updated, archived: true, version: 3 };
    const restored = { ...archived, archived: false, version: 4 };
    const request = vi.fn()
      .mockResolvedValueOnce(response([manager, owner]))
      .mockResolvedValueOnce(response(created, 201))
      .mockResolvedValueOnce(response(updated))
      .mockResolvedValueOnce(response(archived))
      .mockResolvedValueOnce(response(restored));
    renderManager([manager, owner], request);
    await waitFor(() => expect(screen.getAllByRole("button", { name: /edit team/i })).toHaveLength(2));

    fireEvent.click(screen.getByRole("button", { name: /new calendar/i }));
    fireEvent.change(screen.getByLabelText("Calendar name"), { target: { value: "New calendar" } });
    fireEvent.click(screen.getByRole("button", { name: "Create calendar" }));
    await waitFor(() => expect(request).toHaveBeenCalledWith("/api/v1/calendars", expect.objectContaining({ method: "POST" })));

    fireEvent.click(screen.getAllByRole("button", { name: /edit team/i })[0]);
    fireEvent.change(screen.getByLabelText("Calendar name"), { target: { value: "Renamed" } });
    fireEvent.click(screen.getByRole("button", { name: "Save changes" }));
    await waitFor(() => expect(request).toHaveBeenCalledWith("/api/v1/calendars/2", expect.objectContaining({ method: "PATCH" })));

    fireEvent.click(screen.getByRole("button", { name: /archive renamed/i }));
    await waitFor(() => expect(request).toHaveBeenCalledWith("/api/v1/calendars/2/archive", expect.objectContaining({ method: "POST" })));
    expect(await screen.findByRole("button", { name: /restore renamed/i })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: /restore renamed/i }));
    await waitFor(() => expect(request).toHaveBeenCalledWith("/api/v1/calendars/2/restore", expect.objectContaining({ method: "POST" })));
  });

  it("shows a safe error when a stale visible control is rejected", async () => {
    const request = vi.fn().mockResolvedValueOnce(response([manager])).mockResolvedValueOnce(response({ message: "no" }, 403));
    renderManager([manager], request);
    await screen.findByText("Team");
    fireEvent.click(screen.getByRole("button", { name: /archive team/i }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Your calendar access changed. The list was refreshed.");
  });

  it("closes sharing with Escape and restores focus to its trigger", async () => {
    const request = vi.fn()
      .mockResolvedValueOnce(response([owner]))
      .mockResolvedValueOnce(response([]));
    renderManager([owner], request);
    const trigger = await screen.findByRole("button", { name: /manage sharing for team/i });
    fireEvent.click(trigger);
    const dialog = await screen.findByRole("dialog", { name: /share team/i });

    expect(screen.getByRole("button", { name: "Close sharing" })).toHaveFocus();
    fireEvent.keyDown(dialog, { key: "Escape" });

    expect(screen.queryByRole("dialog", { name: /share team/i })).not.toBeInTheDocument();
    await waitFor(() => expect(trigger).toHaveFocus());
  });

  it("returns focus to the sharing control that opened the dialog", async () => {
    const secondOwner = { ...owner, id: 9, name: "Second team" };
    const request = vi.fn()
      .mockResolvedValueOnce(response([owner, secondOwner]))
      .mockResolvedValueOnce(response([]));
    renderManager([owner, secondOwner], request);
    const trigger = await screen.findByRole("button", { name: /manage sharing for team/i });
    fireEvent.click(trigger);
    const dialog = await screen.findByRole("dialog", { name: /share team/i });

    fireEvent.keyDown(dialog, { key: "Escape" });

    await waitFor(() => expect(trigger).toHaveFocus());
  });

  it("removes stale controls after an authorization denial without showing response details", async () => {
    const request = vi.fn()
      .mockResolvedValueOnce(response([manager]))
      .mockResolvedValueOnce(response({ message: "private backend detail" }, 403))
      .mockResolvedValueOnce(response([]));
    renderManager([manager], request);
    await screen.findByText("Team");

    fireEvent.click(screen.getByRole("button", { name: /archive team/i }));

    expect(await screen.findByRole("alert")).toHaveTextContent("Your calendar access changed. The list was refreshed.");
    await waitFor(() => expect(screen.queryByText("Team")).not.toBeInTheDocument());
    expect(screen.queryByText("private backend detail")).not.toBeInTheDocument();
  });

  it("uses responsive form and dialog hooks for narrow viewports", async () => {
    renderManager([]);
    await screen.findByText("No calendars yet.");
    fireEvent.click(screen.getByRole("button", { name: /new calendar/i }));
    expect(screen.getByRole("form", { name: "Create calendar" })).toHaveClass("calendar-form");

    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(screen.getByRole("region", { name: "Calendars" })).toHaveClass("calendar-management");
  });
});
