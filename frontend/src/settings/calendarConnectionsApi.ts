import type { ApiClient } from "../auth/api";

export interface CalendarConnectionPassword {
  id: number;
  label: string;
  created_at: number;
  last_used_at: number | null;
}

export interface AppleCalendarConnection {
  server_url: string;
  principal_id: string | null;
  credentials: CalendarConnectionPassword[];
  last_successful_access_at: number | null;
}

export interface IssuedCalendarPassword {
  password: CalendarConnectionPassword;
  clear_password: string;
  server_url: string;
  username: string;
}

export class CalendarConnectionsApiError extends Error {
  constructor(readonly status: number) {
    super(`Calendar connection request failed (${status})`);
  }
}

async function json<T>(response: Response): Promise<T> {
  if (!response.ok) throw new CalendarConnectionsApiError(response.status);
  return response.json() as Promise<T>;
}

async function expectOk(response: Response): Promise<void> {
  if (!response.ok) throw new CalendarConnectionsApiError(response.status);
}

export function getAppleCalendarConnection(api: ApiClient): Promise<AppleCalendarConnection> {
  return api.request("/api/v1/calendar-connections/apple").then(json<AppleCalendarConnection>);
}

export function createAppleCalendarPassword(api: ApiClient, label: string): Promise<IssuedCalendarPassword> {
  return api
    .request("/api/v1/calendar-connections/apple/passwords", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ label }),
    })
    .then(json<IssuedCalendarPassword>);
}

export function revokeAppleCalendarPassword(api: ApiClient, id: number): Promise<void> {
  return api
    .request(`/api/v1/calendar-connections/apple/passwords/${id}`, { method: "DELETE" })
    .then(expectOk);
}

export function disconnectAppleCalendar(api: ApiClient): Promise<void> {
  return api
    .request("/api/v1/calendar-connections/apple", { method: "DELETE" })
    .then(expectOk);
}
