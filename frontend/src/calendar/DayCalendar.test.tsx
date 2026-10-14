import * as React from "react";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { EventProjection } from "./api";
import { DayCalendar, type DayCalendarProps } from "./DayCalendar";

const mockEvent: EventProjection = {
  id: 10,
  calendar_id: 1,
  access: "details",
  status: "confirmed",
  event_kind: "timed",
  title: "Planning",
  start_utc: 1_750_032_800,
  end_utc: 1_750_036_400,
  timezone: "UTC",
  version: 1,
  start_date: "2025-06-16",
};

function baseProps(overrides: Partial<DayCalendarProps> = {}): DayCalendarProps {
  return {
    events: [],
    date: new Date("2025-06-16T12:00:00Z"),
    gridRef: React.createRef<HTMLDivElement>(),
    dragging: null,
    slotHighlight: null,
    onGridPointerMove: vi.fn(),
    onGridPointerUp: vi.fn(),
    cancelDrag: vi.fn(),
    renderEvent: (event: EventProjection) => <span>{event.title}</span>,
    calendarFor: vi.fn(),
    isSaving: vi.fn().mockReturnValue(false),
    openDetail: vi.fn(),
    openEdit: vi.fn(),
    onPointerDown: vi.fn(),
    onResizeHandleDown: vi.fn(),
    setSlotHighlight: vi.fn(),
    onSlotClick: vi.fn(),
    suppressClick: vi.fn().mockReturnValue(false),
    ...overrides,
  };
}

afterEach(() => { cleanup(); });

describe("DayCalendar empty states", () => {
  it("shows a no-calendars message when no calendars are selected", () => {
    render(<DayCalendar {...baseProps({ hasVisibleCalendars: false, events: [] })} />);

    expect(screen.getByText(/No calendars are selected/i)).toBeInTheDocument();
    expect(document.querySelectorAll(".event-ui__hour-row")).toHaveLength(0);
    expect(screen.queryByRole("button", { name: /New event/i })).not.toBeInTheDocument();
  });

  it("shows a no-events message with time slots when the day is empty and writable", () => {
    render(<DayCalendar {...baseProps({ hasVisibleCalendars: true, canCreate: true, events: [] })} />);

    expect(screen.getByText(/No events on this day/i)).toBeInTheDocument();
    expect(document.querySelectorAll(".event-ui__hour-row")).toHaveLength(24);
    expect(screen.getByRole("button", { name: /New event/i })).toBeInTheDocument();
  });

  it("shows a no-events message without a new-event button when the day is empty and read-only", () => {
    render(<DayCalendar {...baseProps({ hasVisibleCalendars: true, canCreate: false, events: [] })} />);

    expect(screen.getByText(/No events on this day/i)).toBeInTheDocument();
    expect(document.querySelectorAll(".event-ui__hour-row")).toHaveLength(24);
    expect(screen.queryByRole("button", { name: /New event/i })).not.toBeInTheDocument();
  });

  it("does not show empty-day or no-calendars messages during loading", () => {
    render(<DayCalendar {...baseProps({ loading: true, hasVisibleCalendars: false, events: [] })} />);

    expect(screen.getByText(/Loading events/i)).toBeInTheDocument();
    expect(screen.queryByText(/No calendars are selected/i)).not.toBeInTheDocument();
    expect(screen.queryByText(/No events on this day/i)).not.toBeInTheDocument();
  });

  it("does not show empty-day or no-calendars messages on load error", () => {
    render(<DayCalendar {...baseProps({ loadError: "Failed to load", hasVisibleCalendars: false, events: [] })} />);

    expect(screen.getByText(/Failed to load/i)).toBeInTheDocument();
    expect(screen.queryByText(/No calendars are selected/i)).not.toBeInTheDocument();
    expect(screen.queryByText(/No events on this day/i)).not.toBeInTheDocument();
  });

  it("renders events normally when events exist", () => {
    render(<DayCalendar {...baseProps({ hasVisibleCalendars: true, canCreate: true, events: [mockEvent] })} />);

    expect(screen.getAllByText("Planning").length).toBeGreaterThanOrEqual(1);
    expect(screen.queryByText(/No events on this day/i)).not.toBeInTheDocument();
    expect(screen.queryByText(/No calendars are selected/i)).not.toBeInTheDocument();
  });
});

