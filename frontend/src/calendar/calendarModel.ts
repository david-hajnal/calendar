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

export function shiftDate(date: DateKey, view: CalendarView, direction: -1 | 1): DateKey {
  const next = new Date(`${date}T00:00:00`);
  if (view === "month") {
    const day = next.getDate();
    next.setDate(1); next.setMonth(next.getMonth() + direction);
    const lastDay = new Date(next.getFullYear(), next.getMonth() + 1, 0).getDate();
    next.setDate(Math.min(day, lastDay));
  } else next.setDate(next.getDate() + direction * (view === "week" ? 7 : view === "agenda" ? 31 : 1));
  return dateKey(next);
}

export function eventsOnDay(events: readonly EventProjection[], day: DateKey): EventProjection[] {
  const from = new Date(`${day}T00:00:00`);
  const to = new Date(from); to.setDate(to.getDate() + 1);
  return events.filter(event => event.event_kind === "all_day"
    ? event.start_date != null && event.end_date != null && event.start_date <= day && day < event.end_date
    : event.start_utc != null && event.end_utc != null && event.start_utc < to.getTime() / 1000 && event.end_utc > from.getTime() / 1000);
}


export interface EventIdentity { calendarId: number; eventId: number; recurrenceId?: string | number }
export function identityOf(event: EventProjection): EventIdentity {
  return { calendarId: event.calendar_id, eventId: event.id, ...(event.recurrence_id != null || event.recurrence_date != null ? { recurrenceId: event.recurrence_id ?? event.recurrence_date } : {}) };
}
export function sameIdentity(event: EventProjection, identity: EventIdentity) {
  return event.calendar_id === identity.calendarId && event.id === identity.eventId && (event.recurrence_id ?? event.recurrence_date) === identity.recurrenceId;
}
