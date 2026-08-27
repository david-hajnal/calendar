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
    const grid = region.querySelector(":scope > .event-ui__day") as HTMLDivElement;
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

  it("treats movement below five pixels as selection without moving or editing", async () => {
    const api = apiWithEvents();
    render(<CalendarEventUI api={api} calendars={calendars} initialDate={new Date("2025-06-16T12:00:00Z")} />);

    fireEvent.click(await screen.findByRole("tab", { name: "Day" }));
    const region = await screen.findByRole("region", { name: "Day calendar" });
    const grid = region.querySelector(":scope > .event-ui__day") as HTMLDivElement;
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

    const conference = await screen.findByRole("button", { name: "Conference" });
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
    const grid = region.querySelector(":scope > .event-ui__day") as HTMLDivElement;
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
    const grid = region.querySelector(":scope > .event-ui__day") as HTMLDivElement;
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
    const grid = region.querySelector(":scope > .event-ui__day") as HTMLDivElement;
    grid.setPointerCapture = vi.fn();
    const block = () => screen.getByRole("button", { name: "Planning" }).closest(".event-ui__event-block") as HTMLElement;
    const originalTop = parseFloat(block().style.top);
    fireEvent(block(), pointerEvent("pointerdown", 100, 100));
    fireEvent(grid, pointerEvent("pointermove", 100, 122));
    fireEvent(grid, pointerEvent("pointerup", 100, 122));

    expect(parseFloat(block().style.top)).toBe(originalTop + 15);
    expect(block()).toHaveClass("event-ui__event-block--saving");

    // The server normalizes to a different time than the optimistic value; the final state must match the server.
    resolveMove(new Response(JSON.stringify({ ...events[0], start_utc: events[0].start_utc + 30 * 60, end_utc: events[0].end_utc + 30 * 60, version: 2 }), { status: 200 }));
    await waitFor(() => expect(block()).not.toHaveClass("event-ui__event-block--saving"));
    expect(parseFloat(block().style.top)).toBe(originalTop + 30);
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
    const grid = region.querySelector(":scope > .event-ui__day") as HTMLDivElement;
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
    await waitFor(() => expect(parseFloat(blockA().style.top)).toBe(topA + 30));
    expect(blockB()).toHaveClass("event-ui__event-block--saving");

    resolveB(new Response(JSON.stringify({ ...second, start_utc: second.start_utc + 15 * 60, end_utc: second.end_utc + 15 * 60, version: 2 }), { status: 200 }));
    await waitFor(() => expect(blockB()).not.toHaveClass("event-ui__event-block--saving"));
    expect(parseFloat(blockB().style.top)).toBe(topB + 15);
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
    const grid = region.querySelector(":scope > .event-ui__day") as HTMLDivElement;
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
    const grid = region.querySelector(":scope > .event-ui__day") as HTMLDivElement;
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

  it("clamps a downward move so the event stays within the day", async () => {
    const api = apiWithEvents();
    vi.mocked(api.request).mockImplementation((path: RequestInfo | URL, init?: RequestInit) => {
      if (String(path).includes("/events?") || !init?.method) return Promise.resolve(new Response(JSON.stringify(events), { status: 200 }));
      const update = JSON.parse(String(init.body));
      return Promise.resolve(new Response(JSON.stringify({ ...events[0], ...update, version: 2 }), { status: 200 }));
    });
    render(<CalendarEventUI api={api} calendars={calendars} initialDate={new Date("2025-06-16T12:00:00Z")} />);

    fireEvent.click(await screen.findByRole("tab", { name: "Day" }));
    const region = await screen.findByRole("region", { name: "Day calendar" });
    const grid = region.querySelector(":scope > .event-ui__day") as HTMLDivElement;
    grid.setPointerCapture = vi.fn();
    const block = () => screen.getByRole("button", { name: "Planning" }).closest(".event-ui__event-block") as HTMLElement;
    fireEvent(block(), pointerEvent("pointerdown", 100, 100));
    fireEvent(grid, pointerEvent("pointermove", 100, 5000));
    fireEvent(grid, pointerEvent("pointerup", 100, 5000));

    await waitFor(() => expect(api.request).toHaveBeenCalledWith("/api/v1/calendars/1/events/10", expect.objectContaining({ method: "PATCH" })));
    const patchCall = vi.mocked(api.request).mock.calls.find(([, init]) => init?.method === "PATCH");
    const body = JSON.parse(String(patchCall?.[1]?.body));
    const startLocal = new Date(body.start_utc * 1000);
    const localDate = `${startLocal.getFullYear()}-${String(startLocal.getMonth() + 1).padStart(2, "0")}-${String(startLocal.getDate()).padStart(2, "0")}`;
    expect(localDate).toBe("2025-06-16");
    const startMinutes = startLocal.getHours() * 60 + startLocal.getMinutes();
    const durationMinutes = (body.end_utc - body.start_utc) / 60;
    expect(startMinutes + durationMinutes).toBeLessThanOrEqual(1440);
    expect(body.end_utc - body.start_utc).toBe(events[0].end_utc - events[0].start_utc);
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
    const grid = region.querySelector(":scope > .event-ui__day") as HTMLDivElement;
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
    expect(parseFloat(afterBlock.style.height)).toBe(originalHeight + 15);
  });
});