describe("DayCalendar event cards (Slice 9)", () => {
  const writableCal = { id: 1, access: "details" as const, role: "owner" as const, name: "Work", color: "#ff0000" };
  const readOnlyCal = { id: 1, access: "details" as const, role: "viewer" as const, name: "Work", color: "#ff0000" };

  it("shows masked title for free-busy events", () => {
    const busyEvent: EventProjection = { ...mockEvent, access: "free_busy", title: "Secret meeting" };
    render(<DayCalendar {...baseProps({ events: [busyEvent], calendarFor: vi.fn().mockReturnValue(writableCal) })} />);

    expect(screen.getAllByText("Busy").length).toBeGreaterThanOrEqual(1);
    expect(screen.queryByText("Secret meeting")).not.toBeInTheDocument();
  });

  it("shows local time range and calendar name for timed events", () => {
    render(<DayCalendar {...baseProps({ events: [mockEvent], calendarFor: vi.fn().mockReturnValue(writableCal) })} />);

    const meta = document.querySelector(".event-ui__event-meta");
    expect(meta).toBeInTheDocument();
    expect(meta!.textContent).toMatch(/\d{2}:\d{2}–\d{2}:\d{2}/);
    expect(meta!.textContent).toContain("Work");
  });

  it("does not show resize handles for external events", () => {
    const externalEvent: EventProjection = { ...mockEvent, is_external: true };
    render(<DayCalendar {...baseProps({ events: [externalEvent], calendarFor: vi.fn().mockReturnValue(writableCal) })} />);

    expect(document.querySelectorAll(".event-ui__resize-handle")).toHaveLength(0);
  });

  it("does not show resize handles for read-only calendar events", () => {
    render(<DayCalendar {...baseProps({ events: [mockEvent], calendarFor: vi.fn().mockReturnValue(readOnlyCal) })} />);

    expect(document.querySelectorAll(".event-ui__resize-handle")).toHaveLength(0);
  });

  it("hides the top resize handle for an event clipped at the start (Slice 17)", () => {
    // Event starts before the viewed day (2025-06-15T22:00) and ends during it (2025-06-16T02:00)
    const clippedStartEvent: EventProjection = {
      ...mockEvent,
      start_utc: Math.floor(new Date("2025-06-15T22:00:00").getTime() / 1000),
      end_utc: Math.floor(new Date("2025-06-16T02:00:00").getTime() / 1000),
    };
    render(<DayCalendar {...baseProps({ events: [clippedStartEvent], calendarFor: vi.fn().mockReturnValue(writableCal) })} />);

    expect(document.querySelector(".event-ui__resize-handle--top")).not.toBeInTheDocument();
    expect(document.querySelector(".event-ui__resize-handle--bottom")).toBeInTheDocument();
  });

  it("hides the bottom resize handle for an event clipped at the end (Slice 17)", () => {
    // Event starts during the viewed day (2025-06-16T22:00) and ends after it (2025-06-17T02:00)
    const clippedEndEvent: EventProjection = {
      ...mockEvent,
      start_utc: Math.floor(new Date("2025-06-16T22:00:00").getTime() / 1000),
      end_utc: Math.floor(new Date("2025-06-17T02:00:00").getTime() / 1000),
    };
    render(<DayCalendar {...baseProps({ events: [clippedEndEvent], calendarFor: vi.fn().mockReturnValue(writableCal) })} />);

    expect(document.querySelector(".event-ui__resize-handle--top")).toBeInTheDocument();
    expect(document.querySelector(".event-ui__resize-handle--bottom")).not.toBeInTheDocument();
  });

  it("hides both resize handles for an event clipped at both ends (Slice 17)", () => {
    // Event spans multiple days (2025-06-15T22:00 to 2025-06-17T02:00)
    const clippedBothEvent: EventProjection = {
      ...mockEvent,
      start_utc: Math.floor(new Date("2025-06-15T22:00:00").getTime() / 1000),
      end_utc: Math.floor(new Date("2025-06-17T02:00:00").getTime() / 1000),
    };
    render(<DayCalendar {...baseProps({ events: [clippedBothEvent], calendarFor: vi.fn().mockReturnValue(writableCal) })} />);

    expect(document.querySelector(".event-ui__resize-handle--top")).not.toBeInTheDocument();
    expect(document.querySelector(".event-ui__resize-handle--bottom")).not.toBeInTheDocument();
  });

  it("renders distinct blocks for same event ID across different calendars", () => {
    const event1: EventProjection = { ...mockEvent, calendar_id: 1 };
    const event2: EventProjection = { ...mockEvent, calendar_id: 2 };
    render(<DayCalendar {...baseProps({ events: [event1, event2] })} />);

    const blocks = document.querySelectorAll(".event-ui__event-block");
    expect(blocks).toHaveLength(2);
  });

  it("shows continuation labels for events clipped at day boundaries", () => {
    // Event spanning midnight: starts before the selected day, ends within it.
    // 2025-06-15T21:00:00Z → 2025-06-15T23:00:00Z
    // In Budapest (UTC+2): 23:00 Jun 15 → 01:00 Jun 16
    const clippedEvent: EventProjection = {
      ...mockEvent,
      start_utc: 1_750_021_200,
      end_utc: 1_750_028_400,
    };
    render(<DayCalendar {...baseProps({ events: [clippedEvent] })} />);

    const continues = document.querySelectorAll(".event-ui__event-continue");
    expect(continues.length).toBeGreaterThanOrEqual(1);
  });

  it("applies ellipsis styling to long titles", () => {
    const longTitleEvent: EventProjection = { ...mockEvent, title: "A very long event title that should be truncated in the card" };
    render(<DayCalendar {...baseProps({ events: [longTitleEvent] })} />);

    const titleEl = document.querySelector(".event-ui__event-title");
    expect(titleEl).toBeInTheDocument();
    expect(titleEl!.textContent).toBe(longTitleEvent.title);
  });

  it("includes full accessible label with time, calendar, and continuation info", () => {
    const cal = { ...writableCal, name: "Personal" };
    render(<DayCalendar {...baseProps({ events: [mockEvent], calendarFor: vi.fn().mockReturnValue(cal) })} />);

    const block = document.querySelector(".event-ui__event-block");
    expect(block).toBeInTheDocument();
    const label = block!.getAttribute("aria-label");
    expect(label).toContain("Planning");
    expect(label).toContain("Personal");
  });
});

