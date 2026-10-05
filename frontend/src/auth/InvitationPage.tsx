import { useEffect, useState, type FormEvent } from "react";
import { useAuth } from "./session";
import { useLinkToken } from "./tokenLocation";

export function InvitationPage() {
  const { api } = useAuth();
  const token = useLinkToken();
  const [email, setEmail] = useState<string | null>(null);
  const [password, setPassword] = useState("");
  const [confirmation, setConfirmation] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [unavailable, setUnavailable] = useState(!token);
  const [submitting, setSubmitting] = useState(false);
  const [accepted, setAccepted] = useState(false);
  useEffect(() => {
    let active = true;
    if (token) void api.request(`/api/v1/auth/invitations/preview?token=${encodeURIComponent(token)}`).then(async (response) => {
      if (!response.ok) throw new Error("Invitation is invalid or expired.");
      const data = await response.json() as { email: string };
      if (active) setEmail(data.email);
    }).catch(() => { if (active) setUnavailable(true); });
    return () => { active = false; };
  }, [api, token]);
  async function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault(); setError(null);
    if (password !== confirmation) { setError("Passwords must match."); return; }
    if (Array.from(password).length < 12 || new TextEncoder().encode(password).length > 72) { setError("Use at least 12 characters and at most 72 bytes for your password."); return; }
    setSubmitting(true);
    try {
      const response = await api.request("/api/v1/auth/invitations/consume", {
        method: "POST", headers: { "content-type": "application/json" },
        body: JSON.stringify({ token, password, password_confirmation: confirmation }),
      });
      if (!response.ok) throw new Error("Your account could not be created. The invitation may have expired or been used.");
      setPassword(""); setConfirmation(""); setAccepted(true);
    } catch (reason) { setError(reason instanceof Error ? reason.message : "Please try again."); }
    finally { setSubmitting(false); }
  }
  return <main className="app-page app-page--auth"><section className="auth-card">
    <h1>{accepted ? "Account created" : "Create your password"}</h1>
    {accepted ? <><p className="app-message app-message--success" role="status">Invitation accepted. Sign in with your new password.</p><a href="/login">Sign in</a></>
      : unavailable ? <><p className="app-message app-message--error" role="alert">Invitation is invalid or expired.</p><p>Ask your admin to resend it, or sign in if you already registered.</p><a href="/login">Sign in</a></>
      : email ? <><p>Create a password for <strong>{email}</strong>.</p><form className="auth-form" onSubmit={(event) => void submit(event)}>
        <label className="auth-form__field" htmlFor="invite-password"><span>Password</span><input id="invite-password" type="password" autoComplete="new-password" required minLength={12} value={password} onChange={(event) => setPassword(event.target.value)} /></label>
        <label className="auth-form__field" htmlFor="invite-confirmation"><span>Confirm password</span><input id="invite-confirmation" type="password" autoComplete="new-password" required value={confirmation} onChange={(event) => setConfirmation(event.target.value)} /></label>
        <p>Use at least 12 characters. Maximum 72 bytes.</p>
        {error && <p role="alert">{error}</p>}
        <button className="app-button app-button--primary" disabled={submitting} type="submit">{submitting ? "Creating account…" : "Create account"}</button>
      </form></> : <p role="status">Checking invitation…</p>}
  </section></main>;
}
