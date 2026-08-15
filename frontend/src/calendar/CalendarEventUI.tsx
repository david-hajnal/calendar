import { useCallback, useEffect, useMemo, useRef, useState, type FormEvent } from "react";

import type { ApiClient } from "../auth/api";
import { CalendarApiError, createEvent, listExpandedEvents, updateEvent, type EventPayload, type EventProjection } from "./api";
import { setReminder, removeReminder } from "./reminderApi";
import type { Calendar } from "./CalendarManagement";
import "./CalendarEventUI.css";

type CalendarView = "month" | "week" | "day" | "agenda";
type Draft = { title: string; start: string; end: string; calendarId: number; recurrenceRule: string };

const viewLabels: Record<CalendarView, string> = { month: "Month", week: "Week", day: "Day", agenda: "Agenda" };

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

function eventTop(event: EventProjection): number | null {
  if (event.event_kind === "all_day" || event.start_utc == null) return null;
  const d = new Date(event.start_utc * 1000);
  return d.getHours() * 60 + d.getMinutes();
}

function eventHeight(event: EventProjection): number {
  if (event.event_kind === "all_day" || event.start_utc == null || event.end_utc == null) return 60;
  return Math.max((event.end_utc - event.start_utc) / 60, 15);
}

function currentTimeTop(): number {
  const now = new Date();
  return now.getHours() * 60 + now.getMinutes();
}

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
  const [slotHighlight, setSlotHighlight] = useState<{ dayIndex: number; hour: number } | null>(null);
  const gridRef = useRef<HTMLDivElement>(null);
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

  useEffect(() => {
    if ((view === "day" || view === "week") && gridRef.current) {
      const now = new Date();
      const top = now.getHours() * 60 + now.getMinutes() - 200;
      gridRef.current.scrollTop = Math.max(0, top);
    }
  }, [view]);
  const displayed = events.filter((event) => visible.has(event.calendar_id)).sort((a, b) => eventTime(a) - eventTime(b));
  const calendarFor = (event: EventProjection) => calendars.find((calendar) => calendar.id === event.calendar_id);

  function openNew() {
    const start = Math.floor(date.getTime() / 1000) + 9 * 3600;
    setDraft({ title: "", start: inputTime(start), end: inputTime(start + 3600), calendarId: calendars.find(writable)?.id ?? 0, recurrenceRule: "" });
    setEditing("new"); setSelected(null); setError(null);
  }

  function slotDate(dayIndex: number, hour: number, view: CalendarView): Date {
    if (view === "day") return new Date(date.getFullYear(), date.getMonth(), date.getDate(), hour, 0);
    const weekStart = startOfDay(date);
    const monday = addDays(weekStart, -weekStart.getDay());
    return new Date(monday.getFullYear(), monday.getMonth(), monday.getDate() + dayIndex, hour, 0);
  }

  function onSlotClick(dayIndex: number, hour: number, view: CalendarView) {
    const start = slotDate(dayIndex, hour, view);
    const end = new Date(start);
    end.setHours(end.getHours() + 1);
    setDraft({
      title: "",
      start: inputTime(Math.floor(start.getTime() / 1000)),
      end: inputTime(Math.floor(end.getTime() / 1000)),
      calendarId: calendars.find(writable)?.id ?? 0,
      recurrenceRule: "",
    });
    setEditing("new");
    setSelected(null);
    setError(null);
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
    const cal = calendarFor(event);
    const readonly = !editable(event, cal);
    const name = external(event) ? `${title(event)} (read-only external event)` : title(event);
    const accentColor = cal?.color ?? 'var(--color-primary)';
    const bgColor = `${accentColor}1a`;
    return <div key={`${event.id}-${event.recurrence_id ?? event.recurrence_date ?? ""}`} className="event-chip" style={{ borderLeftColor: accentColor, background: bgColor }}>
      <button type="button" aria-label={name} className="event-chip__text" draggable={!readonly} onDragStart={(drag) => { if (!readonly) drag.dataTransfer.setData("text/plain", String(event.id)); }} onClick={() => setSelected(event)}>{title(event)}</button>
      {!readonly && <button type="button" className="event-chip__move" aria-label={`Move ${title(event)} later`} onClick={() => void move(event, 1)} title="Move later">
        <span className="material-symbols-outlined" style={{ fontSize: '16px' }}>chevron_right</span>
      </button>}
    </div>;
  }
  function renderDayHeader(dayIndex: number) {
     const days = ['Sun', 'Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat'];
     return <div key={dayIndex} className="event-grid__day-header" style={{ gridColumn: dayIndex + 1 }}>{days[dayIndex]}</div>;
   }
   const today = useMemo(() => startOfDay(new Date()), []);
   const isToday = (d: Date) => startOfDay(d).getTime() === today.getTime();
   const miniCalDays = useMemo(() => {
     const from = startOfDay(new Date(date.getFullYear(), date.getMonth(), 1));
     const to = addDays(from, 42);
     const days: Date[] = [];
     let current = startOfDay(from);
     while (current < to) { days.push(new Date(current)); current = addDays(current, 1); }
     return days;
   }, [date]);
   const miniCalMonth = `${date.toLocaleString('default', { month: 'short' })} ${date.getFullYear()}`;
   return <section className="event-ui" aria-labelledby="events-heading">
     {/* Desktop sidebar */}
     <aside className="event-ui__sidebar">
       <div className="event-ui__sidebar-header">
         <h3 className="typography-headline-md">Visible calendars</h3>
       </div>
       <div className="event-ui__sidebar-list">
         {calendars.map((calendar) => <label key={calendar.id} className="event-ui__sidebar-toggle">
           <input type="checkbox" checked={visible.has(calendar.id)} onChange={() => setVisible((current) => { const next = new Set(current); if (next.has(calendar.id)) next.delete(calendar.id); else next.add(calendar.id); return next; })} />
           <span className="event-ui__sidebar-dot" style={{ background: calendar.color || 'var(--color-primary)' }} />
           <span className="typography-body-md">{calendar.name ?? "Busy calendar"}</span>
         </label>)}
       </div>
       <div className="event-ui__sidebar-footer">
         <button type="button" className="event-ui__sidebar-footer-btn" onClick={() => window.location.pathname = "/calendars"}>
           <span className="material-symbols-outlined" style={{ fontSize: '18px' }}>settings</span>
           Settings
         </button>
         <button type="button" className="event-ui__sidebar-footer-btn" onClick={() => window.location.href = "/logout"}>
           <span className="material-symbols-outlined" style={{ fontSize: '18px' }}>logout</span>
           Sign Out
         </button>
       </div>
     </aside>
    <header className="event-ui__toolbar">
      <h2 id="events-heading" className="typography-headline-md">Events</h2>
      <button type="button" className="event-ui__new-btn" onClick={openNew} disabled={!calendars.some(writable)} aria-label="New event">
        <span className="material-symbols-outlined" style={{ fontSize: '18px' }}>add</span>
        New event
      </button>
    </header>
    <div className="event-ui__controls">
      <div className="segmented-control" role="tablist" aria-label="Calendar view">
        {(Object.keys(viewLabels) as CalendarView[]).map((item) => <button key={item} type="button" role="tab" aria-pressed={view === item} className="segmented-control__button" onClick={() => setView(item)}>{viewLabels[item]}</button>)}
      </div>
      <div className="event-ui__date-nav">
        <button type="button" className="event-ui__nav-btn" onClick={() => setDate((current) => addDays(current, view === "month" ? -30 : -1))}><span className="material-symbols-outlined" style={{ fontSize: '20px' }}>chevron_left</span></button>
        <strong className="typography-body-lg" aria-live="polite">{formatRange(view, date)}</strong>
        <button type="button" className="event-ui__nav-btn" onClick={() => setDate((current) => addDays(current, view === "month" ? 30 : 1))}><span className="material-symbols-outlined" style={{ fontSize: '20px' }}>chevron_right</span></button>
        <button type="button" className="event-ui__today-btn" onClick={() => setDate(new Date())}>Today</button>
      </div>
    </div>
    {/* Mini calendar for mobile */}
    <div className="event-ui__mini-cal">
      <div className="event-ui__mini-cal-header">
        <span className="typography-label-md" style={{ color: 'var(--color-on-surface-variant)', textTransform: 'uppercase', letterSpacing: '0.05em' }}>{miniCalMonth}</span>
      </div>
      <div className="event-ui__mini-cal-grid">
        {['S','M','T','W','T','F','S'].map((d, i) => <span key={`h-${i}`} className="event-ui__mini-cal-day-header">{d}</span>)}
        {miniCalDays.map((day, idx) => {
          const isCurrentMonth = day.getMonth() === date.getMonth();
          const isTodayDate = isToday(day);
          const hasEvents = displayed.some(e => e.start_date === day.toISOString().split('T')[0]);
          const cal = hasEvents ? calendarFor(displayed.find(e => e.start_date === day.toISOString().split('T')[0])!) : null;
          const accentColor = cal?.color || 'var(--color-primary)';
          return <button key={idx} type="button" className={`event-ui__mini-cal-day ${!isCurrentMonth ? 'event-ui__mini-cal-day--other' : ''} ${isTodayDate ? 'event-ui__mini-cal-day--today' : ''}`} onClick={() => setDate(startOfDay(day))} style={{ background: isTodayDate ? accentColor : undefined, color: isTodayDate ? '#fff' : undefined }}>
            {day.getDate()}
            {hasEvents && !isTodayDate && <span className="event-ui__mini-cal-dot" style={{ background: accentColor }} />}
          </button>;
        })}
      </div>
    </div>
    {error && <p role="alert" className="app-message app-message--error">{error} {error.includes("changed elsewhere") && <button type="button" className="app-button" style={{ fontSize: '0.8125rem', padding: '0.25rem 0.5rem' }} onClick={() => { setEditing(null); setSelected(null); void reload(); }}>Reload events</button>}</p>}
    {loading ? <div className="event-ui__loading" role="status"><div className="spinner" /> <span className="typography-body-md">Loading events…</span></div> :
      <section role="region" aria-label={view === "agenda" ? "Agenda" : `${view[0].toUpperCase()}${view.slice(1)} calendar`} className={`event-ui__${view}`}>
        {view === "month" && (
          <>
            <div className="event-grid__headers">{[0,1,2,3,4,5,6].map(renderDayHeader)}</div>
            <ul role="list" aria-label="Month calendar" className="event-grid">
              {(() => {
                const from = startOfDay(new Date(date.getFullYear(), date.getMonth(), 1));
                const startOffset = from.getDay();
                const totalCells = 42;
                const cells: { date: Date; events: EventProjection[]; isCurrentMonth: boolean; isToday: boolean }[] = [];
                for (let i = 0; i < totalCells; i++) {
                  const d = addDays(from, i - startOffset);
                  const dateStr = d.toISOString().split('T')[0];
                  const dayEvents = displayed.filter(e => e.start_date === dateStr);
                  cells.push({ date: d, events: dayEvents, isCurrentMonth: d.getMonth() === date.getMonth(), isToday: isToday(d) });
                }
                return cells.map((cell, idx) => <li key={idx} className={`event-grid__cell ${cell.isCurrentMonth ? '' : 'event-grid__cell--other-month'}`} style={{ gridColumn: (idx % 7) + 1, gridRow: Math.floor(idx / 7) + 1 }}>
                  <span className={`event-grid__day ${cell.isToday ? 'event-grid__day--today' : ''}`}>{cell.date.getDate()}</span>
                  {cell.events.map(renderEvent)}
                </li>);
              })()}
            </ul>
          </>
        )}
        {view === "day" && (
          <div ref={gridRef} className="event-ui__day" onWheel={(e) => e.currentTarget.scrollTop += e.deltaY}>
            <div className="event-ui__time-column">
              {Array.from({ length: 24 }, (_, h) => (
                <div key={h} className="event-ui__time-label">
                  <span>{h === 0 ? "12 AM" : h < 12 ? `${h} AM` : h === 12 ? "12 PM" : `${h - 12} PM`}</span>
                </div>
              ))}
            </div>
            <div className="event-ui__day-column" onMouseLeave={() => setSlotHighlight(null)}>
              {Array.from({ length: 24 }, (_, h) => (
                <div key={h} className="event-ui__hour-row" onClick={() => onSlotClick(0, h, "day")} onMouseEnter={() => setSlotHighlight({ dayIndex: 0, hour: h })} />
              ))}
              {displayed.filter((e) => e.event_kind !== "all_day" && eventTop(e) !== null).map((event) => {
                const top = eventTop(event)!;
                const height = eventHeight(event);
                const cal = calendarFor(event);
                const accentColor = cal?.color || "var(--color-primary)";
                const bgColor = `${accentColor}1a`;
                return (
                  <div
                    key={`${event.id}-${event.recurrence_id ?? event.recurrence_date ?? ""}`}
                    className="event-ui__event-block"
                    style={{ top: `${top}px`, height: `${height}px`, borderLeftColor: accentColor, background: bgColor }}
                    onClick={() => openEdit(event)}
                  >
                    <button type="button" className="event-chip__text" draggable={false} onClick={() => openEdit(event)}>
                      {title(event)}
                    </button>
                  </div>
                );
              })}
              <div className="event-ui__current-time" style={{ top: `${currentTimeTop()}px` }} />
              {slotHighlight && <div className="event-ui__slot-highlight" style={{ top: `${slotHighlight.hour * 60}px` }} />}
            </div>
          </div>
        )}
        {view === "week" && (
          <div ref={gridRef} className="event-ui__week" onWheel={(e) => e.currentTarget.scrollTop += e.deltaY}>
            <div className="event-ui__time-column">
              {Array.from({ length: 24 }, (_, h) => (
                <div key={h} className="event-ui__time-label">
                  <span>{h === 0 ? "12 AM" : h < 12 ? `${h} AM` : h === 12 ? "12 PM" : `${h - 12} PM`}</span>
                </div>
              ))}
            </div>
            {Array.from({ length: 7 }, (_, dayIndex) => {
              const dayEvents = displayed.filter((e) => {
                if (e.event_kind === "all_day" || e.start_utc == null) return false;
                const d = new Date(e.start_utc * 1000);
                const weekStart = startOfDay(date);
                const monday = addDays(weekStart, -weekStart.getDay());
                const dayDate = new Date(monday.getFullYear(), monday.getMonth(), monday.getDate() + dayIndex);
                return startOfDay(d).getTime() === dayDate.getTime();
              });
              return (
                <div key={dayIndex} className="event-ui__week-day-column">
                  {Array.from({ length: 24 }, (_, h) => (
                    <div key={h} className="event-ui__week-hour-row" onClick={() => onSlotClick(dayIndex, h, "week")} />
                  ))}
                  {dayEvents.map((event) => {
                    const top = eventTop(event)!;
                    const height = eventHeight(event);
                    const cal = calendarFor(event);
                    const accentColor = cal?.color || "var(--color-primary)";
                    const bgColor = `${accentColor}1a`;
                    return (
                      <div
                        key={`${event.id}-${event.recurrence_id ?? event.recurrence_date ?? ""}`}
                        className="event-ui__event-block"
                        style={{ top: `${top}px`, height: `${height}px`, borderLeftColor: accentColor, background: bgColor }}
                        onClick={() => openEdit(event)}
                      >
                        <button type="button" className="event-chip__text" draggable={false} onClick={() => openEdit(event)}>
                          {title(event)}
                        </button>
                      </div>
                    );
                  })}
                  <div className="event-ui__current-time" style={{ top: `${currentTimeTop()}px` }} />
                  {slotHighlight && slotHighlight.dayIndex === dayIndex && (
                    <div className="event-ui__slot-highlight" style={{ top: `${slotHighlight.hour * 60}px` }} />
                  )}
                </div>
              );
            })}
          </div>
        )}
        {view === "agenda" && <ul role="list" aria-label="Agenda" className="event-ui__agenda">{displayed.map(renderEvent)}</ul>}
        {displayed.length === 0 && <p className="typography-body-md" style={{ color: 'var(--color-on-surface-variant)', textAlign: 'center', padding: '2rem 0' }}>No events in this range.</p>}
      </section>
    }
    {selected && <aside className="event-ui__detail" aria-label="Event details">
      <div className="event-ui__detail-header">
        <span className="event-ui__detail-accent" style={{ background: calendarFor(selected)?.color || 'var(--color-primary)' }} />
        <div className="event-ui__detail-title-row">
          <h3 className="typography-headline-md" style={{ margin: 0 }}>{title(selected)}</h3>
          <button type="button" className="event-ui__detail-close" onClick={() => setSelected(null)} aria-label="Close event details"><span className="material-symbols-outlined" style={{ fontSize: '20px' }}>close</span></button>
        </div>
      </div>
      {external(selected) && <p className="typography-body-md" style={{ color: 'var(--color-on-surface-variant)', fontStyle: 'italic' }}>This external event is read-only.</p>}
      {!external(selected) && !editable(selected, calendarFor(selected)) && <p className="typography-body-md" style={{ color: 'var(--color-on-surface-variant)', fontStyle: 'italic' }}>This event is read-only.</p>}
      {editable(selected, calendarFor(selected)) && <button type="button" className="app-button app-button--primary" style={{ fontSize: '0.8125rem', marginTop: '0.75rem' }} onClick={() => openEdit(selected)}>Edit event</button>}
      <div className="event-ui__detail-meta">
        {selected.start_utc && selected.end_utc && <p className="typography-body-md"><span className="material-symbols-outlined" style={{ fontSize: '16px', verticalAlign: 'middle', marginRight: '0.375rem' }}>schedule</span>{new Date(selected.start_utc * 1000).toLocaleTimeString('en-US', { hour: 'numeric', minute: '2-digit' })} - {new Date(selected.end_utc * 1000).toLocaleTimeString('en-US', { hour: 'numeric', minute: '2-digit' })}</p>}
        {selected.start_utc && <p className="typography-body-md"><span className="material-symbols-outlined" style={{ fontSize: '16px', verticalAlign: 'middle', marginRight: '0.375rem' }}>event</span>{new Date(selected.start_utc * 1000).toLocaleDateString('en-US', { month: 'short', day: 'numeric', year: 'numeric' })}</p>}
      </div>
      {selected.location && <div className="event-ui__detail-location">
        <p className="typography-body-md"><span className="material-symbols-outlined" style={{ fontSize: '16px', verticalAlign: 'middle', marginRight: '0.375rem' }}>location_on</span>{selected.location}</p>
      </div>}
      {selected.description && <div className="event-ui__detail-description">
        <p className="typography-body-md">{selected.description}</p>
      </div>}
      <ReminderRow api={api} calendarId={selected.calendar_id} eventId={selected.id} eventTitle={title(selected)} eventStartUtc={selected.start_utc || 0} />
    </aside>}
    {editing && <form className="event-ui__editor" onSubmit={save} aria-label={editing === "new" ? "Create event" : "Edit event"}>
      <div className="event-ui__editor-header">
        <h3 className="typography-headline-md">{editing === "new" ? "New event" : "Edit event"}</h3>
        <button type="button" className="event-ui__editor-close" onClick={() => setEditing(null)} aria-label="Close editor"><span className="material-symbols-outlined" style={{ fontSize: '20px' }}>close</span></button>
      </div>
      <div className="event-ui__editor-grid">
        <label className="event-ui__editor-field">
          <span className="typography-label-md">Title</span>
          <input required value={draft.title} onChange={(event) => setDraft({ ...draft, title: event.target.value })} placeholder="Event title" />
        </label>
        <label className="event-ui__editor-field">
          <span className="typography-label-md">Start</span>
          <input type="datetime-local" required value={draft.start} onChange={(event) => setDraft({ ...draft, start: event.target.value })} />
        </label>
        <label className="event-ui__editor-field">
          <span className="typography-label-md">End</span>
          <input type="datetime-local" required value={draft.end} onChange={(event) => setDraft({ ...draft, end: event.target.value })} />
        </label>
        <label className="event-ui__editor-field">
          <span className="typography-label-md">Recurrence</span>
          <input aria-label="Recurrence rule" placeholder="FREQ=WEEKLY" value={draft.recurrenceRule} onChange={(event) => setDraft({ ...draft, recurrenceRule: event.target.value })} />
        </label>
        <label className="event-ui__editor-field">
          <span className="typography-label-md">Calendar</span>
          <select value={draft.calendarId} onChange={(event) => setDraft({ ...draft, calendarId: Number(event.target.value) })}>{calendars.filter(writable).map((calendar) => <option key={calendar.id} value={calendar.id}>{calendar.name}</option>)}</select>
        </label>
      </div>
      <div className="event-ui__editor-actions">
        <button type="submit" className="app-button app-button--primary">Save event</button>
        <button type="button" className="app-button" onClick={() => setEditing(null)}>Cancel</button>
      </div>
    </form>}
    {/* Mobile FAB */}
    <button type="button" className="event-ui__fab" onClick={openNew} disabled={!calendars.some(writable)} aria-label="New event mobile">
      <span className="material-symbols-outlined" style={{ fontSize: '24px' }}>add</span>
    </button>
  </section>;
}

