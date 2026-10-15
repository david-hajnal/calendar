import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { ApiClient } from "../auth/api";
import { CalendarEventUI } from "./CalendarEventUI";
import { CalendarNavigationProvider } from "./calendarNavigation";
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

function pointerEvent(type: string, clientX: number, clientY = 100) {
  const pointer = new MouseEvent(type, { bubbles: true, clientX, clientY });
  Object.defineProperty(pointer, "pointerId", { value: 1 });
  return pointer;
}

function localInputValue(seconds: number) {
  const date = new Date(seconds * 1000);
  return new Date(date.getTime() - date.getTimezoneOffset() * 60_000).toISOString().slice(0, 16);
}

function monthCell(day: string) {
  const month = screen.getByRole("list", { name: "Month calendar" });
  return Array.from(month.querySelectorAll<HTMLElement>(".event-grid__cell:not(.event-grid__cell--other-month)"))
    .find((cell) => cell.querySelector(".event-grid__day")?.textContent === day)!;
}

afterEach(() => { cleanup(); vi.unstubAllGlobals(); });

describe("CalendarEventUI", () => {
  it("renders a backend-shaped timed event in its month cell", async () => {
    const timedEvent = {
      id: 10,
      calendar_id: 1,
      access: "details" as const,
      status: "confirmed" as const,
      event_kind: "timed" as const,
      title: "Planning",
      start_utc: 1_750_032_800,
      end_utc: 1_750_036_400,
      timezone: "UTC",
      version: 1,
    };
    const api = apiWithEvents();
    vi.mocked(api.request).mockImplementation((path: RequestInfo | URL) => Promise.resolve(
      new Response(JSON.stringify(String(path).includes("/events?") ? [timedEvent] : []), { status: 200 }),
    ));

    render(<CalendarEventUI api={api} calendars={calendars} initialDate={new Date("2025-06-16T12:00:00Z")} />);

    expect(await screen.findByText("Planning")).toBeInTheDocument();
  });

  it("preserves the full accessible name for a very long event title in month view", async () => {
    const longTitle = "Extremely long event title that would normally distort the calendar grid layout when rendered as a single line without proper containment";
    const longEvent = {
      id: 99,
      calendar_id: 1,
      access: "details" as const,
      status: "confirmed" as const,
      event_kind: "timed" as const,
      title: longTitle,
      start_utc: 1_750_032_800,
      end_utc: 1_750_036_400,
      timezone: "UTC",
      version: 1,
    };
    const api = apiWithEvents();
    vi.mocked(api.request).mockImplementation((path: RequestInfo | URL) => Promise.resolve(
      new Response(JSON.stringify(String(path).includes("/events?") ? [longEvent] : []), { status: 200 }),
    ));

    render(<CalendarEventUI api={api} calendars={calendars} initialDate={new Date("2025-06-16T12:00:00Z")} />);

    const button = await screen.findByRole("button", { name: longTitle });
    expect(button).toBeInTheDocument();
    expect(button.textContent).toBe(longTitle);
  });

  it.each(["Day", "Week"])("renders a backend-shaped all-day event in %s view", async (view) => {
    const allDayEvent = {
      id: 12,
      calendar_id: 1,
      access: "details" as const,
      status: "confirmed" as const,
      event_kind: "all_day" as const,
      title: "Conference",
      start_date: "2025-06-16",
      end_date: "2025-06-17",
      version: 1,
    };
    const api = apiWithEvents();
    vi.mocked(api.request).mockImplementation((path: RequestInfo | URL) => Promise.resolve(
      new Response(JSON.stringify(String(path).includes("/events?") ? [allDayEvent] : []), { status: 200 }),
    ));

    render(<CalendarEventUI api={api} calendars={calendars} initialDate={new Date("2025-06-16T12:00:00Z")} />);
    fireEvent.click(await screen.findByRole("tab", { name: view }));

    expect((await screen.findAllByText("Conference")).length).toBeGreaterThanOrEqual(1);
  });

  it("renders a multi-day all-day event on every day it spans in month view", async () => {
    const multiDayEvent = {
      id: 13,
      calendar_id: 1,
      access: "details" as const,
      status: "confirmed" as const,
      event_kind: "all_day" as const,
      title: "Multi-day trip",
      start_date: "2025-06-16",
      end_date: "2025-06-19",
      version: 1,
    };
    const api = apiWithEvents();
    vi.mocked(api.request).mockImplementation((path: RequestInfo | URL) => Promise.resolve(
      new Response(JSON.stringify(String(path).includes("/events?") ? [multiDayEvent] : []), { status: 200 }),
    ));

    render(<CalendarEventUI api={api} calendars={calendars} initialDate={new Date("2025-06-16T12:00:00Z")} />);

    const month = await screen.findByRole("list", { name: "Month calendar" });
    const chips = month.querySelectorAll(".event-chip__text");
    const titles = Array.from(chips).map((el) => el.textContent);
    // Event spans Jun 16, 17, 18 (end_date Jun 19 is exclusive)
    expect(titles.filter((t) => t === "Multi-day trip")).toHaveLength(3);
  });

  it("does not let a stale month response overwrite newer day events", async () => {
    let resolveMonth!: (response: Response) => void;
    let resolveDay!: (response: Response) => void;
    const monthResponse = new Promise<Response>((resolve) => { resolveMonth = resolve; });
    const dayResponse = new Promise<Response>((resolve) => { resolveDay = resolve; });
    const api = apiWithEvents();
    vi.mocked(api.request).mockImplementation((path: RequestInfo | URL) => {
      const query = new URL(String(path), "http://localhost").searchParams;
      const span = Number(query.get("to")) - Number(query.get("from"));
      return span === 86_400 ? dayResponse : monthResponse;
    });
    const dayEvent = {
      id: 10,
      calendar_id: 1,
      access: "details" as const,
      status: "confirmed" as const,
      event_kind: "timed" as const,
      title: "Planning",
      start_utc: 1_750_032_800,
      end_utc: 1_750_036_400,
      timezone: "UTC",
      version: 1,
    };

    render(<CalendarEventUI api={api} calendars={[calendars[0]]} initialDate={new Date("2025-06-16T12:00:00Z")} />);
    fireEvent.click(screen.getByRole("tab", { name: "Day" }));
    await act(async () => { resolveDay(new Response(JSON.stringify([dayEvent]), { status: 200 })); });
    expect((await screen.findAllByText("Planning")).length).toBeGreaterThanOrEqual(1);

    await act(async () => { resolveMonth(new Response(JSON.stringify([]), { status: 200 })); });
    expect(screen.getAllByText("Planning").length).toBeGreaterThanOrEqual(1);
  });

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

  it("creates an all-day event from the editor", async () => {
    const api = apiWithEvents();
    render(<CalendarEventUI api={api} calendars={calendars} initialDate={new Date("2025-06-16T12:00:00Z")} />);

    fireEvent.click(await screen.findByRole("button", { name: "New event" }));
    fireEvent.change(screen.getByLabelText("Title"), { target: { value: "Conference" } });
    fireEvent.click(screen.getByLabelText("All day"));
    fireEvent.change(screen.getByLabelText("Start date"), { target: { value: "2025-06-20" } });
    fireEvent.change(screen.getByLabelText("End date (exclusive)"), { target: { value: "2025-06-22" } });
    fireEvent.click(screen.getByRole("button", { name: "Save event" }));

    await waitFor(() => expect(api.request).toHaveBeenCalledWith("/api/v1/calendars/1/events", expect.objectContaining({ method: "POST" })));
    const postCall = vi.mocked(api.request).mock.calls.find(([, init]) => init?.method === "POST");
    const body = JSON.parse(String(postCall?.[1]?.body));
    expect(body.start_date).toBe("2025-06-20");
    expect(body.end_date).toBe("2025-06-22");
    expect(body.start_utc).toBeUndefined();
    expect(body.end_utc).toBeUndefined();
    expect(body.timezone).toBeUndefined();
  });

  it("defaults the all-day end date to the day after start when toggled on", async () => {
    render(<CalendarEventUI api={apiWithEvents()} calendars={calendars} initialDate={new Date("2025-06-16T12:00:00Z")} />);

    fireEvent.click(await screen.findByRole("button", { name: "New event" }));
    fireEvent.click(screen.getByLabelText("All day"));

    const start = (screen.getByLabelText("Start date") as HTMLInputElement).value;
    const end = (screen.getByLabelText("End date (exclusive)") as HTMLInputElement).value;
    const expected = new Date(`${start}T00:00:00`);
    expected.setDate(expected.getDate() + 1);
    expect(end).toBe(`${expected.getFullYear()}-${String(expected.getMonth() + 1).padStart(2, "0")}-${String(expected.getDate()).padStart(2, "0")}`);
  });

  it("focuses the editor form when the New event button is clicked", async () => {
    render(<CalendarEventUI api={apiWithEvents()} calendars={calendars} initialDate={new Date("2025-06-16T12:00:00Z")} />);

    fireEvent.click(await screen.findByRole("button", { name: "New event" }));
    const form = screen.getByRole("form", { name: "Create event" });

    await waitFor(() => {
      expect(form).toContainElement(document.activeElement as HTMLElement);
    });
  });

  it("scrolls the editor form into view when the New event button is clicked", async () => {
    const scrollIntoViewSpy = vi.fn();
    Element.prototype.scrollIntoView = scrollIntoViewSpy;

    try {
      render(<CalendarEventUI api={apiWithEvents()} calendars={calendars} initialDate={new Date("2025-06-16T12:00:00Z")} />);

      fireEvent.click(await screen.findByRole("button", { name: "New event" }));

      await waitFor(() => {
        expect(scrollIntoViewSpy).toHaveBeenCalled();
      });
    } finally {
      delete (Element.prototype as { scrollIntoView?: unknown }).scrollIntoView;
    }
  });

  it("edits an all-day event with prefilled dates", async () => {
    const allDay = { id: 12, calendar_id: 1, access: "details" as const, status: "confirmed" as const, event_kind: "all_day" as const, title: "Conference", start_date: "2025-06-16", end_date: "2025-06-18", version: 1 };
    const api = apiWithEvents();
    vi.mocked(api.request).mockImplementation((path: RequestInfo | URL, init?: RequestInit) => {
      if (String(path).includes("/events?") || !init?.method) return Promise.resolve(new Response(JSON.stringify([allDay]), { status: 200 }));
      const update = JSON.parse(String(init.body));
      return Promise.resolve(new Response(JSON.stringify({ ...allDay, ...update, version: 2 }), { status: 200 }));
    });
    render(<CalendarEventUI api={api} calendars={calendars} initialDate={new Date("2025-06-16T12:00:00Z")} />);

    const [conferenceBtn] = await screen.findAllByRole("button", { name: "Conference" });
    fireEvent.doubleClick(conferenceBtn);
    expect(screen.getByRole("form", { name: "Edit event" })).toBeInTheDocument();
    expect(screen.getByLabelText("All day")).toBeChecked();
    expect(screen.getByLabelText("Start date")).toHaveValue("2025-06-16");
    expect(screen.getByLabelText("End date (exclusive)")).toHaveValue("2025-06-18");
    fireEvent.change(screen.getByLabelText("Start date"), { target: { value: "2025-06-20" } });
    fireEvent.change(screen.getByLabelText("End date (exclusive)"), { target: { value: "2025-06-23" } });
    fireEvent.click(screen.getByRole("button", { name: "Save event" }));

    await waitFor(() => expect(api.request).toHaveBeenCalledWith("/api/v1/calendars/1/events/12", expect.objectContaining({ method: "PATCH" })));
    const patchCall = vi.mocked(api.request).mock.calls.find(([, init]) => init?.method === "PATCH");
    const body = JSON.parse(String(patchCall?.[1]?.body));
    expect(body.start_date).toBe("2025-06-20");
    expect(body.end_date).toBe("2025-06-23");
    expect(body.start_utc).toBeUndefined();
  });

  it("shows the all-day date range in the detail panel", async () => {
    const allDay = { id: 12, calendar_id: 1, access: "details" as const, status: "confirmed" as const, event_kind: "all_day" as const, title: "Conference", start_date: "2025-06-16", end_date: "2025-06-18", version: 1 };
    const api = apiWithEvents();
    vi.mocked(api.request).mockImplementation((path: RequestInfo | URL) => {
      if (String(path).includes("/events?")) return Promise.resolve(new Response(JSON.stringify([allDay]), { status: 200 }));
      return Promise.resolve(new Response(JSON.stringify([]), { status: 200 }));
    });
    render(<CalendarEventUI api={api} calendars={calendars} initialDate={new Date("2025-06-16T12:00:00Z")} />);

    const [conferenceBtn] = await screen.findAllByRole("button", { name: "Conference" });
    fireEvent.click(conferenceBtn);
    const detail = screen.getByRole("complementary", { name: "Event details" });
    expect(detail).toHaveTextContent(/Jun 16/);
    expect(detail).toHaveTextContent(/Jun 17/);
  });

  it("moves an all-day event later by one day preserving span", async () => {
    const allDay = { id: 12, calendar_id: 1, access: "details" as const, status: "confirmed" as const, event_kind: "all_day" as const, title: "Conference", start_date: "2025-06-16", end_date: "2025-06-18", version: 1 };
    const api = apiWithEvents();
    vi.mocked(api.request).mockImplementation((path: RequestInfo | URL, init?: RequestInit) => {
      if (String(path).includes("/events?") || !init?.method) return Promise.resolve(new Response(JSON.stringify([allDay]), { status: 200 }));
      const update = JSON.parse(String(init.body));
      return Promise.resolve(new Response(JSON.stringify({ ...allDay, ...update, version: 2 }), { status: 200 }));
    });
    render(<CalendarEventUI api={api} calendars={calendars} initialDate={new Date("2025-06-16T12:00:00Z")} />);

    const [moveBtn] = await screen.findAllByRole("button", { name: "Move Conference later" });
    fireEvent.click(moveBtn);

    await waitFor(() => expect(api.request).toHaveBeenCalledWith("/api/v1/calendars/1/events/12", expect.objectContaining({ method: "PATCH" })));
    const patchCall = vi.mocked(api.request).mock.calls.find(([, init]) => init?.method === "PATCH");
    const body = JSON.parse(String(patchCall?.[1]?.body));
    expect(body.start_date).toBe("2025-06-17");
    expect(body.end_date).toBe("2025-06-19");
    expect(body.start_utc).toBeUndefined();
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

  it("shows a load error banner when the initial GET fails and does not display events", async () => {
    const api = apiWithEvents();
    vi.mocked(api.request).mockImplementation((path: RequestInfo | URL) => {
      if (String(path).includes("/events?")) return Promise.resolve(new Response(null, { status: 500 }));
      return Promise.resolve(new Response(JSON.stringify([]), { status: 200 }));
    });
    render(<CalendarEventUI api={api} calendars={calendars} initialDate={new Date("2025-06-16T12:00:00Z")} />);

    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("We could not load events. Please try again.");
    expect(screen.getByRole("button", { name: "Reload events" })).toBeInTheDocument();
    expect(screen.queryByText("Planning")).not.toBeInTheDocument();
  });

  it("retains loaded events and shows a mutation error banner (not a load error) when a move fails", async () => {
    const api = apiWithEvents();
    vi.mocked(api.request).mockImplementation((path: RequestInfo | URL, init?: RequestInit) => {
      if (String(path).includes("/events?") || !init?.method) return Promise.resolve(new Response(JSON.stringify(events), { status: 200 }));
      return Promise.resolve(new Response(null, { status: 500 }));
    });
    render(<CalendarEventUI api={api} calendars={calendars} initialDate={new Date("2025-06-16T12:00:00Z")} />);

    fireEvent.click(await screen.findByRole("tab", { name: "Day" }));
    const region = await screen.findByRole("region", { name: "Day calendar" });
    const grid = region.querySelector(":scope > .event-ui__day-region") as HTMLDivElement;
    grid.setPointerCapture = vi.fn();
    const block = screen.getByRole("button", { name: "Planning" }).closest(".event-ui__event-block") as HTMLElement;
    fireEvent(block, pointerEvent("pointerdown", 100, 100));
    fireEvent(grid, pointerEvent("pointermove", 100, 122));
    fireEvent(grid, pointerEvent("pointerup", 100, 122));

    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("We could not move this event.");
    expect(alert).not.toHaveTextContent("We could not load events");
    expect(screen.getByRole("button", { name: "Planning" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Reload events" })).not.toBeInTheDocument();
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
    await screen.findAllByText("Planning");
    const eventBlock = document.querySelector(".event-ui__day-timeline .event-ui__event-block");
    expect(eventBlock).toBeInTheDocument();
    expect(screen.queryAllByRole("img", { name: /resize/i })).toHaveLength(0);
  });

  it("does not show resize handles on external events", async () => {
    render(<CalendarEventUI api={apiWithEvents()} calendars={calendars} initialDate={new Date("2025-06-16T12:00:00Z")} />);

    const dayBtn = await screen.findByRole("tab", { name: "Day" });
    fireEvent.click(dayBtn);
    await screen.findAllByText("Planning");
    const externalBtn = screen.getByRole("button", { name: "Imported holiday" });
    expect(externalBtn).toHaveAttribute("draggable", "false");
  });

  it("drags an editable timed event to another weekday and persists the new position", async () => {
    const api = apiWithEvents();
    vi.mocked(api.request).mockImplementation((path: RequestInfo | URL, init?: RequestInit) => {
      if (String(path).includes("/events?") || !init?.method) {
        return Promise.resolve(new Response(JSON.stringify(events), { status: 200 }));
      }
      const update = JSON.parse(String(init.body));
      return Promise.resolve(new Response(JSON.stringify({
        ...events[0],
        start_utc: update.start_utc,
        end_utc: update.end_utc,
        start_date: "2025-06-17",
        version: 2,
      }), { status: 200 }));
    });
    render(<CalendarEventUI api={api} calendars={calendars} initialDate={new Date("2025-06-16T12:00:00Z")} />);

    fireEvent.click(await screen.findByRole("tab", { name: "Week" }));
    const region = await screen.findByRole("region", { name: "Week calendar" });
    const grid = region.querySelector(":scope > .event-ui__week") as HTMLDivElement;
    const columns = Array.from(grid.querySelectorAll<HTMLElement>(".event-ui__week-day-column"));
    vi.spyOn(grid, "getBoundingClientRect").mockReturnValue({ left: 0, width: 756, right: 756, top: 0, bottom: 1440, height: 1440, x: 0, y: 0, toJSON: () => ({}) });
    grid.setPointerCapture = vi.fn();
    const eventBlock = screen.getByRole("button", { name: "Planning" }).closest(".event-ui__event-block") as HTMLElement;
    fireEvent(eventBlock, pointerEvent("pointerdown", 206));
    fireEvent(eventBlock, pointerEvent("pointermove", 306));
    await waitFor(() => expect(columns[2].querySelector(".event-ui__slot-highlight")).toBeInTheDocument());
    fireEvent(eventBlock, pointerEvent("pointerup", 306));

    await waitFor(() => expect(api.request).toHaveBeenCalledWith(
      "/api/v1/calendars/1/events/10",
      expect.objectContaining({ method: "PATCH" }),
    ));
    const patchCall = vi.mocked(api.request).mock.calls.find(([, init]) => init?.method === "PATCH");
    const body = JSON.parse(String(patchCall?.[1]?.body));
    expect(body.start_utc).toBe(events[0].start_utc + 86_400);
    expect(body.end_utc).toBe(events[0].end_utc + 86_400);
    await waitFor(() => expect(columns[2]).toContainElement(screen.getByRole("button", { name: "Planning" })));
  });

  it("handles captured day-grid movement and snaps it to fifteen minutes", async () => {
    const api = apiWithEvents();
    vi.mocked(api.request).mockImplementation((path: RequestInfo | URL, init?: RequestInit) => {
      if (String(path).includes("/events?") || !init?.method) return Promise.resolve(new Response(JSON.stringify(events), { status: 200 }));
      const update = JSON.parse(String(init.body));
      return Promise.resolve(new Response(JSON.stringify({ ...events[0], ...update, version: 2 }), { status: 200 }));
    });
    render(<CalendarEventUI api={api} calendars={calendars} initialDate={new Date("2025-06-16T12:00:00Z")} />);

    fireEvent.click(await screen.findByRole("tab", { name: "Day" }));
    const region = await screen.findByRole("region", { name: "Day calendar" });
    const grid = region.querySelector(":scope > .event-ui__day-region") as HTMLDivElement;
    grid.setPointerCapture = vi.fn();
    const block = screen.getByRole("button", { name: "Planning" }).closest(".event-ui__event-block")!;
    fireEvent(block, pointerEvent("pointerdown", 100, 100));
    fireEvent(grid, pointerEvent("pointermove", 100, 122));
    fireEvent(grid, pointerEvent("pointerup", 100, 122));

    await waitFor(() => expect(api.request).toHaveBeenCalledWith(
      "/api/v1/calendars/1/events/10",
      expect.objectContaining({ method: "PATCH" }),
    ));
    const patchCall = vi.mocked(api.request).mock.calls.find(([, init]) => init?.method === "PATCH");
    const body = JSON.parse(String(patchCall?.[1]?.body));
    expect(body.start_utc).toBe(events[0].start_utc + 15 * 60);
    expect(body.end_utc).toBe(events[0].end_utc + 15 * 60);
  });

  it("clicks a month cell to create a one-hour event at 9 AM on that date", async () => {
    const api = apiWithEvents();
    render(<CalendarEventUI api={api} calendars={calendars} initialDate={new Date("2025-06-16T12:00:00Z")} />);

    const month = await screen.findByRole("list", { name: "Month calendar" });
    const june17 = Array.from(month.querySelectorAll<HTMLElement>(".event-grid__cell:not(.event-grid__cell--other-month)"))
      .find((cell) => cell.querySelector(".event-grid__day")?.textContent === "17");
    fireEvent.click(june17!);

    expect(screen.getByRole("form", { name: "Create event" })).toBeInTheDocument();
    expect(screen.getByLabelText("Start")).toHaveValue("2025-06-17T09:00");
    expect(screen.getByLabelText("End")).toHaveValue("2025-06-17T10:00");
    fireEvent.change(screen.getByLabelText("Title"), { target: { value: "Month planning" } });
    fireEvent.click(screen.getByRole("button", { name: "Save event" }));

    await waitFor(() => expect(api.request).toHaveBeenCalledWith(
      "/api/v1/calendars/1/events",
      expect.objectContaining({ method: "POST" }),
    ));
    const postCall = vi.mocked(api.request).mock.calls.find(([, init]) => init?.method === "POST");
    const body = JSON.parse(String(postCall?.[1]?.body));
    expect(body.start_utc).toBe(Math.floor(new Date("2025-06-17T09:00").getTime() / 1000));
    expect(body.end_utc).toBe(Math.floor(new Date("2025-06-17T10:00").getTime() / 1000));
  });

  it("does not create from an empty slot without a writable calendar", async () => {
    render(<CalendarEventUI api={apiWithEvents()} calendars={[calendars[1]]} initialDate={new Date("2025-06-16T12:00:00Z")} />);

    fireEvent.click(await screen.findByRole("tab", { name: "Day" }));
    const day = await screen.findByRole("region", { name: "Day calendar" });
    fireEvent.click(day.querySelector(".event-ui__hour-row")!);

    expect(screen.queryByRole("form", { name: "Create event" })).not.toBeInTheDocument();
  });

  it("disables the New event button when no calendar is writable", async () => {
    render(<CalendarEventUI api={apiWithEvents()} calendars={[calendars[1]]} initialDate={new Date("2025-06-16T12:00:00Z")} />);

    expect(await screen.findByRole("button", { name: "New event" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "New event mobile" })).toBeDisabled();
  });

  it("treats movement below five pixels as selection without moving or editing", async () => {
    const api = apiWithEvents();
    render(<CalendarEventUI api={api} calendars={calendars} initialDate={new Date("2025-06-16T12:00:00Z")} />);

    fireEvent.click(await screen.findByRole("tab", { name: "Day" }));
    const region = await screen.findByRole("region", { name: "Day calendar" });
    const grid = region.querySelector(":scope > .event-ui__day-region") as HTMLDivElement;
    grid.setPointerCapture = vi.fn();
    const eventButton = screen.getByRole("button", { name: "Planning" });
    const eventBlock = eventButton.closest(".event-ui__event-block") as HTMLElement;
    fireEvent(eventBlock, pointerEvent("pointerdown", 100, 100));
    fireEvent(eventBlock, pointerEvent("pointermove", 103, 102));
    fireEvent(eventBlock, pointerEvent("pointerup", 103, 102));
    fireEvent.click(eventButton);

    expect(vi.mocked(api.request).mock.calls.some(([, init]) => init?.method === "PATCH")).toBe(false);
    expect(screen.getByLabelText("Event details")).toBeInTheDocument();
    expect(screen.queryByRole("form", { name: "Edit event" })).not.toBeInTheDocument();
  });

  it("double-clicks an editable event to open a prefilled editor", async () => {
    render(<CalendarEventUI api={apiWithEvents()} calendars={calendars} initialDate={new Date("2025-06-16T12:00:00Z")} />);

    fireEvent.doubleClick(await screen.findByRole("button", { name: "Planning" }));

    expect(screen.getByRole("form", { name: "Edit event" })).toBeInTheDocument();
    expect(screen.getByLabelText("Title")).toHaveValue("Planning");
    expect(screen.getByLabelText("Start")).toHaveValue(localInputValue(events[0].start_utc));
    expect(screen.getByLabelText("End")).toHaveValue(localInputValue(events[0].end_utc));
    expect(screen.getByLabelText("Calendar")).toHaveValue("1");
  });

  it("drags a timed month event to another date while preserving its time and duration", async () => {
    const api = apiWithEvents();
    vi.mocked(api.request).mockImplementation((path: RequestInfo | URL, init?: RequestInit) => {
      if (String(path).includes("/events?") || !init?.method) return Promise.resolve(new Response(JSON.stringify(events), { status: 200 }));
      const update = JSON.parse(String(init.body));
      return Promise.resolve(new Response(JSON.stringify({ ...events[0], ...update, start_date: "2025-06-18", version: 2 }), { status: 200 }));
    });
    render(<CalendarEventUI api={api} calendars={calendars} initialDate={new Date("2025-06-16T12:00:00Z")} />);

    const planning = await screen.findByRole("button", { name: "Planning" });
    const june18 = monthCell("18");
    fireEvent(planning, pointerEvent("pointerdown", 100));
    fireEvent(planning, pointerEvent("pointermove", 106));
    fireEvent(june18, pointerEvent("pointermove", 300));
    expect(june18).toHaveClass("event-grid__cell--move-target");
    fireEvent(june18, pointerEvent("pointerup", 300));

    await waitFor(() => expect(api.request).toHaveBeenCalledWith(
      "/api/v1/calendars/1/events/10",
      expect.objectContaining({ method: "PATCH" }),
    ));
    const patchCall = vi.mocked(api.request).mock.calls.find(([, init]) => init?.method === "PATCH");
    const body = JSON.parse(String(patchCall?.[1]?.body));
    expect(body.start_utc).toBe(events[0].start_utc + 2 * 86_400);
    expect(body.end_utc).toBe(events[0].end_utc + 2 * 86_400);
    await waitFor(() => expect(june18).toContainElement(screen.getByRole("button", { name: "Planning" })));
  });

  it("drags an all-day month event while preserving its day span", async () => {
    const allDay = { id: 12, calendar_id: 1, access: "details" as const, status: "confirmed" as const, event_kind: "all_day" as const, title: "Conference", start_date: "2025-06-16", end_date: "2025-06-18", version: 1 };
    const api = apiWithEvents();
    vi.mocked(api.request).mockImplementation((path: RequestInfo | URL, init?: RequestInit) => {
      if (String(path).includes("/events?") || !init?.method) return Promise.resolve(new Response(JSON.stringify([allDay]), { status: 200 }));
      const update = JSON.parse(String(init.body));
      return Promise.resolve(new Response(JSON.stringify({ ...allDay, ...update, version: 2 }), { status: 200 }));
    });
    render(<CalendarEventUI api={api} calendars={calendars} initialDate={new Date("2025-06-16T12:00:00Z")} />);

    const [conference] = await screen.findAllByRole("button", { name: "Conference" });
    const june19 = monthCell("19");
    fireEvent(conference, pointerEvent("pointerdown", 100));
    fireEvent(conference, pointerEvent("pointermove", 106));
    fireEvent(june19, pointerEvent("pointerenter", 300));
    fireEvent(june19, pointerEvent("pointerup", 300));

    await waitFor(() => expect(api.request).toHaveBeenCalledWith(
      "/api/v1/calendars/1/events/12",
      expect.objectContaining({ method: "PATCH" }),
    ));
    const patchCall = vi.mocked(api.request).mock.calls.find(([, init]) => init?.method === "PATCH");
    const body = JSON.parse(String(patchCall?.[1]?.body));
    expect(body.start_date).toBe("2025-06-19");
    expect(body.end_date).toBe("2025-06-21");
  });

  it("moves only the displayed recurring occurrence", async () => {
    const occurrence = { ...events[0], recurrence_rule: "FREQ=WEEKLY", recurrence_id: 77 };
    const api = apiWithEvents();
    vi.mocked(api.request).mockImplementation((path: RequestInfo | URL, init?: RequestInit) => {
      if (String(path).includes("/events?") || !init?.method) return Promise.resolve(new Response(JSON.stringify([occurrence]), { status: 200 }));
      const update = JSON.parse(String(init.body));
      return Promise.resolve(new Response(JSON.stringify({ ...occurrence, ...update, version: 2 }), { status: 200 }));
    });
    render(<CalendarEventUI api={api} calendars={calendars} initialDate={new Date("2025-06-16T12:00:00Z")} />);

    fireEvent.click(await screen.findByRole("tab", { name: "Day" }));
    const region = await screen.findByRole("region", { name: "Day calendar" });
    const grid = region.querySelector(":scope > .event-ui__day-region") as HTMLDivElement;
    grid.setPointerCapture = vi.fn();
    const block = screen.getByRole("button", { name: "Planning" }).closest(".event-ui__event-block")!;
    fireEvent(block, pointerEvent("pointerdown", 100, 100));
    fireEvent(block, pointerEvent("pointermove", 100, 115));
    fireEvent(block, pointerEvent("pointerup", 100, 115));

    await waitFor(() => expect(api.request).toHaveBeenCalledWith(
      "/api/v1/calendars/1/events/10/occurrences/77",
      expect.objectContaining({ method: "PATCH" }),
    ));
    const patchCall = vi.mocked(api.request).mock.calls.find(([path, init]) => String(path).includes("/occurrences/77") && init?.method === "PATCH");
    const body = JSON.parse(String(patchCall?.[1]?.body));
    expect(body.start_utc).toBe(occurrence.start_utc + 15 * 60);
    expect(body.end_utc).toBe(occurrence.end_utc + 15 * 60);
  });

  it.each(["Escape", "pointercancel"])("cancels an active move with %s without sending an update", async (cancellation) => {
    const api = apiWithEvents();
    render(<CalendarEventUI api={api} calendars={calendars} initialDate={new Date("2025-06-16T12:00:00Z")} />);

    fireEvent.click(await screen.findByRole("tab", { name: "Day" }));
    const region = await screen.findByRole("region", { name: "Day calendar" });
    const grid = region.querySelector(":scope > .event-ui__day-region") as HTMLDivElement;
    grid.setPointerCapture = vi.fn();
    const block = screen.getByRole("button", { name: "Planning" }).closest(".event-ui__event-block")!;
    fireEvent(block, pointerEvent("pointerdown", 100, 100));
    fireEvent(block, pointerEvent("pointermove", 100, 115));
    if (cancellation === "Escape") fireEvent.keyDown(window, { key: "Escape" });
    else fireEvent(block, pointerEvent("pointercancel", 100, 115));
    fireEvent(block, pointerEvent("pointerup", 100, 115));

    expect(vi.mocked(api.request).mock.calls.some(([, init]) => init?.method === "PATCH")).toBe(false);
    expect(region.querySelector(".event-ui__slot-highlight")).not.toBeInTheDocument();
  });

  it("applies a move optimistically then replaces it with the server projection", async () => {
    let resolveMove: (response: Response) => void = () => {};
    const movePromise = new Promise<Response>((resolve) => { resolveMove = resolve; });
    const api = apiWithEvents();
    vi.mocked(api.request).mockImplementation((path: RequestInfo | URL, init?: RequestInit) => {
      if (String(path).includes("/events?") || !init?.method) return Promise.resolve(new Response(JSON.stringify(events), { status: 200 }));
      return movePromise;
    });
    render(<CalendarEventUI api={api} calendars={calendars} initialDate={new Date("2025-06-16T12:00:00Z")} />);

    fireEvent.click(await screen.findByRole("tab", { name: "Day" }));
    const region = await screen.findByRole("region", { name: "Day calendar" });
    const grid = region.querySelector(":scope > .event-ui__day-region") as HTMLDivElement;
    grid.setPointerCapture = vi.fn();
    const block = () => screen.getByRole("button", { name: "Planning" }).closest(".event-ui__event-block") as HTMLElement;
    const originalTop = parseFloat(block().style.top);
    fireEvent(block(), pointerEvent("pointerdown", 100, 100));
    fireEvent(grid, pointerEvent("pointermove", 100, 122));
    fireEvent(grid, pointerEvent("pointerup", 100, 122));

    expect(parseFloat(block().style.top)).toBeCloseTo(originalTop + 15, 10);
    expect(block()).toHaveClass("event-ui__event-block--saving");

    // The server normalizes to a different time than the optimistic value; the final state must match the server.
    resolveMove(new Response(JSON.stringify({ ...events[0], start_utc: events[0].start_utc + 30 * 60, end_utc: events[0].end_utc + 30 * 60, version: 2 }), { status: 200 }));
    await waitFor(() => expect(block()).not.toHaveClass("event-ui__event-block--saving"));
    expect(parseFloat(block().style.top)).toBeCloseTo(originalTop + 30, 10);
  });

  it("settles a move even when another event's move is still in flight", async () => {
    const second = { ...events[0], id: 20, title: "Review", start_utc: events[0].start_utc + 7200, end_utc: events[0].end_utc + 7200 };
    const both = [events[0], second];
    let resolveA: (response: Response) => void = () => {};
    let resolveB: (response: Response) => void = () => {};
    const pendingA = new Promise<Response>((resolve) => { resolveA = resolve; });
    const pendingB = new Promise<Response>((resolve) => { resolveB = resolve; });
    const api = apiWithEvents();
    vi.mocked(api.request).mockImplementation((path: RequestInfo | URL, init?: RequestInit) => {
      if (String(path).includes("/events?") || !init?.method) return Promise.resolve(new Response(JSON.stringify(both), { status: 200 }));
      if (String(path).includes("/events/10")) return pendingA;
      if (String(path).includes("/events/20")) return pendingB;
      return Promise.resolve(new Response(null, { status: 200 }));
    });
    render(<CalendarEventUI api={api} calendars={calendars} initialDate={new Date("2025-06-16T12:00:00Z")} />);

    fireEvent.click(await screen.findByRole("tab", { name: "Day" }));
    const region = await screen.findByRole("region", { name: "Day calendar" });
    const grid = region.querySelector(":scope > .event-ui__day-region") as HTMLDivElement;
    grid.setPointerCapture = vi.fn();
    const blockA = () => screen.getByRole("button", { name: "Planning" }).closest(".event-ui__event-block") as HTMLElement;
    const blockB = () => screen.getByRole("button", { name: "Review" }).closest(".event-ui__event-block") as HTMLElement;
    const topA = parseFloat(blockA().style.top);
    const topB = parseFloat(blockB().style.top);

    fireEvent(blockA(), pointerEvent("pointerdown", 100, 100));
    fireEvent(grid, pointerEvent("pointermove", 100, 122));
    fireEvent(grid, pointerEvent("pointerup", 100, 122));
    fireEvent(blockB(), pointerEvent("pointerdown", 100, 100));
    fireEvent(grid, pointerEvent("pointermove", 100, 122));
    fireEvent(grid, pointerEvent("pointerup", 100, 122));

    // Resolve A while B is still in flight: A must settle to its server value, and B must keep its saving state.
    resolveA(new Response(JSON.stringify({ ...events[0], start_utc: events[0].start_utc + 30 * 60, end_utc: events[0].end_utc + 30 * 60, version: 2 }), { status: 200 }));
    await waitFor(() => expect(parseFloat(blockA().style.top)).toBeCloseTo(topA + 30, 10));
    expect(blockB()).toHaveClass("event-ui__event-block--saving");

    resolveB(new Response(JSON.stringify({ ...second, start_utc: second.start_utc + 15 * 60, end_utc: second.end_utc + 15 * 60, version: 2 }), { status: 200 }));
    await waitFor(() => expect(blockB()).not.toHaveClass("event-ui__event-block--saving"));
    expect(parseFloat(blockB().style.top)).toBeCloseTo(topB + 15, 10);
  });

  it("restores the exact original projection and reports an error when a move fails", async () => {
    const api = apiWithEvents();
    vi.mocked(api.request).mockImplementation((path: RequestInfo | URL, init?: RequestInit) => {
      if (String(path).includes("/events?") || !init?.method) return Promise.resolve(new Response(JSON.stringify(events), { status: 200 }));
      return Promise.resolve(new Response(null, { status: 500 }));
    });
    render(<CalendarEventUI api={api} calendars={calendars} initialDate={new Date("2025-06-16T12:00:00Z")} />);

    fireEvent.click(await screen.findByRole("tab", { name: "Day" }));
    const region = await screen.findByRole("region", { name: "Day calendar" });
    const grid = region.querySelector(":scope > .event-ui__day-region") as HTMLDivElement;
    grid.setPointerCapture = vi.fn();
    const block = () => screen.getByRole("button", { name: "Planning" }).closest(".event-ui__event-block") as HTMLElement;
    const originalTop = parseFloat(block().style.top);
    fireEvent(block(), pointerEvent("pointerdown", 100, 100));
    fireEvent(grid, pointerEvent("pointermove", 100, 122));
    fireEvent(grid, pointerEvent("pointerup", 100, 122));

    await waitFor(() => expect(screen.getByRole("alert")).toHaveTextContent("We could not move this event."));
    expect(parseFloat(block().style.top)).toBe(originalTop);
    expect(block()).not.toHaveClass("event-ui__event-block--saving");
  });

  it("restores the event and offers a reload after a version conflict on move", async () => {
    const api = apiWithEvents();
    vi.mocked(api.request).mockImplementation((path: RequestInfo | URL, init?: RequestInit) => {
      if (String(path).includes("/events?") || !init?.method) return Promise.resolve(new Response(JSON.stringify(events), { status: 200 }));
      return Promise.resolve(new Response(null, { status: 409 }));
    });
    render(<CalendarEventUI api={api} calendars={calendars} initialDate={new Date("2025-06-16T12:00:00Z")} />);

    fireEvent.click(await screen.findByRole("tab", { name: "Day" }));
    const region = await screen.findByRole("region", { name: "Day calendar" });
    const grid = region.querySelector(":scope > .event-ui__day-region") as HTMLDivElement;
    grid.setPointerCapture = vi.fn();
    const block = () => screen.getByRole("button", { name: "Planning" }).closest(".event-ui__event-block") as HTMLElement;
    const originalTop = parseFloat(block().style.top);
    fireEvent(block(), pointerEvent("pointerdown", 100, 100));
    fireEvent(grid, pointerEvent("pointermove", 100, 122));
    fireEvent(grid, pointerEvent("pointerup", 100, 122));

    await waitFor(() => expect(screen.getByRole("alert")).toHaveTextContent("This event changed elsewhere. Reload it before saving again."));
    expect(parseFloat(block().style.top)).toBe(originalTop);
    expect(screen.getByRole("button", { name: "Reload events" })).toBeInTheDocument();
  });

  it("moves a downward event by the full delta without clamping (Slice 16)", async () => {
    const api = apiWithEvents();
    vi.mocked(api.request).mockImplementation((path: RequestInfo | URL, init?: RequestInit) => {
      if (String(path).includes("/events?") || !init?.method) return Promise.resolve(new Response(JSON.stringify(events), { status: 200 }));
      const update = JSON.parse(String(init.body));
      return Promise.resolve(new Response(JSON.stringify({ ...events[0], ...update, version: 2 }), { status: 200 }));
    });
    render(<CalendarEventUI api={api} calendars={calendars} initialDate={new Date("2025-06-16T12:00:00Z")} />);

    fireEvent.click(await screen.findByRole("tab", { name: "Day" }));
    const region = await screen.findByRole("region", { name: "Day calendar" });
    const grid = region.querySelector(":scope > .event-ui__day-region") as HTMLDivElement;
    grid.setPointerCapture = vi.fn();
    const block = () => screen.getByRole("button", { name: "Planning" }).closest(".event-ui__event-block") as HTMLElement;
    fireEvent(block(), pointerEvent("pointerdown", 100, 100));
    fireEvent(grid, pointerEvent("pointermove", 100, 5000));
    fireEvent(grid, pointerEvent("pointerup", 100, 5000));

    await waitFor(() => expect(api.request).toHaveBeenCalledWith("/api/v1/calendars/1/events/10", expect.objectContaining({ method: "PATCH" })));
    const patchCall = vi.mocked(api.request).mock.calls.find(([, init]) => init?.method === "PATCH");
    const body = JSON.parse(String(patchCall?.[1]?.body));
    // Duration is preserved
    expect(body.end_utc - body.start_utc).toBe(events[0].end_utc - events[0].start_utc);
    // Both timestamps shifted by the same delta (5000px snapped to 15-min = 5010 min)
    const delta = body.start_utc - events[0].start_utc;
    expect(body.end_utc - events[0].end_utc).toBe(delta);
    // Delta is a multiple of 15 minutes
    expect(delta % (15 * 60)).toBe(0);
  });

  it("suppresses the detail click after a completed drag (Slice 17)", async () => {
    const api = apiWithEvents();
    vi.mocked(api.request).mockImplementation((path: RequestInfo | URL, init?: RequestInit) => {
      if (String(path).includes("/events?") || !init?.method) return Promise.resolve(new Response(JSON.stringify(events), { status: 200 }));
      const update = JSON.parse(String(init.body));
      return Promise.resolve(new Response(JSON.stringify({ ...events[0], ...update, version: 2 }), { status: 200 }));
    });
    render(<CalendarEventUI api={api} calendars={calendars} initialDate={new Date("2025-06-16T12:00:00Z")} />);

    fireEvent.click(await screen.findByRole("tab", { name: "Day" }));
    const region = await screen.findByRole("region", { name: "Day calendar" });
    const grid = region.querySelector(":scope > .event-ui__day-region") as HTMLDivElement;
    grid.setPointerCapture = vi.fn();
    const block = () => screen.getByRole("button", { name: "Planning" }).closest(".event-ui__event-block") as HTMLElement;

    // Perform a drag
    fireEvent(block(), pointerEvent("pointerdown", 100, 100));
    fireEvent(grid, pointerEvent("pointermove", 100, 130));
    fireEvent(grid, pointerEvent("pointerup", 100, 130));

    // Click should be suppressed
    fireEvent.click(block());
    expect(screen.queryByRole("complementary", { name: "Event details" })).not.toBeInTheDocument();
  });

  it("suppresses the detail click after a completed resize (Slice 17)", async () => {
    const api = apiWithEvents();
    vi.mocked(api.request).mockImplementation((path: RequestInfo | URL, init?: RequestInit) => {
      if (String(path).includes("/events?") || !init?.method) return Promise.resolve(new Response(JSON.stringify(events), { status: 200 }));
      const update = JSON.parse(String(init.body));
      return Promise.resolve(new Response(JSON.stringify({ ...events[0], ...update, version: 2 }), { status: 200 }));
    });
    render(<CalendarEventUI api={api} calendars={calendars} initialDate={new Date("2025-06-16T12:00:00Z")} />);

    fireEvent.click(await screen.findByRole("tab", { name: "Day" }));
    const region = await screen.findByRole("region", { name: "Day calendar" });
    const grid = region.querySelector(":scope > .event-ui__day-region") as HTMLDivElement;
    grid.setPointerCapture = vi.fn();
    const block = () => screen.getByRole("button", { name: "Planning" }).closest(".event-ui__event-block") as HTMLElement;
    const handle = block().querySelector(".event-ui__resize-handle--bottom") as HTMLElement;

    // Perform a resize
    fireEvent(handle, pointerEvent("pointerdown", 100, 100));
    fireEvent(grid, pointerEvent("pointermove", 100, 130));
    fireEvent(grid, pointerEvent("pointerup", 100, 130));

    // Click should be suppressed
    fireEvent.click(block());
    expect(screen.queryByRole("complementary", { name: "Event details" })).not.toBeInTheDocument();
  });

  it("does not suppress a simple click without a prior drag (Slice 17)", async () => {
    render(<CalendarEventUI api={apiWithEvents()} calendars={calendars} initialDate={new Date("2025-06-16T12:00:00Z")} />);

    fireEvent.click(await screen.findByRole("tab", { name: "Day" }));
    await screen.findByRole("region", { name: "Day calendar" });
    const block = screen.getByRole("button", { name: "Planning" }).closest(".event-ui__event-block") as HTMLElement;

    // Simple click should open detail
    fireEvent.click(block);
    expect(screen.getByRole("complementary", { name: "Event details" })).toBeInTheDocument();
  });

  it("does not suppress a click after a cancelled gesture (Slice 17)", async () => {
    render(<CalendarEventUI api={apiWithEvents()} calendars={calendars} initialDate={new Date("2025-06-16T12:00:00Z")} />);

    fireEvent.click(await screen.findByRole("tab", { name: "Day" }));
    const region = await screen.findByRole("region", { name: "Day calendar" });
    const grid = region.querySelector(":scope > .event-ui__day-region") as HTMLDivElement;
    grid.setPointerCapture = vi.fn();
    const block = () => screen.getByRole("button", { name: "Planning" }).closest(".event-ui__event-block") as HTMLElement;

    // Start a drag then cancel
    fireEvent(block(), pointerEvent("pointerdown", 100, 100));
    fireEvent(grid, pointerEvent("pointercancel", 100, 130));

    // Click should NOT be suppressed
    fireEvent.click(block());
    expect(screen.getByRole("complementary", { name: "Event details" })).toBeInTheDocument();
  });

  it("top resize in day view preserves the end time (Slice 17)", async () => {
    const api = apiWithEvents();
    vi.mocked(api.request).mockImplementation((path: RequestInfo | URL, init?: RequestInit) => {
      if (String(path).includes("/events?") || !init?.method) return Promise.resolve(new Response(JSON.stringify(events), { status: 200 }));
      const update = JSON.parse(String(init.body));
      return Promise.resolve(new Response(JSON.stringify({ ...events[0], ...update, version: 2 }), { status: 200 }));
    });
    render(<CalendarEventUI api={api} calendars={calendars} initialDate={new Date("2025-06-16T12:00:00Z")} />);

    fireEvent.click(await screen.findByRole("tab", { name: "Day" }));
    const region = await screen.findByRole("region", { name: "Day calendar" });
    const grid = region.querySelector(":scope > .event-ui__day-region") as HTMLDivElement;
    grid.setPointerCapture = vi.fn();
    const block = () => screen.getByRole("button", { name: "Planning" }).closest(".event-ui__event-block") as HTMLElement;
    const handle = block().querySelector(".event-ui__resize-handle--top") as HTMLElement;

    // Drag the top handle down by 30px (30 minutes)
    fireEvent(handle, pointerEvent("pointerdown", 100, 100));
    fireEvent(grid, pointerEvent("pointermove", 100, 130));
    fireEvent(grid, pointerEvent("pointerup", 100, 130));

    await waitFor(() => expect(api.request).toHaveBeenCalledWith("/api/v1/calendars/1/events/10", expect.objectContaining({ method: "PATCH" })));
    const patchCall = vi.mocked(api.request).mock.calls.find(([, init]) => init?.method === "PATCH");
    const body = JSON.parse(String(patchCall?.[1]?.body));
    // End is preserved, start shifted later by 30 minutes
    expect(body.end_utc).toBe(events[0].end_utc);
    expect(body.start_utc).toBe(events[0].start_utc + 30 * 60);
  });

  it("bottom resize in day view preserves the start time (Slice 17)", async () => {
    const api = apiWithEvents();
    vi.mocked(api.request).mockImplementation((path: RequestInfo | URL, init?: RequestInit) => {
      if (String(path).includes("/events?") || !init?.method) return Promise.resolve(new Response(JSON.stringify(events), { status: 200 }));
      const update = JSON.parse(String(init.body));
      return Promise.resolve(new Response(JSON.stringify({ ...events[0], ...update, version: 2 }), { status: 200 }));
    });
    render(<CalendarEventUI api={api} calendars={calendars} initialDate={new Date("2025-06-16T12:00:00Z")} />);

    fireEvent.click(await screen.findByRole("tab", { name: "Day" }));
    const region = await screen.findByRole("region", { name: "Day calendar" });
    const grid = region.querySelector(":scope > .event-ui__day-region") as HTMLDivElement;
    grid.setPointerCapture = vi.fn();
    const block = () => screen.getByRole("button", { name: "Planning" }).closest(".event-ui__event-block") as HTMLElement;
    const handle = block().querySelector(".event-ui__resize-handle--bottom") as HTMLElement;

    // Drag the bottom handle down by 30px (30 minutes)
    fireEvent(handle, pointerEvent("pointerdown", 100, 100));
    fireEvent(grid, pointerEvent("pointermove", 100, 130));
    fireEvent(grid, pointerEvent("pointerup", 100, 130));

    await waitFor(() => expect(api.request).toHaveBeenCalledWith("/api/v1/calendars/1/events/10", expect.objectContaining({ method: "PATCH" })));
    const patchCall = vi.mocked(api.request).mock.calls.find(([, init]) => init?.method === "PATCH");
    const body = JSON.parse(String(patchCall?.[1]?.body));
    // Start is preserved, end shifted later by 30 minutes
    expect(body.start_utc).toBe(events[0].start_utc);
    expect(body.end_utc).toBe(events[0].end_utc + 30 * 60);
  });

  it("deletes an event from the detail panel", async () => {
    const api = apiWithEvents();
    vi.mocked(api.request).mockImplementation((path: RequestInfo | URL, init?: RequestInit) => {
      if (String(path).includes("/events?") || !init?.method) return Promise.resolve(new Response(JSON.stringify(events), { status: 200 }));
      if (init?.method === "DELETE") return Promise.resolve(new Response(null, { status: 204 }));
      return Promise.resolve(new Response(null, { status: 200 }));
    });
    vi.spyOn(window, "confirm").mockReturnValue(true);
    render(<CalendarEventUI api={api} calendars={calendars} initialDate={new Date("2025-06-16T12:00:00Z")} />);

    fireEvent.click(await screen.findByRole("button", { name: "Planning" }));
    fireEvent.click(screen.getByRole("button", { name: /Delete event/ }));

    await waitFor(() => expect(api.request).toHaveBeenCalledWith("/api/v1/calendars/1/events/10", expect.objectContaining({ method: "DELETE" })));
    await waitFor(() => expect(screen.queryByRole("complementary", { name: "Event details" })).not.toBeInTheDocument());
    expect(window.confirm).toHaveBeenCalledWith(expect.stringContaining("Planning"));
  });

  it("does not delete when confirmation is declined", async () => {
    const api = apiWithEvents();
    vi.spyOn(window, "confirm").mockReturnValue(false);
    render(<CalendarEventUI api={api} calendars={calendars} initialDate={new Date("2025-06-16T12:00:00Z")} />);

    fireEvent.click(await screen.findByRole("button", { name: "Planning" }));
    fireEvent.click(screen.getByRole("button", { name: /Delete event/ }));

    expect(vi.mocked(api.request).mock.calls.some(([, init]) => init?.method === "DELETE")).toBe(false);
    expect(screen.getByRole("complementary", { name: "Event details" })).toBeInTheDocument();
  });

  it("does not show delete button for external events", async () => {
    render(<CalendarEventUI api={apiWithEvents()} calendars={calendars} initialDate={new Date("2025-06-16T12:00:00Z")} />);

    fireEvent.click(await screen.findByRole("button", { name: "Imported holiday (read-only external event)" }));
    expect(screen.queryByRole("button", { name: /Delete event/ })).not.toBeInTheDocument();
  });

  it("deletes a recurring occurrence via the occurrence endpoint", async () => {
    const occurrence = { ...events[0], recurrence_rule: "FREQ=WEEKLY", recurrence_id: 77 };
    const api = apiWithEvents();
    vi.mocked(api.request).mockImplementation((path: RequestInfo | URL, init?: RequestInit) => {
      if (String(path).includes("/events?") || !init?.method) return Promise.resolve(new Response(JSON.stringify([occurrence]), { status: 200 }));
      if (init?.method === "DELETE") return Promise.resolve(new Response(null, { status: 204 }));
      return Promise.resolve(new Response(null, { status: 200 }));
    });
    vi.spyOn(window, "confirm").mockReturnValue(true);
    render(<CalendarEventUI api={api} calendars={calendars} initialDate={new Date("2025-06-16T12:00:00Z")} />);

    fireEvent.click(await screen.findByRole("button", { name: "Planning" }));
    fireEvent.click(screen.getByRole("button", { name: /Delete event/ }));

    await waitFor(() => expect(api.request).toHaveBeenCalledWith("/api/v1/calendars/1/events/10/occurrences/77", expect.objectContaining({ method: "DELETE" })));
  });

  it("resizes an event without turning the interaction into a move", async () => {
    const api = apiWithEvents();
    vi.mocked(api.request).mockImplementation((path: RequestInfo | URL, init?: RequestInit) => {
      if (String(path).includes("/events?") || !init?.method) return Promise.resolve(new Response(JSON.stringify(events), { status: 200 }));
      const update = JSON.parse(String(init.body));
      return Promise.resolve(new Response(JSON.stringify({ ...events[0], ...update, version: 2 }), { status: 200 }));
    });
    render(<CalendarEventUI api={api} calendars={calendars} initialDate={new Date("2025-06-16T12:00:00Z")} />);

    fireEvent.click(await screen.findByRole("tab", { name: "Day" }));
    const region = await screen.findByRole("region", { name: "Day calendar" });
    const grid = region.querySelector(":scope > .event-ui__day-region") as HTMLDivElement;
    grid.setPointerCapture = vi.fn();
    const block = screen.getByRole("button", { name: "Planning" }).closest(".event-ui__event-block") as HTMLElement;
    const bottomHandle = block.querySelector(".event-ui__resize-handle--bottom") as HTMLElement;
    const originalTop = parseFloat(block.style.top);
    const originalHeight = parseFloat(block.style.height);
    fireEvent(bottomHandle, pointerEvent("pointerdown", 100, 100));
    fireEvent(grid, pointerEvent("pointermove", 100, 122));
    fireEvent(grid, pointerEvent("pointerup", 100, 122));

    await waitFor(() => expect(api.request).toHaveBeenCalledWith("/api/v1/calendars/1/events/10", expect.objectContaining({ method: "PATCH" })));
    const patchCall = vi.mocked(api.request).mock.calls.find(([, init]) => init?.method === "PATCH");
    const body = JSON.parse(String(patchCall?.[1]?.body));
    expect(body.start_utc).toBe(events[0].start_utc);
    expect(body.end_utc - body.start_utc).toBe((events[0].end_utc - events[0].start_utc) + 15 * 60);
    const afterBlock = screen.getByRole("button", { name: "Planning" }).closest(".event-ui__event-block") as HTMLElement;
    expect(parseFloat(afterBlock.style.top)).toBe(originalTop);
    expect(parseFloat(afterBlock.style.height)).toBeCloseTo(originalHeight + 15, 10);
  });

  it("does not duplicate the day grid class between the section and the day region", async () => {
    render(<CalendarEventUI api={apiWithEvents()} calendars={calendars} initialDate={new Date("2025-06-16T12:00:00Z")} />);

    fireEvent.click(await screen.findByRole("tab", { name: "Day" }));
    const section = await screen.findByRole("region", { name: "Day calendar" });
    expect(section).toHaveClass("event-ui__day");
    const region = section.querySelector(":scope > .event-ui__day-region") as HTMLElement;
    expect(region).toBeInTheDocument();
    expect(region).not.toHaveClass("event-ui__day");
  });

  it("shows loading status in day view during a delayed request, then shows events when the request resolves", async () => {
    let resolveSignal!: () => void;
    const signal = new Promise<void>((resolve) => { resolveSignal = resolve; });
    const api = apiWithEvents();
    vi.mocked(api.request).mockImplementation((path: RequestInfo | URL) => {
      if (String(path).includes("/events?")) {
        // Each call gets its own Response object to avoid "Body already read" errors
        return signal.then(() => new Response(JSON.stringify(events), { status: 200 }));
      }
      return Promise.resolve(new Response(JSON.stringify([]), { status: 200 }));
    });

    render(<CalendarEventUI api={api} calendars={calendars} initialDate={new Date("2025-06-16T12:00:00Z")} />);
    fireEvent.click(screen.getByRole("tab", { name: "Day" }));

    // Loading status should be visible while the request is pending
    expect(screen.getByRole("status")).toHaveTextContent("Loading events…");
    expect(screen.queryByRole("button", { name: "Planning" })).not.toBeInTheDocument();

    // Resolve the request
    await act(async () => { resolveSignal(); });

    // Events should now be visible
    expect(await screen.findByRole("button", { name: "Planning" })).toBeInTheDocument();
    expect(screen.queryByRole("status")).not.toBeInTheDocument();
  });

  it("shows load error with retry in day view when the request fails, then shows events after successful retry", async () => {
    let callCount = 0;
    const api = apiWithEvents();
    vi.mocked(api.request).mockImplementation((path: RequestInfo | URL) => {
      if (String(path).includes("/events?")) {
        callCount++;
        // Each reload makes one call per calendar (2 calendars = 2 calls).
        // First two reloads (4 calls) fail; retry (calls 5+) succeeds.
        if (callCount <= 4) return Promise.resolve(new Response(null, { status: 500 }));
        return Promise.resolve(new Response(JSON.stringify(events), { status: 200 }));
      }
      return Promise.resolve(new Response(JSON.stringify([]), { status: 200 }));
    });

    render(<CalendarEventUI api={api} calendars={calendars} initialDate={new Date("2025-06-16T12:00:00Z")} />);
    fireEvent.click(screen.getByRole("tab", { name: "Day" }));

    // Load error should be visible with a Retry button (scoped to day region to avoid the banner alert)
    const dayRegion = await screen.findByRole("region", { name: "Day calendar" });
    const alert = await waitFor(() => {
      const el = dayRegion.querySelector('[role="alert"]');
      if (!el) throw new Error("alert not found");
      return el;
    });
    expect(alert).toHaveTextContent("We could not load events");
    const retryButton = screen.getByRole("button", { name: "Retry" });
    expect(retryButton).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Planning" })).not.toBeInTheDocument();

    // Click Retry
    fireEvent.click(retryButton);

    // Events should now be visible
    expect(await screen.findByRole("button", { name: "Planning" })).toBeInTheDocument();
  });

  it("does not treat mutation errors as load failures in day view", async () => {
    const api = apiWithEvents();
    vi.mocked(api.request).mockImplementation((path: RequestInfo | URL, init?: RequestInit) => {
      if (String(path).includes("/events?") || !init?.method) return Promise.resolve(new Response(JSON.stringify(events), { status: 200 }));
      return Promise.resolve(new Response(null, { status: 500 }));
    });

    render(<CalendarEventUI api={api} calendars={calendars} initialDate={new Date("2025-06-16T12:00:00Z")} />);
    fireEvent.click(await screen.findByRole("tab", { name: "Day" }));
    const region = await screen.findByRole("region", { name: "Day calendar" });
    const grid = region.querySelector(":scope > .event-ui__day-region") as HTMLDivElement;
    grid.setPointerCapture = vi.fn();
    const block = screen.getByRole("button", { name: "Planning" }).closest(".event-ui__event-block") as HTMLElement;
    fireEvent(block, pointerEvent("pointerdown", 100, 100));
    fireEvent(grid, pointerEvent("pointermove", 100, 122));
    fireEvent(grid, pointerEvent("pointerup", 100, 122));

    // Mutation error should be shown (not a load error)
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("We could not move this event.");
    expect(alert).not.toHaveTextContent("We could not load events");

    // Events should still be visible
    expect(screen.getByRole("button", { name: "Planning" })).toBeInTheDocument();

    // No "Retry" button for the load error (only the banner's "Reload events" if applicable)
    expect(screen.queryByRole("button", { name: "Retry" })).not.toBeInTheDocument();
  });

});

it("mobile Agenda shows Busy and read-only external labels without exposing busy metadata", async () => {
  vi.stubGlobal("innerWidth", 390);
  const api = apiWithEvents();
  vi.mocked(api.request).mockImplementation(async () => new Response(JSON.stringify([
    { ...events[0], access: "free_busy", title: "Hidden title", location: "Hidden room" },
    events[1],
  ]), { status: 200 }));
  render(<CalendarEventUI api={api} calendars={calendars} initialDate={new Date("2025-06-16T12:00:00Z")} />);
  expect(await screen.findByRole("button", { name: /Busy/ })).toBeInTheDocument();
  expect(screen.getByText("Read-only external event")).toBeInTheDocument();
  expect(screen.queryByText("Hidden title")).not.toBeInTheDocument();
  expect(screen.queryByText("Hidden room")).not.toBeInTheDocument();
});

it("mobile Agenda offers a retry after a failed live read", async () => {
  vi.stubGlobal("innerWidth", 390);
  const api = apiWithEvents();
  vi.mocked(api.request).mockResolvedValue(new Response(null, { status: 500 }));
  render(<CalendarEventUI api={api} calendars={calendars} initialDate={new Date("2025-06-16T12:00:00Z")} />);
  expect(await screen.findByRole("alert")).toHaveTextContent("We could not load events");
  vi.mocked(api.request).mockImplementation(async () => new Response(JSON.stringify(events), { status: 200 }));
  fireEvent.click(screen.getByRole("button", { name: "Retry events" }));
  expect(await screen.findByRole("button", { name: /Planning/ })).toBeInTheDocument();
  expect(screen.queryByRole("alert")).not.toBeInTheDocument();
});

it("mobile Month selects readable day events without creating and New uses that day", async () => {
  vi.stubGlobal("innerWidth", 390);
  const api = apiWithEvents();
  render(<CalendarEventUI api={api} calendars={calendars} initialDate={new Date("2025-06-16T12:00:00Z")} />);
  await screen.findByRole("button", { name: /Planning/ });
  fireEvent.click(screen.getByRole("tab", { name: "Month" }));
  fireEvent.click(await screen.findByRole("button", { name: /June 17, 2025.*0 events/ }));
  expect(screen.queryByRole("form", { name: "Create event" })).not.toBeInTheDocument();
  expect(screen.getByText("No events on this day.")).toBeInTheDocument();
  fireEvent.click(screen.getByRole("button", { name: "New event" }));
  expect(screen.getByLabelText("Start")).toHaveValue("2025-06-17T09:00");
});


it("mobile details hide the calendar, focus the heading, and return through history", async () => {
  vi.stubGlobal("matchMedia", vi.fn().mockReturnValue({ matches: true, addEventListener: vi.fn(), removeEventListener: vi.fn() }));
  window.history.replaceState({}, "", "/dashboard?view=agenda&date=2025-06-16");
  render(<CalendarNavigationProvider userId={7}><CalendarEventUI api={apiWithEvents()} calendars={calendars} /></CalendarNavigationProvider>);
  const card = await screen.findByRole("button", { name: /Planning/ });
  vi.stubGlobal("scrollTo", vi.fn());
  card.focus(); fireEvent.click(card);
  const heading = await screen.findByRole("heading", { name: "Planning" });
  await waitFor(() => expect(heading).toHaveFocus());
  expect(screen.queryByRole("region", { name: "Agenda" })).not.toBeInTheDocument();
  fireEvent.click(screen.getByRole("button", { name: "Back to calendar" }));
  await waitFor(() => expect(screen.queryByRole("complementary", { name: "Event details" })).not.toBeInTheDocument());
  await waitFor(() => expect(card).toHaveFocus());
});


it("revoked mobile detail identity shows unavailable instead of another calendar event", async () => {
  vi.stubGlobal("matchMedia", vi.fn().mockReturnValue({ matches: true, addEventListener: vi.fn(), removeEventListener: vi.fn() }));
  vi.stubGlobal("scrollTo", vi.fn());
  window.history.replaceState({}, "", "/dashboard?view=agenda&date=2025-06-16");
  const api = apiWithEvents();
  const { rerender } = render(<CalendarNavigationProvider userId={7}><CalendarEventUI api={api} calendars={calendars} /></CalendarNavigationProvider>);
  fireEvent.click(await screen.findByRole("button", { name: /Planning/ }));
  await screen.findByRole("heading", { name: "Planning" });
  rerender(<CalendarNavigationProvider userId={7}><CalendarEventUI api={api} calendars={[calendars[1]]} /></CalendarNavigationProvider>);
  expect(await screen.findByRole("heading", { name: "Event unavailable" })).toBeInTheDocument();
  expect(screen.queryByRole("button", { name: "Edit event" })).not.toBeInTheDocument();
  fireEvent.click(screen.getByRole("button", { name: "Back to calendar" }));
  await waitFor(() => expect(screen.getByRole("heading", { name: "Events" })).toHaveFocus());
});

describe("CalendarEventUI scroll restoration (Slice 14)", () => {
  it("restores zero page scroll and timeline coordinates after async load", async () => {
    vi.stubGlobal("matchMedia", vi.fn().mockReturnValue({ matches: false, addEventListener: vi.fn(), removeEventListener: vi.fn() }));
    const scrollToMock = vi.fn();
    vi.stubGlobal("scrollTo", scrollToMock);
    // Set up history state with a saved scroll of all zeros
    window.history.replaceState({ commoncalCalendar: { version: 1, userId: 7, chainId: "test", index: 0, snapshot: { view: "day", anchorDate: "2025-06-16", selectedDay: "2025-06-16", visibleCalendarIds: null, scroll: { page: 0, timelineTop: 0, timelineLeft: 0 } }, surface: { kind: "calendar" } } }, "", "/dashboard?view=day&date=2025-06-16");
    render(<CalendarNavigationProvider userId={7}><CalendarEventUI api={apiWithEvents()} calendars={calendars} /></CalendarNavigationProvider>);
    await screen.findByRole("button", { name: "Planning" });
    // Wait for the rAF restoration to fire
    await new Promise((r) => setTimeout(r, 50));
    expect(scrollToMock).toHaveBeenCalledWith(0, 0);
  });

  it("restores nonzero page scroll and timeline coordinates after async load", async () => {
    vi.stubGlobal("matchMedia", vi.fn().mockReturnValue({ matches: false, addEventListener: vi.fn(), removeEventListener: vi.fn() }));
    const scrollToMock = vi.fn();
    vi.stubGlobal("scrollTo", scrollToMock);
    window.history.replaceState({ commoncalCalendar: { version: 1, userId: 7, chainId: "test", index: 0, snapshot: { view: "day", anchorDate: "2025-06-16", selectedDay: "2025-06-16", visibleCalendarIds: null, scroll: { page: 150, timelineTop: 300, timelineLeft: 0 } }, surface: { kind: "calendar" } } }, "", "/dashboard?view=day&date=2025-06-16");
    render(<CalendarNavigationProvider userId={7}><CalendarEventUI api={apiWithEvents()} calendars={calendars} /></CalendarNavigationProvider>);
    await screen.findByRole("button", { name: "Planning" });
    await new Promise((r) => setTimeout(r, 50));
    expect(scrollToMock).toHaveBeenCalledWith(0, 150);
  });

  it("restoration wins over anchor when saved scroll exists", async () => {
    vi.stubGlobal("matchMedia", vi.fn().mockReturnValue({ matches: false, addEventListener: vi.fn(), removeEventListener: vi.fn() }));
    vi.stubGlobal("scrollTo", vi.fn());
    window.history.replaceState({ commoncalCalendar: { version: 1, userId: 7, chainId: "test", index: 0, snapshot: { view: "day", anchorDate: "2025-06-16", selectedDay: "2025-06-16", visibleCalendarIds: null, scroll: { page: 0, timelineTop: 500, timelineLeft: 0 } }, surface: { kind: "calendar" } } }, "", "/dashboard?view=day&date=2025-06-16");
    render(<CalendarNavigationProvider userId={7}><CalendarEventUI api={apiWithEvents()} calendars={calendars} /></CalendarNavigationProvider>);
    await screen.findByRole("button", { name: "Planning" });
    await new Promise((r) => setTimeout(r, 50));
    // The day region should have scrollTop = 500 (restored), not the anchor value
    const dayRegion = document.querySelector(".event-ui__day-region");
    expect(dayRegion).not.toBeNull();
    expect(dayRegion!.scrollTop).toBe(500);
  });
});
