import { useCallback, useEffect, useState, type FormEvent } from "react";

import type { ApiClient } from "../auth/api";
import { useAuth } from "../auth/session";
import {
  createAppleCalendarPassword,
  disconnectAppleCalendar,
  getAppleCalendarConnection,
  revokeAppleCalendarPassword,
  type AppleCalendarConnection,
  type IssuedCalendarPassword,
} from "./calendarConnectionsApi";
import "./CalendarConnections.css";

export function CalendarConnections({ api }: { api: ApiClient }) {
  const { state } = useAuth();
  const username = state.status === "authenticated" ? state.session.user.email : "";
  const [connection, setConnection] = useState<AppleCalendarConnection | null>(null);
  const [loading, setLoading] = useState(true);
  const [loadError, setLoadError] = useState(false);
  const [label, setLabel] = useState("");
  const [creating, setCreating] = useState(false);
  const [issued, setIssued] = useState<IssuedCalendarPassword | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);

  const load = useCallback(async () => {
    setLoadError(false);
    setLoading(true);
    try {
      setConnection(await getAppleCalendarConnection(api));
    } catch {
      setLoadError(true);
    } finally {
      setLoading(false);
    }
  }, [api]);

  useEffect(() => {
    void load();
  }, [load]);

  async function generate(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const trimmed = label.trim();
    if (!trimmed) return;
    setCreating(true);
    setActionError(null);
    setIssued(null);
    try {
      const result = await createAppleCalendarPassword(api, trimmed);
      setIssued(result);
      setLabel("");
      setCopied(false);
      await load();
    } catch {
      setActionError("We could not generate a password. Please try again.");
    } finally {
      setCreating(false);
    }
  }

  async function revoke(id: number) {
    setActionError(null);
    try {
      await revokeAppleCalendarPassword(api, id);
      await load();
    } catch {
      setActionError("We could not revoke that password. Please try again.");
    }
  }

  async function disconnect() {
    if (!window.confirm("Disconnect Apple Calendar and revoke all connection passwords?")) return;
    setActionError(null);
    try {
      await disconnectAppleCalendar(api);
      setIssued(null);
      await load();
    } catch {
      setActionError("We could not disconnect Apple Calendar. Please try again.");
    }
  }

  async function copyPassword() {
    if (!issued) return;
    try {
      await navigator.clipboard.writeText(issued.clear_password);
      setCopied(true);
    } catch {
      setCopied(false);
    }
  }

  if (loading && connection === null) {
    return (
      <section className="calendar-connections" aria-labelledby="calendar-connections-heading" aria-busy="true">
        <h2 id="calendar-connections-heading">Apple Calendar</h2>
        <p className="app-message app-message--status" role="status">
          Loading your Apple Calendar connection…
        </p>
      </section>
    );
  }

  if (loadError) {
    return (
      <section className="calendar-connections" aria-labelledby="calendar-connections-heading">
        <h2 id="calendar-connections-heading">Apple Calendar</h2>
        <p className="app-message app-message--error" role="alert">
          We could not load your Apple Calendar connection.
        </p>
        <button className="app-button app-button--primary" type="button" onClick={() => void load()}>
          Retry
        </button>
      </section>
    );
  }

  return (
    <section className="calendar-connections" aria-labelledby="calendar-connections-heading">
      <h2 id="calendar-connections-heading">Apple Calendar</h2>
      <p className="calendar-connections__intro">
        Connect Apple Calendar to sync your Happening calendars.
      </p>

      <div className="calendar-connections__details">
        <div className="calendar-connections__field">
          <span>Server</span>
          <code>{connection?.server_url ?? "…"}</code>
        </div>
        <div className="calendar-connections__field">
          <span>Username</span>
          <code>{username}</code>
        </div>
      </div>

      {issued && (
        <div className="calendar-connections__secret" role="region" aria-label="New connection password">
          <p className="calendar-connections__secret-warning" role="alert">
            Copy this password now. It is shown only once and cannot be retrieved again.
          </p>
          <div className="calendar-connections__secret-row">
            <code className="calendar-connections__secret-value">{issued.clear_password}</code>
            <button className="app-button" type="button" onClick={() => void copyPassword()}>
              {copied ? "Copied" : "Copy"}
            </button>
          </div>
        </div>
      )}

      <form className="calendar-connections__form" onSubmit={generate}>
        <label className="calendar-connections__label" htmlFor="connection-label">
          <span>Device label</span>
          <input
            id="connection-label"
            type="text"
            value={label}
            placeholder="e.g. My iPhone"
            maxLength={120}
            onChange={(event) => setLabel(event.target.value)}
          />
        </label>
        <button
          className="app-button app-button--primary"
          type="submit"
          disabled={creating || !label.trim()}
        >
          {creating ? "Generating…" : "Generate password"}
        </button>
      </form>

      {actionError && (
        <p className="app-message app-message--error" role="alert">
          {actionError}
        </p>
      )}

      <h3 className="calendar-connections__section-title">Connected devices</h3>
      {connection && connection.credentials.length > 0 ? (
        <ul className="calendar-connections__list">
          {connection.credentials.map((credential) => (
            <li key={credential.id} className="calendar-connections__item">
              <div className="calendar-connections__item-info">
                <span className="calendar-connections__item-label">{credential.label}</span>
                <span className="calendar-connections__item-meta">
                  {credential.last_used_at
                    ? `Last used ${new Date(credential.last_used_at * 1000).toLocaleString()}`
                    : "Never used"}
                </span>
              </div>
              <button
                className="app-button calendar-connections__revoke"
                type="button"
                onClick={() => void revoke(credential.id)}
              >
                Revoke
              </button>
            </li>
          ))}
        </ul>
      ) : (
        <p className="calendar-connections__empty">No connected devices yet.</p>
      )}

      <button
        className="app-button calendar-connections__disconnect"
        type="button"
        onClick={() => void disconnect()}
      >
        Disconnect Apple Calendar
      </button>
    </section>
  );
}
