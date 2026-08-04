import { useCallback, useEffect, useMemo, useState, type FormEvent } from "react";

import type { ApiClient } from "../auth/api";
import { CalendarApiError, createEvent, listExpandedEvents, updateEvent, type EventPayload, type EventProjection } from "./api";
import type { Calendar } from "./CalendarManagement";
import "./CalendarEventUI.css";

type CalendarView = "month" | "week" | "day" | "agenda";
type Draft = { title: string; start: string; end: string; calendarId: number; recurrenceRule: string };

const viewLabels: Record<CalendarView, string> = { month: "Month view", week: "Week view", day: "Day view", agenda: "Agenda view" };

function writable(calendar: Calendar | undefined) {
  return calendar?.access === "details" && ["owner", "manager", "editor"].includes(calendar.role);
}

function external(event: EventProjection) { return event.is_external === true || event.read_only === true; }
function editable(event: EventProjection, calendar: Calendar | undefined) { return writable(calendar) && event.access === "details" && !external(event) && event.version !== undefined; }
function title(event: EventProjection) { return event.title ?? "Busy"; }
function eventTime(event: EventProjection) { return event.start_utc ?? Date.parse(`${event.start_date}T00:00:00Z`) / 1000; }
function inputTime(seconds: number) { return new Date(seconds * 1000).toISOString().slice(0, 16); }
function startOfDay(value: Date) { const copy = new Date(value); copy.setHours(0, 0, 0, 0); return copy; }
function addDays(value: Date, days: number) { const copy = new Date(value); copy.setDate(copy.getDate() + days); return copy; }
function rangeFor(view: CalendarView, date: Date) {
  const start = startOfDay(date);
  if (view === "month") { start.setDate(1); start.setDate(start.getDate() - start.getDay()); return { from: start, to: addDays(start, 42) }; }
  if (view === "week") return { from: addDays(start, -start.getDay()), to: addDays(start, 7 - start.getDay()) };
  if (view === "day") return { from: start, to: addDays(start, 1) };
  return { from: start, to: addDays(start, 31) };
}
function formatRange(view: CalendarView, date: Date) { return new Intl.DateTimeFormat(undefined, { month: "long", year: "numeric", ...(view === "day" ? { day: "numeric" } : {}) }).format(date); }

function payload(draft: Draft): EventPayload {
  return { title: draft.title, description: null, location: null, status: "confirmed", start_utc: Math.floor(new Date(draft.start).getTime() / 1000), end_utc: Math.floor(new Date(draft.end).getTime() / 1000), timezone: Intl.DateTimeFormat().resolvedOptions().timeZone || "UTC", ...(draft.recurrenceRule ? { recurrence_rule: draft.recurrenceRule } : {}) };
}