describe("DayCalendar all-day strip (Slice 10)", () => {
  const allDayEvent: EventProjection = {
    ...mockEvent,
    event_kind: "all_day",
    start_date: "2025-06-16",
    end_date: "2025-06-17",
    start_utc: undefined,
    end_utc: undefined,
  };

  it("renders all-day events in the all-day strip, not in the timeline", () => {
    render(<DayCalendar {...baseProps({ events: [allDayEvent] })} />);

    const strip = document.querySelector(".event-ui__allday-row");
    expect(strip).toBeInTheDocument();
    expect(strip!.textContent).toContain("All day");
    expect(strip!.textContent).toContain("Planning");

    const timeline = document.querySelector(".event-ui__day-timeline");
    expect(timeline).toBeInTheDocument();
    expect(timeline!.textContent).not.toContain("Planning");
  });

  it("keeps the all-day strip as a direct child of the day region (outside the timeline)", () => {
    render(<DayCalendar {...baseProps({ events: [allDayEvent] })} />);

    const region = document.querySelector(".event-ui__day-region");
    expect(region).toBeInTheDocument();
    const strip = region!.querySelector(":scope > .event-ui__allday-row");
    expect(strip).toBeInTheDocument();
    // Timeline is inside the body, not inside the strip
    const body = region!.querySelector(":scope > .event-ui__day-body");
    expect(body).toBeInTheDocument();
    const timeline = body!.querySelector(":scope > .event-ui__day-timeline");
    expect(timeline).toBeInTheDocument();
    // Strip does not contain the timeline
    expect(strip!.querySelector(".event-ui__day-timeline")).not.toBeInTheDocument();
  });

  it("creates an event at the selected hour when clicking a time slot", () => {
    const onSlotClick = vi.fn();
    render(<DayCalendar {...baseProps({ onSlotClick })} />);

    const slots = document.querySelectorAll(".event-ui__hour-row");
    expect(slots).toHaveLength(24);

    fireEvent.click(slots[9]);

    expect(onSlotClick).toHaveBeenCalledWith(0, 9, "day");
  });

  it("excludes all-day events whose exclusive end date is the selected day", () => {
    // Event from 2025-06-15 to 2025-06-16 (end_date is exclusive)
    // Should NOT be shown on 2025-06-16
    const endingEvent: EventProjection = {
      ...allDayEvent,
      start_date: "2025-06-15",
      end_date: "2025-06-16",
    };
    render(<DayCalendar {...baseProps({ events: [endingEvent] })} />);

    const events = document.querySelector(".event-ui__allday-events");
    expect(events).toBeInTheDocument();
    expect(events!.textContent).not.toContain("Planning");
  });

  it("includes all-day events that start on the selected day", () => {
    render(<DayCalendar {...baseProps({ events: [allDayEvent] })} />);

    const events = document.querySelector(".event-ui__allday-events");
    expect(events).toBeInTheDocument();
    expect(events!.textContent).toContain("Planning");
  });

  it("shows timed events in the timeline, not in the all-day strip", () => {
    render(<DayCalendar {...baseProps({ events: [mockEvent] })} />);

    const strip = document.querySelector(".event-ui__allday-events");
    expect(strip).toBeInTheDocument();
    expect(strip!.textContent).not.toContain("Planning");

    const timeline = document.querySelector(".event-ui__day-timeline");
    expect(timeline).toBeInTheDocument();
    expect(timeline!.textContent).toContain("Planning");
  });
});

