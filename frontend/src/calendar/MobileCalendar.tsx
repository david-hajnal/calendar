import type { Calendar } from "./CalendarManagement";
import type { EventProjection } from "./api";
import type { CalendarSnapshot } from "./calendarNavigation";
import { eventsOnDay, dateKey, eventExternal, eventEditable, eventTitle, type CalendarView } from "./calendarModel";
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
  onSelectDay(day: string): void;
  onOpenEvent(event: EventProjection, control: HTMLButtonElement): void;
  onCreate(): void;
  onRetry(): void;
}

export function MobileCalendar(props: MobileCalendarProps) {
  const month = props.snapshot.view === "month";
  const anchor = new Date(`${props.snapshot.anchorDate}T00:00:00`);
  const groups = new Map<string, EventProjection[]>();
  if (month) groups.set(props.snapshot.selectedDay, eventsOnDay(props.events, props.snapshot.selectedDay));
  else for (let offset = 0; offset < 31; offset++) {
    const date = new Date(anchor); date.setDate(date.getDate() + offset);
    const day = dateKey(date);
    const events = eventsOnDay(props.events, day);
    if (events.length) groups.set(day, events);
  }
  const monthStart = new Date(anchor.getFullYear(), anchor.getMonth(), 1);
  const monthDays = Array.from({ length: 42 }, (_, index) => {
    const date = new Date(monthStart); date.setDate(index - monthStart.getDay() + 1);
    return { date, day: dateKey(date) };
  });
  const dateFormat = new Intl.DateTimeFormat(undefined, { weekday: "long", day: "numeric", month: "long", year: "numeric" });
  const timeFormat = new Intl.DateTimeFormat(undefined, { hour: "2-digit", minute: "2-digit" });
  return <section className="mobile-calendar" aria-label={month ? "Month calendar" : "Agenda"}>
    <h2 id="events-heading" tabIndex={-1}>Events</h2>
    <div className="mobile-calendar__views" role="tablist" aria-label="Calendar view">
      {(["agenda", "month", "day", "week"] as const).map(view => <button key={view} type="button" role="tab" aria-selected={props.snapshot.view === view} onClick={() => props.onViewChange(view)}>{view[0].toUpperCase() + view.slice(1)}</button>)}
    </div>
    <p aria-live="polite">{month ? new Intl.DateTimeFormat(undefined, { month: "long", year: "numeric" }).format(anchor) : dateFormat.format(anchor)}</p>
    <div className="mobile-calendar__date-nav">
      <button type="button" aria-label={month ? "Previous month" : "Earlier"} onClick={() => props.onNavigate(-1)}>{month ? "Previous" : "Earlier"}</button>
      <button type="button" onClick={props.onToday}>Today</button>
      <button type="button" aria-label={month ? "Next month" : "Later"} onClick={() => props.onNavigate(1)}>{month ? "Next" : "Later"}</button>
    </div>
    {month && <div className="mobile-calendar__month" role="group" aria-label="Month dates" aria-busy={props.loading}>
      {["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"].map(day => <abbr key={day} title={day}>{day.slice(0, 1)}</abbr>)}
      {monthDays.map(({ date, day }) => {
        const count = eventsOnDay(props.events, day).length;
        return <button type="button" key={day} aria-pressed={day === props.snapshot.selectedDay} aria-label={`${dateFormat.format(date)} · ${props.loading ? "Loading events" : `${count} events`}`} className={date.getMonth() === anchor.getMonth() ? "" : "mobile-calendar__other-month"} onClick={() => props.onSelectDay(day)}>
          <span aria-hidden="true">{date.getDate()}</span><span className="mobile-calendar__count" aria-hidden="true">{count > 0 && !props.loading ? count : "·"}</span>
        </button>;
      })}
    </div>}
    <button type="button" disabled={!props.canCreate} onClick={props.onCreate}>New event</button>
    {props.error && <p role="alert">{props.error} <button type="button" onClick={props.onRetry}>Retry events</button></p>}
    {props.loading ? <p role="status">Loading events…</p> : props.error ? null : groups.size === 0 ? <p>No events in this range.</p> : [...groups].sort(([a], [b]) => a.localeCompare(b)).map(([day, events]) => <div key={day}>
      <h3>{dateFormat.format(new Date(`${day}T00:00:00`))}</h3>
      {events.length === 0 && <p>No events on this day.</p>}
      <ul className="mobile-calendar__events">{events.map(event => {
        const calendar = props.calendars.find(c => c.id === event.calendar_id);
        const details = event.access === "details";
        const name = eventTitle(event);
        const external = eventExternal(event);
        const readonly = !eventEditable(event, calendar);
        return <li key={`${event.calendar_id}:${event.id}:${event.recurrence_id ?? event.recurrence_date ?? "base"}`}>
          <button className="mobile-calendar__event" type="button" onClick={click => props.onOpenEvent(event, click.currentTarget)} style={{ borderLeftColor: calendar?.color ?? "var(--color-primary)" }}>
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
