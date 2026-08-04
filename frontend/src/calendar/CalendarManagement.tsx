import { useCallback, useEffect, useRef, useState, type FormEvent, type KeyboardEvent } from "react";

import type { ApiClient } from "../auth/api";
import { archiveCalendar, createCalendar, deleteCalendar as deleteCalendarRequest, isCalendarAccessChange, listCalendarAcl, listCalendars, restoreCalendar, revokeCalendarAcl, setCalendarAcl, transferCalendarOwnership, updateCalendar, type CalendarAclEntry, type ShareableCalendarRole } from "./api";
import "./CalendarManagement.css";

export type CalendarRole = "owner" | "manager" | "editor" | "viewer" | "free_busy_viewer";

export interface Calendar {
  id: number;
  access: "details" | "free_busy";
  role: CalendarRole;
  owner_user_id?: number;
  name?: string;
  description?: string | null;
  color?: string;
  default_timezone?: string;
  default_event_visibility?: string;
  default_notification_rules_json?: string | null;
  archived?: boolean;
  version?: number;
}

interface CalendarSettings {
  name: string;
  description: string;
  color: string;
  default_timezone: string;
  default_event_visibility: string;
}

const blankSettings: CalendarSettings = {
  name: "", description: "", color: "#2563eb", default_timezone: "UTC", default_event_visibility: "private",
};

function settingsFor(calendar?: Calendar): CalendarSettings {
  return calendar === undefined ? blankSettings : {
    name: calendar.name ?? "", description: calendar.description ?? "", color: calendar.color ?? "#2563eb",
    default_timezone: calendar.default_timezone ?? "UTC", default_event_visibility: calendar.default_event_visibility ?? "private",
  };
}

function payload(settings: CalendarSettings) {
  return { ...settings, description: settings.description || null, default_notification_rules_json: null };
}

function canManage(calendar: Calendar) {
  return calendar.access === "details" && (calendar.role === "owner" || calendar.role === "manager");
}

function canDelete(calendar: Calendar) {
  return calendar.access === "details" && calendar.role === "owner";
}

const shareableRoles: { value: ShareableCalendarRole; label: string }[] = [
  { value: "manager", label: "Manager" }, { value: "editor", label: "Editor" }, { value: "viewer", label: "Viewer" }, { value: "free_busy_viewer", label: "Free/busy viewer" },
];