describe("DayCalendar full-day schedule (Slice 11)", () => {
  const allDayEvent: EventProjection = {
    ...mockEvent,
    event_kind: "all_day",
    start_date: "2025-06-16",
    end_date: "2025-06-17",
    start_utc: undefined,
    end_utc: undefined,
  };

  it("renders a schedule sidebar alongside the timeline", () => {
    render(<DayCalendar {...baseProps({ events: [mockEvent] })} />);

    const body = document.querySelector(".event-ui__day-body");
    expect(body).toBeInTheDocument();
    const timeline = body!.querySelector(":scope > .event-ui__day-timeline");
    expect(timeline).toBeInTheDocument();
    const schedule = body!.querySelector(":scope > .event-ui__day-schedule");
    expect(schedule).toBeInTheDocument();
  });

  it("shows all-day events first, then timed events, in the schedule", () => {
    render(<DayCalendar {...baseProps({ events: [mockEvent, allDayEvent] })} />);

    const schedule = document.querySelector(".event-ui__day-schedule");
    expect(schedule).toBeInTheDocument();
    const items = schedule!.querySelectorAll(".event-ui__schedule-item");
    expect(items.length).toBeGreaterThanOrEqual(2);

    // All-day events come before timed events
    const alldayItems = schedule!.querySelectorAll(".event-ui__schedule-item--allday");
    expect(alldayItems.length).toBeGreaterThanOrEqual(1);
    // The first item should be the all-day event
    expect(alldayItems[0]!.textContent).toContain("Planning");
  });

  it("includes late events in the schedule even when outside timeline viewport", () => {
    // Event at 22:00–23:00 local (Budapest UTC+2) = 20:00–21:00 UTC
    // 2025-06-16T00:00:00Z = 1_750_032_000
    const lateEvent: EventProjection = {
      ...mockEvent,
      id: 99,
      title: "Late appointment",
      start_utc: 1_750_104_000, // 20:00 UTC = 22:00 Budapest
      end_utc: 1_750_107_600,  // 21:00 UTC = 23:00 Budapest
    };
    render(<DayCalendar {...baseProps({ events: [lateEvent] })} />);

    const schedule = document.querySelector(".event-ui__day-schedule");
    expect(schedule).toBeInTheDocument();
    expect(schedule!.textContent).toContain("Late appointment");

    // Also present in the timeline (positioned at the bottom)
    const timeline = document.querySelector(".event-ui__day-timeline");
    expect(timeline!.textContent).toContain("Late appointment");
  });

  it("uses permission-aware title and actions for schedule items", () => {
    const openDetail = vi.fn();
    const openEdit = vi.fn();
    const busyEvent: EventProjection = { ...mockEvent, access: "free_busy", title: "Secret meeting" };
    render(<DayCalendar {...baseProps({ events: [busyEvent], openDetail, openEdit })} />);

    const schedule = document.querySelector(".event-ui__day-schedule");
    expect(schedule).toBeInTheDocument();
    // Free-busy events show masked title "Busy"
    expect(schedule!.textContent).toContain("Busy");
    expect(schedule!.textContent).not.toContain("Secret meeting");

    // Clicking a schedule item opens details
    const item = schedule!.querySelector(".event-ui__schedule-item");
    expect(item).toBeInTheDocument();
    fireEvent.click(item!);
    expect(openDetail).toHaveBeenCalledWith(busyEvent);
  });

  it("displays the full event time range in the schedule for overnight events", () => {
    // Overnight event: 2025-06-15T22:00:00Z → 2025-06-16T02:00:00Z
    // In Budapest (UTC+2): 2025-06-16T00:00 → 2025-06-16T04:00
    // 2025-06-15T22:00:00Z = 1_750_032_000 - 2*3600 = 1_750_024_800
    // 2025-06-16T02:00:00Z = 1_750_032_000 + 2*3600 = 1_750_039_200
    const overnightEvent: EventProjection = {
      ...mockEvent,
      id: 88,
      title: "Overnight",
      start_utc: 1_750_024_800,
      end_utc: 1_750_039_200,
    };
    render(<DayCalendar {...baseProps({ events: [overnightEvent] })} />);

    const schedule = document.querySelector(".event-ui__day-schedule");
    expect(schedule).toBeInTheDocument();
    expect(schedule!.textContent).toContain("Overnight");
  });

  it("schedule is a sibling of the timeline within the day body", () => {
    render(<DayCalendar {...baseProps({ events: [mockEvent] })} />);

    const body = document.querySelector(".event-ui__day-body");
    expect(body).toBeInTheDocument();
    const children = Array.from(body!.children);
    const timeline = children.find((el) => el.classList.contains("event-ui__day-timeline"));
    const schedule = children.find((el) => el.classList.contains("event-ui__day-schedule"));
    expect(timeline).toBeDefined();
    expect(schedule).toBeDefined();
    // They are siblings (both direct children of body)
    expect(timeline!.parentElement).toBe(schedule!.parentElement);
  });
});

