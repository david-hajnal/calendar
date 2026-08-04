import { useEffect, useState, type FormEvent } from "react";

import type { ApiClient } from "../auth/api";
import type { Calendar } from "./CalendarManagement";
import { configureCompositeViewPublication, createCompositeView, createCompositeViewPublication, listCalendars, listCompositeViews, replaceCompositeViewCalendars, rotateCompositeViewPublication, updateCompositeView, type CompositeView, type CompositeViewCalendar, type PublicViewConfiguration, type PublicViewProjection } from "./api";

interface EditorState {
  view: CompositeView | null;
  name: string;
  calendars: CompositeViewCalendar[];
}

function editableCalendars(calendars: Calendar[]) {
  return calendars.filter((calendar) => calendar.access === "details" && calendar.name !== undefined && calendar.color !== undefined);
}

function editorFor(view: CompositeView | null): EditorState {
  return { view, name: view?.name ?? "", calendars: view ? [...view.calendars].sort((left, right) => left.position - right.position) : [] };
}

function renumber(calendars: CompositeViewCalendar[]) {
  return calendars.map((calendar, position) => ({ ...calendar, position }));
}

function expirationInput(expiresAt: number) {
  return new Date(expiresAt * 1_000).toISOString().slice(0, 16);
}

function publicationConfiguration({ projection, display_timezone, expires_at }: PublicViewConfiguration): PublicViewConfiguration {
  return { projection, display_timezone, expires_at };
}

