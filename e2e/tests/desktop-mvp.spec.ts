import { expect, test as base, webkit, type APIRequestContext, type BrowserContext, type Page } from "@playwright/test";

import { messages } from "../support/mailbox";
import { isolatedBackend } from "../support/isolatedBackend";

let baseURL: string;
let outbox: string;
type Backend = Awaited<ReturnType<typeof isolatedBackend>>;
const test = base.extend<{ backend: Backend }>({
  backend: [async ({}, use) => {
    const backend = await isolatedBackend();
    baseURL = backend.origin; outbox = backend.outbox;
    try { await use(backend); } finally { await backend.stop(); }
  }, { auto: true }],
  baseURL: async ({ backend }, use) => { await use(backend.origin); },
});

type Auth = { csrf: string; request: APIRequestContext };
type User = { id: number; email: string };

async function activate(page: Page, path: string): Promise<Auth> {
  await page.goto(path);
  const email = await page.locator("strong").textContent();
  await page.getByLabel("Password", { exact: true }).fill("e2e-member-password-123");
  await page.getByLabel("Confirm password").fill("e2e-member-password-123");
  await page.getByRole("button", { name: "Create account" }).click();
  await expect(page.getByRole("heading", { name: "Account created" })).toBeVisible();
  const response = await page.context().request.post("/api/v1/auth/password-login", { data: { email, password: "e2e-member-password-123" } });
  await expect(response).toBeOK();
  const csrf = (await response.json() as { csrf_token: string }).csrf_token;
  await page.goto("/dashboard");
  return { csrf, request: page.context().request };
}

async function loginDefaultAdmin(page: Page): Promise<Auth> {
  const response = await page.context().request.post("/api/v1/auth/password-login", {
    data: { email: "admin@localhost", password: "admin-default-password-2026" },
  });
  await expect(response).toBeOK();
  const { csrf_token } = await response.json() as { csrf_token: string };
  await page.goto("/");
  await expect(page.getByRole("button", { name: "Calendars" })).toBeVisible();
  return { csrf: csrf_token, request: page.context().request };
}

async function post<T>(auth: Auth, path: string, data?: unknown): Promise<T> {
  const response = await auth.request.post(path, { headers: { "x-csrf-token": auth.csrf, origin: baseURL, "sec-fetch-site": "same-origin" }, data });
  await expect(response).toBeOK();
  return response.json() as Promise<T>;
}

async function patch<T>(auth: Auth, path: string, data: unknown): Promise<T> {
  const response = await auth.request.patch(path, { headers: { "x-csrf-token": auth.csrf, origin: baseURL, "sec-fetch-site": "same-origin" }, data });
  await expect(response).toBeOK();
  return response.json() as Promise<T>;
}

async function put(auth: Auth, path: string, data: unknown): Promise<void> {
  const response = await auth.request.put(path, { headers: { "x-csrf-token": auth.csrf, origin: baseURL, "sec-fetch-site": "same-origin" }, data });
  await expect(response).toBeOK();
}

async function get<T>(auth: Auth, path: string): Promise<T> {
  const response = await auth.request.get(path);
  await expect(response).toBeOK();
  return response.json() as Promise<T>;
}

async function invitationToken(email: string) {
  await expect.poll(async () => (await messages(outbox)).find((item) => item.recipient === email && item.message_type === "invitation")?.authentication_link).toBeTruthy();
  const link = (await messages(outbox)).find((item) => item.recipient === email && item.message_type === "invitation")!.authentication_link!;
  return new URL(link).searchParams.get("token")!;
}

