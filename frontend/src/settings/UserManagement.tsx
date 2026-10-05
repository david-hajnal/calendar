import { useCallback, useEffect, useRef, useState, type FormEvent } from "react";
import type { ApiClient } from "../auth/api";
import { AdminEmailChangeError, changeUserEmail, disableUser, inviteUser, listUsers, resendInvitation, type ManagedUser, type UserPage } from "./accountApi";

export function UserManagement({ api, currentUserId }: { api: ApiClient; currentUserId: number }) {
  const [users, setUsers] = useState<UserPage | null>(null);
  const [page, setPage] = useState(1);
  const [email, setEmail] = useState("");
  const [loadError, setLoadError] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [message, setMessage] = useState<string | null>(null);
  const [submitting, setSubmitting] = useState(false);
  const [resending, setResending] = useState<number | null>(null);
  const [disableTarget, setDisableTarget] = useState<ManagedUser | null>(null);
  const [disabling, setDisabling] = useState(false);
  const cancelButton = useRef<HTMLButtonElement>(null);
  const disableTrigger = useRef<HTMLButtonElement | null>(null);
  const [emailTarget, setEmailTarget] = useState<ManagedUser | null>(null);
  const [newEmail, setNewEmail] = useState("");
  const [changingEmail, setChangingEmail] = useState(false);
  const emailInput = useRef<HTMLInputElement>(null);
  const emailTrigger = useRef<HTMLButtonElement | null>(null);
  const busy = submitting || resending !== null || disabling || changingEmail;
  useEffect(() => { if (emailTarget) emailInput.current?.focus(); else emailTrigger.current?.focus(); }, [emailTarget]);
  useEffect(() => { if (disableTarget) cancelButton.current?.focus(); else disableTrigger.current?.focus(); }, [disableTarget]);
  const [refresh, setRefresh] = useState(0);
  const reload = useCallback(() => setRefresh((value) => value + 1), []);
  useEffect(() => {
    let active = true;
    void listUsers(api, page).then((data) => { if (active) { setUsers(data); setLoadError(null); } })
      .catch((reason: unknown) => { if (active) setLoadError(reason instanceof Error ? reason.message : "We could not load users."); });
    return () => { active = false; };
  }, [api, page, refresh]);

  async function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault(); setSubmitting(true); setError(null); setMessage(null);
    try {
      await inviteUser(api, email);
      setMessage(`Invitation sent to ${email.trim().toLowerCase()}.`);
      setEmail(""); setPage(1); reload();
    } catch (reason) { setError(reason instanceof Error ? reason.message : "The invitation could not be sent."); }
    finally { setSubmitting(false); reload(); }
  }
  async function resend(user: ManagedUser) {
    if (user.invitation_id == null) return;
    setResending(user.id); setError(null); setMessage(null);
    try { await resendInvitation(api, user.invitation_id); setMessage(`Invitation sent to ${user.email}. The previous link no longer works.`); }
    catch (reason) { setError(reason instanceof Error ? reason.message : "The invitation could not be sent."); }
    finally { setResending(null); reload(); }
  }
  function cancelDisable() {
    setDisableTarget(null);
  }
  async function confirmDisable() {
    if (!disableTarget) return;
    setDisabling(true); setError(null); setMessage(null);
    try { await disableUser(api, disableTarget.id); setMessage(`Access disabled for ${disableTarget.email}.`); setDisableTarget(null); }
    catch (reason) { setError(reason instanceof Error ? reason.message : "The account could not be disabled."); }
    finally { setDisabling(false); reload(); }
  }
  async function submitEmailChange(event: FormEvent<HTMLFormElement>) {
    event.preventDefault(); if (!emailTarget) return;
    setChangingEmail(true); setError(null); setMessage(null);
    try {
      await changeUserEmail(api,emailTarget.id,newEmail);
      setMessage(emailTarget.status === "invited" ? `Invitation sent to ${newEmail.trim().toLowerCase()}. The previous address and link no longer work.` : `Confirmation sent to ${newEmail.trim().toLowerCase()}. ${emailTarget.email} stays active until confirmed.`);
      setEmailTarget(null); setNewEmail("");
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : "Could not change email.");
      if (reason instanceof AdminEmailChangeError && reason.invitedAddressChanged) setEmailTarget(null);
    } finally { setChangingEmail(false); reload(); }
  }
  return <section aria-labelledby="users-heading">
    <h2 id="users-heading">Users</h2>
    <p>Invite people to create a password and join CommonCal.</p>
    <form className="auth-form settings-invite" onSubmit={(event) => void submit(event)}>
      <label className="auth-form__field" htmlFor="invite-email"><span>Email address</span>
        <input id="invite-email" type="email" autoComplete="email" required maxLength={254} value={email} onChange={(event) => setEmail(event.target.value)} />
      </label>
      <button className="app-button app-button--primary" disabled={busy} type="submit">{submitting ? "Sending…" : "Send invitation"}</button>
    </form>
    {disableTarget && <section className="settings-notice" role="group" aria-labelledby="disable-heading">
      <h3 id="disable-heading">Disable user?</h3>
      <p>Disable access for <strong>{disableTarget.email}</strong>? This ends browser sessions and revokes invitations and connected calendar and MCP credentials.</p>
      <p>The account and calendars are retained.</p>
      <div className="settings-actions">
        <button ref={cancelButton} className="app-button" type="button" disabled={disabling} onClick={cancelDisable}>Cancel</button>
        <button className="app-button app-button--primary" type="button" disabled={busy} onClick={() => void confirmDisable()}>{disabling ? "Disabling…" : "Confirm disable"}</button>
      </div>
    </section>}
    {emailTarget && <section className="settings-notice" aria-labelledby="change-email-heading">
      <h3 id="change-email-heading">Change email for {emailTarget.email}</h3>
      <p>{emailTarget.status === "invited" ? "The new address receives a replacement invitation. The previous invitation stops working immediately." : "The new address must be confirmed. The current email stays active until then; confirmation ends browser sessions."}</p>
      <form className="auth-form settings-invite" onSubmit={(event) => void submitEmailChange(event)}>
        <label className="auth-form__field" htmlFor="admin-new-email"><span>New email address</span><input ref={emailInput} id="admin-new-email" type="email" autoComplete="email" required maxLength={254} value={newEmail} onChange={(event) => setNewEmail(event.target.value)} /></label>
        <div className="settings-actions"><button className="app-button" type="button" disabled={changingEmail} onClick={() => setEmailTarget(null)}>Cancel email change</button>
          <button className="app-button app-button--primary" type="submit" disabled={busy}>{changingEmail ? "Sending…" : emailTarget.status === "invited" ? "Send replacement invitation" : "Send email confirmation"}</button></div>
      </form>
    </section>}
    {error && <p role="alert">{error}</p>}
    {message && <p role="status">{message}</p>}
    {loadError && <div><p role="alert">{loadError}</p><button type="button" onClick={reload}>Retry</button></div>}
    {!users && !loadError && <p role="status">Loading users…</p>}
    {users && <>
      <table className="settings-users"><caption>Account statuses</caption><thead><tr><th scope="col">Email</th><th scope="col">Status</th><th scope="col">Actions</th></tr></thead>
        <tbody>{users.users.map((user) => <tr key={user.id}><td>{user.email}{user.pending_email_change && <p>Awaiting confirmation: <strong>{user.pending_email_change.email}</strong></p>}</td><td><span className="settings-status">{user.status}</span></td><td>{user.status === "invited" && user.invitation_id != null && <button className="app-button" type="button" disabled={busy} onClick={() => void resend(user)}>{resending === user.id ? "Sending…" : "Resend invitation"}</button>}
          {user.status !== "inactive" && user.id !== currentUserId && <button className="app-button" type="button" disabled={busy || disableTarget !== null || emailTarget !== null} onClick={(event) => { disableTrigger.current = event.currentTarget; setError(null); setMessage(null); setDisableTarget(user); }}>Disable user</button>}
          {(user.status === "registered" || user.status === "invited") && <button className="app-button" type="button" disabled={busy || disableTarget !== null || emailTarget !== null} onClick={(event) => { emailTrigger.current = event.currentTarget; setError(null); setMessage(null); setNewEmail(""); setEmailTarget(user); }}>Change email</button>}
          {(user.status === "pending" || user.status === "inactive") && <span>Email changes unavailable</span>}
          {user.id === currentUserId && <span>Your account</span>}
          {user.status === "inactive" && <span>Access disabled</span>}
          {user.status === "pending" && <span>Awaiting approval</span>}
        </td></tr>)}</tbody>
      </table>
      {users.users.length === 0 && <p>No users on this page.</p>}
      <nav aria-label="Users pagination"><button type="button" disabled={page === 1} onClick={() => setPage(page - 1)}>Previous</button>
        <span> Page {page} </span><button type="button" disabled={page * users.per_page >= users.total} onClick={() => setPage(page + 1)}>Next</button></nav>
    </>}
  </section>;
}
