import { useEffect, useState, useCallback } from "react";

import type { ApiClient } from "../auth/api";
import "./CompositeViewManagement.css";

interface CompositeView {
  id: number;
  owner_user_id: number;
  name: string;
  version: number;
  created_at: number;
  updated_at: number;
  calendars: { calendar_id: number; position: number; color: string }[];
}

interface Calendar {
  id: number;
  access: "details" | "free_busy" | "none";
  role: "owner" | "manager" | "viewer" | "free_busy_viewer";
  name: string;
  color: string;
  archived?: boolean;
}

interface Publication {
  token: string;
  projection: "full_details" | "title_and_time" | "free_busy";
  display_timezone: string;
  expires_at: number;
  revoked: boolean;
  version: number;
  caldav_enabled?: boolean;
  caldav_url?: string | null;
}

function compositeView(value: unknown): CompositeView | null {
  if (typeof value !== "object" || value === null) return null;
  const raw = value as Record<string, unknown>;
  if (typeof raw.id !== "number" || typeof raw.name !== "string") return null;
  return {
    id: raw.id,
    owner_user_id: typeof raw.owner_user_id === "number" ? raw.owner_user_id : 0,
    name: raw.name,
    version: typeof raw.version === "number" ? raw.version : 0,
    created_at: typeof raw.created_at === "number" ? raw.created_at : 0,
    updated_at: typeof raw.updated_at === "number" ? raw.updated_at : 0,
    calendars: Array.isArray(raw.calendars) ? raw.calendars.map((c: unknown) => {
      if (typeof c !== "object" || c === null) return { calendar_id: 0, position: 0, color: "" };
      const rc = c as Record<string, unknown>;
      return {
        calendar_id: typeof rc.calendar_id === "number" ? rc.calendar_id : 0,
        position: typeof rc.position === "number" ? rc.position : 0,
        color: typeof rc.color === "string" ? rc.color : "",
      };
    }) : [],
  };
}

function calendar(value: unknown): Calendar | null {
  if (typeof value !== "object" || value === null) return null;
  const raw = value as Record<string, unknown>;
  return {
    id: typeof raw.id === "number" ? raw.id : 0,
    access: ["details", "free_busy", "none"].includes(String(raw.access)) ? raw.access as Calendar["access"] : "none",
    role: typeof raw.role === "string" ? raw.role as Calendar["role"] : "viewer",
    name: typeof raw.name === "string" ? raw.name : "",
    color: typeof raw.color === "string" ? raw.color : "#000000",
    archived: raw.archived === true,
  };
}

function parsePublication(value: unknown, fallbackToken?: string): Publication | null {
  if (typeof value !== "object" || value === null) return null;
  const raw = value as Record<string, unknown>;
  if (typeof raw.projection !== "string" || typeof raw.display_timezone !== "string" || typeof raw.expires_at !== "number" || typeof raw.version !== "number") return null;
  return {
    token: typeof raw.token === "string" ? raw.token : (fallbackToken ?? ""),
    projection: raw.projection as Publication["projection"],
    display_timezone: raw.display_timezone,
    expires_at: raw.expires_at,
    revoked: raw.revoked === true,
    version: raw.version,
    caldav_enabled: raw.caldav_enabled === true,
    caldav_url: typeof raw.caldav_url === "string" ? raw.caldav_url : null,
  };
}