function SharingDialog({ api, calendar, onClose, onCalendarChanged, onAccessDenied }: { api: ApiClient; calendar: Calendar; onClose: () => void; onCalendarChanged: (calendar: Calendar) => void; onAccessDenied: () => void }) {
  const [entries, setEntries] = useState<CalendarAclEntry[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [userId, setUserId] = useState("");
  const [newRole, setNewRole] = useState<ShareableCalendarRole>("viewer");
  const [roleEdits, setRoleEdits] = useState<Record<number, ShareableCalendarRole>>({});
  const [transferTarget, setTransferTarget] = useState<number | null>(null);
  const closeButton = useRef<HTMLButtonElement>(null);
  const owner = calendar.role === "owner";

  useEffect(() => {
    closeButton.current?.focus();
    let active = true;
    void listCalendarAcl(api, calendar.id).then((result) => { if (active) setEntries(result); }).catch((reason: unknown) => {
      if (!active) return;
      if (isCalendarAccessChange(reason)) onAccessDenied();
      else setError("We could not load collaborators.");
    });
    return () => { active = false; };
  }, [api, calendar.id, onAccessDenied]);

  function onKeyDown(event: KeyboardEvent<HTMLDivElement>) {
    if (event.key === "Escape") {
      event.preventDefault();
      if (transferTarget !== null) setTransferTarget(null);
      else onClose();
    }
  }

  async function saveRole(entryUserId: number, role: ShareableCalendarRole) {
    setError(null);
    try {
      const updated = await setCalendarAcl(api, calendar.id, entryUserId, role);
      setEntries((current) => current.some((entry) => entry.user_id === entryUserId) ? current.map((entry) => entry.user_id === entryUserId ? updated : entry) : [...current, updated]);
      setRoleEdits((current) => ({ ...current, [entryUserId]: updated.role as ShareableCalendarRole }));
    } catch (reason) { if (isCalendarAccessChange(reason)) onAccessDenied(); else setError("We could not save this collaborator's role."); }
  }

  async function revoke(entryUserId: number) {
    setError(null);
    try { await revokeCalendarAcl(api, calendar.id, entryUserId); setEntries((current) => current.filter((entry) => entry.user_id !== entryUserId)); }
    catch (reason) { if (isCalendarAccessChange(reason)) onAccessDenied(); else setError("We could not revoke this collaborator's access."); }
  }

  async function transfer() {
    if (transferTarget === null || calendar.version === undefined) return;
    setError(null);
    try {
      const updated = await transferCalendarOwnership(api, calendar.id, transferTarget, calendar.version);
      onCalendarChanged(updated);
      onClose();
    } catch (reason) { if (isCalendarAccessChange(reason)) onAccessDenied(); else setError("We could not transfer ownership."); }
  }

  return <div className="calendar-dialog" role="dialog" aria-modal="true" aria-labelledby="sharing-heading" onKeyDown={onKeyDown}>
    <h3 id="sharing-heading">Share {calendar.name ?? "calendar"}</h3>
    <button ref={closeButton} type="button" onClick={onClose}>Close sharing</button>
    {error && <p role="alert">{error}</p>}
    <form className="calendar-form" onSubmit={(event) => { event.preventDefault(); const id = Number(userId); if (!Number.isInteger(id) || id <= 0) { setError("Enter a valid user ID."); return; } void saveRole(id, newRole); }} aria-label="Add collaborator">
      <label>User ID<input type="number" min="1" required value={userId} onChange={(event) => setUserId(event.target.value)} /></label>
      <label>Role<select value={newRole} onChange={(event) => setNewRole(event.target.value as ShareableCalendarRole)}>{shareableRoles.map((role) => <option key={role.value} value={role.value}>{role.label}</option>)}</select></label>
      <button type="submit">Add collaborator</button>
    </form>
    <ul aria-label="Collaborators">{entries.map((entry) => <li key={entry.user_id}>
      <strong>User {entry.user_id}</strong>{entry.role === "owner" ? <span> — Owner</span> : <>
        <label>Role for user {entry.user_id}<select value={roleEdits[entry.user_id] ?? entry.role} onChange={(event) => setRoleEdits((current) => ({ ...current, [entry.user_id]: event.target.value as ShareableCalendarRole }))}>{shareableRoles.map((role) => <option key={role.value} value={role.value}>{role.label}</option>)}</select></label>
        <button type="button" onClick={() => void saveRole(entry.user_id, roleEdits[entry.user_id] ?? entry.role)}>Save role for user {entry.user_id}</button>
        <button type="button" onClick={() => void revoke(entry.user_id)}>Revoke access for user {entry.user_id}</button>
      </>}</li>)}</ul>
    {owner && <section aria-labelledby="transfer-heading"><h4 id="transfer-heading">Transfer ownership</h4>
      <p>Transferring ownership makes you a manager.</p>
      {transferTarget === null ? <button type="button" onClick={() => setTransferTarget(entries.find((entry) => entry.role !== "owner")?.user_id ?? null)} disabled={!entries.some((entry) => entry.role !== "owner")}>Transfer ownership</button> :
        <div className="calendar-dialog calendar-dialog--nested" role="dialog" aria-modal="true" aria-labelledby="transfer-confirmation-heading"><h4 id="transfer-confirmation-heading">Confirm ownership transfer</h4><p>User {transferTarget} will become the owner.</p><button type="button" onClick={() => void transfer()}>Confirm transfer</button><button type="button" onClick={() => setTransferTarget(null)}>Cancel transfer</button></div>}
    </section>}
  </div>;
}

export function CalendarManagement({ api }: { api: ApiClient }) {
  const [calendars, setCalendars] = useState<Calendar[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [editing, setEditing] = useState<Calendar | null | undefined>(undefined);
  const [settings, setSettings] = useState<CalendarSettings>(blankSettings);
  const [submitting, setSubmitting] = useState(false);
  const [sharing, setSharing] = useState<Calendar | null>(null);
  const shareTrigger = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    let active = true;
    void (async () => {
      try {
        const result = await listCalendars(api);
        if (active) setCalendars(result);
      } catch {
        if (active) setError("We could not load your calendars. Please try again.");
      } finally {
        if (active) setLoading(false);
      }
    })();
    return () => { active = false; };
  }, [api]);

  function openCreate() { setError(null); setSettings(blankSettings); setEditing(null); }
  function openEdit(calendar: Calendar) { setError(null); setSettings(settingsFor(calendar)); setEditing(calendar); }
  function replace(calendar: Calendar) { setCalendars((current) => current.map((item) => item.id === calendar.id ? calendar : item)); }
  const refreshAfterAccessChange = useCallback(async () => {
    setEditing(undefined); setSharing(null);
    try { setCalendars(await listCalendars(api)); }
    catch { setCalendars([]); }
    setError("Your calendar access changed. The list was refreshed.");
  }, [api]);

  async function save(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    setSubmitting(true); setError(null);
    const creating = editing === null;
    try {
      const calendar = creating
        ? await createCalendar(api, payload(settings))
        : await updateCalendar(api, editing!.id, { ...payload(settings), version: editing!.version! });
      setCalendars((current) => creating ? [...current, calendar] : current.map((item) => item.id === calendar.id ? calendar : item));
      setEditing(undefined);
    } catch (reason) {
      if (isCalendarAccessChange(reason)) await refreshAfterAccessChange();
      else setError(creating ? "We could not create this calendar." : "We could not save this calendar.");
    } finally { setSubmitting(false); }
  }

  async function setArchived(calendar: Calendar, archived: boolean) {
    setError(null);
    try {
      const updated = archived ? await archiveCalendar(api, calendar.id) : await restoreCalendar(api, calendar.id);
      replace(updated);
    } catch (reason) { if (isCalendarAccessChange(reason)) await refreshAfterAccessChange(); else setError(`We could not ${archived ? "archive" : "restore"} this calendar.`); }
  }

  async function deleteCalendar(calendar: Calendar) {
    setError(null);
    try {
      await deleteCalendarRequest(api, calendar.id);
      setCalendars((current) => current.filter((item) => item.id !== calendar.id));
    } catch (reason) { if (isCalendarAccessChange(reason)) await refreshAfterAccessChange(); else setError("We could not delete this calendar."); }
  }

  if (loading) return <section aria-busy="true"><p role="status">Loading calendars…</p></section>;
  return <section className="calendar-management" aria-labelledby="calendars-heading">
    <header><h2 id="calendars-heading">Calendars</h2><button type="button" onClick={openCreate}>New calendar</button></header>
    {error && <p role="alert">{error}</p>}
    {editing !== undefined && <form className="calendar-form" onSubmit={save} aria-label={editing === null ? "Create calendar" : `Edit ${editing.name ?? "calendar"}`}>
      <h3>{editing === null ? "Create calendar" : "Edit calendar"}</h3>
      <label>Calendar name<input required value={settings.name} onChange={(event) => setSettings({ ...settings, name: event.target.value })} /></label>
      <label>Description<textarea value={settings.description} onChange={(event) => setSettings({ ...settings, description: event.target.value })} /></label>
      <label>Color<input type="color" value={settings.color} onChange={(event) => setSettings({ ...settings, color: event.target.value })} /></label>
      <label>Time zone<input required value={settings.default_timezone} onChange={(event) => setSettings({ ...settings, default_timezone: event.target.value })} /></label>
      <label>Default visibility<select value={settings.default_event_visibility} onChange={(event) => setSettings({ ...settings, default_event_visibility: event.target.value })}><option value="private">Private</option><option value="default">Default</option></select></label>
      <button type="submit" disabled={submitting}>{submitting ? "Saving…" : editing === null ? "Create calendar" : "Save changes"}</button>
      <button type="button" onClick={() => setEditing(undefined)} disabled={submitting}>Cancel</button>
    </form>}
    {calendars.length === 0 ? <p>No calendars yet.</p> : <ul>
      {calendars.map((calendar) => <li key={calendar.id}>
        <article>
          <h3>{calendar.name ?? "Busy calendar"}</h3>
          {calendar.description && <p>{calendar.description}</p>}
          <p>Role: {calendar.role.replaceAll("_", " ")}{calendar.archived ? " · Archived" : ""}</p>
          {canManage(calendar) && <><button type="button" aria-label={`Edit ${calendar.name ?? "calendar"}`} onClick={() => openEdit(calendar)}>Edit</button>
            <button type="button" aria-label={`${calendar.archived ? "Restore" : "Archive"} ${calendar.name ?? "calendar"}`} onClick={() => void setArchived(calendar, !calendar.archived)}>{calendar.archived ? "Restore" : "Archive"}</button></>}
          {canManage(calendar) && <button type="button" aria-label={`Manage sharing for ${calendar.name ?? "calendar"}`} onClick={(event) => { shareTrigger.current = event.currentTarget; setSharing(calendar); }}>Sharing</button>}
          {canDelete(calendar) && <button type="button" aria-label={`Delete ${calendar.name ?? "calendar"}`} onClick={() => void deleteCalendar(calendar)}>Delete</button>}
        </article>
      </li>)}
    </ul>}
    {sharing && <SharingDialog api={api} calendar={sharing} onClose={() => { setSharing(null); queueMicrotask(() => shareTrigger.current?.focus()); }} onCalendarChanged={(calendar) => { replace(calendar); setSharing(calendar); }} onAccessDenied={() => { void refreshAfterAccessChange(); }} />}
  </section>;
}
