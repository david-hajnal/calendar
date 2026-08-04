import { useEffect, useState } from "react";

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

export function PublicViewPage({ token, fetcher = fetch, now = defaultNow }: { token: string; fetcher?: Fetcher; now?: () => number }) {
  const [view, setView] = useState<PublicViewMetadata | null>(null);
  const [events, setEvents] = useState<PublicEvent[]>([]);
  const [failed, setFailed] = useState(false);
  const [mode, setMode] = useState<"month" | "agenda">("month");
  useEffect(() => {
    let active = true;
    const from = Math.floor(now());
    void publicJson(fetcher, `/api/v1/public/views/${encodeURIComponent(token)}`, metadata).then(async (nextView) => {
      const nextEvents = await publicJson(fetcher, `/api/v1/public/views/${encodeURIComponent(token)}/events?from=${from}&to=${from + 42 * 24 * 60 * 60}`, (value) => Array.isArray(value) ? value.map((event) => publicEvent(event, nextView.projection)).filter((event): event is PublicEvent => event !== null) : null);
      if (active) { setView(nextView); setEvents(nextEvents); }
    }).catch(() => { if (active) setFailed(true); });
    return () => { active = false; };
  }, [fetcher, now, token]);
  if (failed) return <main><p role="alert">This public link is unavailable.</p></main>;
  if (view === null) return <main aria-busy="true"><p role="status">Loading public calendar…</p></main>;
  const renderEvent = (event: PublicEvent, index: number) => <li key={index} className="public-view__event"><strong>{event.busy ? "Busy" : event.title}</strong>{event.description && <p>{event.description}</p>}{event.location && <p>{event.location}</p>}</li>;
  return <main className="public-view"><h1>{view.name}</h1>
    <div className="public-view__controls" aria-label="Public calendar view"><button type="button" aria-pressed={mode === "month"} onClick={() => setMode("month")}>Month view</button><button type="button" aria-pressed={mode === "agenda"} onClick={() => setMode("agenda")}>Agenda view</button></div>
    {mode === "month" ? <section role="region" aria-label="Public month"><ul aria-label="Public month events" className="public-view__month">{events.map(renderEvent)}</ul></section> : <section role="region" aria-label="Public agenda"><ul role="list" aria-label="Public agenda events" className="public-view__agenda">{events.map(renderEvent)}</ul></section>}
  </main>;
}
