import type { ApiClient } from "../auth/api";
import type { Calendar } from "./CalendarManagement";

export type ShareableCalendarRole = "manager" | "editor" | "viewer" | "free_busy_viewer";

export interface CalendarAclEntry {
  user_id: number;
  role: "owner" | ShareableCalendarRole;
  created_at: number;
  updated_at: number;
}

export interface CalendarSettingsPayload {
  name: string;
  description: string | null;
  color: string;
  default_timezone: string;
  default_event_visibility: string;
  default_notification_rules_json: string | null;
}

export interface CalendarUpdatePayload extends CalendarSettingsPayload {
  version: number;
}

export interface CompositeViewCalendar {
  calendar_id: number;
  position: number;
  color: string;
}

export interface CompositeView {
  id: number;
  owner_user_id: number;
  name: string;
  version: number;
  created_at: number;
  updated_at: number;
  calendars: CompositeViewCalendar[];
}

export interface CompositeViewMutationPayload {
  name: string;
}

export interface CompositeViewCalendarsPayload {
  calendars: CompositeViewCalendar[];
}

export type PublicViewProjection = "full_details" | "title_and_time" | "free_busy";

export interface PublicViewConfiguration {
  projection: PublicViewProjection;
  display_timezone: string;
  expires_at: number;
}

export interface IssuedPublicView extends PublicViewConfiguration {
  token: string;
  revoked: boolean;
  version: number;
}

export type EventStatus = "tentative" | "confirmed" | "cancelled";

export interface TimedEventPayload {
  title: string;
  description: string | null;
  location: string | null;
  status: EventStatus;
  start_utc: number;
  end_utc: number;
  timezone: string;
  recurrence_rule?: string;
}

export interface AllDayEventPayload {
  title: string;
  description: string | null;
  location: string | null;
  status: EventStatus;
  start_date: string;
  end_date: string;
  recurrence_rule?: string;
}

export type EventPayload = TimedEventPayload | AllDayEventPayload;

export interface EventProjection {
  id: number;
  calendar_id: number;
  access: "details" | "free_busy";
  status: EventStatus;
  event_kind: "timed" | "all_day";
  title?: string;
  description?: string | null;
  location?: string | null;
  start_utc?: number;
  end_utc?: number;
  timezone?: string;
  start_date?: string;
  end_date?: string;
  created_by_user_id?: number;
  last_edited_by_user_id?: number;
  version?: number;
  created_at?: number;
  updated_at?: number;
  recurrence_rule?: string;
  series_id?: number;
  recurrence_id?: number;
  recurrence_date?: string;
  /** Supplied by APIs that expose imported or otherwise immutable projections. */
  is_external?: boolean;
  read_only?: boolean;
}

export interface EventRange {
  from: number;
  to: number;
}

export type EventUpdatePayload = EventPayload & { calendar_id: number; version: number };
export type OccurrenceUpdatePayload = EventPayload & { version: number };

export class CalendarApiError extends Error {
  constructor(readonly status: number) {
    super(`Calendar request failed (${status})`);
  }
}

export function isCalendarAccessChange(error: unknown): boolean {
  return error instanceof CalendarApiError && [401, 403, 404, 409].includes(error.status);
}

async function json<T>(response: Response): Promise<T> {
  if (!response.ok) throw new CalendarApiError(response.status);
  return response.json() as Promise<T>;
}

export function listCalendars(api: ApiClient) { return api.request("/api/v1/calendars").then(json<Calendar[]>); }
export function createCalendar(api: ApiClient, settings: CalendarSettingsPayload) {
  return api.request("/api/v1/calendars", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(settings) }).then(json<Calendar>);
}
export function updateCalendar(api: ApiClient, id: number, settings: CalendarUpdatePayload) {
  return api.request(`/api/v1/calendars/${id}`, { method: "PATCH", headers: { "content-type": "application/json" }, body: JSON.stringify(settings) }).then(json<Calendar>);
}
export function archiveCalendar(api: ApiClient, id: number) { return api.request(`/api/v1/calendars/${id}/archive`, { method: "POST" }).then(json<Calendar>); }
export function restoreCalendar(api: ApiClient, id: number) { return api.request(`/api/v1/calendars/${id}/restore`, { method: "POST" }).then(json<Calendar>); }
export function deleteCalendar(api: ApiClient, id: number) {
  return api.request(`/api/v1/calendars/${id}`, { method: "DELETE" }).then((response) => { if (!response.ok) throw new CalendarApiError(response.status); });
}
export function listCalendarAcl(api: ApiClient, id: number) { return api.request(`/api/v1/calendars/${id}/acl`).then(json<CalendarAclEntry[]>); }
export function setCalendarAcl(api: ApiClient, calendarId: number, userId: number, role: ShareableCalendarRole) {
  return api.request(`/api/v1/calendars/${calendarId}/acl/${userId}`, { method: "PUT", headers: { "content-type": "application/json" }, body: JSON.stringify({ role }) }).then(json<CalendarAclEntry>);
}
export function revokeCalendarAcl(api: ApiClient, calendarId: number, userId: number) {
  return api.request(`/api/v1/calendars/${calendarId}/acl/${userId}`, { method: "DELETE" }).then((response) => { if (!response.ok) throw new CalendarApiError(response.status); });
}
export function transferCalendarOwnership(api: ApiClient, calendarId: number, newOwnerUserId: number, version: number) {
  return api.request(`/api/v1/calendars/${calendarId}/transfer`, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ new_owner_user_id: newOwnerUserId, version }) }).then(json<Calendar>);
}

