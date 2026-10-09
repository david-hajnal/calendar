import type { Calendar } from "./CalendarManagement";
import type { EventProjection } from "./api";
import type { CalendarSnapshot } from "./calendarNavigation";
import { dateKey, eventExternal, eventEditable, eventTitle, type CalendarView } from "./calendarModel";
import "./MobileCalendar.css";

interface MobileCalendarProps {
  snapshot: CalendarSnapshot;
  events: readonly EventProjection[];
  calendars: readonly Calendar[];
  loading: boolean;
  error: string | null;
  canCreate: boolean;
  onViewChange(view: CalendarView): void;
  onNavigate(direction: -1 | 1): void;
  onToday(): void;
  onOpenEvent(event: EventProjection): void;
  onCreate(): void;
  onRetry(): void;
}

export function MobileCalendar(props: MobileCalendarProps) {
  const groups = new Map<string, EventProjection[]>();
  for (const event of props.events) {
    const day = event.event_kind === "all_day" ? event.start_date : event.start_utc == null ? undefined : dateKey(new Date(event.start_utc * 1000));
    if (!day) continue;
    const group = groups.get(day) ?? []; group.push(event); groups.set(day, group);
  }
  const dateFormat = new Intl.DateTimeFormat(undefined, { weekday: "long", day: "numeric", month: "long", year: "numeric" });
  const timeFormat = new Intl.DateTimeFormat(undefined, { hour: "2-digit", minute: "2-digit" });
  return <section className="mobile-calendar" aria-label="Agenda">
    <h2 id="events-heading">Events</h2>
    <div className="mobile-calendar__views" role="tablist" aria-label="Calendar view">
      {(["agenda", "month", "day", "week"] as const).map(view => <button key={view} type="button" role="tab" aria-selected={props.snapshot.view === view} onClick={() => props.onViewChange(view)}>{view[0].toUpperCase() + view.slice(1)}</button>)}
    </div>
    <p>{dateFormat.format(new Date(`${props.snapshot.anchorDate}T00:00:00`))}</p>
    <div className="mobile-calendar__date-nav">
      <button type="button" onClick={() => props.onNavigate(-1)}>Earlier</button>
      <button type="button" onClick={props.onToday}>Today</button>
      <button type="button" onClick={() => props.onNavigate(1)}>Later</button>
    </div>
    <button type="button" disabled={!props.canCreate} onClick={props.onCreate}>New event</button>
    {props.error && <p role="alert">{props.error} <button type="button" onClick={props.onRetry}>Retry events</button></p>}
    {props.loading ? <p role="status">Loading events…</p> : props.error ? null : groups.size === 0 ? <p>No events in this range.</p> : [...groups].sort(([a], [b]) => a.localeCompare(b)).map(([day, events]) => <div key={day}>
      <h3>{dateFormat.format(new Date(`${day}T00:00:00`))}</h3>
      <ul className="mobile-calendar__events">{events.map(event => {
        const calendar = props.calendars.find(c => c.id === event.calendar_id);
        const details = event.access === "details";
        const name = eventTitle(event);
        const external = eventExternal(event);
        const readonly = !eventEditable(event, calendar);
        return <li key={`${event.calendar_id}:${event.id}:${event.recurrence_id ?? event.recurrence_date ?? "base"}`}>
          <button className="mobile-calendar__event" type="button" onClick={() => props.onOpenEvent(event)} style={{ borderLeftColor: calendar?.color ?? "var(--color-primary)" }}>
            <strong>{name}</strong>
            <span>{event.event_kind === "all_day" ? "All day" : `${event.start_utc == null ? "" : timeFormat.format(new Date(event.start_utc * 1000))}–${event.end_utc == null ? "" : timeFormat.format(new Date(event.end_utc * 1000))}`}</span>
            <span>{calendar?.access === "details" ? calendar.name : "Busy calendar"}{details && event.location ? ` · ${event.location}` : ""}</span>
            {readonly && <span>{external ? "Read-only external event" : "Read-only"}</span>}
          </button>
        </li>;
      })}</ul>
    </div>)}
  </section>;
}
