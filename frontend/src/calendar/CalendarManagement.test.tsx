import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { CalendarManagement } from "./CalendarManagement";
import type { ApiClient } from "../auth/api";

const manager = {
  id: 2, access: "details", role: "manager", owner_user_id: 1, name: "Team", description: "Planning", color: "#123456",
  default_timezone: "UTC", default_event_visibility: "private", default_notification_rules_json: null, archived: false, version: 1,
};
const owner = { ...manager, id: 7, role: "owner", owner_user_id: 7 };
const editor = { ...manager, id: 8, role: "editor" };
const viewer = { ...manager, role: "viewer" };
const freeBusyViewer = { ...manager, id: 9, access: "free_busy", role: "free_busy_viewer" };

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
  it("offers import to owner manager and editor only", async () => {
    const namedOwner = { ...owner, name: "Owner calendar" };
    const namedManager = { ...manager, id: 12, name: "Manager calendar" };
    const namedEditor = { ...editor, name: "Editor calendar" };
    const archivedOwner = { ...owner, id: 10, name: "Archived", archived: true };
    renderManager([namedOwner, namedManager, namedEditor, viewer, freeBusyViewer, archivedOwner]);
    await screen.findByText("Active Calendars");

    expect(screen.getByRole("button", { name: "Import ICS to Owner calendar" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Import ICS to Manager calendar" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Import ICS to Editor calendar" })).toBeInTheDocument();
    expect(screen.getAllByRole("button", { name: /import ics to/i })).toHaveLength(3);
    expect(screen.queryByRole("button", { name: "Edit Editor calendar" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Manage sharing for Editor calendar" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Import ICS to Archived" })).not.toBeInTheDocument();
  });

  it("validates the selected ICS file before upload", async () => {
    const request = renderManager([owner]);
    fireEvent.click(await screen.findByRole("button", { name: "Import ICS to Team" }));
    const input = screen.getByLabelText("ICS file");

    fireEvent.change(input, { target: { files: [new File(["not a calendar"], "events.txt", { type: "text/plain" })] } });
    expect(screen.getByRole("alert")).toHaveTextContent("ends in .ics");
    expect(screen.getByRole("button", { name: "Import" })).toBeDisabled();

    const oversized = new File([new Uint8Array(1_048_577)], "events.ics", { type: "text/calendar" });
    fireEvent.change(input, { target: { files: [oversized] } });
    expect(screen.getByRole("alert")).toHaveTextContent("1 MiB or smaller");
    expect(request).toHaveBeenCalledTimes(1);
  });

  it("uploads once and reports the imported event count", async () => {
    let resolveImport!: (response: Response) => void;
    const pending = new Promise<Response>((resolve) => { resolveImport = resolve; });
    const request = vi.fn().mockResolvedValueOnce(response([owner])).mockReturnValueOnce(pending);
    renderManager([owner], request);
    fireEvent.click(await screen.findByRole("button", { name: "Import ICS to Team" }));
    fireEvent.change(screen.getByLabelText("ICS file"), { target: { files: [new File(["BEGIN:VCALENDAR"], "events.ics", { type: "text/calendar" })] } });

    const submit = screen.getByRole("button", { name: "Import" });
    fireEvent.click(submit);
    expect(await screen.findByRole("button", { name: "Importing…" })).toBeDisabled();
    fireEvent.click(screen.getByRole("button", { name: "Importing…" }));
    expect(request).toHaveBeenCalledTimes(2);

    resolveImport(response({ imported_events: 2, imported_exceptions: 0 }, 201));
    expect(await screen.findByText("2 events imported")).toBeInTheDocument();
    expect(screen.getByRole("dialog", { name: "Import ICS to Team" })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Done" }));
    expect(screen.queryByRole("dialog", { name: "Import ICS to Team" })).not.toBeInTheDocument();
  });

  it.each([
    ["invalid_calendar_file", "valid supported ICS calendar"],
    ["calendar_import_limit_exceeded", "1 MiB or 1,000 events"],
    ["unexpected_code", "could not import this calendar"],
  ])("shows a safe message for %s failures", async (code, safeText) => {
    const request = vi.fn()
      .mockResolvedValueOnce(response([owner]))
      .mockResolvedValueOnce(response({ error: { code, message: "private backend detail" } }, 400));
    renderManager([owner], request);
    fireEvent.click(await screen.findByRole("button", { name: "Import ICS to Team" }));
    fireEvent.change(screen.getByLabelText("ICS file"), { target: { files: [new File(["bad"], "events.ics", { type: "text/calendar" })] } });
    fireEvent.click(screen.getByRole("button", { name: "Import" }));

    expect(await screen.findByRole("alert")).toHaveTextContent(safeText);
    expect(screen.queryByText("private backend detail")).not.toBeInTheDocument();
  });

  it("closes import with Escape and restores the correct trigger", async () => {
    const secondOwner = { ...owner, id: 11, name: "Second team" };
    renderManager([owner, secondOwner]);
    const trigger = await screen.findByRole("button", { name: "Import ICS to Second team" });
    fireEvent.click(trigger);
    const dialog = screen.getByRole("dialog", { name: "Import ICS to Second team" });
    expect(screen.getByRole("button", { name: "Close import" })).toHaveFocus();

    fireEvent.keyDown(dialog, { key: "Escape" });
    expect(screen.queryByRole("dialog", { name: "Import ICS to Second team" })).not.toBeInTheDocument();
    await waitFor(() => expect(trigger).toHaveFocus());
  });

  it("refreshes calendar access after an import denial", async () => {
    const request = vi.fn()
      .mockResolvedValueOnce(response([editor]))
      .mockResolvedValueOnce(response({ error: { code: "not_found", message: "private backend detail" } }, 404))
      .mockResolvedValueOnce(response([]));
    renderManager([editor], request);
    fireEvent.click(await screen.findByRole("button", { name: "Import ICS to Team" }));
    fireEvent.change(screen.getByLabelText("ICS file"), { target: { files: [new File(["bad"], "events.ics", { type: "text/calendar" })] } });
    fireEvent.click(screen.getByRole("button", { name: "Import" }));

    expect(await screen.findByRole("alert")).toHaveTextContent("Your calendar access changed. The list was refreshed.");
    await waitFor(() => expect(screen.queryByText("Team")).not.toBeInTheDocument());
    expect(screen.queryByText("private backend detail")).not.toBeInTheDocument();
  });

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
