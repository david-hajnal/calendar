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
  name: "", description: "", color: "#3b82f6", default_timezone: "UTC", default_event_visibility: "private",
};

function settingsFor(calendar?: Calendar): CalendarSettings {
  return calendar === undefined ? blankSettings : {
    name: calendar.name ?? "", description: calendar.description ?? "", color: calendar.color ?? "#3b82f6",
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

const CALENDAR_COLORS = [
  "#ef4444", "#f97316", "#f59e0b", "#10b981", "#3b82f6",
  "#8b5cf6", "#ec4899", "#06b6d4", "#84cc16", "#f43f5e",
];

function initialsFor(email: string): string {
  const parts = email.split("@")[0].split(/[._-]/);
  if (parts.length >= 2) return (parts[0][0] + parts[parts.length - 1][0]).toUpperCase();
  return email.slice(0, 2).toUpperCase();
}

function roleLabel(role: string): string {
  return role === "owner" ? "Owner" : role.replaceAll("_", " ").replace(/\b\w/g, (c) => c.toUpperCase());
}

// eslint-disable-next-line @typescript-eslint/no-unused-vars
function SharingDialog({ api, calendar, onClose, onCalendarChanged, onAccessDenied, triggerEl }: { api: ApiClient; calendar: Calendar; onClose: () => void; onCalendarChanged: (calendar: Calendar) => void; onAccessDenied: () => void; triggerEl: HTMLButtonElement | null }) {
  const [entries, setEntries] = useState<CalendarAclEntry[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [userId, setUserId] = useState("");
  const [newRole, setNewRole] = useState<ShareableCalendarRole>("viewer");
  const [roleEdits, setRoleEdits] = useState<Record<number, ShareableCalendarRole>>({});
  const [transferTarget, setTransferTarget] = useState<number | null>(null);
  const closeButton = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    closeButton.current?.focus();
  }, []);

  useEffect(() => {
    let active = true;
    void listCalendarAcl(api, calendar.id).then((result) => { if (active) setEntries(result); }).catch((reason: unknown) => {
      if (!active) return;
      if (isCalendarAccessChange(reason)) onAccessDenied();
      else setError("We could not load collaborators.");
    });
    return () => { active = false; };
  }, [api, calendar.id, onAccessDenied]);

  useEffect(() => {
    const dialog = closeButton.current?.closest("[role=\"dialog\"]");
    if (!dialog) return;
    const handler = (e: Event) => {
      const keyboardEvent = e as unknown as KeyboardEvent;
      if (keyboardEvent.key === "Escape") {
        keyboardEvent.preventDefault();
        if (transferTarget !== null) setTransferTarget(null);
        else onClose();
      }
    };
    dialog.addEventListener("keydown", handler);
    return () => { dialog.removeEventListener("keydown", handler); };
  }, [transferTarget]);

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

  const ownerEntry = entries.find((e) => e.role === "owner");
  const nonOwnerEntries = entries.filter((e) => e.role !== "owner");

  return <div className="sharing-dialog" role="dialog" aria-modal="true" aria-labelledby="sharing-heading">
    <div className="sharing-dialog__card">
      <div className="sharing-dialog__header">
        <h3 id="sharing-heading">Share {calendar.name ?? "calendar"}</h3>
        <button ref={closeButton} className="sharing-dialog__close" type="button" onClick={onClose} aria-label="Close sharing">
          <span className="material-symbols-outlined">close</span>
        </button>
      </div>
      <div className="sharing-dialog__body">
        {error && <p className="sharing-dialog__error" role="alert">{error}</p>}
        <div className="sharing-dialog__invite">
          <h4 className="sharing-dialog__invite-title">Invite Collaborators</h4>
          <form className="sharing-dialog__invite-row" onSubmit={(event) => { event.preventDefault(); const id = Number(userId); if (!Number.isInteger(id) || id <= 0) { setError("Enter a valid user ID."); return; } void saveRole(id, newRole); }}>
            <div className="sharing-dialog__invite-input-wrapper">
              <span className="material-symbols-outlined">person_add</span>
              <input className="sharing-dialog__invite-input" type="number" min="1" placeholder="Enter User ID" aria-label="User ID" required value={userId} onChange={(event) => setUserId(event.target.value)} />
            </div>
            <select className="sharing-dialog__invite-select" aria-label="Role" value={newRole} onChange={(event) => setNewRole(event.target.value as ShareableCalendarRole)}>
              {shareableRoles.map((role) => <option key={role.value} value={role.value}>{role.label}</option>)}
            </select>
            <button className="sharing-dialog__send-btn" type="submit">Send Invite</button>
          </form>
        </div>
        <div>
          <h4 className="sharing-dialog__collaborators-title">Current Collaborators</h4>
          <ul className="sharing-dialog__collaborator-list">
            {ownerEntry && <li className="sharing-dialog__owner-row">
              <div className="sharing-dialog__collab-info">
                <div className="sharing-dialog__avatar sharing-dialog__avatar--owner">{initialsFor(calendar.name ?? "ME")}</div>
                <div className="sharing-dialog__collab-details">
                  <div className="sharing-dialog__collab-name">{ownerEntry.user_id}</div>
                  <div className="sharing-dialog__collab-role"><span className="dot" />Owner</div>
                </div>
              </div>
              {nonOwnerEntries.length > 0 && transferTarget === null &&
                <button className="sharing-dialog__transfer-btn" type="button" onClick={() => setTransferTarget(nonOwnerEntries.find((e) => e.role !== "owner")?.user_id ?? null)}>
                  <span className="material-symbols-outlined">swap_horiz</span> Transfer ownership
                </button>
              }
              {transferTarget !== null && <div className="sharing-dialog__confirm" role="dialog" aria-modal="true" aria-labelledby="transfer-confirmation-heading">
                <h4 id="transfer-confirmation-heading">Confirm ownership transfer</h4>
                <p>User {transferTarget} will become the owner. You will become a manager.</p>
                <div className="sharing-dialog__confirm-actions">
                  <button className="btn-discard" type="button" onClick={() => setTransferTarget(null)}>Cancel</button>
                  <button className="btn-confirm" type="button" onClick={() => void transfer()}>Confirm transfer</button>
                </div>
              </div>}
            </li>}
            {nonOwnerEntries.map((entry) => <li key={entry.user_id} className="sharing-dialog__collaborator">
              <div className="sharing-dialog__collab-info">
                <div className="sharing-dialog__avatar">{initialsFor(String(entry.user_id))}</div>
                <div className="sharing-dialog__collab-details">
                  <div className="sharing-dialog__collab-name">User {entry.user_id}</div>
                  <div className="sharing-dialog__collab-role sharing-dialog__collab-role--primary">{roleLabel(entry.role)}</div>
                </div>
              </div>
              <div className="sharing-dialog__collab-actions">
                <label className="sharing-dialog__role-label">Role for user {entry.user_id}
                  <select className="sharing-dialog__role-select" value={roleEdits[entry.user_id] ?? entry.role} onChange={(event) => setRoleEdits((current) => ({ ...current, [entry.user_id]: event.target.value as ShareableCalendarRole }))}>
                    {shareableRoles.map((role) => <option key={role.value} value={role.value}>{role.label}</option>)}
                  </select>
                </label>
                <button className="sharing-dialog__icon-btn" type="button" onClick={() => void saveRole(entry.user_id, roleEdits[entry.user_id] ?? entry.role)} aria-label={`Save role for user ${entry.user_id}`}>
                  <span className="material-symbols-outlined">save</span>
                </button>
                <button className="sharing-dialog__icon-btn sharing-dialog__icon-btn--danger" type="button" onClick={() => void revoke(entry.user_id)} aria-label={`Revoke access for user ${entry.user_id}`}>
                  <span className="material-symbols-outlined">person_remove</span>
                </button>
              </div>
            </li>)}
          </ul>
        </div>
      </div>
      <div className="sharing-dialog__footer">
        <button className="btn-cancel" type="button" onClick={onClose}>Cancel</button>
        <button className="btn-primary" type="button" onClick={onClose}>Done</button>
      </div>
    </div>
  </div>;
}

function ColorSwatchPicker({ value, onChange }: { value: string; onChange: (color: string) => void }) {
  return <div className="color-swatches-picker" role="radiogroup" aria-label="Theme color">
    {CALENDAR_COLORS.map((color) =>
      <button key={color} type="button" className={`color-swatch-picker${value === color ? " color-swatch-picker--active" : ""}`}
        style={{ background: color }} aria-pressed={value === color} role="radio"
        aria-label={`Color ${color}`} tabIndex={value === color ? 0 : -1}
        onClick={() => onChange(color)} />
    )}
  </div>;
}

function CalendarFormModal({ editing, settings, setSettings, onSave, onCancel, submitting }: {
  editing: Calendar | null;
  settings: CalendarSettings;
  setSettings: (s: CalendarSettings) => void;
  onSave: (event: FormEvent<HTMLFormElement>) => void;
  onCancel: () => void;
  submitting: boolean;
}) {
  const title = editing === null ? "New Calendar" : "Edit Calendar";
  return <div className="calendar-modal-overlay" onClick={onCancel}>
    <div className="calendar-modal" onClick={(e) => e.stopPropagation()} role="dialog" aria-modal="true" aria-labelledby="modal-heading">
      <div className="calendar-modal__header">
        <h3 id="modal-heading">{title}</h3>
        <button className="calendar-modal__close" type="button" onClick={onCancel} aria-label="Close">
          <span className="material-symbols-outlined" aria-hidden="true">close</span>
        </button>
      </div>
      <form className="calendar-form" onSubmit={onSave} aria-label={editing === null ? "Create calendar" : `Edit ${editing.name ?? "calendar"}`}>
        <div className="calendar-modal__body">
          <label>Calendar name
            <input required value={settings.name} onChange={(event) => setSettings({ ...settings, name: event.target.value })} placeholder="e.g., Team Syncs" />
          </label>
          <label>Description <span className="optional">(Optional)</span>
            <textarea value={settings.description} onChange={(event) => setSettings({ ...settings, description: event.target.value })} placeholder="What is this calendar for?" rows={3} />
          </label>
          <label>Theme Color
            <ColorSwatchPicker value={settings.color} onChange={(color) => setSettings({ ...settings, color })} />
          </label>
          <div className="calendar-modal__grid">
            <label>Timezone
              <select value={settings.default_timezone} onChange={(event) => setSettings({ ...settings, default_timezone: event.target.value })}>
                <option>Pacific Time (PT)</option>
                <option>Eastern Time (ET)</option>
                <option>UTC</option>
              </select>
            </label>
            <label>Visibility
              <select value={settings.default_event_visibility} onChange={(event) => setSettings({ ...settings, default_event_visibility: event.target.value })}>
                <option value="private">Private</option>
                <option value="default">Default</option>
                <option value="public">Public</option>
              </select>
            </label>
          </div>
        </div>
        <div className="calendar-modal__footer">
          <button className="btn-cancel" type="button" onClick={onCancel}>Cancel</button>
          <button className="btn-primary" type="submit" disabled={submitting}>{submitting ? "Saving\u2026" : editing === null ? "Create calendar" : "Save changes"}</button>
        </div>
      </form>
    </div>
  </div>;
}

function CalendarCard({ calendar, onEdit, onShare, onArchive, onDelete, triggerRef }: {
  calendar: Calendar;
  onEdit: () => void;
  onShare: () => void;
  onArchive: () => void;
  onDelete?: () => void;
  triggerRef: React.RefObject<HTMLButtonElement | null>;
}) {
  const archived = !!calendar.archived;
  const color = calendar.color || "#3b82f6";
  return <article className={`calendar-card${archived ? " calendar-card--archived" : ""}`}>
    <div className="calendar-card__accent" style={{ background: color }} />
    <div className="calendar-card__body">
      <div className="calendar-card__top">
        <div className="calendar-card__title-row">
          <div className="calendar-card__color-dot" style={{ background: color }} />
          <h3 className="calendar-card__title">{calendar.name ?? "Busy calendar"}</h3>
        </div>
        <span className={`calendar-card__badge${archived ? " calendar-card__badge--archived" : ""}`}>
          {archived ? <><span className="material-symbols-outlined">archive</span> Archived</> : roleLabel(calendar.role)}
        </span>
      </div>
      {calendar.description && <p className="calendar-card__description">{calendar.description}</p>}
      <div className="calendar-card__footer">
        <div className="calendar-card__actions">
          {canManage(calendar) && <button className="calendar-card__action-btn" type="button" onClick={onEdit} aria-label={`Edit ${calendar.name ?? "calendar"}`} title="Edit">
            <span className="material-symbols-outlined">edit</span>
          </button>}
          {canManage(calendar) && <button className="calendar-card__action-btn" type="button" onClick={(e) => { triggerRef.current = e.currentTarget; onShare(); }} aria-label={`Manage sharing for ${calendar.name ?? "calendar"}`} title="Sharing">
            <span className="material-symbols-outlined">group</span>
          </button>}
        </div>
        {archived ? <>
          {canManage(calendar) && <button className="calendar-card__restore-btn" type="button" onClick={onArchive} aria-label={`Restore ${calendar.name ?? "calendar"}`}>
            <span className="material-symbols-outlined">unarchive</span> Restore
          </button>}
          {canDelete(calendar) && <button className="calendar-card__delete-btn" type="button" onClick={onDelete} aria-label={`Delete ${calendar.name ?? "calendar"}`}>
            <span className="material-symbols-outlined">delete</span>
          </button>}
        </> : canManage(calendar) && <div className="calendar-card__hover-actions">
          <button className="calendar-card__action-btn" type="button" onClick={onArchive} aria-label={`Archive ${calendar.name ?? "calendar"}`} title="Archive">
            <span className="material-symbols-outlined">archive</span>
          </button>
        </div>}
      </div>
    </div>
  </article>;
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
  const prevSharingRef = useRef<Calendar | null>(null);

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

  useEffect(() => {
    if (prevSharingRef.current !== null && sharing === null) {
      prevSharingRef.current = null;
      shareTrigger.current?.focus();
    }
    prevSharingRef.current = sharing;
  }, [sharing]);

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

  if (loading) return <section aria-busy="true"><p className="calendar-loading" role="status">Loading calendars\u2026</p></section>;

  const activeCalendars = calendars.filter((c) => !c.archived);
  const archivedCalendars = calendars.filter((c) => c.archived);

  return <section className="calendar-management" aria-labelledby="calendars-heading">
    <header className="calendar-management__header">
      <div>
        <h2 id="calendars-heading">Calendars</h2>
        <p>Manage your calendars, sharing settings, and default behaviors.</p>
      </div>
      <button className="calendar-management__new-btn" type="button" onClick={openCreate}>
        <span className="material-symbols-outlined">add</span>
        New calendar
      </button>
    </header>

    {error && <p className="app-message app-message--error" role="alert">{error}</p>}

    {editing !== undefined && <CalendarFormModal editing={editing} settings={settings} setSettings={setSettings} onSave={save} onCancel={() => setEditing(undefined)} submitting={submitting} />}

    {activeCalendars.length > 0 && <h3 className="calendar-section-title">Active Calendars</h3>}
    <div className="calendar-grid">
      {activeCalendars.map((calendar) => <CalendarCard key={calendar.id} calendar={calendar}
        onEdit={() => openEdit(calendar)}
        onShare={() => { setSharing(calendar); }}
        onArchive={() => void setArchived(calendar, true)}
        triggerRef={shareTrigger}
      />)}
    </div>

    {archivedCalendars.length > 0 && <h3 className="calendar-section-title">Archived Calendars</h3>}
    <div className="calendar-grid">
      {archivedCalendars.map((calendar) => <CalendarCard key={calendar.id} calendar={calendar}
        onEdit={() => openEdit(calendar)}
        onShare={() => { setSharing(calendar); }}
        onArchive={() => void setArchived(calendar, false)}
        triggerRef={shareTrigger}
        onDelete={() => void deleteCalendar(calendar)}
      />)}
    </div>

    {calendars.length === 0 && <div className="calendar-empty">
      <div className="calendar-empty__icon"><span className="material-symbols-outlined">calendar_month</span></div>
      <h3>No calendars yet.</h3>
      <p>Create your first calendar to start managing events.</p>
      <button className="calendar-management__new-btn" type="button" onClick={openCreate} aria-label="Create your first calendar">
        <span className="material-symbols-outlined">add</span>
        New calendar
      </button>
    </div>}

    {sharing && <SharingDialog api={api} calendar={sharing} onClose={() => setSharing(null)} onCalendarChanged={(calendar) => { replace(calendar); setSharing(calendar); }} onAccessDenied={() => { void refreshAfterAccessChange(); }} triggerEl={shareTrigger.current} />}
  </section>;
}
