import { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState, type ReactNode } from "react";

import { createApiClient, type ApiClient, type Fetcher } from "./api";

export interface User {
  id: number;
  email: string;
  display_name: string | null;
  is_superadmin: boolean;
}

export interface Session {
  user: User;
  csrf_token: string;
  created_at: number;
  last_seen_at: number;
  expires_at: number;
}

export type AuthState =
  | { status: "loading" }
  | { status: "authenticated"; session: Session }
  | { status: "unauthenticated" }
  | { status: "error"; error: Error };

interface AuthContextValue {
  state: AuthState;
  api: ApiClient;
  establishSession(session: Session, csrfToken: string): void;
  completeAuthentication(csrfToken: string): Promise<void>;
  reloadSession(): Promise<void>;
  logout(): Promise<void>;
}

const AuthContext = createContext<AuthContextValue | null>(null);

export function AuthProvider({ children, fetcher, loadSession = true }: { children: ReactNode; fetcher?: Fetcher; loadSession?: boolean }) {
  const api = useRef(createApiClient(fetcher)).current;
  const [state, setState] = useState<AuthState>({ status: "loading" });

  const reloadSession = useCallback(async () => {
    setState({ status: "loading" });
    try {
      const response = await api.request("/api/v1/auth/session");
      if (response.status === 401) {
        api.setCsrfToken(null);
        setState({ status: "unauthenticated" });
        return;
      }
      if (!response.ok) {
        throw new Error(`Unable to load session (${response.status})`);
      }
      const data = (await response.json()) as Session;
      if (data.csrf_token) {
        api.setCsrfToken(data.csrf_token);
      }
      setState({ status: "authenticated", session: data });
    } catch (error) {
      setState({ status: "error", error: error instanceof Error ? error : new Error("Unable to load session") });
    }
  }, [api]);

  const completeAuthentication = useCallback(async (csrfToken: string) => {
    api.setCsrfToken(csrfToken);
    await reloadSession();
  }, [api, reloadSession]);

  useEffect(() => {
    if (loadSession) void reloadSession();
  }, [loadSession, reloadSession]);

  const value = useMemo<AuthContextValue>(() => ({
    state,
    api,
    establishSession(session, csrfToken) {
      api.setCsrfToken(csrfToken);
      setState({ status: "authenticated", session });
    },
    completeAuthentication,
    reloadSession,
    async logout() {
      await api.logout();
      setState({ status: "unauthenticated" });
    },
  }), [api, completeAuthentication, reloadSession, state]);

  return <AuthContext.Provider value={value}>{children}</AuthContext.Provider>;
}

export function useAuth(): AuthContextValue {
  const context = useContext(AuthContext);
  if (context === null) {
    throw new Error("useAuth must be used within an AuthProvider");
  }
  return context;
}
