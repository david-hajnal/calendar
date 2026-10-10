import { writeFile } from "node:fs/promises";
import { expect, test as base } from "@playwright/test";
import { isolatedBackend } from "../support/isolatedBackend";

const test = base.extend<{ backend: Awaited<ReturnType<typeof isolatedBackend>> }>({
  backend: [async ({}, use) => {
    const backend = await isolatedBackend();
    try { await use(backend); } finally { await backend.stop(); }
  }, { auto: true }],
  baseURL: async ({ backend }, use) => { await use(backend.origin); },
});

test("live mobile Agenda preserves calendar context through Settings", async ({ page, backend }, testInfo) => {
  test.skip(testInfo.project.name !== "mobile", "Mobile Agenda journey");
  const login = await page.request.post("/api/v1/auth/password-login", { data: { email: "admin@localhost", password: "admin-default-password-2026" } });
  await expect(login).toBeOK();
  const headers = { "x-csrf-token": (await login.json()).csrf_token as string, origin: backend.origin, "sec-fetch-site": "same-origin" };
  const calendarResponse = await page.request.post("/api/v1/calendars", { headers, data: { name: "Mobile Work", description: null, color: "#2563eb", default_timezone: "UTC", default_event_visibility: "private", default_notification_rules_json: null } });
  await expect(calendarResponse).toBeOK();
  const calendar = await calendarResponse.json() as { id: number };
  for (let i = 0; i < 8; i++) {
    const start = new Date(2026, 9, 9 + i, 9, 30).getTime() / 1000;
    const event = await page.request.post(`/api/v1/calendars/${calendar.id}/events`, { headers, data: { title: `Live planning ${i}: priorities and a title that wraps on a phone`, description: "Live API fixture", location: "Meeting room 2", status: "confirmed", start_utc: start, end_utc: start + 3600, timezone: "UTC" } });
    await expect(event).toBeOK();
  }
  await page.goto("/dashboard?date=2026-10-09&source=bookmark");
  const agenda = page.getByRole("region", { name: "Agenda", exact: true });
  await expect(agenda).toBeVisible();
  await expect(agenda.getByRole("button", { name: /Live planning 0:/ })).toBeVisible();
  await expect(page.getByText("Sample Agenda")).toHaveCount(0);
  await expect(page).toHaveURL(/view=agenda/);
  const evidence = new URL("../../docs/plans/mobile-usability/evidence/", import.meta.url).pathname;
  await page.screenshot({ path: `${evidence}slice-02-live-agenda-390.png`, scale: "css" });
  const metrics = await agenda.evaluate(el => ({ font: getComputedStyle(el).fontSize, width: el.getBoundingClientRect().width, viewport: innerWidth, targetHeights: [...el.querySelectorAll("button")].map(button => button.getBoundingClientRect().height) }));
  expect(metrics.font).toBe("16px");
  expect(metrics.width).toBeLessThanOrEqual(metrics.viewport);
  expect(metrics.targetHeights.every(height => height >= 44)).toBe(true);
  await page.evaluate(() => window.scrollTo(0, 500));
  await expect.poll(() => page.evaluate(() => history.state.commoncalCalendar.snapshot.scroll.page)).toBeGreaterThan(100);
  const scroll = await page.evaluate(() => window.scrollY);
  await page.screenshot({ path: `${evidence}slice-02-live-agenda-390-scrolled.png`, scale: "css" });
  // Header remains reachable while the Agenda is scrolled.
  await page.getByRole("button", { name: "Settings", exact: true }).click();
  await page.getByRole("button", { name: "Return to calendar" }).click();
  await expect(agenda.getByRole("button", { name: /Live planning 0:/ })).toBeAttached();
  await expect.poll(() => page.evaluate(() => window.scrollY)).toBe(scroll);
  await page.getByRole("tab", { name: "Day", exact: true }).click();
  await expect(page.getByRole("region", { name: "Day calendar" })).toBeVisible();
  await page.locator(".mobile-calendar-filters summary").click();
  await page.locator(".mobile-calendar-filters").getByRole("checkbox", { name: "Mobile Work" }).uncheck();
  await page.getByRole("button", { name: "Settings", exact: true }).click();
  await page.getByRole("button", { name: "Return to calendar" }).click();
  await expect(page.getByRole("region", { name: "Day calendar" })).toBeVisible();
  await expect(page).toHaveURL(/date=2026-10-09/);
  await expect(page).toHaveURL(/source=bookmark/);
  await expect(page).toHaveURL(/view=day/);
  await page.locator(".mobile-calendar-filters summary").click();
  await expect(page.locator(".mobile-calendar-filters").getByRole("checkbox", { name: "Mobile Work" })).not.toBeChecked();
  await writeFile(`${evidence}slice-02-browser.json`, JSON.stringify({ browser: "WebKit", viewport: { width: 390, height: 664 }, source: "isolated live backend", liveEvents: 8, metrics, restoredScroll: scroll, settingsReturn: true, retainedDate: "2026-10-09", retainedView: "day", emptyFiltersRetained: true }, null, 2));
});

