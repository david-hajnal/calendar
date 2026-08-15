import { useEffect, useRef, useState, type FormEvent } from "react";

import type { Fetcher } from "./auth/api";
import { AuthProvider, useAuth } from "./auth/session";
import { ThemeProvider } from "./theme/themeContext";
import { CalendarManagement } from "./calendar/CalendarManagement";
import { CalendarEventUI } from "./calendar/CalendarEventUI";
import { CompositeViewManagement } from "./calendar/CompositeViewManagement";
import { listCalendars } from "./calendar/api";
import type { Calendar } from "./calendar/CalendarManagement";
import { PublicViewPage } from "./public/PublicView";
import { NotificationDropdown } from "./notification/NotificationDropdown";
import { listNotifications } from "./notification/api";
import { DevLoginPage } from "./dev-login";
import { useTheme } from "./theme/themeContext";

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
  const { api, completeAuthentication } = useAuth();
  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");
  const [method, setMethod] = useState<"link" | "password">("link");
  const [submitting, setSubmitting] = useState(false);
  const [submitted, setSubmitted] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    setSubmitting(true);
    setError(null);
    try {
      if (method === "password") {
        const response = await api.request("/api/v1/auth/password-login", {
          method: "POST",
          headers: { "content-type": "application/json" },
          body: JSON.stringify({ email, password }),
        });
        if (!response.ok) {
          const data = await response.json() as { code?: string; message?: string };
          throw new Error(data.message ?? "Login failed");
        }
        const data = await response.json() as ConsumptionResponse;
        await completeAuthentication(data.csrf_token);
        window.history.replaceState({}, "", "/dashboard");
        window.dispatchEvent(new PopStateEvent("popstate"));
      } else {
        const response = await api.request("/api/v1/auth/login-links", {
          method: "POST",
          headers: { "content-type": "application/json" },
          body: JSON.stringify({ email }),
        });
        if (!response.ok) throw new Error("request failed");
        setSubmitted(true);
      }
    } catch (err) {
      setError(err instanceof Error ? err.message : "An unexpected error occurred");
    } finally {
      setSubmitting(false);
    }
  }

  return <main className="app-page app-page--auth">
    <div className="ambient-bg" aria-hidden="true">
      <div className="ambient-bg__blob ambient-bg__blob--primary" />
      <div className="ambient-bg__blob ambient-bg__blob--secondary" />
      <div className="ambient-bg__blob ambient-bg__blob--tertiary" />
    </div>
    <section className="auth-card" aria-labelledby="login-heading">
      <div className="auth-card__eyebrow-container">
        <span className="material-symbols-outlined eyebrow-icon" style={{ fontSize: '24px' }}>calendar_month</span>
        <span className="auth-card__eyebrow-text">CommonCal</span>
      </div>
      <h1 id="login-heading" className="typography-headline-lg">Sign in</h1>
      {submitted ? (
        <div className="auth-card__success">
          <span className="material-symbols-outlined fill" style={{ fontSize: '48px', color: 'var(--color-on-tertiary-container)' }}>check_circle</span>
          <p role="status" className="typography-body-lg" style={{ margin: '0.75rem 0 0.25rem', color: 'var(--color-on-surface)' }}>Check your email for a login link if the account is eligible.</p>
          <p className="typography-body-md" style={{ margin: 0, color: 'var(--color-on-surface-variant)' }}>We sent a link to your email.</p>
          <button type="button" className="app-button" style={{ marginTop: '1rem', fontSize: '0.8125rem', color: 'var(--color-primary)' }} onClick={() => { setSubmitted(false); setEmail(""); setError(null); }}>Try a different email</button>
        </div>
      ) : (
          <>
          <div className="auth-method-toggle">
            <button className={`auth-method-toggle__button ${method === "link" ? "auth-method-toggle__button--active" : ""}`} type="button" onClick={() => { setMethod("link"); setSubmitted(false); setEmail(""); setError(null); }}>Email link</button>
            <button className={`auth-method-toggle__button ${method === "password" ? "auth-method-toggle__button--active" : ""}`} type="button" onClick={() => { setMethod("password"); setSubmitted(false); setEmail(""); setPassword(""); setError(null); }}>Password</button>
          </div>
          {method === "link" ? (
            <p className="auth-card__intro">Use your email address to receive a secure sign-in link.</p>
          ) : (
            <p className="auth-card__intro">Sign in with your email and password.</p>
          )}
          <form className="auth-form" onSubmit={submit}>
            <label className="auth-form__field" htmlFor="email"><span>Email address</span>
              <div className="auth-form__input-wrapper">
                <span className="material-symbols-outlined" style={{ fontSize: '20px', color: 'var(--color-on-surface-variant)', pointerEvents: 'none' }}>mail</span>
                <input id="email" type="email" autoComplete="email" required value={email} onChange={(event) => setEmail(event.target.value)} />
              </div>
            </label>
            {method === "password" && (
              <label className="auth-form__field" htmlFor="password"><span>Password</span>
                <div className="auth-form__input-wrapper">
                  <span className="material-symbols-outlined" style={{ fontSize: '20px', color: 'var(--color-on-surface-variant)', pointerEvents: 'none' }}>lock</span>
                  <input id="password" type="password" autoComplete="current-password" required value={password} onChange={(event) => setPassword(event.target.value)} />
                </div>
              </label>
            )}
            <button className="app-button app-button--primary" type="submit" disabled={submitting}>
              {submitting && <span className="spinner" style={{ width: '1rem', height: '1rem', border: '2px solid rgba(255,255,255,0.3)', borderTopColor: '#fff', marginRight: '0.5rem' }} />}
              {submitting ? (method === "password" ? "Signing in…" : "Sending login link…") : (method === "password" ? "Sign in" : "Email me a login link")}
            </button>
          </form>
        </>
      )}
      {error && <p className="app-message app-message--error" role="alert">{error}</p>}
    </section>
    <div className="auth-footer">
      <a href="#">Privacy Policy</a>
      <span className="auth-footer__sep">•</span>
      <a href="#">Terms of Service</a>
    </div>
    <div className="auth-mobile-link">
      <a href="#">Create an account</a>
    </div>
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
    <div className="ambient-bg" aria-hidden="true">
      <div className="ambient-bg__blob ambient-bg__blob--primary" />
      <div className="ambient-bg__blob ambient-bg__blob--secondary" />
      <div className="ambient-bg__blob ambient-bg__blob--tertiary" />
    </div>
    <section className="auth-card" aria-labelledby="authentication-heading">
      <div className="auth-card__eyebrow-container">
        <span className="material-symbols-outlined eyebrow-icon" style={{ fontSize: '24px' }}>calendar_month</span>
        <span className="auth-card__eyebrow-text">CommonCal</span>
      </div>
      <h1 id="authentication-heading" className="typography-headline-lg">{kind === "invitation" ? "Accept invitation" : "Signing in"}</h1>
      {result === "loading" && <p className="app-message app-message--status" role="status">Completing sign-in…</p>}
      {result === "success" && <div className="auth-card__success">
        <span className="material-symbols-outlined fill" style={{ fontSize: '48px', color: 'var(--color-on-tertiary-container)' }}>check_circle</span>
        <p className="app-message app-message--success" style={{ margin: '0.75rem 0 0.25rem', color: 'var(--color-on-surface)' }}>{success}</p>
      </div>}
      {result === "failure" && <p className="app-message app-message--error" role="alert">{failure}</p>}
    </section>
  </main>;
}

