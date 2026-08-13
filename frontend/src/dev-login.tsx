import { useEffect, useState, type FormEvent } from "react";

import { useAuth } from "./auth/session";

function navigate(target: string) {
  window.history.replaceState({}, "", target);
  window.dispatchEvent(new PopStateEvent("popstate"));
}

export function DevLoginPage() {
  const { completeAuthentication } = useAuth();
  const [email, setEmail] = useState(() => {
    const params = new URLSearchParams(window.location.search);
    return params.get("email") ?? "";
  });
  const [displayName, setDisplayName] = useState(() => {
    const params = new URLSearchParams(window.location.search);
    return params.get("display_name") ?? "";
  });
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    const csrfToken = new URLSearchParams(window.location.search).get("csrf_token");
    if (csrfToken) {
      void completeAuthentication(csrfToken);
    }
  }, [completeAuthentication]);

  async function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    setSubmitting(true);
    setError(null);
    try {
      const params = new URLSearchParams();
      if (email) params.set("email", email);
      if (displayName) params.set("display_name", displayName);
      const url = `/api/v1/dev/login?${params.toString()}`;
      const response = await fetch(url, {
        method: "GET",
        credentials: "same-origin",
      });
      if (!response.ok) throw new Error("dev login failed");
      const location = response.headers.get("location");
      if (location) {
        navigate(location);
      }
    } catch {
      setError("We could not complete the sign-in. Please try again.");
    } finally {
      setSubmitting(false);
    }
  }

  return (
    <main className="app-page app-page--auth">
      <section className="auth-card" aria-labelledby="dev-login-heading">
        <p className="auth-card__eyebrow">CommonCal</p>
        <h1 id="dev-login-heading">Developer sign in</h1>
        <p className="auth-card__intro">
          Enter an email address to sign in as that user. A new account will be
          created if one does not exist.
        </p>
        <form className="auth-form" onSubmit={submit}>
          <label className="auth-form__field" htmlFor="dev-email">
            <span>Email address</span>
            <input
              id="dev-email"
              type="email"
              autoComplete="email"
              required
              value={email}
              onChange={(event) => setEmail(event.target.value)}
            />
          </label>
          <label className="auth-form__field" htmlFor="dev-display-name">
            <span>Display name (optional)</span>
            <input
              id="dev-display-name"
              type="text"
              value={displayName}
              onChange={(event) => setDisplayName(event.target.value)}
            />
          </label>
          <button
            className="app-button app-button--primary"
            type="submit"
            disabled={submitting}
          >
            {submitting ? "Signing in…" : "Sign in"}
          </button>
        </form>
        {error && (
          <p className="app-message app-message--error" role="alert">
            {error}
          </p>
        )}
      </section>
    </main>
  );
}
