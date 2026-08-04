import { execFile } from "node:child_process";
import { promisify } from "node:util";

import { expect, test, type APIRequestContext, type BrowserContext, type Page } from "@playwright/test";

import { messages } from "../support/mailbox";

const exec = promisify(execFile);
const outbox = process.env.E2E_EMAIL_OUTBOX ?? ".e2e/outbox.ndjson";
const database = process.env.DATABASE_PATH ?? ".e2e/commoncal.sqlite";
const sessionSecret = process.env.E2E_SESSION_SECRET ?? "commoncal-e2e-session-secret";

type Auth = { csrf: string; request: APIRequestContext };
type User = { id: number; email: string };

async function bootstrap(email: string) {
  const { stdout } = await exec("cargo", ["run", "--quiet", "--manifest-path", "backend/Cargo.toml", "--", "bootstrap-superadmin", email, "E2E Admin"], {
    env: { ...process.env, APP_ENV: "development", DATABASE_PATH: database, SESSION_SECRET: sessionSecret },
  });
  const token = /^token=(.+)$/m.exec(stdout)?.[1];
  if (!token) throw new Error("Bootstrap did not return an invitation token");
  return token;
}

async function activate(page: Page, path: string): Promise<Auth> {
  const completed = page.waitForResponse((response) => response.url().includes("/consume") && response.request().method() === "POST");
  await page.goto(path);
  const response = await completed;
  await expect(page.getByRole("heading", { name: "CommonCal" })).toBeVisible();
  return { csrf: (await response.json() as { csrf_token: string }).csrf_token, request: page.context().request };
}

async function post<T>(auth: Auth, path: string, data?: unknown): Promise<T> {
  const response = await auth.request.post(path, { headers: { "x-csrf-token": auth.csrf }, data });
  await expect(response).toBeOK();
  return response.json() as Promise<T>;
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
  test.skip(({ browserName }) => browserName !== "chromium", "Chromium journey");

  test("bootstrap, collaboration, publication, controlled ICS, notification delivery, and mobile primary views", async ({ browser, page }, testInfo) => {
    test.skip(testInfo.project.name !== "desktop", "mobile coverage runs from the isolated desktop fixture");
    const suffix = testInfo.project.name;
    const superadmin = `admin.${suffix}@e2e.example.test`;
    const member = `member.${suffix}@e2e.example.test`;
    const adminToken = await bootstrap(superadmin);
    const admin = await activate(page, `/invitations/consume?token=${encodeURIComponent(adminToken)}`);

    await post<{ id: number }>(admin, "/api/v1/admin/invitations", { email: member, display_name: "E2E Member" });
    const memberToken = await invitationToken(member);
    const memberContext: BrowserContext = await browser.newContext();
    const memberPage = await memberContext.newPage();
    const memberAuth = await activate(memberPage, `/invitations/consume?token=${encodeURIComponent(memberToken)}`);
    const users = await get<User[]>(admin, "/api/v1/admin/users");
    const memberUser = users.find((user) => user.email === member);
    expect(memberUser).toBeDefined();

    const calendar = await post<{ id: number; version: number }>(admin, "/api/v1/calendars", { name: "E2E Team", description: "Private E2E description", color: "#2563eb", default_timezone: "UTC", default_event_visibility: "private", default_notification_rules_json: null });
    await post(admin, `/api/v1/calendars/${calendar.id}/acl/${memberUser!.id}`, { role: "editor" });
    const start = Math.floor(Date.UTC(2026, 7, 10, 9, 0, 0) / 1000);
    const event = await post<{ id: number }>(memberAuth, `/api/v1/calendars/${calendar.id}/events`, { title: "E2E recurring planning", description: "Must not be public", location: "Secret room", status: "confirmed", start_utc: start, end_utc: start + 3600, timezone: "UTC", recurrence_rule: "FREQ=WEEKLY;COUNT=3" });
    expect(event.id).toBeGreaterThan(0);

    const view = await post<{ id: number }>(admin, "/api/v1/views", { name: "E2E published schedule" });
    await post(admin, `/api/v1/views/${view.id}/calendars`, { calendars: [{ calendar_id: calendar.id, position: 0, color: "#2563eb" }] });
    const publication = await post<{ token: string }>(admin, `/api/v1/views/${view.id}/publication`, { projection: "title_and_time", display_timezone: "UTC", expires_at: start + 30 * 24 * 3600 });
    const publicPage = await browser.newPage();
    await publicPage.goto(`/public/views/${publication.token}`);
    await expect(publicPage.getByText("E2E recurring planning")).toBeVisible();
    await expect(publicPage.getByText("Must not be public")).toHaveCount(0);
    await expect(publicPage.getByText("Secret room")).toHaveCount(0);

    const feed = await post<{ id: number }>(admin, `/api/v1/calendars/${calendar.id}/external-feeds`, { source_url: "https://fixture.invalid/controlled.ics", refresh_interval_seconds: 3600 });
    await post(admin, `/api/v1/external-feeds/${feed.id}/refresh`);
    const imported = await get<Array<{ title: string; read_only?: boolean; is_external?: boolean }>>(admin, `/api/v1/calendars/${calendar.id}/events?from=${Date.UTC(2026, 0, 1) / 1000}&to=${Date.UTC(2026, 1, 1) / 1000}`);
    expect(imported).toContainEqual(expect.objectContaining({ title: "Imported E2E event", read_only: true, is_external: true }));

    // The notification support endpoint is development-only. It makes notification display
    // observable without waiting for the production worker's periodic schedule.
    await post(admin, "/api/v1/test-support/notifications", { event_id: event.id });
    await page.goto("/");
    await expect(page.getByRole("status", { name: "Notifications" })).toContainText("E2E recurring planning");
    {
      const mobileContext = await browser.newContext({
        storageState: await page.context().storageState(),
        viewport: { width: 390, height: 844 },
        isMobile: true,
      });
      const mobilePage = await mobileContext.newPage();
      await mobilePage.goto("/");
      await expect(mobilePage.getByRole("heading", { name: "Events" })).toBeVisible();
      for (const view of ["Month view", "Week view", "Day view", "Agenda view"]) {
        await mobilePage.getByRole("button", { name: view }).click();
      }
      await expect(mobilePage.getByRole("region", { name: "Agenda" })).toBeVisible();
      await mobileContext.close();
    }
    await memberContext.close();
  });
});
