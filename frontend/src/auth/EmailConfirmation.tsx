import { useState } from "react";
import { useAuth } from "./session";
import { useLinkToken } from "./tokenLocation";

export function EmailConfirmation() {
  const { api } = useAuth();
  const token = useLinkToken();
  const [completed, setCompleted] = useState(false);
  const [unavailable, setUnavailable] = useState(!token);
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  async function confirm() {
    setError(null); setSubmitting(true);
    try {
      const response = await api.request("/api/v1/auth/email-changes/consume", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ token }) });
      if (response.status === 400 || response.status === 409) { setUnavailable(true); return; }
      if (response.status === 429) throw new Error("Too many attempts. Please wait and try again.");
      if (!response.ok) throw new Error("We could not confirm your email. Please try again.");
      setCompleted(true);
    } catch (reason) { setError(reason instanceof Error ? reason.message : "Please try again."); }
    finally { setSubmitting(false); }
  }
  return <main className="app-page app-page--auth"><section className="auth-card" aria-labelledby="email-confirm-heading">
    <h1 id="email-confirm-heading">Confirm email change</h1>
    {completed ? <><p className="app-message app-message--success" role="status">Your email has been changed. Sign in with your new email.</p><p>Calendar clients using your email as username need the new address. Their connection passwords stay valid.</p><a href="/login">Sign in</a></>
      : unavailable ? <><p className="app-message app-message--error" role="alert">Email confirmation link is invalid, expired, or unavailable.</p><a href="/settings/account">Request a new confirmation in Account settings</a></>
      : <><p>Confirm your new email address. This ends your browser sessions; your account and calendars stay the same.</p>{error && <p className="app-message app-message--error" role="alert">{error}</p>}<button className="app-button app-button--primary" type="button" disabled={submitting} onClick={() => void confirm()}>{submitting ? "Confirming…" : "Confirm new email"}</button></>}
  </section></main>;
}