const PRESET_MINUTES = [5, 15, 30, 60];

function ReminderRow({ api, calendarId, eventId, eventTitle, eventStartUtc }: { api: ApiClient; calendarId: number; eventId: number; eventTitle: string; eventStartUtc: number }) {
  const [activeMinutes, setActiveMinutes] = useState<number | null>(null);
  const [customMinutes, setCustomMinutes] = useState("");
  const [loading, setLoading] = useState(false);

  useEffect(() => {
    let active = true;
    void (async () => {
      try {
        const res = await api.request(`/api/v1/calendars/${calendarId}/events/${eventId}/reminder`);
        if (active && res.ok) {
          const data = await res.json() as { reminder_minutes: number };
          setActiveMinutes(data.reminder_minutes);
        }
      } catch {
        // ignore
      }
    })();
    return () => { active = false; };
  }, [api, calendarId, eventId]);

  const handleSet = async (minutes: number) => {
    setLoading(true);
    try {
      await setReminder(api, calendarId, eventId, minutes);
      setActiveMinutes(minutes);
      setCustomMinutes("");
      if ("Notification" in window) {
        const perm = await Notification.requestPermission();
        if (perm === "granted") {
          new Notification(`Reminder set for ${eventTitle}`, { body: `You'll be notified ${minutes} minute${minutes > 1 ? "s" : ""} before the event.`, tag: `reminder-${eventId}` });
        }
      }
    } catch {
      // ignore
    } finally {
      setLoading(false);
    }
  };

  const handleRemove = async () => {
    setLoading(true);
    try {
      await removeReminder(api, calendarId, eventId);
      setActiveMinutes(null);
      setCustomMinutes("");
    } catch {
      // ignore
    } finally {
      setLoading(false);
    }
  };

  const handleCustom = async () => {
    const mins = parseInt(customMinutes, 10);
    if (mins >= 1 && mins <= 10080) {
      await handleSet(mins);
    }
  };

  const formatMinutes = (m: number): string => {
    if (m < 60) return `${m}m`;
    if (m === 60) return "1h";
    const hours = Math.floor(m / 60);
    const mins = m % 60;
    if (mins === 0) return `${hours}h`;
    return `${hours}h ${mins}m`;
  };

  return (
    <div className="event-ui__reminder">
      <div className="event-ui__reminder-label">
        <span className="material-symbols-outlined" style={{ fontSize: '16px' }}>notifications</span>
        Reminder
      </div>
      {activeMinutes === null ? (
        <div className="event-ui__reminder-options">
          {PRESET_MINUTES.map((m) => (
            <button
              key={m}
              type="button"
              className="event-ui__reminder-btn"
              onClick={() => void handleSet(m)}
              disabled={loading}
            >
              {formatMinutes(m)}
            </button>
          ))}
          <label className="event-ui__reminder-custom">
            <input
              type="number"
              min={1}
              max={10080}
              placeholder="custom"
              value={customMinutes}
              onChange={(e) => setCustomMinutes(e.target.value)}
              onKeyDown={(e) => { if (e.key === "Enter") void handleCustom(); }}
            />
          </label>
        </div>
      ) : (
        <div className="event-ui__reminder-active">
          <span className="event-ui__reminder-active-text">
            <span className="material-symbols-outlined" style={{ fontSize: '14px', verticalAlign: 'middle' }}>notifications</span>
            Remind {formatMinutes(activeMinutes)} before
          </span>
          <button type="button" className="event-ui__reminder-remove" onClick={handleRemove} disabled={loading}>
            Remove
          </button>
        </div>
      )}
    </div>
  );
}