function ThemeToggle() {
  const { resolvedTheme, toggle } = useTheme();
  const icon = resolvedTheme === "dark" ? "light_mode" : "dark_mode";
  return (
    <button
      className="app-nav__button app-nav__button--quiet"
      type="button"
      aria-label={`Switch to ${resolvedTheme === "dark" ? "light" : "dark"} mode`}
      onClick={() => toggle()}
    >
      <span className="material-symbols-outlined" style={{ fontSize: '20px' }}>{icon}</span>
    </button>
  );
}

function AuthenticatedShell() {
  const { state, api, reloadSession, logout } = useAuth();
  const location = useLocation();
  const lastNotifsRef = useRef<number[]>([]);

  useEffect(() => {
    void (async () => {
      try {
        const notifs = await listNotifications(api);
        const ids = notifs.map((n) => n.id);
        const newNotifs = notifs.filter((n) => !lastNotifsRef.current.includes(n.id));
        if ("Notification" in window && newNotifs.length > 0) {
          const perm = await Notification.requestPermission();
          if (perm === "granted") {
            for (const n of newNotifs) {
              new Notification(n.event_title, { body: `Reminder: ${n.event_title}`, tag: String(n.id) });
            }
          }
        }
        lastNotifsRef.current = ids;
      } catch {
        // polling error — will retry next tick
      }
    })();
    const interval = window.setInterval(async () => {
      try {
        const notifs = await listNotifications(api);
        const ids = notifs.map((n) => n.id);
        const newNotifs = notifs.filter((n) => !lastNotifsRef.current.includes(n.id));
        if ("Notification" in window && newNotifs.length > 0) {
          const perm = await Notification.requestPermission();
          if (perm === "granted") {
            for (const n of newNotifs) {
              new Notification(n.event_title, { body: `Reminder: ${n.event_title}`, tag: String(n.id) });
            }
          }
        }
        lastNotifsRef.current = ids;
      } catch {
        // polling error
      }
    }, 30000);
    return () => { if (interval) clearInterval(interval); };
  }, [api]);

  if (state.status === "loading") return <main className="app-page app-page--state" aria-busy="true"><section className="state-card"><p className="app-message app-message--status" role="status">Loading your session…</p></section></main>;
  if (state.status === "error") return <main className="app-page app-page--state"><section className="state-card"><p className="app-message app-message--error" role="alert">We could not load your session.</p><button className="app-button app-button--primary" type="button" onClick={() => void reloadSession()}>Retry</button></section></main>;
  if (state.status === "unauthenticated") return <LoginRedirect location={location} />;

  const name = state.session.user.display_name ?? state.session.user.email;
  const initials = name.split(/[\s.]+/).slice(0, 2).map((n) => n[0]).join('').toUpperCase().slice(0, 2);
  const activeTab = window.location.pathname === "/calendars" ? "calendars" : window.location.pathname === "/shared" ? "shared" : "calendar";
  const isMobile = typeof window !== "undefined" && window.innerWidth <= 768;

  return <main className="app-shell">
    {/* Fixed top header */}
    <header className="app-header">
      <div className="app-header__identity">
        <span className="material-symbols-outlined" style={{ fontSize: '24px', color: 'var(--color-primary)' }}>calendar_month</span>
        <h1>CommonCal</h1>
      </div>
      {/* Desktop nav tabs */}
      {!isMobile && <nav className="app-nav" aria-label="Primary navigation">
        <button className={`app-nav__button ${activeTab === "calendar" ? "app-nav__button--active" : ""}`} type="button" onClick={() => navigate("/dashboard")}>Calendar</button>
        <button className={`app-nav__button ${activeTab === "calendars" ? "app-nav__button--active" : ""}`} type="button" onClick={() => navigate("/calendars")}>Calendars</button>
        <button className={`app-nav__button ${activeTab === "shared" ? "app-nav__button--active" : ""}`} type="button" onClick={() => navigate("/shared")}>Composite views</button>
      </nav>}
      <div className="app-header__actions">
        <NotificationDropdown api={api} />
        <button className="app-nav__button app-nav__button--quiet" type="button" aria-label="Settings"><span className="material-symbols-outlined" style={{ fontSize: '20px' }}>settings</span></button>
        <ThemeToggle />
        <div className="avatar" aria-label={`Signed in as ${name}`} title={name}>{initials}<span className="avatar__name">{name}</span><span className="avatar__email">{state.session.user.email}</span></div>
        <button className="app-nav__button app-nav__button--quiet" type="button" aria-label="Sign out" onClick={() => void logout()}>
          <span className="material-symbols-outlined" style={{ fontSize: '20px' }}>logout</span>
        </button>
      </div>
    </header>
    {/* Mobile bottom nav */}
    {isMobile && <nav className="bottom-nav" aria-label="Primary navigation">
      <button className={`bottom-nav__item ${activeTab === "calendar" ? "bottom-nav__item--active" : ""}`} type="button" onClick={() => navigate("/dashboard")}>
        <span className="bottom-nav__item__icon-wrapper"><span className="material-symbols-outlined" style={{ fontSize: '20px' }}>calendar_month</span></span>
        <span>Calendar</span>
      </button>
      <button className={`bottom-nav__item ${activeTab === "calendars" ? "bottom-nav__item--active" : ""}`} type="button" onClick={() => navigate("/calendars")}>
        <span className="bottom-nav__item__icon-wrapper"><span className="material-symbols-outlined" style={{ fontSize: '20px' }}>list_alt</span></span>
        <span>Calendars</span>
      </button>
      <button className={`bottom-nav__item ${activeTab === "shared" ? "bottom-nav__item--active" : ""}`} type="button" onClick={() => navigate("/shared")}>
        <span className="bottom-nav__item__icon-wrapper"><span className="material-symbols-outlined" style={{ fontSize: '20px' }}>dashboard</span></span>
        <span>Views</span>
      </button>
    </nav>}
    <div className="app-shell__content">
      {window.location.pathname === "/calendars" && <CalendarManagement api={api} />}
      {window.location.pathname === "/shared" && <CompositeViewManagement api={api} />}
      {window.location.pathname !== "/calendars" && window.location.pathname !== "/shared" && <CalendarPage api={api} />}
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
    const target = safeRedirectTarget(location) ?? "/dashboard";
    navigate(`/login?redirect=${encodeURIComponent(target)}`);
  }, [location]);
  return <main className="app-page app-page--state" aria-busy="true"><section className="state-card"><p className="app-message app-message--status" role="status">Redirecting to sign in…</p></section></main>;
}

function AuthRoutes() {
  const location = useLocation();
  const pathname = window.location.pathname;
  if (pathname === "/login") return <LoginRequestPage />;
  if (pathname === "/dev-login") return <DevLoginPage />;
  if (pathname === "/invitations/consume") return <TokenConsumptionPage kind="invitation" />;
  if (pathname === "/login/consume") return <TokenConsumptionPage kind="login" />;
  return <AuthenticatedShell key={location} />;
}

export function App({ fetcher }: { fetcher?: Fetcher }) {
  const publicToken = /^\/public\/views\/([^/]+)$/.exec(window.location.pathname)?.[1];
  if (publicToken) return <PublicViewPage token={publicToken} fetcher={fetcher} />;
  const isTokenConsumption = window.location.pathname === "/invitations/consume" || window.location.pathname === "/login/consume" || window.location.pathname === "/dev-login";
  return <ThemeProvider><AuthProvider fetcher={fetcher} loadSession={!isTokenConsumption}><AuthRoutes /></AuthProvider></ThemeProvider>;
}
