import { useEffect, useState, type FormEvent } from "react";
import type { ApiClient } from "../auth/api";
import { changeOwnEmail, getAccount, type AccountSummary } from "./accountApi";

export function AccountSettings({ api }: { api: ApiClient }) {
  const [account, setAccount] = useState<AccountSummary | null>(null);
  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [submitting, setSubmitting] = useState(false);
  const [reload, setReload] = useState(0);
  useEffect(() => {
    let active = true;
    void getAccount(api).then((value) => { if (active) { setAccount(value); setError(null); } }).catch((reason: unknown) => { if (active) setError(reason instanceof Error ? reason.message : "Could not load account."); });
    return () => { active = false; };
  }, [api, reload]);
  async function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault(); setError(null); setNotice(null); setSubmitting(true);
    try {
      await changeOwnEmail(api, email, password);
      setNotice("Confirmation sent. Your current email stays active until you confirm the new address.");
      setEmail(""); setPassword(""); setReload((value) => value + 1);
    } catch (reason) { setError(reason instanceof Error ? reason.message : "Could not change email."); setPassword(""); }
    finally { setSubmitting(false); }
  }
  return <section aria-labelledby="account-heading">
    <h2 id="account-heading">Account</h2>
    {error && <p className="app-message app-message--error" role="alert">{error}</p>}
    {!account ? <><p>Loading account…</p>{error && <button type="button" className="app-button" onClick={() => setReload((value) => value + 1)}>Retry</button>}</> : <>
      <p>Current email: <strong>{account.email}</strong></p>
      {account.pending_email_change && <p className="settings-notice">Awaiting confirmation at <strong>{account.pending_email_change.email}</strong>. Link expires {new Date(account.pending_email_change.expires_at * 1000).toLocaleString()}. Sending another confirmation replaces the previous link.</p>}
      {notice && <p className="app-message app-message--success" role="status">{notice}</p>}
      {!account.has_password ? <p>Set a password using <a href="/forgot-password">Forgot password</a> before changing your email.</p> : <form className="auth-form settings-invite" onSubmit={(event) => void submit(event)}>
        <label className="auth-form__field" htmlFor="account-email"><span>New email address</span><input id="account-email" type="email" autoComplete="email" required maxLength={254} value={email} onChange={(event) => setEmail(event.target.value)} /></label>
        <label className="auth-form__field" htmlFor="account-password"><span>Current password</span><input id="account-password" type="password" autoComplete="current-password" required value={password} onChange={(event) => setPassword(event.target.value)} /></label>
        <p>Confirming the new address ends your browser sessions. Your calendars and connection passwords stay with this account. Calendar clients using your email as username will need the new address.</p>
        <button className="app-button app-button--primary" type="submit" disabled={submitting}>{submitting ? "Sending confirmation…" : "Send confirmation"}</button>
      </form>}
    </>}
  </section>;
}