export function listCompositeViews(api: ApiClient) { return api.request("/api/v1/views").then(json<CompositeView[]>); }
export function createCompositeView(api: ApiClient, view: CompositeViewMutationPayload) {
  return api.request("/api/v1/views", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(view) }).then(json<CompositeView>);
}
export function updateCompositeView(api: ApiClient, id: number, view: CompositeViewMutationPayload) {
  return api.request(`/api/v1/views/${id}`, { method: "PATCH", headers: { "content-type": "application/json" }, body: JSON.stringify(view) }).then(json<CompositeView>);
}
export function replaceCompositeViewCalendars(api: ApiClient, id: number, view: CompositeViewCalendarsPayload) {
  return api.request(`/api/v1/views/${id}/calendars`, { method: "PUT", headers: { "content-type": "application/json" }, body: JSON.stringify(view) }).then(json<CompositeView>);
}
export function createCompositeViewPublication(api: ApiClient, id: number, configuration: PublicViewConfiguration) {
  return api.request(`/api/v1/views/${id}/publication`, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(configuration) }).then(json<IssuedPublicView>);
}
export function configureCompositeViewPublication(api: ApiClient, id: number, configuration: PublicViewConfiguration) {
  return api.request(`/api/v1/views/${id}/publication`, { method: "PATCH", headers: { "content-type": "application/json" }, body: JSON.stringify(configuration) }).then(json<Omit<IssuedPublicView, "token">>);
}
export function rotateCompositeViewPublication(api: ApiClient, id: number) {
  return api.request(`/api/v1/views/${id}/publication/rotate`, { method: "POST" }).then(json<IssuedPublicView>);
}

function eventPath(calendarId: number) {
  return `/api/v1/calendars/${calendarId}/events`;
}

export function listExpandedEvents(api: ApiClient, calendarIds: readonly number[], range: EventRange): Promise<EventProjection[]> {
  const query = `from=${encodeURIComponent(range.from)}&to=${encodeURIComponent(range.to)}`;
  return Promise.all(calendarIds.map((calendarId) => api.request(`${eventPath(calendarId)}?${query}`).then(json<EventProjection[]>))).then((events) => events.flat());
}

export function createEvent(api: ApiClient, calendarId: number, event: EventPayload) {
  return api.request(eventPath(calendarId), { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(event) }).then(json<EventProjection>);
}

export function updateEvent(api: ApiClient, calendarId: number, eventId: number, event: EventUpdatePayload) {
  return api.request(`${eventPath(calendarId)}/${eventId}`, { method: "PATCH", headers: { "content-type": "application/json" }, body: JSON.stringify(event) }).then(json<EventProjection>);
}

export function deleteEvent(api: ApiClient, calendarId: number, eventId: number) {
  return api.request(`${eventPath(calendarId)}/${eventId}`, { method: "DELETE" }).then((response) => { if (!response.ok) throw new CalendarApiError(response.status); });
}

export function updateEventOccurrence(api: ApiClient, calendarId: number, eventId: number, recurrenceId: string | number, event: OccurrenceUpdatePayload) {
  return api.request(`${eventPath(calendarId)}/${eventId}/occurrences/${encodeURIComponent(recurrenceId)}`, { method: "PATCH", headers: { "content-type": "application/json" }, body: JSON.stringify(event) }).then(json<EventProjection>);
}

export function deleteEventOccurrence(api: ApiClient, calendarId: number, eventId: number, recurrenceId: string | number, version: number) {
  return api.request(`${eventPath(calendarId)}/${eventId}/occurrences/${encodeURIComponent(recurrenceId)}`, { method: "DELETE", headers: { "content-type": "application/json" }, body: JSON.stringify({ version }) }).then((response) => { if (!response.ok) throw new CalendarApiError(response.status); });
}