test.describe("desktop MVP journey", () => {
  test.skip(({ browserName }) => browserName !== "firefox", "Firefox desktop journey");

  test("authentication, collaboration, publication, ICS import and feed handling, notification delivery, and mobile primary views", async ({ browser, page }, testInfo) => {
    test.setTimeout(60_000);
    test.skip(testInfo.project.name !== "desktop", "mobile coverage runs from the isolated desktop fixture");
    const suffix = testInfo.project.name;
    const member = `member.${suffix}@e2e.example.test`;
    const admin = await loginDefaultAdmin(page);

    await post<{ id: number }>(admin, "/api/v1/admin/invitations", { email: member, display_name: "E2E Member" });
    const memberToken = await invitationToken(member);
    const memberContext: BrowserContext = await browser.newContext();
    const memberPage = await memberContext.newPage();
    const memberAuth = await activate(memberPage, `/invitations/consume?token=${encodeURIComponent(memberToken)}`);
    const users = await get<{ users: User[] }>(admin, "/api/v1/admin/users");
    const memberUser = users.users.find((user) => user.email === member);
    expect(memberUser).toBeDefined();

    const calendar = await post<{ id: number; version: number }>(admin, "/api/v1/calendars", { name: "E2E Team", description: "Private E2E description", color: "#2563eb", default_timezone: "UTC", default_event_visibility: "private", default_notification_rules_json: null });
    await put(admin, `/api/v1/calendars/${calendar.id}/acl/${memberUser!.id}`, { role: "editor" });
    const start = Math.floor(Date.now() / 1000) + 3600;
    const event = await post<{ id: number }>(memberAuth, `/api/v1/calendars/${calendar.id}/events`, { title: "E2E recurring planning", description: "Must not be public", location: "Secret room", status: "confirmed", start_utc: start, end_utc: start + 3600, timezone: "UTC", recurrence_rule: "FREQ=WEEKLY;COUNT=3" });
    expect(event.id).toBeGreaterThan(0);

    const view = await post<{ id: number }>(admin, "/api/v1/views", { name: "E2E published schedule" });
    await put(admin, `/api/v1/views/${view.id}/calendars`, { calendars: [{ calendar_id: calendar.id, position: 0, color: "#2563eb" }] });
    const publication = await post<{ token: string }>(admin, `/api/v1/views/${view.id}/publication`, { projection: "title_and_time", display_timezone: "UTC", expires_at: start + 30 * 24 * 3600 });
    const publicPage = await browser.newPage();
    await publicPage.goto(`/public/views/${publication.token}`);
    await expect(publicPage.getByText("E2E recurring planning").first()).toBeVisible();
    await expect(publicPage.getByText("Must not be public")).toHaveCount(0);
    await expect(publicPage.getByText("Secret room")).toHaveCount(0);

    const feed = await post<{ id: number }>(admin, `/api/v1/calendars/${calendar.id}/external-feeds`, { source_url: "https://fixture.invalid/controlled.ics", refresh_interval_seconds: 3600 });
    await post(admin, `/api/v1/external-feeds/${feed.id}/refresh`);
    const imported = await get<Array<{ title: string; read_only?: boolean; is_external?: boolean }>>(admin, `/api/v1/calendars/${calendar.id}/events?from=${Date.UTC(2026, 0, 1) / 1000}&to=${Date.UTC(2026, 1, 1) / 1000}`);
    expect(imported).toContainEqual(expect.objectContaining({ title: "Imported E2E event", read_only: true, is_external: true }));

    await memberPage.goto("/calendars");
    await memberPage.getByRole("button", { name: "Import ICS to E2E Team" }).click();
    await memberPage.getByLabel("ICS file").setInputFiles(new URL("../support/import.ics", import.meta.url).pathname);
    await memberPage.getByRole("button", { name: "Import", exact: true }).click();
    await expect(memberPage.getByRole("status")).toHaveText("2 events imported");
    await memberPage.getByRole("button", { name: "Done" }).click();

    const nativeImported = await get<Array<{
      id: number; title?: string; version?: number; event_kind?: string; start_utc?: number; end_utc?: number;
      timezone?: string; start_date?: string; end_date?: string; read_only?: boolean; is_external?: boolean;
    }>>(memberAuth, `/api/v1/calendars/${calendar.id}/events?from=${Date.UTC(2026, 7, 1) / 1000}&to=${Date.UTC(2026, 8, 1) / 1000}`);
    const timedImported = nativeImported.find((item) => item.title === "ICS import timed event");
    const allDayImported = nativeImported.find((item) => item.title === "ICS import all-day event");
    expect(timedImported).toMatchObject({ event_kind: "timed" });
    expect(allDayImported).toMatchObject({ event_kind: "all_day" });
    for (const imported of [timedImported, allDayImported]) {
      expect(imported).not.toHaveProperty("is_external");
      expect(imported).not.toHaveProperty("read_only");
    }

    await patch(memberAuth, `/api/v1/calendars/${calendar.id}/events/${timedImported!.id}`, {
      calendar_id: calendar.id, version: timedImported!.version, title: "ICS import timed event (edited)",
      description: "Native timed event fixture", location: "E2E room", status: "confirmed",
      start_utc: timedImported!.start_utc, end_utc: timedImported!.end_utc, timezone: timedImported!.timezone,
    });
    await patch(memberAuth, `/api/v1/calendars/${calendar.id}/events/${allDayImported!.id}`, {
      calendar_id: calendar.id, version: allDayImported!.version, title: "ICS import all-day event (edited)",
      description: "Native all-day event fixture", location: null, status: "confirmed",
      start_date: allDayImported!.start_date, end_date: allDayImported!.end_date,
    });

    // The notification support endpoint is development-only. It makes notification display
    // observable without waiting for the production worker's periodic schedule.
    const notification = await admin.request.post("/api/v1/test-support/notifications", { headers: { "x-csrf-token": admin.csrf, origin: baseURL, "sec-fetch-site": "same-origin" }, data: { event_id: event.id } });
    await expect(notification).toBeOK();
    await page.goto("/");
    await page.getByRole("button", { name: "Notifications", exact: true }).click();
    await expect(page.locator(".notif-dropdown")).toContainText("E2E recurring planning");
    {
      const mobileBrowser = await webkit.launch();
      const mobileContext = await mobileBrowser.newContext({
        baseURL,
        storageState: await page.context().storageState(),
        viewport: { width: 390, height: 844 },
        isMobile: true,
        hasTouch: true,
      });
      try {
      const mobilePage = await mobileContext.newPage();
      await mobilePage.goto("/");
      await expect(mobilePage.getByRole("heading", { name: "Events" })).toBeVisible();
      for (const view of ["Month", "Week", "Day", "Agenda"]) {
        await mobilePage.getByRole("tab", { name: view, exact: true }).click();
      }
      await expect(mobilePage.getByRole("region", { name: "Agenda" })).toBeVisible();
      } finally { await mobileContext.close(); await mobileBrowser.close(); }
    }
    await memberContext.close();
  });
});
