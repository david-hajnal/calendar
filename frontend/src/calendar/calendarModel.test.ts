import { expect, it } from "vitest";
import { eventsOnDay, shiftDate } from "./calendarModel";
import type { EventProjection } from "./api";

it("moves actual months with leap/end-of-month clamping and correct view strides", () => {
  expect(shiftDate("2025-01-31", "month", 1)).toBe("2025-02-28");
  expect(shiftDate("2024-01-31", "month", 1)).toBe("2024-02-29");
  expect(shiftDate("2026-12-31", "month", 1)).toBe("2027-01-31");
  expect(shiftDate("2026-03-31", "month", -1)).toBe("2026-02-28");
  expect(shiftDate("2026-10-09", "week", 1)).toBe("2026-10-16");
  expect(shiftDate("2026-10-09", "day", -1)).toBe("2026-10-08");
  expect(shiftDate("2026-10-09", "agenda", 1)).toBe("2026-11-09");
});

it("uses local day overlap and exclusive ends for all-day and overnight timed events", () => {
  const timed: EventProjection = { id: 1, calendar_id: 1, access: "details", status: "confirmed", event_kind: "timed", start_utc: new Date(2026, 9, 9, 23, 30).getTime() / 1000, end_utc: new Date(2026, 9, 10, 0, 30).getTime() / 1000 };
  const allDay: EventProjection = { id: 2, calendar_id: 1, access: "details", status: "confirmed", event_kind: "all_day", start_date: "2026-10-09", end_date: "2026-10-11" };
  expect(eventsOnDay([timed, allDay], "2026-10-09").map(e => e.id)).toEqual([1, 2]);
  expect(eventsOnDay([timed, allDay], "2026-10-10").map(e => e.id)).toEqual([1, 2]);
  expect(eventsOnDay([timed, allDay], "2026-10-11")).toEqual([]);
  const midnight = { ...timed, end_utc: new Date(2026, 9, 10).getTime() / 1000 };
  expect(eventsOnDay([midnight], "2026-10-10")).toEqual([]);
});

it("keeps local DST days bounded by the next calendar midnight", () => {
  for (const [month, day] of [[2, 8], [2, 29], [10, 1]]) {
    const nextDay: EventProjection = { id: 1, calendar_id: 1, access: "details", status: "confirmed", event_kind: "timed", start_utc: new Date(2026, month, day + 1, 0, 15).getTime() / 1000, end_utc: new Date(2026, month, day + 1, 0, 45).getTime() / 1000 };
    const dayKey = `2026-${String(month + 1).padStart(2, "0")}-${String(day).padStart(2, "0")}`;
    expect(eventsOnDay([nextDay], dayKey)).toEqual([]);
  }
});
