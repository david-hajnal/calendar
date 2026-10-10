import { useEffect, useState } from "react";

export type NavigationMode = "push" | "replace";
export interface LocationSnapshot {
  pathname: string;
  search: string;
  hash: string;
  href: string;
  state: unknown;
}

function currentLocation(): LocationSnapshot {
  const { pathname, search, hash } = window.location;
  return { pathname, search, hash, href: `${pathname}${search}${hash}`, state: window.history.state };
}

export function useLocation(): LocationSnapshot {
  const [location, setLocation] = useState(currentLocation);
  useEffect(() => {
    const update = () => setLocation(currentLocation());
    window.addEventListener("popstate", update);
    return () => window.removeEventListener("popstate", update);
  }, []);
  return location;
}

function localTarget(value: string): URL | null {
  if (!value.startsWith("/") || value.startsWith("//") || value.includes("\\") || [...value].some(char => char.charCodeAt(0) < 32)) return null;
  try {
    const target = new URL(value, window.location.origin);
    return target.origin === window.location.origin ? target : null;
  } catch { return null; }
}

export function safeContinuation(value: string | null): string | null {
  if (value === null) return null;
  const target = localTarget(value);
  if (!target) return null;
  let path: string;
  try { path = decodeURIComponent(target.pathname); } catch { return null; }
  if (["/login", "/dev-login", "/invitations/consume", "/invitations/accept", "/email/confirm", "/password-reset"].some(route => path === route || path.startsWith(`${route}/`))) return null;
  // One-time authentication data must never be retained in continuation URLs.
  if (["token", "csrf_token", "password"].some(key => target.searchParams.has(key))) return null;
  return `${target.pathname}${target.search}${target.hash}`;
}

export function navigate(target: string, options: { mode?: NavigationMode } = {}): void {
  const destination = localTarget(target);
  if (!destination) throw new Error("Navigation requires a local URL");
  const href = `${destination.pathname}${destination.search}${destination.hash}`;
  const mode = options.mode ?? "push";
  if (mode === "push" && href === currentLocation().href) return;
  if (mode === "replace") window.history.replaceState({}, "", href);
  else window.history.pushState({}, "", href);
  window.dispatchEvent(new PopStateEvent("popstate"));
}