test("signed-in mobile Back and Forward follow sections without returning to login", async ({ page }, testInfo) => {
  test.skip(testInfo.project.name !== "mobile", "Mobile navigation journey");
  await page.goto("/forgot-password");
  await page.goto("/login?redirect=%2Fdashboard%3Fview%3Dagenda%26date%3D2026-10-09");
  await page.getByRole("button", { name: "Password", exact: true }).click();
  await page.getByLabel("Email address").fill("admin@localhost");
  await page.getByLabel("Password", { exact: true }).fill("admin-default-password-2026");
  const beforeSignIn = await page.evaluate(() => history.length);
  await page.getByRole("button", { name: "Sign in", exact: true }).click();
  await expect(page.getByRole("region", { name: "Agenda", exact: true })).toBeVisible();
  expect(await page.evaluate(() => history.length)).toBe(beforeSignIn);
  await page.getByRole("button", { name: "Settings", exact: true }).click();
  await expect(page.getByRole("heading", { name: "Account", exact: true })).toBeVisible();
  expect(await page.evaluate(() => history.length)).toBe(beforeSignIn + 1);
  await page.goBack();
  await expect(page.getByRole("region", { name: "Agenda", exact: true })).toBeVisible();
  await expect(page).toHaveURL(/date=2026-10-09/);
  await expect(page.getByRole("heading", { name: "Sign in", exact: true })).toHaveCount(0);
  await page.goForward();
  await expect(page.getByRole("heading", { name: "Account", exact: true })).toBeVisible();
  await page.goBack();
  await expect(page.getByRole("region", { name: "Agenda", exact: true })).toBeVisible();
  // The entry predecessor remains leaveable, without fabricating a Back loop.
  await page.goBack();
  await expect(page).toHaveURL(/forgot-password$/);
  await page.goto("/login?redirect=%2Fsettings%2Faccount");
  await expect(page.getByRole("heading", { name: "Account", exact: true })).toBeVisible();
  await expect(page.getByRole("heading", { name: "Sign in", exact: true })).toHaveCount(0);
  const evidence = new URL("../../docs/plans/mobile-usability/evidence/", import.meta.url).pathname;
  await page.screenshot({ path: `${evidence}slice-03-signed-in-login-return.png`, scale: "css" });
  await page.goto("/login?redirect=%2Flogin%2Fconsume%3Ftoken%3Dnever-use");
  await expect(page.getByRole("region", { name: "Agenda", exact: true })).toBeVisible();
  await expect(page).toHaveURL(/dashboard/);
  let consentDocumentNavigation = false;
  await page.route("**/consent?handoff=mobile-test", async route => {
    consentDocumentNavigation = route.request().isNavigationRequest();
    await route.fulfill({ contentType: "text/html", body: "<h1>Core consent fixture</h1>" });
  });
  await page.goto("/login?redirect=%2Fconsent%3Fhandoff%3Dmobile-test");
  await expect(page.getByRole("heading", { name: "Core consent fixture" })).toBeVisible();
  expect(consentDocumentNavigation).toBe(true);
  await writeFile(`${evidence}slice-03-browser.json`, JSON.stringify({ browser: "WebKit", passwordLoginReplaces: true, settingsPushes: true, nativeBackForward: true, calendarContext: "agenda / 2026-10-09", entryCanLeave: true, authenticatedLoginGuard: true, consumptionLoopRejected: true, consentDocumentNavigation }, null, 2));
});
