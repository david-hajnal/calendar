import { useEffect, useState, useMemo } from "react";

import type { Fetcher } from "../auth/api";
import "./PublicView.css";

const defaultNow = () => Date.now() / 1_000;

/** This is intentionally separate from authenticated calendar models: only listed fields cross the public boundary. */
export interface PublicViewMetadata {
  name: string;
  projection: "full_details" | "title_and_time" | "free_busy";
  display_timezone: string;
  expires_at: number;
}

export interface PublicEvent {
  title?: string;
  description?: string;
  location?: string;
  status?: "tentative" | "confirmed" | "cancelled";
  event_kind: "timed" | "all_day";
  start_utc?: number;
  end_utc?: number;
  timezone?: string;
  start_date?: string;
  end_date?: string;
  busy?: true;
}

function text(value: unknown): string | undefined { return typeof value === "string" ? value : undefined; }
function number(value: unknown): number | undefined { return typeof value === "number" ? value : undefined; }

function publicEvent(value: unknown, projection: PublicViewMetadata["projection"]): PublicEvent | null {
  if (typeof value !== "object" || value === null) return null;
  const raw = value as Record<string, unknown>;
  if (raw.event_kind !== "timed" && raw.event_kind !== "all_day") return null;
  const includeTitle = projection === "full_details" || projection === "title_and_time";
  const includeDetails = projection === "full_details";
  return { title: includeTitle ? text(raw.title) : undefined, description: includeDetails ? text(raw.description) : undefined, location: includeDetails ? text(raw.location) : undefined, status: includeDetails && (raw.status === "tentative" || raw.status === "confirmed" || raw.status === "cancelled") ? raw.status : undefined, event_kind: raw.event_kind, start_utc: number(raw.start_utc), end_utc: number(raw.end_utc), timezone: includeTitle ? text(raw.timezone) : undefined, start_date: text(raw.start_date), end_date: text(raw.end_date), busy: projection === "free_busy" ? true : undefined };
}

function metadata(value: unknown): PublicViewMetadata | null {
  if (typeof value !== "object" || value === null) return null;
  const raw = value as Record<string, unknown>;
  if (typeof raw.name !== "string" || typeof raw.display_timezone !== "string" || typeof raw.expires_at !== "number" || !["full_details", "title_and_time", "free_busy"].includes(String(raw.projection))) return null;
  return { name: raw.name, projection: raw.projection as PublicViewMetadata["projection"], display_timezone: raw.display_timezone, expires_at: raw.expires_at };
}

async function publicJson<T>(fetcher: Fetcher, path: string, parse: (value: unknown) => T | null): Promise<T> {
  const response = await fetcher(path, { credentials: "omit" });
  const parsed = parse(await response.json());
  if (!response.ok || parsed === null) throw new Error("Invalid public response");
  return parsed;
}

function formatMonthYear(date: Date): string {
  return date.toLocaleDateString(undefined, { month: "long", year: "numeric" });
}

function getMonthGrid(date: Date) {
  const year = date.getFullYear();
  const month = date.getMonth();
  const firstDay = new Date(year, month, 1);
  const startOffset = firstDay.getDay();
  const startDate = new Date(year, month, 1 - startOffset);

  const cells: { date: Date; isCurrentMonth: boolean }[] = [];
  const current = new Date(startDate);
  for (let i = 0; i < 42; i++) {
    cells.push({ date: new Date(current), isCurrentMonth: current.getMonth() === month });
    current.setDate(current.getDate() + 1);
  }
  return cells;
}

function eventsForDay(events: PublicEvent[], date: Date): PublicEvent[] {
  const target = new Date(Date.UTC(date.getFullYear(), date.getMonth(), date.getDate()));
  const next = new Date(target.getTime() + 86400000);
  return events.filter((event) => {
    if (event.event_kind === "all_day" && event.start_date && event.end_date) {
      const start = new Date(event.start_date + "T00:00:00Z");
      const end = new Date(event.end_date + "T00:00:00Z");
      return start < next && end > target;
    }
    if (event.start_utc != null) {
      const start = new Date(event.start_utc * 1000);
      const end = event.end_utc != null ? new Date(event.end_utc * 1000) : start;
      return start < next && end > target;
    }
    return false;
  });
}

