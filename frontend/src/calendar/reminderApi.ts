import type { ApiClient } from "../auth/api";

export interface ReminderResponse {
  reminder_id: string;
}

export function setReminder(
  api: ApiClient,
  calendarId: number,
  eventId: number,
  minutes: number,
): Promise<ReminderResponse> {
  return api
    .request(`/api/v1/calendars/${calendarId}/events/${eventId}/reminder`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ reminder_minutes: minutes }),
    })
    .then((res) => res.json() as Promise<ReminderResponse>);
}

export function removeReminder(
  api: ApiClient,
  calendarId: number,
  eventId: number,
): Promise<void> {
  return api
    .request(`/api/v1/calendars/${calendarId}/events/${eventId}/reminder`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ reminder_minutes: null }),
    })
    .then(() => {});
}

export async function getReminder(
  api: ApiClient,
  calendarId: number,
  eventId: number,
): Promise<number | null> {
  try {
    const res = await api.request(
      `/api/v1/calendars/${calendarId}/events/${eventId}/reminder`,
    );
    if (!res.ok) return null;
    const data = await res.json() as { reminder_minutes: number };
    return data.reminder_minutes;
  } catch {
    return null;
  }
}