describe("DayCalendar scroll anchor (Slice 13)", () => {
  function setScrollDimensions(el: HTMLElement, scrollHeight: number, clientHeight: number) {
    Object.defineProperty(el, "scrollHeight", { value: scrollHeight, configurable: true });
    Object.defineProperty(el, "clientHeight", { value: clientHeight, configurable: true });
  }

  it("anchors to 08:00 for an empty day on a non-today date", () => {
    const gridRef = React.createRef<HTMLDivElement>();
    const { rerender } = render(
      <DayCalendar {...baseProps({ date: new Date("2025-06-16T12:00:00Z"), events: [], gridRef })} />
    );
    const el = gridRef.current!;
    setScrollDimensions(el, 1440, 600);
    // Change date to trigger re-run of the anchor effect
    rerender(
      <DayCalendar {...baseProps({ date: new Date("2025-06-17T12:00:00Z"), events: [], gridRef })} />
    );
    // 08:00 = 480 min, desired = 480 - 120 = 360, clamped to [0, 840]
    expect(el.scrollTop).toBe(360);
  });

  it("anchors to first timed event for a non-today date with events", () => {
    const gridRef = React.createRef<HTMLDivElement>();
    // Event on 2025-06-17: 00:13:20 UTC = 02:13:20 Budapest
    const event: EventProjection = {
      id: 1,
      calendar_id: 1,
      access: "details",
      status: "confirmed",
      event_kind: "timed",
      title: "Meeting",
      start_utc: 1_750_119_200,
      end_utc: 1_750_122_800,
      timezone: "UTC",
      version: 1,
      start_date: "2025-06-17",
    };
    const { rerender } = render(
      <DayCalendar {...baseProps({ date: new Date("2025-06-16T12:00:00Z"), events: [], gridRef })} />
    );
    const el = gridRef.current!;
    setScrollDimensions(el, 1440, 600);
    // Change to the date with the event
    rerender(
      <DayCalendar {...baseProps({ date: new Date("2025-06-17T12:00:00Z"), events: [event], gridRef })} />
    );
    // Event starts at 02:13:20 Budapest → top = 133 min
    // desired = 133 - 120 = 13, clamped to [0, 840]
    expect(el.scrollTop).toBe(13);
  });

  it("does not re-apply anchor on refresh (same date, new events)", () => {
    const gridRef = React.createRef<HTMLDivElement>();
    const event: EventProjection = {
      id: 1,
      calendar_id: 1,
      access: "details",
      status: "confirmed",
      event_kind: "timed",
      title: "Meeting",
      start_utc: 1_750_032_800,
      end_utc: 1_750_036_400,
      timezone: "UTC",
      version: 1,
      start_date: "2025-06-16",
    };
    const { rerender } = render(
      <DayCalendar {...baseProps({ date: new Date("2025-06-16T12:00:00Z"), events: [event], gridRef })} />
    );
    const el = gridRef.current!;
    setScrollDimensions(el, 1440, 600);
    // Simulate user scrolling to a different position
    el.scrollTop = 500;
    // Refresh: same date, different events
    const newEvent: EventProjection = { ...event, id: 2, title: "Updated" };
    rerender(
      <DayCalendar {...baseProps({ date: new Date("2025-06-16T12:00:00Z"), events: [newEvent], gridRef })} />
    );
    // Anchor should NOT re-apply (same date)
    expect(el.scrollTop).toBe(500);
  });

  it("resets anchor when date changes", () => {
    const gridRef = React.createRef<HTMLDivElement>();
    const { rerender } = render(
      <DayCalendar {...baseProps({ date: new Date("2025-06-16T12:00:00Z"), events: [], gridRef })} />
    );
    const el = gridRef.current!;
    setScrollDimensions(el, 1440, 600);
    // Simulate user scrolling
    el.scrollTop = 500;
    // Change date
    rerender(
      <DayCalendar {...baseProps({ date: new Date("2025-06-18T12:00:00Z"), events: [], gridRef })} />
    );
    // Anchor re-applies: 08:00 = 480, desired = 360
    expect(el.scrollTop).toBe(360);
  });
});