export function PublicViewPage({ token, fetcher = fetch, now = defaultNow }: { token: string; fetcher?: Fetcher; now?: () => number }) {
  const [view, setView] = useState<PublicViewMetadata | null>(null);
  const [events, setEvents] = useState<PublicEvent[]>([]);
  const [failed, setFailed] = useState(false);
  const [mode, setMode] = useState<"month" | "agenda">("month");
  const [currentDate, setCurrentDate] = useState(() => new Date(now()));

  useEffect(() => {
    let active = true;
    const from = Math.floor(now());
    void publicJson(fetcher, `/api/v1/public/views/${encodeURIComponent(token)}`, metadata).then(async (nextView) => {
      const nextEvents = await publicJson(fetcher, `/api/v1/public/views/${encodeURIComponent(token)}/events?from=${from}&to=${from + 42 * 24 * 60 * 60}`, (value) => Array.isArray(value) ? value.map((event) => publicEvent(event, nextView.projection)).filter((event): event is PublicEvent => event !== null) : null);
      if (active) { setView(nextView); setEvents(nextEvents); setFailed(false); }
    }).catch(() => { if (active) setFailed(true); });
    return () => { active = false; };
  }, [fetcher, now, token]);

  const monthCells = useMemo(() => view ? getMonthGrid(currentDate) : [], [view, currentDate]);
  const dayNames = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];

  if (failed) return <main className="public-view" role="alert">
    <div className="public-view__error">
      <span className="material-symbols-outlined public-view__error-icon" style={{ fontSize: '48px', color: 'var(--color-error)' }}>error_outline</span>
      <h2 className="typography-headline-lg" style={{ color: 'var(--color-on-surface)', margin: '1rem 0 0.5rem' }}>Unavailable</h2>
      <p className="typography-body-lg" style={{ color: 'var(--color-on-surface-variant)', maxWidth: '28rem', margin: '0 auto' }}>This public link is unavailable. It may have expired or the owner has restricted access.</p>
    </div>
  </main>;
  if (view === null) return <main className="public-view public-view--loading">
    <div className="public-view__loading">
      <span className="material-symbols-outlined" style={{ fontSize: '32px', color: 'var(--color-primary)' }}>progress_activity</span>
      <p className="typography-headline-md" style={{ color: 'var(--color-on-surface)' }}>Loading public calendar…</p>
    </div>
  </main>;

  const renderMonthGrid = () => {
    const rows: React.ReactNode[] = [];
    for (let i = 0; i < monthCells.length; i += 7) {
      const row = monthCells.slice(i, i + 7);
      rows.push(
        <div key={i} className="public-view__grid-row">
          {row.map((cell, j) => {
            const isToday = cell.date.toDateString() === new Date(now()).toDateString();
            const isCurrentMonth = cell.isCurrentMonth;
            const dayEvents = eventsForDay(events, cell.date);
            return <div key={j} className={`public-view__day-cell ${!isCurrentMonth ? 'public-view__day-cell--other' : ''} ${isToday ? 'public-view__day-cell--today' : ''}`} onClick={() => setCurrentDate(cell.date)}>
              <span className={`public-view__day-number ${!isCurrentMonth ? 'public-view__day-number--other' : ''} ${isToday ? 'public-view__day-number--today' : ''}`}>{cell.date.getDate()}</span>
              <div className="public-view__day-events">
                {dayEvents.map((event, idx) => {
                  const isBusy = event.busy === true;
                  const statusColor = event.status === "tentative" ? "var(--color-tertiary)" : event.status === "cancelled" ? "var(--color-error)" : "var(--color-primary)";
                  const chipBg = isBusy ? "transparent" : `rgb(70 72 212 / 10%)`;
                  return <div key={idx} className={`public-view__event-chip ${isBusy ? 'public-view__event-chip--busy' : ''}`} style={isBusy ? { borderLeftColor: 'var(--color-outline)' } : { borderLeftColor: statusColor, backgroundColor: chipBg }} title={event.title}>
                    <span className="public-view__event-chip-text typography-label-md" style={{ color: isBusy ? 'var(--color-on-surface-variant)' : 'var(--color-on-primary-container)' }}>
                      {isBusy ? "Busy" : event.title}
                    </span>
                    {event.description && <span className="public-view__event-detail typography-label-sm" style={{ color: 'var(--color-on-surface-variant)' }}>{event.description}</span>}
                    {event.location && <span className="public-view__event-detail typography-label-sm" style={{ color: 'var(--color-on-surface-variant)' }}>{event.location}</span>}
                    {event.start_utc && <span className="public-view__event-time typography-label-sm" style={{ color: 'var(--color-on-surface-variant)' }}>{new Date(event.start_utc * 1000).toLocaleTimeString()}</span>}
                  </div>;
                })}
              </div>
            </div>;
          })}
        </div>
      );
    }
    return rows;
  };

  return <main className="public-view">
    <header className="public-view__header">
      <div className="public-view__header-left">
        <h1 className="typography-display public-view__header-title">{view.name}</h1>
        <p className="public-view__header-date">
          <span className="material-symbols-outlined fill" style={{ fontSize: '18px', color: 'var(--color-on-surface-variant)' }}>calendar_month</span>
          {formatMonthYear(currentDate)}
        </p>
      </div>
          <div className="public-view__header-controls">
        <div className="segmented-control" role="tablist" aria-label="View mode">
          <button type="button" role="button" aria-pressed={mode === "month"} aria-label="Month view" className={`segmented-control__button ${mode === "month" ? "segmented-control__button--active" : ""}`} onClick={() => setMode("month")}>Month view</button>
          <button type="button" role="button" aria-pressed={mode === "agenda"} aria-label="Agenda view" className={`segmented-control__button ${mode === "agenda" ? "segmented-control__button--active" : ""}`} onClick={() => setMode("agenda")}>Agenda view</button>
        </div>
      </div>
    </header>
    {mode === "month" ? <section role="region" aria-label="Public month" className="public-view__month">
      <div className="public-view__grid-header">
        {dayNames.map((day) => <div key={day} className="public-view__grid-header-cell typography-label-md" style={{ color: 'var(--color-on-surface-variant)' }}>{day}</div>)}
      </div>
      <div className="public-view__grid-body">
        {renderMonthGrid()}
      </div>
    </section> : <section role="region" aria-label="Public agenda">
      <ul role="list" aria-label="Public agenda events" className="public-view__agenda">{events.map(renderEvent)}</ul>
    </section>}
    {view.expires_at && <p className="public-view__expiry typography-label-md" style={{ color: 'var(--color-on-surface-variant)', textAlign: 'center', marginTop: '1rem' }}>
      <span className="material-symbols-outlined" style={{ fontSize: '14px', verticalAlign: 'middle', marginRight: '0.25rem' }}>info</span>
      This link expires: {new Date(view.expires_at * 1000).toLocaleDateString()}
    </p>}
  </main>;
}

function renderEvent(event: PublicEvent, index: number) {
  const isBusy = event.busy === true;
  const statusColor = event.status === "tentative" ? "var(--color-tertiary)" : event.status === "cancelled" ? "var(--color-error)" : "var(--color-primary)";
  return <li key={index} className={`public-view__event ${isBusy ? 'public-view__event--busy' : ''}`}>
    <strong className="public-view__event-title" style={isBusy ? {} : { borderLeftColor: statusColor }}>{isBusy ? "Busy" : event.title}</strong>
    {event.description && <p className="public-view__event-detail">{event.description}</p>}
    {event.location && <p className="public-view__event-detail"><span className="material-symbols-outlined" style={{ fontSize: '14px', verticalAlign: 'middle', marginRight: '0.25rem' }}>place</span>{event.location}</p>}
    {event.start_utc && <p className="public-view__event-time"><span className="material-symbols-outlined" style={{ fontSize: '14px', verticalAlign: 'middle', marginRight: '0.25rem' }}>schedule</span>{new Date(event.start_utc * 1000).toLocaleString()}</p>}
  </li>;
}