export function CompositeViewManagement({ api }: { api: ApiClient }) {
  const [views, setViews] = useState<CompositeView[]>([]);
  const [calendars, setCalendars] = useState<Calendar[]>([]);
  const [selectedView, setSelectedView] = useState<CompositeView | null>(null);
  const [editingName, setEditingName] = useState("");
  const [calendarColors, setCalendarColors] = useState<Record<number, string>>({});
  const [publication, setPublication] = useState<Publication | null>(null);
  const [caldavEnabled, setCaldavEnabled] = useState(false);
  const [detailLevel, setDetailLevel] = useState<"full_details" | "title_and_time" | "free_busy">("full_details");
  const [expiresAt, setExpiresAt] = useState("");
  const [loading, setLoading] = useState(true);
  const [calendarsLoaded, setCalendarsLoaded] = useState(false);
  const [pendingCalendarId, setPendingCalendarId] = useState<number | null>(null);
  const [unsavedCalendarChanges, setUnsavedCalendarChanges] = useState(false);

  useEffect(() => {
    let active = true;
    api.request("/api/v1/views").then((res) => {
      if (!active) return;
      if (res.ok) return res.json().then((value) => { const parsed = (value as unknown[]).map(compositeView).filter((c): c is CompositeView => c !== null); if (active) { setViews(parsed); setLoading(false); } });
      if (active) setLoading(false);
    }).catch(() => { if (active) setLoading(false); });
    return () => { active = false; };
  }, [api]);

  const fetchCalendars = useCallback(() => {
    api.request("/api/v1/calendars").then((res) => {
      if (res.ok) res.json().then((value) => { const parsed = (value as unknown[]).map(calendar).filter((c): c is Calendar => c !== null); setCalendars(parsed); setCalendarsLoaded(true); });
    });
  }, [api]);

  const openEditor = useCallback((view: CompositeView) => {
    setSelectedView(view);
    setEditingName(view.name);
    const colors: Record<number, string> = {};
    for (const cal of view.calendars) colors[cal.calendar_id] = cal.color;
    setCalendarColors(colors);
    if (!calendarsLoaded) fetchCalendars();
    api.request(`/api/v1/views/${view.id}/publication`).then((res) => {
      if (res.ok) res.json().then((value) => { const pub = parsePublication(value); if (pub) { setPublication(pub); setDetailLevel(pub.projection); setExpiresAt(pub.expires_at ? new Date(pub.expires_at * 1000).toISOString().slice(0, 16) : ""); setCaldavEnabled(pub.caldav_enabled ?? false); } else { setPublication(null); setCaldavEnabled(false); } });
    }).catch(() => { setPublication(null); setCaldavEnabled(false); });
  }, [api, calendarsLoaded, fetchCalendars]);

  const saveName = () => {
    if (!selectedView) return;
    api.request(`/api/v1/views/${selectedView.id}`, { method: "PATCH", body: JSON.stringify({ name: editingName }) }).then((res) => {
      if (res.ok) res.json().then((value) => { const updated = compositeView(value); if (updated) { setViews((prev) => prev.map((v) => v.id === updated.id ? updated : v)); setSelectedView(updated); } });
    });
  };

  const publishView = () => {
    if (!selectedView) return;
    const expires_at = expiresAt ? Math.floor(new Date(expiresAt).getTime() / 1_000) : undefined;
    api.request(`/api/v1/views/${selectedView.id}/publication`, { method: "POST", body: JSON.stringify({ projection: detailLevel, display_timezone: "UTC", expires_at }) }).then((res) => {
      if (res.ok) res.json().then((value) => { const pub = parsePublication(value); if (pub) setPublication(pub); });
    });
  };

  const savePublication = () => {
    if (!selectedView || !publication) return;
    const expires_at = expiresAt ? Math.floor(new Date(expiresAt).getTime() / 1_000) : publication.expires_at;
    api.request(`/api/v1/views/${selectedView.id}/publication`, { method: "PATCH", body: JSON.stringify({ projection: detailLevel, display_timezone: "UTC", expires_at }) }).then((res) => {
      if (res.ok) res.json().then((value) => { const pub = parsePublication(value, publication.token); if (pub) setPublication(pub); });
    });
  };

  const rotateLink = () => {
    if (!selectedView) return;
    api.request(`/api/v1/views/${selectedView.id}/publication/rotate`, { method: "POST" }).then((res) => {
      if (res.ok) res.json().then((value) => { const pub = parsePublication(value); if (pub) setPublication(pub); });
    });
  };

  const toggleCaldav = () => {
    if (!selectedView || !publication) return;
    const newEnabled = !caldavEnabled;
    api.request(`/api/v1/views/${selectedView.id}/publication`, { method: "PATCH", headers: { "content-type": "application/json" }, body: JSON.stringify({ projection: detailLevel, display_timezone: "UTC", expires_at: publication.expires_at, caldav_enabled: newEnabled }) }).then((res) => {
      if (res.ok) res.json().then((value) => { const pub = parsePublication(value, publication.token); if (pub) { setPublication(pub); setCaldavEnabled(pub.caldav_enabled ?? false); } });
    });
  };

  const addCalendar = (calendarId: number) => {
    if (!selectedView) return;
    const cal = calendars.find((c) => c.id === calendarId);
    if (!cal || selectedView.calendars.some((c) => c.calendar_id === calendarId)) return;
    const maxPos = selectedView.calendars.length > 0 ? Math.max(...selectedView.calendars.map((c) => c.position)) : -1;
    const newCalendars = [...selectedView.calendars, { calendar_id: calendarId, position: maxPos + 1, color: cal.color }];
    const updated = { ...selectedView, calendars: newCalendars };
    setSelectedView(updated);
    setCalendarColors((prev) => ({ ...prev, [calendarId]: cal.color }));
    setViews((prev) => prev.map((v) => v.id === updated.id ? updated : v));
    setUnsavedCalendarChanges(true);
  };

  const removeCalendar = (calendarId: number) => {
    if (!selectedView) return;
    const newCalendars = selectedView.calendars.filter((c) => c.calendar_id !== calendarId).map((c, i) => ({ ...c, position: i }));
    const updated = { ...selectedView, calendars: newCalendars };
    setSelectedView(updated);
    setCalendarColors((prev) => { const next = { ...prev }; delete next[calendarId]; return next; });
    setViews((prev) => prev.map((v) => v.id === updated.id ? updated : v));
    setUnsavedCalendarChanges(true);
  };

  const moveCalendar = (calendarId: number, direction: "up" | "down") => {
    if (!selectedView) return;
    const idx = selectedView.calendars.findIndex((c) => c.calendar_id === calendarId);
    if (idx < 0) return;
    const swapIdx = direction === "up" ? idx - 1 : idx + 1;
    if (swapIdx < 0 || swapIdx >= selectedView.calendars.length) return;
    const newCalendars = [...selectedView.calendars];
    const temp = { ...newCalendars[idx] };
    newCalendars[idx] = { ...newCalendars[swapIdx], position: temp.position };
    newCalendars[swapIdx] = { ...temp, position: newCalendars[swapIdx].position };
    const updated = { ...selectedView, calendars: newCalendars };
    setSelectedView(updated);
    setViews((prev) => prev.map((v) => v.id === updated.id ? updated : v));
    setUnsavedCalendarChanges(true);
  };

  const saveCalendars = () => {
    if (!selectedView) return;
    api.request(`/api/v1/views/${selectedView.id}/calendars`, { method: "PUT", body: JSON.stringify({ calendars: selectedView.calendars.map((c) => ({ calendar_id: c.calendar_id, position: c.position, color: calendarColors[c.calendar_id] || c.color })) }) }).then((res) => {
      if (res.ok) res.json().then((value) => { const updated = compositeView(value); if (updated) { setSelectedView(updated); setViews((prev) => prev.map((v) => v.id === updated.id ? updated : v)); setUnsavedCalendarChanges(false); } });
    });
  };

  const isPublished = (view: CompositeView) => publication && publication.token && view.calendars.length > 0;

  const sortedCalendars = calendars.filter((c) => !c.archived && c.access === "details").sort((a, b) => a.name.localeCompare(b.name));
  const selectedCalendars = selectedView ? [...selectedView.calendars].sort((a, b) => a.position - b.position) : [];

  const availableCalendars = sortedCalendars.filter((c) => !selectedCalendars.some((sc) => sc.calendar_id === c.id));

  if (loading) return <div className="composite-view-management"><p className="typography-body-lg" style={{ color: "var(--color-on-surface-variant)" }}>Loading…</p></div>;

  return <div className="composite-view-management">
    <div className="composite-view-management__grid">
      <div className="composite-view-management__list">
        {views.length === 0 ? <div className="composite-view-management__empty">
          <span className="material-symbols-outlined" style={{ fontSize: "48px", color: "var(--color-on-surface-variant)" }}>view_agenda</span>
          <h2 className="typography-headline-md" style={{ color: "var(--color-on-surface)", margin: "0.75rem 0 0.5rem" }}>No composite views yet.</h2>
          <p className="typography-body-sm" style={{ color: "var(--color-on-surface-variant)", margin: "0 0 1rem" }}>Create your first composite view to combine multiple calendars.</p>
          <button type="button" className="btn btn--primary" onClick={() => { setSelectedView({ id: 0, owner_user_id: 0, name: "", version: 0, created_at: 0, updated_at: 0, calendars: [] }); setEditingName(""); setCalendarColors({}); setPublication(null); setDetailLevel("full_details"); setExpiresAt(""); if (!calendarsLoaded) fetchCalendars(); }}>New composite view</button>
        </div> : <ul role="list" aria-label="Composite views" className="composite-view-management__list-items">
          {views.map((view) => <li key={view.id} className={`composite-view-management__card ${selectedView?.id === view.id ? "composite-view-management__card--active" : ""}`}>
            <div className="composite-view-management__card-content">
              <span className="typography-headline-sm" style={{ color: "var(--color-on-surface)" }}>{view.name}</span>
              <div className="composite-view-management__card-meta">
                <span className="composite-view-management__pill" style={{ backgroundColor: view.calendars.length > 0 ? view.calendars[0].color : "var(--color-outline)" }} />
                <span className="typography-label-sm" style={{ color: "var(--color-on-surface-variant)" }}>{view.calendars.length} calendar{view.calendars.length !== 1 ? "s" : ""}</span>
                {isPublished(view) ? <span className="composite-view-management__badge" style={{ color: "var(--color-primary)" }}>
                  <span className="material-symbols-outlined" style={{ fontSize: "14px" }}>visibility</span>Published
                </span> : <span className="composite-view-management__badge" style={{ color: "var(--color-on-surface-variant)" }}>
                  <span className="material-symbols-outlined" style={{ fontSize: "14px" }}>group</span>{view.calendars.length}
                </span>}
              </div>
            </div>
            <button type="button" className="btn btn--text" aria-label={`Edit ${view.name}`} onClick={() => openEditor(view)}>Edit {view.name}</button>
          </li>)}
        </ul>}
      </div>
      {selectedView && <div className="composite-view-management__editor">
        <div className="composite-view-management__editor-panel">
          <div className="composite-view-management__editor-header">
            <h2 className="typography-headline-sm" style={{ color: "var(--color-on-surface)" }}>{selectedView.id === 0 ? "New view" : "Edit view"}</h2>
          </div>
          <div className="composite-view-management__editor-form">
            <div className="composite-view-management__form-section">
              <label className="composite-view-management__label" htmlFor="view-name">View name</label>
              <input id="view-name" type="text" className="composite-view-management__input" value={editingName} onChange={(e) => setEditingName(e.target.value)} />
              {selectedView.id === 0 ? <button type="button" className="btn btn--primary btn--sm" onClick={() => { api.request("/api/v1/views", { method: "POST", body: JSON.stringify({ name: editingName }) }).then((res) => { if (res.ok) res.json().then((value) => { const created = compositeView(value); if (created) { setSelectedView(created); setViews((prev) => [...prev, created]); } }); }); }}>Create view</button> : <button type="button" className="btn btn--primary btn--sm" onClick={saveName}>Save view name</button>}
            </div>
            <div className="composite-view-management__form-section">
              <h3 className="typography-title-md" style={{ color: "var(--color-on-surface)", margin: "0 0 0.75rem" }}>Calendars</h3>
              {selectedCalendars.length === 0 ? <p className="typography-body-sm" style={{ color: "var(--color-on-surface-variant)" }}>No calendars added yet.</p> : <ul aria-label="Calendars in view" className="composite-view-management__calendar-list">
                {selectedCalendars.map((entry) => {
                  const cal = calendars.find((c) => c.id === entry.calendar_id);
                  if (!cal) return null;
                  const idx = selectedCalendars.findIndex((e) => e.calendar_id === entry.calendar_id);
                  return <li key={entry.calendar_id} className="composite-view-management__calendar-item">
                    <span className="material-symbols-outlined composite-view-management__drag-handle" style={{ color: "var(--color-on-surface-variant)" }}>drag_indicator</span>
                    <span className="composite-view-management__swatch" style={{ backgroundColor: calendarColors[entry.calendar_id] || cal.color }} />
                    <span className="typography-body-md" style={{ color: "var(--color-on-surface)" }}>{cal.name}</span>
                    <span className="composite-view-management__color-label">Color for {cal.name}</span><input type="color" className="composite-view-management__color-input" aria-label={`Color for ${cal.name}`} value={calendarColors[entry.calendar_id] || cal.color} onChange={(e) => setCalendarColors((prev) => ({ ...prev, [entry.calendar_id]: e.target.value }))} />
                    <div className="composite-view-management__calendar-actions">
                      {idx > 0 && <button type="button" className="btn btn--icon" aria-label={`Move ${cal.name} up`} onClick={() => moveCalendar(entry.calendar_id, "up")}>
                        <span className="material-symbols-outlined" style={{ fontSize: "18px" }}>arrow_upward</span>
                      </button>}
                      {idx < selectedCalendars.length - 1 && <button type="button" className="btn btn--icon" aria-label={`Move ${cal.name} down`} onClick={() => moveCalendar(entry.calendar_id, "down")}>
                        <span className="material-symbols-outlined" style={{ fontSize: "18px" }}>arrow_downward</span>
                      </button>}
                      <button type="button" className="btn btn--icon btn--danger" aria-label={`Remove ${cal.name}`} onClick={() => removeCalendar(entry.calendar_id)}>
                        <span className="material-symbols-outlined" style={{ fontSize: "18px" }}>close</span>
                      </button>
                    </div>
                  </li>;
                })}
              </ul>}
              {unsavedCalendarChanges && <div className="composite-view-management__save-bar">
                <button type="button" className="btn btn--primary btn--sm" onClick={saveCalendars}>Save view calendars</button>
              </div>}
              {availableCalendars.length > 0 && <div className="composite-view-management__add-calendar">
                <select aria-label="Add calendar" className="composite-view-management__select" onChange={(e) => { if (e.target.value) { setPendingCalendarId(Number(e.target.value)); e.target.value = ""; } }}>
                  <option value="">Add calendar</option>
                  {availableCalendars.map((cal) => <option key={cal.id} value={cal.id}>{cal.name}</option>)}
                </select>
                <button type="button" className="btn btn--secondary btn--sm" onClick={() => { if (pendingCalendarId) { addCalendar(pendingCalendarId); setPendingCalendarId(null); } }}>Add calendar to view</button>
              </div>}
            </div>
            <div className="composite-view-management__form-section">
              <h3 className="typography-title-md" style={{ color: "var(--color-on-surface)", margin: "0 0 0.75rem" }}>Publication</h3>
              <div className="composite-view-management__publication-container">
                <span className="material-symbols-outlined" style={{ fontSize: "20px", color: "var(--color-primary)" }}>public</span>
                <span className="typography-title-sm" style={{ color: "var(--color-on-surface)" }}>Make public</span>
              </div>
              <div className="composite-view-management__publication-grid">
                <div className="composite-view-management__publication-field">
                  <label className="composite-view-management__label" htmlFor="detail-level">Public detail level</label>
                  <select id="detail-level" className="composite-view-management__select" aria-label="Public detail level" value={detailLevel} onChange={(e) => setDetailLevel(e.target.value as typeof detailLevel)}>
                    <option value="full_details">Full details</option>
                    <option value="title_and_time">Title and time</option>
                    <option value="free_busy">Busy only</option>
                  </select>
                </div>
                <div className="composite-view-management__publication-field">
                  <label className="composite-view-management__label" htmlFor="expiry">Public link expires at</label>
                  <input id="expiry" type="datetime-local" className="composite-view-management__input" aria-label="Public link expires at" value={expiresAt} onChange={(e) => setExpiresAt(e.target.value)} />
                </div>
              </div>
              {!publication ? <button type="button" className="btn btn--primary" onClick={publishView}>Publish view</button> : <div className="composite-view-management__publication-actions">
                <a href={`/public/views/${publication.token}`} role="link" aria-label="Current public link" className="composite-view-management__public-link">{window.location.origin}/public/views/{publication.token}</a>
                <button type="button" className="btn btn--primary btn--sm" onClick={savePublication}>Save publication</button>
                <button type="button" className="btn btn--secondary btn--sm" onClick={rotateLink}>Rotate public link</button>
                <div className="composite-view-management__caldav-section">
                  <label className="composite-view-management__caldav-label">
                    <input type="checkbox" checked={caldavEnabled} onChange={toggleCaldav} />
                    <span>Apple Calendar (CalDAV)</span>
                  </label>
                  {caldavEnabled && publication.caldav_url && <a href={publication.caldav_url} target="_blank" rel="noreferrer" className="composite-view-management__caldav-link">Subscribe via webcal://</a>}
                </div>
              </div>}
            </div>
          </div>
        </div>
      </div>}
    </div>
  </div>;
}