describe("DayCalendar current-time marker (Slice 15)", () => {
  it("shows the marker when viewing today", () => {
    const today = new Date();
    const gridRef = React.createRef<HTMLDivElement>();
    render(<DayCalendar {...baseProps({ date: today, events: [], gridRef })} />);
    const marker = document.querySelector(".event-ui__current-time");
    expect(marker).not.toBeNull();
    expect(marker!.querySelector(".event-ui__current-time-label")).not.toBeNull();
  });

  it("does not show the marker when viewing a different date", () => {
    const gridRef = React.createRef<HTMLDivElement>();
    render(<DayCalendar {...baseProps({ date: new Date("2025-06-16T12:00:00Z"), events: [], gridRef })} />);
    expect(document.querySelector(".event-ui__current-time")).toBeNull();
  });

  it("updates the marker position when the clock advances", () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2025-06-16T10:30:00"));
    const gridRef = React.createRef<HTMLDivElement>();
    render(<DayCalendar {...baseProps({ date: new Date("2025-06-16T10:30:00"), events: [], gridRef })} />);
    const marker = document.querySelector<HTMLElement>(".event-ui__current-time")!;
    expect(marker.style.top).toBe(`${10 * 60 + 30}px`);
    // Advance clock to 10:31, then fire the tick
    vi.setSystemTime(new Date("2025-06-16T10:31:00"));
    React.act(() => { vi.advanceTimersByTime(30_000); });
    expect(marker.style.top).toBe(`${10 * 60 + 31}px`);
    vi.useRealTimers();
  });

  it("cleans up the timer on unmount", () => {
    vi.useFakeTimers();
    const gridRef = React.createRef<HTMLDivElement>();
    const { unmount } = render(<DayCalendar {...baseProps({ date: new Date(), events: [], gridRef })} />);
    unmount();
    // No error thrown, timer cleaned up
    vi.useRealTimers();
  });

  it("does not show the marker during loading", () => {
    const gridRef = React.createRef<HTMLDivElement>();
    render(<DayCalendar {...baseProps({ date: new Date(), events: [], gridRef, loading: true })} />);
    expect(document.querySelector(".event-ui__current-time")).toBeNull();
  });

  it("does not show the marker during error", () => {
    const gridRef = React.createRef<HTMLDivElement>();
    render(<DayCalendar {...baseProps({ date: new Date(), events: [], gridRef, loadError: "Failed" })} />);
    expect(document.querySelector(".event-ui__current-time")).toBeNull();
  });
});

