import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { ApiClient } from "../auth/api";
import { CalendarEventUI } from "./CalendarEventUI";
import type { Calendar } from "./CalendarManagement";

const calendars: Calendar[] = [
  { id: 1, name: "Work", color: "#2563eb", role: "owner", access: "details" },
  { id: 2, name: "Shared", color: "#dc2626", role: "viewer", access: "details" },
];
const events = [
  { id: 10, calendar_id: 1, access: "details" as const, status: "confirmed" as const, event_kind: "timed" as const, title: "Planning", start_utc: 1_750_032_800, end_utc: 1_750_036_400, timezone: "UTC", version: 1, start_date: "2025-06-16" },
  { id: 11, calendar_id: 2, access: "details" as const, status: "confirmed" as const, event_kind: "timed" as const, title: "Imported holiday", start_utc: 1_750_036_400, end_utc: 1_750_040_000, timezone: "UTC", version: 1, is_external: true, start_date: "2025-06-16" },
];

function apiWithEvents() {
  return { request: vi.fn().mockImplementation((path: string) => {
    if (path.includes("/events?")) return Promise.resolve(new Response(JSON.stringify(events), { status: 200 }));
    return Promise.resolve(new Response(JSON.stringify([]), { status: 200 }));
  }), csrfToken: null, setCsrfToken: vi.fn(), logout: vi.fn() } as unknown as ApiClient;
}

afterEach(cleanup);

describe("CalendarEventUI", () => {
  it("switches views and creates a timed event in a writable calendar", async () => {
    const api = apiWithEvents();
    render(<CalendarEventUI api={api} calendars={calendars} initialDate={new Date("2025-06-16T12:00:00Z")} />);

    expect(await screen.findByText("Planning")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("tab", { name: "Week" }));
    expect(await screen.findByRole("region", { name: "Week calendar" })).toBeInTheDocument();
    const [newEventBtn] = screen.getAllByRole("button", { name: /New event/ });
    fireEvent.click(newEventBtn);
    fireEvent.change(screen.getByLabelText("Title"), { target: { value: "Standup" } });
    fireEvent.click(screen.getByRole("button", { name: "Save event" }));

    await waitFor(() => expect(api.request).toHaveBeenCalledWith("/api/v1/calendars/1/events", expect.objectContaining({ method: "POST" })));
  });

  it("submits an optional recurrence rule with a new event", async () => {
    const api = apiWithEvents();
    render(<CalendarEventUI api={api} calendars={calendars} initialDate={new Date("2025-06-16T12:00:00Z")} />);

    fireEvent.click(await screen.findByRole("button", { name: "New event" }));
    fireEvent.change(screen.getByLabelText("Title"), { target: { value: "Recurring standup" } });
    fireEvent.change(screen.getByLabelText("Recurrence"), { target: { value: "FREQ=WEEKLY;COUNT=3" } });
    fireEvent.click(screen.getByRole("button", { name: "Save event" }));

    await waitFor(() => expect(api.request).toHaveBeenCalledWith("/api/v1/calendars/1/events", expect.objectContaining({
      body: expect.stringContaining("FREQ=WEEKLY;COUNT=3"),
    })));
  });

  it("does not offer editing or dragging for viewer and external events", async () => {
    render(<CalendarEventUI api={apiWithEvents()} calendars={calendars} initialDate={new Date("2025-06-16T12:00:00Z")} />);

    expect(await screen.findByRole("button", { name: "Imported holiday (read-only external event)" })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Imported holiday (read-only external event)" }));
    expect(screen.getByText("This external event is read-only.")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Edit event" })).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Imported holiday (read-only external event)" })).not.toHaveAttribute("draggable", "true");
  });

  it("offers a reload after a concurrency conflict", async () => {
    const api = apiWithEvents();
    vi.mocked(api.request).mockImplementation((path: RequestInfo | URL, init?: RequestInit) => {
      if (String(path).includes("/events?") || !init?.method) return Promise.resolve(new Response(JSON.stringify(events), { status: 200 }));
      return Promise.resolve(new Response(null, { status: 409 }));
    });
    render(<CalendarEventUI api={api} calendars={calendars} initialDate={new Date("2025-06-16T12:00:00Z")} />);

    fireEvent.click(await screen.findByText("Planning"));
    fireEvent.click(screen.getByRole("button", { name: "Edit event" }));
    fireEvent.click(screen.getByRole("button", { name: "Save event" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("This event changed elsewhere. Reload it before saving again.");
    fireEvent.click(screen.getByRole("button", { name: "Reload events" }));
    await waitFor(() => expect(screen.queryByRole("form", { name: "Edit event" })).not.toBeInTheDocument());
  });

  it("renders an accessible agenda and hides toggled calendars", async () => {
    render(<CalendarEventUI api={apiWithEvents()} calendars={calendars} initialDate={new Date("2025-06-16T12:00:00Z")} />);

    const agendaBtn = await screen.findByRole("tab", { name: "Agenda" });
    fireEvent.click(agendaBtn);
    expect(await screen.findByRole("list", { name: "Agenda" })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("checkbox", { name: "Work" }));
    expect(screen.queryByRole("button", { name: "Planning" })).not.toBeInTheDocument();
  });

  it("shows resize handles on editable events in day view", async () => {
    render(<CalendarEventUI api={apiWithEvents()} calendars={calendars} initialDate={new Date("2025-06-16T12:00:00Z")} />);

    const dayBtn = await screen.findByRole("tab", { name: "Day" });
    fireEvent.click(dayBtn);
    expect(await screen.findByText("Planning")).toBeInTheDocument();
    const eventBlock = screen.getByText("Planning").closest('[class*="event-block"]');
    expect(eventBlock).toBeInTheDocument();
    expect(screen.queryAllByRole("img", { name: /resize/i })).toHaveLength(0);
  });

  it("does not show resize handles on external events", async () => {
    render(<CalendarEventUI api={apiWithEvents()} calendars={calendars} initialDate={new Date("2025-06-16T12:00:00Z")} />);

    const dayBtn = await screen.findByRole("tab", { name: "Day" });
    fireEvent.click(dayBtn);
    await screen.findByText("Planning");
    const externalBtn = screen.getByRole("button", { name: "Imported holiday" });
    expect(externalBtn).toHaveAttribute("draggable", "false");
  });
});
