import { useEffect, useState } from "react";

export const MOBILE_QUERY = "(max-width: 48rem)";

export function useIsMobile() {
  const [isMobile, setIsMobile] = useState(() => (typeof window.matchMedia === "function" ? window.matchMedia(MOBILE_QUERY).matches : window.innerWidth <= 768));
  useEffect(() => {
    if (typeof window.matchMedia !== "function") return;
    const query = window.matchMedia(MOBILE_QUERY);
    const update = (event: MediaQueryListEvent) => setIsMobile(event.matches);
    query.addEventListener("change", update);
    return () => query.removeEventListener("change", update);
  }, []);
  return isMobile;
}