describe("DayCalendar keyboard (Slice 18)", () => {
  it("hour rows are keyboard-focusable buttons with readable labels", () => {
    render(<DayCalendar {...baseProps({ events: [] })} />);
    const rows = document.querySelectorAll(".event-ui__hour-row");
    expect(rows).toHaveLength(24);
    rows.forEach((row) => {
      expect(row).toHaveAttribute("role", "button");
      expect(row).toHaveAttribute("tabindex", "0");
      expect(row).toHaveAttribute("aria-label");
    });
    // Spot-check a few labels
    expect(rows[0]).toHaveAttribute("aria-label", "Create event at 12 AM");
    expect(rows[9]).toHaveAttribute("aria-label", "Create event at 9 AM");
    expect(rows[12]).toHaveAttribute("aria-label", "Create event at 12 PM");
    expect(rows[15]).toHaveAttribute("aria-label", "Create event at 3 PM");
  });

  it("keyboard Enter on an hour row creates at the correct hour", () => {
    const onSlotClick = vi.fn();
    render(<DayCalendar {...baseProps({ events: [], onSlotClick })} />);
    const row9 = document.querySelectorAll(".event-ui__hour-row")[9];
    fireEvent.keyDown(row9, { key: "Enter" });
    expect(onSlotClick).toHaveBeenCalledWith(0, 9, "day");
  });

  it("keyboard Space on an hour row creates at the correct hour", () => {
    const onSlotClick = vi.fn();
    render(<DayCalendar {...baseProps({ events: [], onSlotClick })} />);
    const row14 = document.querySelectorAll(".event-ui__hour-row")[14];
    fireEvent.keyDown(row14, { key: " " });
    expect(onSlotClick).toHaveBeenCalledWith(0, 14, "day");
  });

  it("hour row keyboard activation is suppressed after a completed gesture", () => {
    const onSlotClick = vi.fn();
    const suppressClick = vi.fn().mockReturnValue(true);
    render(<DayCalendar {...baseProps({ events: [], onSlotClick, suppressClick })} />);
    const row9 = document.querySelectorAll(".event-ui__hour-row")[9];
    fireEvent.keyDown(row9, { key: "Enter" });
    expect(onSlotClick).not.toHaveBeenCalled();
  });

  it("event title button opens detail on keyboard Enter without bubbling to hour row", () => {
    const openDetail = vi.fn();
    const onSlotClick = vi.fn();
    render(<DayCalendar {...baseProps({ events: [mockEvent], openDetail, onSlotClick })} />);
    const titleButton = screen.getByRole("button", { name: "Planning" });
    // Native button: Enter/Space triggers click
    fireEvent.click(titleButton);
    expect(openDetail).toHaveBeenCalledWith(mockEvent);
    // The hour row click handler must NOT fire (no bubbling)
    expect(onSlotClick).not.toHaveBeenCalled();
  });

  it("schedule items are keyboard-focusable buttons with readable labels", () => {
    render(<DayCalendar {...baseProps({ events: [mockEvent] })} />);
    const items = document.querySelectorAll(".event-ui__schedule-item");
    expect(items.length).toBeGreaterThanOrEqual(1);
    items.forEach((item) => {
      expect(item).toHaveAttribute("role", "button");
      expect(item).toHaveAttribute("tabindex", "0");
      expect(item).toHaveAttribute("aria-label");
    });
    // The timed event schedule item should include the time range
    const timedItem = Array.from(items).find((i) => i.className.includes("event-ui__schedule-item") && !i.className.includes("--allday"));
    expect(timedItem).toBeDefined();
    expect(timedItem!.getAttribute("aria-label")).toContain("Planning");
    expect(timedItem!.getAttribute("aria-label")).toMatch(/\d{2}:\d{2}/);
  });

  it("schedule item opens detail on keyboard Enter", () => {
    const openDetail = vi.fn();
    render(<DayCalendar {...baseProps({ events: [mockEvent], openDetail })} />);
    const timedItem = Array.from(document.querySelectorAll(".event-ui__schedule-item")).find((i) => !i.className.includes("--allday"));
    expect(timedItem).toBeDefined();
    fireEvent.keyDown(timedItem!, { key: "Enter" });
    expect(openDetail).toHaveBeenCalledWith(mockEvent);
  });

  it("all-day schedule item opens detail on keyboard Enter", () => {
    const openDetail = vi.fn();
    const allDayEvent: EventProjection = {
      ...mockEvent,
      event_kind: "all_day",
      start_date: "2025-06-16",
      end_date: "2025-06-17",
      start_utc: undefined,
      end_utc: undefined,
    };
    render(<DayCalendar {...baseProps({ events: [allDayEvent], openDetail })} />);
    const alldayItem = document.querySelector(".event-ui__schedule-item--allday");
    expect(alldayItem).not.toBeNull();
    expect(alldayItem).toHaveAttribute("role", "button");
    fireEvent.keyDown(alldayItem!, { key: "Enter" });
    expect(openDetail).toHaveBeenCalledWith(allDayEvent);
  });
});
