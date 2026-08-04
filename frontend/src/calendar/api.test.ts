import { describe, expect, it, vi } from "vitest";

import { archiveCalendar, configureCompositeViewPublication, createCalendar, createCompositeView, createCompositeViewPublication, createEvent, deleteEventOccurrence, listCalendars, listCompositeViews, listExpandedEvents, replaceCompositeViewCalendars, restoreCalendar, rotateCompositeViewPublication, updateCalendar, updateCompositeView, updateEventOccurrence } from "./api";
import type { ApiClient } from "../auth/api";

function client() {
  return { request: vi.fn().mockImplementation(() => Promise.resolve(new Response(JSON.stringify([]), { status: 200 }))), csrfToken: null, setCsrfToken: vi.fn(), logout: vi.fn() } as unknown as ApiClient;
}

describe("calendar API", () => {
  it("uses the calendar endpoints and JSON mutation contracts", async () => {
    const api = client();
    const settings = { name: "Team", description: null, color: "#123456", default_timezone: "UTC", default_event_visibility: "private", default_notification_rules_json: null };
    await listCalendars(api);
    await createCalendar(api, settings);
    await updateCalendar(api, 2, { ...settings, version: 1 });
    await archiveCalendar(api, 2);
    await restoreCalendar(api, 2);
    expect(api.request).toHaveBeenCalledWith("/api/v1/calendars", expect.objectContaining({ method: "POST", body: JSON.stringify(settings) }));
    expect(api.request).toHaveBeenCalledWith("/api/v1/calendars/2", expect.objectContaining({ method: "PATCH", body: JSON.stringify({ ...settings, version: 1 }) }));
    expect(api.request).toHaveBeenCalledWith("/api/v1/calendars/2/archive", expect.objectContaining({ method: "POST" }));
    expect(api.request).toHaveBeenCalledWith("/api/v1/calendars/2/restore", expect.objectContaining({ method: "POST" }));
  });
});

describe("composite view API", () => {
  it("uses the typed view endpoints and ordered source-calendar contract", async () => {
    const api = client();
    const calendars = [{ calendar_id: 2, position: 0, color: "#123456" }];
    await listCompositeViews(api);
    await createCompositeView(api, { name: "Team" });
    await updateCompositeView(api, 4, { name: "Updated" });
    await replaceCompositeViewCalendars(api, 4, { calendars });
    expect(api.request).toHaveBeenCalledWith("/api/v1/views");
    expect(api.request).toHaveBeenCalledWith("/api/v1/views", expect.objectContaining({ method: "POST", body: JSON.stringify({ name: "Team" }) }));
    expect(api.request).toHaveBeenCalledWith("/api/v1/views/4", expect.objectContaining({ method: "PATCH", body: JSON.stringify({ name: "Updated" }) }));
    expect(api.request).toHaveBeenCalledWith("/api/v1/views/4/calendars", expect.objectContaining({ method: "PUT", body: JSON.stringify({ calendars }) }));
  });

  it("uses authenticated publication controls with the backend's configuration contract", async () => {
    const api = client();
    const configuration = { projection: "title_and_time" as const, display_timezone: "UTC", expires_at: 1_750_000_000 };
    await createCompositeViewPublication(api, 4, configuration);
    await configureCompositeViewPublication(api, 4, { ...configuration, projection: "free_busy" });
    await rotateCompositeViewPublication(api, 4);
    expect(api.request).toHaveBeenCalledWith("/api/v1/views/4/publication", expect.objectContaining({ method: "POST", body: JSON.stringify(configuration) }));
    expect(api.request).toHaveBeenCalledWith("/api/v1/views/4/publication", expect.objectContaining({ method: "PATCH", body: JSON.stringify({ ...configuration, projection: "free_busy" }) }));
    expect(api.request).toHaveBeenCalledWith("/api/v1/views/4/publication/rotate", expect.objectContaining({ method: "POST" }));
  });
});

describe("event API", () => {
  const timedEvent = {
    title: "Planning", description: "Quarterly plan", location: "Room 1", status: "confirmed" as const,
    start_utc: 1_750_000_100, end_utc: 1_750_003_700, timezone: "Europe/Budapest",
  };

  it("fetches backend-expanded occurrences for every visible calendar without calculating recurrence locally", async () => {
    const api = client();
    vi.mocked(api.request).mockImplementation((path) => Promise.resolve(new Response(JSON.stringify([
      { id: path.toString().includes("/2/") ? 22 : 11, calendar_id: path.toString().includes("/2/") ? 2 : 1, event_kind: "timed", access: "details", status: "confirmed", start_utc: 1, end_utc: 2, series_id: 11, recurrence_id: 1 },
    ]), { status: 200 })));

    await expect(listExpandedEvents(api, [1, 2], { from: 1_750_000_000, to: 1_750_086_400 })).resolves.toMatchObject([
      { id: 11, series_id: 11, recurrence_id: 1 }, { id: 22, series_id: 11, recurrence_id: 1 },
    ]);
    expect(api.request).toHaveBeenCalledWith("/api/v1/calendars/1/events?from=1750000000&to=1750086400");
    expect(api.request).toHaveBeenCalledWith("/api/v1/calendars/2/events?from=1750000000&to=1750086400");
  });

  it("uses event and occurrence mutation contracts including optimistic-concurrency versions", async () => {
    const api = client();
    vi.mocked(api.request).mockImplementation(() => Promise.resolve(new Response(JSON.stringify({ id: 3 }), { status: 200 })));

    await createEvent(api, 2, { ...timedEvent, recurrence_rule: "FREQ=DAILY;COUNT=2" });
    await updateEventOccurrence(api, 2, 3, "1750000100", { version: 4, ...timedEvent });
    await deleteEventOccurrence(api, 2, 3, "1750000100", 4);

    expect(api.request).toHaveBeenCalledWith("/api/v1/calendars/2/events", expect.objectContaining({ method: "POST", body: JSON.stringify({ ...timedEvent, recurrence_rule: "FREQ=DAILY;COUNT=2" }) }));
    expect(api.request).toHaveBeenCalledWith("/api/v1/calendars/2/events/3/occurrences/1750000100", expect.objectContaining({ method: "PATCH", body: JSON.stringify({ version: 4, ...timedEvent }) }));
    expect(api.request).toHaveBeenCalledWith("/api/v1/calendars/2/events/3/occurrences/1750000100", expect.objectContaining({ method: "DELETE", body: JSON.stringify({ version: 4 }) }));
  });
});
