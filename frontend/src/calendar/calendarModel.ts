import type { Calendar } from "./CalendarManagement";
import type { EventProjection } from "./api";

export type CalendarView = "month" | "week" | "day" | "agenda";
export type DateKey = string;

export function dateKey(value: Date): DateKey {
  return `${value.getFullYear()}-${String(value.getMonth() + 1).padStart(2, "0")}-${String(value.getDate()).padStart(2, "0")}`;
}

export function validDateKey(value: unknown): value is DateKey {
  if (typeof value !== "string" || !/^\d{4}-\d{2}-\d{2}$/.test(value)) return false;
  const date = new Date(`${value}T00:00:00`);
  return Number.isFinite(date.getTime()) && dateKey(date) === value;
}

export function validView(value: unknown): value is CalendarView {
  return value === "month" || value === "week" || value === "day" || value === "agenda";
}

export function calendarWritable(calendar: Calendar | undefined) {
  return calendar?.access === "details" && ["owner", "manager", "editor"].includes(calendar.role);
}
export function eventExternal(event: EventProjection) { return event.is_external === true || event.read_only === true; }
export function eventEditable(event: EventProjection, calendar: Calendar | undefined) {
  return calendarWritable(calendar) && event.access === "details" && !eventExternal(event) && event.version !== undefined;
}
export function eventTitle(event: EventProjection) { return event.access === "details" ? event.title ?? "Busy" : "Busy"; }
