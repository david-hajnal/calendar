import { useState, type FormEvent } from "react";
import { useAuth } from "./session";
import { useLinkToken } from "./tokenLocation";

const confirmationMessage = "If the account is eligible, a password reset link will be sent.";
export function PasswordRecoveryRequest() {
  const { api } = useAuth();
  const [email, setEmail] = useState("");
  const [submitting, setSubmitting] = useState(false);
  const [submitted, setSubmitted] = useState(false);
  const [error, setError] = useState<string | null>(null);
  async function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault(); setError(null); setSubmitting(true);
    try {
      const response = await api.request("/api/v1/auth/password-resets", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ email }) });
      if (response.status === 429) throw new Error("Too many recovery requests. Please wait and try again.");
      if (!response.ok) throw new Error("We could not request a reset link. Please try again.");
      setSubmitted(true); setEmail("");
    } catch (reason) { setError(reason instanceof Error ? reason.message : "Please try again."); }
    finally { setSubmitting(false); }
  }
  return <main className="app-page app-page--auth"><section className="auth-card" aria-labelledby="recovery-heading">
    <h1 id="recovery-heading">Forgot password</h1>
    {submitted ? <><p className="app-message app-message--success" role="status">{confirmationMessage}</p><p>Check your email. The link expires in 15 minutes.</p><button className="app-button" type="button" onClick={() => { setSubmitted(false); setError(null); }}>Request another link</button></>
      : <><p>Enter your account email to request a password reset link.</p><form className="auth-form" onSubmit={(event) => void submit(event)}>
        <label className="auth-form__field" htmlFor="recovery-email"><span>Email address</span><input id="recovery-email" type="email" autoComplete="email" required maxLength={254} value={email} onChange={(event) => setEmail(event.target.value)} /></label>
        {error && <p className="app-message app-message--error" role="alert">{error}</p>}
        <button className="app-button app-button--primary" type="submit" disabled={submitting}>{submitting ? "Requesting link…" : "Send reset link"}</button>
      </form></>}
    <a href="/login">Back to sign in</a>
  </section></main>;
}

export function PasswordResetPage() {
  const { api } = useAuth();
  const token = useLinkToken();
  const [password, setPassword] = useState("");
  const [confirmation, setConfirmation] = useState("");
  const [submitting, setSubmitting] = useState(false);
  const [completed, setCompleted] = useState(false);
  const [unavailable, setUnavailable] = useState(!token);
  const [error, setError] = useState<string | null>(null);
  async function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault(); setError(null);
    if (password !== confirmation) { setError("Passwords must match."); return; }
    if (Array.from(password).length < 12 || new TextEncoder().encode(password).length > 72) { setError("Use at least 12 characters and at most 72 bytes for your password."); return; }
    setSubmitting(true);
    try {
      const response = await api.request("/api/v1/auth/password-resets/consume", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ token, password, password_confirmation: confirmation }) });
      if (response.status === 400) { const body = await response.json() as { error?: { code?: string } }; if (body.error?.code === "invalid_reset") { setUnavailable(true); setPassword(""); setConfirmation(""); return; } }
      if (response.status === 429) throw new Error("Too many attempts. Please wait and try again.");
      if (!response.ok) throw new Error("Your password could not be reset. Please try again.");
      setPassword(""); setConfirmation(""); setCompleted(true);
    } catch (reason) { setError(reason instanceof Error ? reason.message : "Please try again."); }
    finally { setSubmitting(false); }
  }
  return <main className="app-page app-page--auth"><section className="auth-card" aria-labelledby="reset-heading">
    <h1 id="reset-heading">{completed ? "Password reset" : "Set a new password"}</h1>
    {completed ? <><p className="app-message app-message--success" role="status">Your password has been changed. Sign in with your new password.</p><a href="/login">Sign in</a></>
      : unavailable ? <><p className="app-message app-message--error" role="alert">Password reset link is invalid or expired.</p><a href="/forgot-password">Request a new reset link</a></>
      : <form className="auth-form" onSubmit={(event) => void submit(event)}>
        <label className="auth-form__field" htmlFor="reset-password"><span>New password</span><input id="reset-password" type="password" autoComplete="new-password" required minLength={12} value={password} onChange={(event) => setPassword(event.target.value)} /></label>
        <label className="auth-form__field" htmlFor="reset-confirmation"><span>Confirm password</span><input id="reset-confirmation" type="password" autoComplete="new-password" required value={confirmation} onChange={(event) => setConfirmation(event.target.value)} /></label>
        <p>Use at least 12 characters. Maximum 72 bytes.</p>
        <p>Resetting ends your previous browser sessions and login links.</p>
        {error && <p className="app-message app-message--error" role="alert">{error}</p>}
        <button className="app-button app-button--primary" type="submit" disabled={submitting}>{submitting ? "Resetting password…" : "Reset password"}</button>
      </form>}
  </section></main>;
}
