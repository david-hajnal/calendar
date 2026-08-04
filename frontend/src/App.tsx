import { useEffect, useState, type FormEvent } from "react";

import type { Fetcher } from "./auth/api";
import { AuthProvider, useAuth } from "./auth/session";
import { CalendarManagement } from "./calendar/CalendarManagement";
import { CalendarEventUI } from "./calendar/CalendarEventUI";
import { CompositeViewManagement } from "./calendar/CompositeViewManagement";
import { listCalendars } from "./calendar/api";
import type { Calendar } from "./calendar/CalendarManagement";
import { PublicViewPage } from "./public/PublicView";
import { NotificationSurface } from "./notification/NotificationSurface";

interface ConsumptionResponse {
  csrf_token: string;
}

function safeRedirectTarget(value: string | null): string | null {
  if (value === null || !value.startsWith("/") || value.startsWith("//")) return null;
  try {
    const target = new URL(value, window.location.origin);
    return target.origin === window.location.origin ? `${target.pathname}${target.search}${target.hash}` : null;
  } catch {
    return null;
  }
}

function navigate(target: string) {
  window.history.replaceState({}, "", target);
  window.dispatchEvent(new PopStateEvent("popstate"));
}

function useLocation() {
  const [location, setLocation] = useState(() => `${window.location.pathname}${window.location.search}${window.location.hash}`);
  useEffect(() => {
    const update = () => setLocation(`${window.location.pathname}${window.location.search}${window.location.hash}`);
    window.addEventListener("popstate", update);
    return () => window.removeEventListener("popstate", update);
  }, []);
  return location;
}

function LoginRequestPage() {
  const { api } = useAuth();
  const [email, setEmail] = useState("");
  const [submitting, setSubmitting] = useState(false);
  const [submitted, setSubmitted] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    setSubmitting(true);
    setError(null);
    try {
      const response = await api.request("/api/v1/auth/login-links", {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ email }),
      });
      if (!response.ok) throw new Error("request failed");
      setSubmitted(true);
    } catch {
      setError("We could not request a login link. Please try again.");
    } finally {
      setSubmitting(false);
    }
  }

  return <main className="app-page app-page--auth">
    <section className="auth-card" aria-labelledby="login-heading">
      <p className="auth-card__eyebrow">CommonCal</p>
      <h1 id="login-heading">Sign in</h1>
      <p className="auth-card__intro">Use your email address to receive a secure sign-in link.</p>
      {submitted ? <p className="app-message app-message--status" role="status">Check your email for a login link if the account is eligible.</p> :
        <form className="auth-form" onSubmit={submit}>
          <label className="auth-form__field" htmlFor="email"><span>Email address</span>
            <input id="email" type="email" autoComplete="email" required value={email} onChange={(event) => setEmail(event.target.value)} />
          </label>
          <button className="app-button app-button--primary" type="submit" disabled={submitting}>{submitting ? "Sending login link…" : "Email me a login link"}</button>
        </form>}
      {error && <p className="app-message app-message--error" role="alert">{error}</p>}
    </section>
  </main>;
}

function TokenConsumptionPage({ kind }: { kind: "invitation" | "login" }) {
  const { api, completeAuthentication } = useAuth();
  const [result, setResult] = useState<"loading" | "success" | "failure">("loading");
  const [{ token, redirect }] = useState(() => {
    const params = new URLSearchParams(window.location.search);
    const requestedRedirect = params.get("redirect");
    return { token: params.get("token"), redirect: safeRedirectTarget(requestedRedirect) ?? (requestedRedirect === null ? null : "/") };
  });
  const endpoint = kind === "invitation" ? "/api/v1/auth/invitations/consume" : "/api/v1/auth/login-links/consume";
  const failure = kind === "invitation" ? "Invitation is invalid or expired." : "Login link is invalid or expired.";
  const success = kind === "invitation" ? "Invitation accepted. You are signed in." : "You are signed in.";

  useEffect(() => {
    const cleanUrl = () => window.history.replaceState({}, "", window.location.pathname);
    cleanUrl();
    if (!token) {
      setResult("failure");
      return;
    }
    let active = true;
    void (async () => {
      try {
        const response = await api.request(endpoint, {
          method: "POST",
          headers: { "content-type": "application/json" },
          body: JSON.stringify({ token }),
        });
        if (!response.ok) throw new Error("token consumption failed");
        const data = await response.json() as ConsumptionResponse;
        await completeAuthentication(data.csrf_token);
        if (active) {
          setResult("success");
          if (redirect !== null) navigate(redirect);
        }
      } catch {
        if (active) setResult("failure");
      }
    })();
    return () => { active = false; };
  }, [api, completeAuthentication, endpoint, redirect, token]);

  return <main className="app-page app-page--auth">
    <section className="auth-card" aria-labelledby="authentication-heading">
      <p className="auth-card__eyebrow">CommonCal</p>
      <h1 id="authentication-heading">{kind === "invitation" ? "Accept invitation" : "Signing in"}</h1>
      {result === "loading" && <p className="app-message app-message--status" role="status">Completing sign-in…</p>}
      {result === "success" && <p className="app-message app-message--success" role="status">{success}</p>}
      {result === "failure" && <p className="app-message app-message--error" role="alert">{failure}</p>}
    </section>
  </main>;
}

