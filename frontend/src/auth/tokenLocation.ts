import { useEffect, useState } from "react";

/** Read during render, strip in an effect so StrictMode keeps the same token. */
export function useLinkToken(): string | null {
  const [token] = useState(() => new URLSearchParams(window.location.search).get("token"));
  useEffect(() => {
    const url = new URL(window.location.href);
    url.searchParams.delete("token");
    window.history.replaceState({}, "", `${url.pathname}${url.search}${url.hash}`);
  }, []);
  return token;
}