export function CompositeViewManagement({ api }: { api: ApiClient }) {
  const [views, setViews] = useState<CompositeView[]>([]);
  const [calendars, setCalendars] = useState<Calendar[]>([]);
  const [editor, setEditor] = useState<EditorState | null>(null);
  const [adding, setAdding] = useState("");
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const [publication, setPublication] = useState<PublicViewConfiguration>({ projection: "full_details", display_timezone: "UTC", expires_at: Math.floor(Date.now() / 1_000) + 7 * 24 * 60 * 60 });
  const [publicToken, setPublicToken] = useState<string | null>(null);

  useEffect(() => {
    let active = true;
    void Promise.all([listCompositeViews(api), listCalendars(api)]).then(([nextViews, nextCalendars]) => {
      if (!active) return;
      setViews(nextViews);
      setCalendars(editableCalendars(nextCalendars));
    }).catch(() => { if (active) setError("We could not load your composite views."); }).finally(() => { if (active) setLoading(false); });
    return () => { active = false; };
  }, [api]);

  function replace(view: CompositeView) {
    setViews((current) => current.some((item) => item.id === view.id) ? current.map((item) => item.id === view.id ? view : item) : [...current, view]);
  }

  async function saveName(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (editor === null) return;
    setSaving(true); setError(null);
    try {
      const saved = editor.view === null ? await createCompositeView(api, { name: editor.name }) : await updateCompositeView(api, editor.view.id, { name: editor.name });
      replace(saved);
      setEditor(editorFor(saved));
    } catch { setError("We could not save this composite view."); }
    finally { setSaving(false); }
  }

  async function saveCalendars() {
    if (editor?.view === null || editor === null) return;
    setSaving(true); setError(null);
    try {
      const saved = await replaceCompositeViewCalendars(api, editor.view.id, { calendars: renumber(editor.calendars) });
      replace(saved);
      setEditor(editorFor(saved));
    } catch { setError("We could not save this view's calendars."); }
    finally { setSaving(false); }
  }

  async function publish() {
    if (editor?.view === null || editor === null) return;
    setSaving(true); setError(null);
    try {
      const issued = await createCompositeViewPublication(api, editor.view.id, publication);
      setPublication(publicationConfiguration(issued));
      setPublicToken(issued.token);
    } catch { setError("We could not publish this view."); }
    finally { setSaving(false); }
  }

  async function rotatePublication() {
    if (editor?.view === null || editor === null) return;
    setSaving(true); setError(null);
    try {
      const issued = await rotateCompositeViewPublication(api, editor.view.id);
      setPublication(publicationConfiguration(issued));
      setPublicToken(issued.token);
    } catch { setError("We could not rotate this public link."); }
    finally { setSaving(false); }
  }

  async function savePublication() {
    if (editor?.view === null || editor === null) return;
    setSaving(true); setError(null);
    try {
      const saved = await configureCompositeViewPublication(api, editor.view.id, publication);
      setPublication(publicationConfiguration(saved));
    } catch { setError("We could not update this public link."); }
    finally { setSaving(false); }
  }

  function addCalendar() {
    if (editor === null || !adding) return;
    const calendar = calendars.find((item) => item.id === Number(adding));
    if (!calendar?.color || editor.calendars.some((item) => item.calendar_id === calendar.id)) return;
    setEditor({ ...editor, calendars: [...editor.calendars, { calendar_id: calendar.id, position: editor.calendars.length, color: calendar.color }] });
    setAdding("");
  }

  function move(calendarId: number, direction: -1 | 1) {
    if (editor === null) return;
    const index = editor.calendars.findIndex((calendar) => calendar.calendar_id === calendarId);
    const destination = index + direction;
    if (index < 0 || destination < 0 || destination >= editor.calendars.length) return;
    const next = [...editor.calendars];
    [next[index], next[destination]] = [next[destination], next[index]];
    setEditor({ ...editor, calendars: renumber(next) });
  }

  if (loading) return <section aria-busy="true"><p role="status">Loading composite views…</p></section>;
  return <section aria-labelledby="composite-views-heading">
    <header><h2 id="composite-views-heading">Composite views</h2><button type="button" onClick={() => { setError(null); setEditor(editorFor(null)); }}>New composite view</button></header>
    {error && <p role="alert">{error}</p>}
    {views.length === 0 ? <p>No composite views yet.</p> : <ul>{views.map((view) => <li key={view.id}><strong>{view.name}</strong> <button type="button" aria-label={`Edit ${view.name}`} onClick={() => { setError(null); setEditor(editorFor(view)); }}>Edit</button></li>)}</ul>}
    {editor && <section aria-labelledby="composite-view-editor-heading">
      <h3 id="composite-view-editor-heading">{editor.view === null ? "Create composite view" : `Edit ${editor.view.name}`}</h3>
      <form onSubmit={saveName} aria-label="Composite view name"><label>View name<input required value={editor.name} onChange={(event) => setEditor({ ...editor, name: event.target.value })} /></label><button type="submit" disabled={saving}>{editor.view === null ? "Create view" : "Save view name"}</button></form>
      {editor.view !== null && <>
        <label>Add calendar<select value={adding} onChange={(event) => setAdding(event.target.value)}><option value="">Select a calendar</option>{calendars.filter((calendar) => !editor.calendars.some((source) => source.calendar_id === calendar.id)).map((calendar) => <option key={calendar.id} value={calendar.id}>{calendar.name}</option>)}</select></label><button type="button" onClick={addCalendar} disabled={!adding}>Add calendar to view</button>
        <ul aria-label="View calendars">{editor.calendars.map((source, index) => { const calendar = calendars.find((item) => item.id === source.calendar_id); const name = calendar?.name ?? `Calendar ${source.calendar_id}`; return <li key={source.calendar_id}><strong>{name}</strong><label>Color for {name}<input type="color" value={source.color} onChange={(event) => setEditor({ ...editor, calendars: editor.calendars.map((item) => item.calendar_id === source.calendar_id ? { ...item, color: event.target.value } : item) })} /></label><button type="button" aria-label={`Move ${name} up`} disabled={index === 0} onClick={() => move(source.calendar_id, -1)}>Up</button><button type="button" aria-label={`Move ${name} down`} disabled={index === editor.calendars.length - 1} onClick={() => move(source.calendar_id, 1)}>Down</button></li>; })}</ul>
        <button type="button" onClick={() => void saveCalendars()} disabled={saving}>Save view calendars</button>
        <section aria-labelledby="publication-heading">
          <h4 id="publication-heading">Public link</h4>
          <label>Public detail level<select value={publication.projection} onChange={(event) => setPublication({ ...publication, projection: event.target.value as PublicViewProjection })}><option value="full_details">Full details</option><option value="title_and_time">Title and time</option><option value="free_busy">Free/busy only</option></select></label>
          <label>Public link expires at<input type="datetime-local" value={expirationInput(publication.expires_at)} onChange={(event) => setPublication({ ...publication, expires_at: Math.floor(new Date(event.target.value).getTime() / 1_000) })} /></label>
          <button type="button" onClick={() => void (publicToken ? savePublication() : publish())} disabled={saving}>{publicToken ? "Save publication" : "Publish view"}</button>
          {publicToken && <><a aria-label="Current public link" href={`/public/views/${publicToken}`}>Current public link</a><button type="button" onClick={() => void rotatePublication()} disabled={saving}>Rotate public link</button></>}
        </section>
      </>}
      <button type="button" onClick={() => setEditor(null)} disabled={saving}>Close editor</button>
    </section>}
  </section>;
}