export function CalendarEventUI({ api, calendars, initialDate = new Date() }: { api: ApiClient; calendars: Calendar[]; initialDate?: Date }) {
  const [view, setView] = useState<CalendarView>("month");
  const [date, setDate] = useState(() => startOfDay(initialDate));
  const [visible, setVisible] = useState(() => new Set(calendars.map((calendar) => calendar.id)));
  const [events, setEvents] = useState<EventProjection[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [selected, setSelected] = useState<EventProjection | null>(null);
  const [editing, setEditing] = useState<EventProjection | "new" | null>(null);
  const firstWritable = calendars.find(writable)?.id ?? calendars[0]?.id ?? 0;
  const [draft, setDraft] = useState<Draft>(() => ({ title: "", start: inputTime(Math.floor(initialDate.getTime() / 1000)), end: inputTime(Math.floor(initialDate.getTime() / 1000) + 3600), calendarId: firstWritable, recurrenceRule: "" }));
  const range = useMemo(() => rangeFor(view, date), [view, date]);
  const visibleCalendarIds = useMemo(() => calendars.filter((calendar) => visible.has(calendar.id)).map((calendar) => calendar.id), [calendars, visible]);
  const visibleCalendarKey = visibleCalendarIds.join(",");

  const reload = useCallback(async () => {
    setLoading(true); setError(null);
    try {
      const result = await listExpandedEvents(api, visibleCalendarIds, { from: Math.floor(range.from.getTime() / 1000), to: Math.floor(range.to.getTime() / 1000) });
      setEvents([...new Map(result.map((item) => [`${item.calendar_id}:${item.id}:${item.recurrence_id ?? item.recurrence_date ?? "base"}`, item])).values()]);
    }
    catch { setError("We could not load events. Please try again."); }
    finally { setLoading(false); }
  }, [api, range.from, range.to, visibleCalendarIds, visibleCalendarKey]);

  useEffect(() => { void reload(); }, [reload]);
  const displayed = events.filter((event) => visible.has(event.calendar_id)).sort((a, b) => eventTime(a) - eventTime(b));
  const calendarFor = (event: EventProjection) => calendars.find((calendar) => calendar.id === event.calendar_id);

  function openNew() {
    const start = Math.floor(date.getTime() / 1000) + 9 * 3600;
    setDraft({ title: "", start: inputTime(start), end: inputTime(start + 3600), calendarId: calendars.find(writable)?.id ?? 0, recurrenceRule: "" });
    setEditing("new"); setSelected(null); setError(null);
  }
  function openEdit(event: EventProjection) {
    if (!editable(event, calendarFor(event)) || event.start_utc === undefined || event.end_utc === undefined) return;
    setDraft({ title: title(event), start: inputTime(event.start_utc), end: inputTime(event.end_utc), calendarId: event.calendar_id, recurrenceRule: event.recurrence_rule ?? "" });
    setEditing(event); setError(null);
  }
  async function save(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    try {
      const saved = editing === "new"
        ? await createEvent(api, draft.calendarId, payload(draft))
        : await updateEvent(api, editing!.calendar_id, editing!.id, { ...payload(draft), calendar_id: draft.calendarId, version: editing!.version! });
      setEvents((current) => editing === "new" ? [...current, saved] : current.map((item) => item.id === saved.id ? saved : item));
      setEditing(null); setSelected(saved);
    } catch (reason) {
      setError(reason instanceof CalendarApiError && reason.status === 409 ? "This event changed elsewhere. Reload it before saving again." : "We could not save this event.");
    }
  }
  async function move(event: EventProjection, direction: number) {
    if (!editable(event, calendarFor(event)) || event.start_utc === undefined || event.end_utc === undefined) return;
    const changed = { ...event, start_utc: event.start_utc + direction * 3600, end_utc: event.end_utc + direction * 3600 };
    try { const saved = await updateEvent(api, event.calendar_id, event.id, { ...payload({ title: title(changed), start: inputTime(changed.start_utc), end: inputTime(changed.end_utc), calendarId: event.calendar_id, recurrenceRule: event.recurrence_rule ?? "" }), calendar_id: event.calendar_id, version: event.version! }); setEvents((current) => current.map((item) => item.id === saved.id ? saved : item)); }
    catch (reason) { setError(reason instanceof CalendarApiError && reason.status === 409 ? "This event changed elsewhere. Reload it before saving again." : "We could not move this event."); }
  }
  function renderEvent(event: EventProjection) {
    const readonly = !editable(event, calendarFor(event));
    const name = external(event) ? `${title(event)} (read-only external event)` : title(event);
    return <li key={`${event.id}-${event.recurrence_id ?? event.recurrence_date ?? ""}`} className="event-ui__event">
      <button type="button" aria-label={name} draggable={!readonly} onDragStart={(drag) => { if (!readonly) drag.dataTransfer.setData("text/plain", String(event.id)); }} onClick={() => setSelected(event)}>{title(event)}</button>
      {!readonly && <button type="button" aria-label={`Move ${title(event)} later`} onClick={() => void move(event, 1)}>Move later</button>}
    </li>;
  }
  return <section className="event-ui" aria-labelledby="events-heading">
    <header className="event-ui__header"><h2 id="events-heading">Events</h2><button type="button" onClick={openNew} disabled={!calendars.some(writable)}>New event</button></header>
    <div className="event-ui__controls" aria-label="Calendar controls">
      {(Object.keys(viewLabels) as CalendarView[]).map((item) => <button type="button" key={item} aria-pressed={view === item} onClick={() => setView(item)}>{viewLabels[item]}</button>)}
      <button type="button" onClick={() => setDate((current) => addDays(current, view === "month" ? -30 : -1))}>Previous</button><strong aria-live="polite">{formatRange(view, date)}</strong><button type="button" onClick={() => setDate((current) => addDays(current, view === "month" ? 30 : 1))}>Next</button>
    </div>
    <fieldset className="event-ui__calendars"><legend>Visible calendars</legend>{calendars.map((calendar) => <label key={calendar.id}><input type="checkbox" checked={visible.has(calendar.id)} onChange={() => setVisible((current) => { const next = new Set(current); if (next.has(calendar.id)) next.delete(calendar.id); else next.add(calendar.id); return next; })} />Show {calendar.name ?? "Busy calendar"}</label>)}</fieldset>
    {error && <p role="alert">{error} {error.includes("changed elsewhere") && <button type="button" onClick={() => { setEditing(null); setSelected(null); void reload(); }}>Reload events</button>}</p>}
    {loading ? <p role="status">Loading events…</p> : <section role="region" aria-label={view === "agenda" ? "Agenda" : `${view[0].toUpperCase()}${view.slice(1)} calendar`}><ul role={view === "agenda" ? "list" : undefined} aria-label={view === "agenda" ? "Agenda" : undefined} className={`event-ui__${view}`}>{displayed.map(renderEvent)}</ul>{displayed.length === 0 && <p>No events in this range.</p>}</section>}
    {selected && <aside className="event-ui__detail" aria-label="Event details"><h3>{title(selected)}</h3>{external(selected) && <p>This external event is read-only.</p>}{!external(selected) && !editable(selected, calendarFor(selected)) && <p>This event is read-only.</p>}{editable(selected, calendarFor(selected)) && <button type="button" onClick={() => openEdit(selected)}>Edit event</button>}<button type="button" onClick={() => setSelected(null)}>Close event details</button></aside>}
    {editing && <form className="event-ui__editor" onSubmit={save} aria-label={editing === "new" ? "Create event" : "Edit event"}><h3>{editing === "new" ? "New event" : "Edit event"}</h3><label>Title<input required value={draft.title} onChange={(event) => setDraft({ ...draft, title: event.target.value })} /></label><label>Start<input type="datetime-local" required value={draft.start} onChange={(event) => setDraft({ ...draft, start: event.target.value })} /></label><label>End<input type="datetime-local" required value={draft.end} onChange={(event) => setDraft({ ...draft, end: event.target.value })} /></label><label>Recurrence rule<input aria-label="Recurrence rule" placeholder="FREQ=WEEKLY" value={draft.recurrenceRule} onChange={(event) => setDraft({ ...draft, recurrenceRule: event.target.value })} /></label><label>Calendar<select value={draft.calendarId} onChange={(event) => setDraft({ ...draft, calendarId: Number(event.target.value) })}>{calendars.filter(writable).map((calendar) => <option key={calendar.id} value={calendar.id}>{calendar.name}</option>)}</select></label><button type="submit">Save event</button><button type="button" onClick={() => setEditing(null)}>Cancel</button></form>}
  </section>;
}