function AuthenticatedShell() {
  const { state, api, logout, reloadSession } = useAuth();
  const location = useLocation();

  if (state.status === "loading") return <main className="app-page app-page--state" aria-busy="true"><section className="state-card"><p className="app-message app-message--status" role="status">Loading your session…</p></section></main>;
  if (state.status === "error") return <main className="app-page app-page--state"><section className="state-card"><p className="app-message app-message--error" role="alert">We could not load your session.</p><button className="app-button app-button--primary" type="button" onClick={() => void reloadSession()}>Retry</button></section></main>;
  if (state.status === "unauthenticated") return <LoginRedirect location={location} />;

  const name = state.session.user.display_name ?? state.session.user.email;
  return <main className="app-shell">
    <header className="app-header">
      <div className="app-header__identity"><h1>CommonCal</h1><p>Signed in as <strong>{name}</strong><span aria-hidden="true"> · </span><span className="app-header__email">{state.session.user.email}</span></p></div>
      <nav className="app-nav" aria-label="Primary navigation"><button className="app-nav__button" type="button" onClick={() => navigate("/")}>Calendar</button><button className="app-nav__button" type="button" onClick={() => navigate("/calendars")}>Calendars</button><button className="app-nav__button" type="button" onClick={() => navigate("/views")}>Composite views</button><button className="app-nav__button app-nav__button--quiet" type="button" onClick={() => void logout()}>Sign out</button></nav>
    </header>
    <div className="app-shell__content">
      <NotificationSurface api={api} />
      {window.location.pathname === "/calendars" && <CalendarManagement api={api} />}
      {window.location.pathname === "/views" && <CompositeViewManagement api={api} />}
      {window.location.pathname !== "/calendars" && window.location.pathname !== "/views" && <CalendarPage api={api} />}
    </div>
  </main>;
}

function CalendarPage({ api }: { api: ReturnType<typeof useAuth>["api"] }) {
  const [calendars, setCalendars] = useState<Calendar[] | null>(null);
  const [error, setError] = useState(false);
  useEffect(() => {
    let active = true;
    void listCalendars(api).then((result) => { if (active) setCalendars(result); }).catch(() => { if (active) setError(true); });
    return () => { active = false; };
  }, [api]);
  if (error) return <p role="alert">We could not load your calendars. Please try again.</p>;
  if (calendars === null) return <p role="status">Loading calendars…</p>;
  return <CalendarEventUI api={api} calendars={calendars} />;
}

function LoginRedirect({ location }: { location: string }) {
  useEffect(() => {
    if (window.location.pathname === "/login") return;
    const target = safeRedirectTarget(location) ?? "/";
    navigate(`/login?redirect=${encodeURIComponent(target)}`);
  }, [location]);
  return <main className="app-page app-page--state" aria-busy="true"><section className="state-card"><p className="app-message app-message--status" role="status">Redirecting to sign in…</p></section></main>;
}

function AuthRoutes() {
  const location = useLocation();
  const pathname = window.location.pathname;
  if (pathname === "/login") return <LoginRequestPage />;
  if (pathname === "/invitations/consume") return <TokenConsumptionPage kind="invitation" />;
  if (pathname === "/login/consume") return <TokenConsumptionPage kind="login" />;
  return <AuthenticatedShell key={location} />;
}

export function App({ fetcher }: { fetcher?: Fetcher }) {
  const publicToken = /^\/public\/views\/([^/]+)$/.exec(window.location.pathname)?.[1];
  if (publicToken) return <PublicViewPage token={publicToken} fetcher={fetcher} />;
  const isTokenConsumption = window.location.pathname === "/invitations/consume" || window.location.pathname === "/login/consume";
  return <AuthProvider fetcher={fetcher} loadSession={!isTokenConsumption}><AuthRoutes /></AuthProvider>;
}
