import { expect, test as base } from "@playwright/test";
import { messages } from "../support/mailbox";
import { isolatedBackend } from "../support/isolatedBackend";
let baseURL: string;
let outbox: string;
type Backend = Awaited<ReturnType<typeof isolatedBackend>>;
const test = base.extend<{ backend: Backend }>({
  backend: [async ({}, use, testInfo) => {
    const backend = await isolatedBackend(testInfo.title.startsWith("bootstrap"), testInfo.title.startsWith("pending"));
    baseURL = backend.origin; outbox = backend.outbox;
    try { await use(backend); } finally { await backend.stop(); }
  }, { auto: true }],
  baseURL: async ({ backend }, use) => { await use(backend.origin); },
});

test("pending and inactive statuses explain restrictions and pending accounts can be disabled", async ({ page }, testInfo) => {
  expect((await page.request.get("/api/v1/dev/login?email=admin%40localhost", { maxRedirects: 0 })).status()).toBe(302);
  await page.goto("/settings/users");
  const pending = page.getByRole("row").filter({ hasText: "pending@example.test" });
  const inactive = page.getByRole("row").filter({ hasText: "inactive@example.test" });
  await expect(pending).toContainText("Awaiting approval");
  await expect(inactive).toContainText("Access disabled");
  for (const row of [pending, inactive]) {
    await expect(row).toContainText("Email changes unavailable");
    await expect(row.getByRole("button", { name: "Change email", exact: true })).toHaveCount(0);
    await expect(row.getByRole("button", { name: "Resend invitation", exact: true })).toHaveCount(0);
  }
  await expect(inactive.getByRole("button", { name: "Disable user", exact: true })).toHaveCount(0);
  await page.screenshot({ path: testInfo.outputPath("pending-inactive-statuses.png"), fullPage: true });
  await pending.getByRole("button", { name: "Disable user", exact: true }).click();
  await page.getByRole("button", { name: "Confirm disable", exact: true }).click();
  await expect(page.getByRole("status")).toHaveText("Access disabled for pending@example.test.");
  await expect(pending).toContainText("inactive");
});

test("invitation submission shows loading, rejects duplicate clicks and permits retry after failure", async ({ page }) => {
  expect((await page.request.get("/api/v1/dev/login?email=admin%40localhost", { maxRedirects: 0 })).status()).toBe(302);
  await page.goto("/settings/users");
  await page.getByLabel("Email address").fill("ui-retry@example.test");
  let release!: () => void;
  const pending = new Promise<void>(resolve => { release = resolve; });
  let attempts = 0;
  await page.route("**/api/v1/admin/invitations", async route => {
    attempts++;
    if (attempts === 1) {
      await pending;
      await route.fulfill({ status: 503, contentType: "application/json", body: JSON.stringify({ error: { code: "invitation_delivery_failed", message: "The invitation could not be sent." } }) });
    } else { await route.continue(); }
  });
  await page.getByRole("button", { name: "Send invitation", exact: true }).click();
  await expect(page.getByRole("button", { name: "Sending…", exact: true })).toBeDisabled();
  expect(attempts).toBe(1);
  release();
  await expect(page.getByRole("alert")).toHaveText("The invitation could not be sent. Use Resend invitation in Users to retry.");
  await expect(page.getByRole("button", { name: "Send invitation", exact: true })).toBeEnabled();
  await page.getByRole("button", { name: "Send invitation", exact: true }).click();
  await expect(page.getByRole("status")).toHaveText("Invitation sent to ui-retry@example.test.");
  expect(attempts).toBe(2);
  await expect.poll(async () => (await messages(outbox)).some(message => message.recipient === "ui-retry@example.test")).toBe(true);
});

test("bootstrap accepts the legacy invitation URL with explicit password setup and admin access", async ({ page, backend }, testInfo) => {
  expect(backend.bootstrapToken).toBeTruthy();
  const url = `${baseURL}/invitations/consume?token=${encodeURIComponent(backend.bootstrapToken!)}`;
  const document = await page.goto(url);
  expect(document!.headers()["cache-control"]).toBe("no-store");
  expect(document!.headers()["referrer-policy"]).toBe("no-referrer");
  await expect(page.getByText("bootstrap@example.test", { exact: true })).toBeVisible();
  await expect.poll(() => new URL(page.url()).searchParams.has("token")).toBe(false);
  expect((await page.request.get("/api/v1/auth/session")).status()).toBe(401);
  await page.getByLabel("Password", { exact: true }).fill("bootstrap-browser-password-123");
  await page.getByLabel("Confirm password", { exact: true }).fill("mismatched-browser-password");
  await page.getByRole("button", { name: "Create account", exact: true }).click();
  await expect(page.getByRole("alert")).toHaveText("Passwords must match.");
  await page.getByLabel("Confirm password", { exact: true }).fill("bootstrap-browser-password-123");
  await page.getByRole("button", { name: "Create account", exact: true }).click();
  await expect(page.getByRole("heading", { name: "Account created" })).toBeVisible();
  expect((await page.request.get("/api/v1/auth/session")).status()).toBe(401);
  await page.getByRole("link", { name: "Sign in", exact: true }).click();
  await page.getByRole("button", { name: "Password", exact: true }).click();
  await page.getByLabel("Email address").fill("bootstrap@example.test");
  await page.getByLabel("Password", { exact: true }).fill("bootstrap-browser-password-123");
  await page.getByRole("button", { name: "Sign in", exact: true }).click();
  await expect(page.getByRole("button", { name: "Settings", exact: true })).toBeVisible();
  await page.goto("/settings/users");
  const row = page.getByRole("row").filter({ hasText: "bootstrap@example.test" });
  await expect(row).toContainText("registered");
  expect((await page.request.get("/api/v1/admin/users")).status()).toBe(200);
  await page.screenshot({ path: testInfo.outputPath("bootstrap-admin-registered.png"), fullPage: true });
  await page.goto(url);
  await expect(page.getByRole("alert")).toHaveText("Invitation is invalid or expired.");
});

test("registered users retain captured email-link login after password-required invitations", async ({ page, browser }, testInfo) => {
  const email = `passwordless-${testInfo.project.name}@example.test`;
  expect((await page.request.get(`/api/v1/dev/login?email=${encodeURIComponent(email)}`, { maxRedirects: 0 })).status()).toBe(302);
  const before = await (await page.request.get("/api/v1/auth/session")).json() as { user: { id: number } };
  const context = await browser.newContext({ baseURL, viewport: page.viewportSize(), isMobile: testInfo.project.name === "mobile", hasTouch: testInfo.project.name === "mobile" });
  try {
    const login = await context.newPage();
    await login.goto("/login");
    await login.getByLabel("Email address").fill(email);
    await login.getByRole("button", { name: "Email me a login link", exact: true }).click();
    await expect(login.getByRole("status")).toContainText("Check your email");
    await expect.poll(async () => (await messages(outbox)).find(message => message.recipient === email && message.message_type === "login_link")?.authentication_link).toBeTruthy();
    const link = (await messages(outbox)).find(message => message.recipient === email && message.message_type === "login_link")!.authentication_link!;
    expect(new URL(link).origin).toBe(baseURL);
    expect(new URL(link).pathname).toBe("/login/consume");
    await login.goto(link);
    await expect(login.getByText("You are signed in.", { exact: true })).toBeVisible();
    await expect.poll(() => new URL(login.url()).searchParams.has("token")).toBe(false);
    const after = await (await login.request.get("/api/v1/auth/session")).json() as { user: { id: number } };
    expect(after.user.id).toBe(before.user.id);
    await login.goto("/dashboard");
    await expect(login.getByRole("button", { name: "Settings", exact: true })).toBeVisible();
  } finally { await context.close(); }
});

test("admin invites; recipient creates a password and logs in; member and missing CSRF are denied", async ({ page, browser }, testInfo) => {
  const email = `invited-${testInfo.project.name}-${Date.now()}@example.test`;
  const password = "invite-browser-password-123";
  const login = await page.request.post("/api/v1/auth/password-login", {
    data: { email: "admin@localhost", password: "admin-default-password-2026" },
  });
  expect(login.ok()).toBeTruthy();
  await page.goto("/settings/account");
  await page.getByRole("button", { name: "Users", exact: true }).click();
  await page.getByLabel("Email address").fill(email);
  await page.getByRole("button", { name: "Send invitation", exact: true }).click();
  await expect(page.getByRole("status")).toHaveText(`Invitation sent to ${email}.`);
  const row = page.getByRole("row").filter({ hasText: email });
  await expect(row).toContainText("invited");
  const beforeUsers = await (await page.request.get("/api/v1/admin/users")).json() as { users: { id: number; email: string }[] };
  const invitedId = beforeUsers.users.find((user) => user.email === email)!.id;
  await page.screenshot({ path: testInfo.outputPath("settings-users-resend.png"), fullPage: true });
  await expect.poll(async () => (await messages(outbox)).find((message) => message.recipient === email)?.authentication_link).toBeTruthy();
  const oldLink = (await messages(outbox)).find((message) => message.recipient === email)!.authentication_link!;
  await row.getByRole("button", { name: "Resend invitation" }).click();
  await expect(page.getByRole("status")).toHaveText(`Invitation sent to ${email}. The previous link no longer works.`);
  await expect.poll(async () => (await messages(outbox)).filter((message) => message.recipient === email).length).toBe(2);
  const link = (await messages(outbox)).filter((message) => message.recipient === email).at(-1)!.authentication_link!;
  const context = await browser.newContext({ viewport: page.viewportSize(), isMobile: testInfo.project.name === "mobile", hasTouch: testInfo.project.name === "mobile" });
  try {
    const recipient = await context.newPage();
    await recipient.goto(oldLink);
    await expect(recipient.getByRole("alert")).toHaveText("Invitation is invalid or expired.");
    await recipient.goto(link);
    await expect(recipient.getByText(email, { exact: true })).toBeVisible();
    await expect.poll(() => new URL(recipient.url()).searchParams.has("token")).toBe(false);
    // Visiting the email link is safe; passwordless login is unavailable until setup.
    expect((await recipient.request.get("/api/v1/auth/session")).status()).toBe(401);
    await recipient.getByLabel("Password", { exact: true }).fill(password);
    await recipient.getByLabel("Confirm password", { exact: true }).fill(password);
    await recipient.getByRole("button", { name: "Create account" }).click();
    await expect(recipient.getByRole("heading", { name: "Account created" })).toBeVisible();
    expect((await recipient.request.get("/api/v1/auth/session")).status()).toBe(401);
    await recipient.screenshot({ path: testInfo.outputPath("invitation-accepted.png"), fullPage: true });
    await recipient.getByRole("link", { name: "Sign in", exact: true }).click();
    await recipient.getByRole("button", { name: "Password", exact: true }).click();
    await recipient.getByLabel("Email address").fill(email);
    await recipient.getByLabel("Password", { exact: true }).fill(password);
    await recipient.getByRole("button", { name: "Sign in", exact: true }).click();
    await expect(recipient.getByRole("button", { name: "Settings", exact: true })).toBeVisible();
    await recipient.goto("/settings/account");
    await expect(recipient.getByRole("heading", { name: "Account", exact: true })).toBeVisible();
    await expect(recipient.getByRole("button", { name: "Users", exact: true })).toHaveCount(0);
    await recipient.goto("/settings/users");
    await expect(recipient.getByRole("alert")).toHaveText("Only admins can manage users.");
    expect((await recipient.request.get("/api/v1/admin/users")).status()).toBe(403);
    const session = await (await recipient.request.get("/api/v1/auth/session")).json() as { csrf_token: string };
    expect((await recipient.request.post("/api/v1/admin/invitations", {
      headers: { "x-csrf-token": session.csrf_token, origin: baseURL, "sec-fetch-site": "same-origin" }, data: { email: "forbidden@example.test" },
    })).status()).toBe(403);
    await recipient.goto(link);
    await expect(recipient.getByRole("alert")).toHaveText("Invitation is invalid or expired.");
  } finally { await context.close(); }
  await page.reload();
  await expect(row).toContainText("registered");
  await expect(row.getByRole("button", { name: "Resend invitation" })).toHaveCount(0);
  const afterUsers = await (await page.request.get("/api/v1/admin/users")).json() as { users: { id: number; email: string }[] };
  expect(afterUsers.users.find((user) => user.email === email)!.id).toBe(invitedId);
  await page.screenshot({ path: testInfo.outputPath("settings-users-registered.png"), fullPage: true });
  expect((await page.request.post("/api/v1/admin/invitations", {
    headers: { origin: baseURL, "sec-fetch-site": "same-origin" }, data: { email: "missing-csrf@example.test" },
  })).status()).toBe(403);
});

test("admin confirms disable; existing browser and CalDAV access stop and invited links fail", async ({ page, browser }, testInfo) => {
  const email = `disable-${testInfo.project.name}-${Date.now()}@example.test`;
  const invitedEmail = `disabled-invited-${testInfo.project.name}-${Date.now()}@example.test`;
  expect((await page.request.get("/api/v1/dev/login?email=admin%40localhost", { maxRedirects: 0 })).status()).toBe(302);
  const context = await browser.newContext({ viewport: page.viewportSize(), isMobile: testInfo.project.name === "mobile", hasTouch: testInfo.project.name === "mobile" });
  try {
    const member = await context.newPage();
    expect((await member.request.get(`${baseURL}/api/v1/dev/login?email=${encodeURIComponent(email)}`, { maxRedirects: 0 })).status()).toBe(302);
    await member.goto(`${baseURL}/settings/account`);
    const session = await (await member.request.get(`${baseURL}/api/v1/auth/session`)).json() as { user: { id: number }; csrf_token: string };
    const issued = await member.request.post(`${baseURL}/api/v1/calendar-connections/apple/passwords`, {
      headers: { "x-csrf-token": session.csrf_token, origin: baseURL, "sec-fetch-site": "same-origin" }, data: { label: "Disable test calendar" },
    });
    expect(issued.status()).toBe(201);
    const credential = await issued.json() as { username: string; clear_password: string };
    const davHeaders = { authorization: `Basic ${Buffer.from(`${credential.username}:${credential.clear_password}`).toString("base64")}` };
    expect((await member.request.fetch(`${baseURL}/dav/`, { method: "PROPFIND", headers: { ...davHeaders, depth: "0" } })).ok()).toBeTruthy();
    expect((await member.request.post(`${baseURL}/api/v1/admin/users/${session.user.id}/suspend`, {
      headers: { "x-csrf-token": session.csrf_token, origin: baseURL, "sec-fetch-site": "same-origin" },
    })).status()).toBe(403);
    await page.goto("/settings/users");
    const self = page.getByRole("row").filter({ hasText: "admin@localhost" });
    await expect(self.getByRole("button", { name: "Disable user", exact: true })).toHaveCount(0);
    const row = page.getByRole("row").filter({ hasText: email });
    expect((await page.request.post(`/api/v1/admin/users/${session.user.id}/suspend`, { headers: { origin: baseURL, "sec-fetch-site": "same-origin" } })).status()).toBe(403);
    await row.getByRole("button", { name: "Disable user", exact: true }).click();
    await expect(page.getByRole("group", { name: "Disable user?" })).toContainText(email);
    await page.getByRole("button", { name: "Cancel", exact: true }).click();
    expect((await member.request.get(`${baseURL}/api/v1/auth/session`)).status()).toBe(200);
    await row.getByRole("button", { name: "Disable user", exact: true }).click();
    await page.evaluate(() => window.scrollTo(0, 0));
    await page.screenshot({ path: testInfo.outputPath("disable-confirmation.png"), fullPage: true });
    await page.getByRole("button", { name: "Confirm disable", exact: true }).click();
    await expect(page.getByRole("status")).toHaveText(`Access disabled for ${email}.`);
    await expect(row).toContainText("inactive");
    await expect(row.getByRole("button", { name: "Disable user", exact: true })).toHaveCount(0);
    expect((await member.request.get(`${baseURL}/api/v1/auth/session`)).status()).toBe(401);
    expect((await member.request.fetch(`${baseURL}/dav/`, { method: "PROPFIND", headers: { ...davHeaders, depth: "0" } })).status()).toBe(401);
    await member.reload();
    await expect(member.getByRole("button", { name: "Password", exact: true })).toBeVisible();
    await page.getByLabel("Email address").fill(invitedEmail);
    await page.getByRole("button", { name: "Send invitation", exact: true }).click();
    await expect(page.getByRole("status")).toHaveText(`Invitation sent to ${invitedEmail}.`);
    await expect.poll(async () => (await messages(outbox)).find((message) => message.recipient === invitedEmail)?.authentication_link).toBeTruthy();
    const link = (await messages(outbox)).find((message) => message.recipient === invitedEmail)!.authentication_link!;
    const invitedRow = page.getByRole("row").filter({ hasText: invitedEmail });
    await invitedRow.getByRole("button", { name: "Disable user", exact: true }).click();
    await page.getByRole("button", { name: "Confirm disable", exact: true }).click();
    await expect(invitedRow).toContainText("inactive");
    await expect(invitedRow.getByRole("button", { name: "Resend invitation", exact: true })).toHaveCount(0);
    await member.goto(link);
    await expect(member.getByRole("alert")).toHaveText("Invitation is invalid or expired.");
    await page.evaluate(() => window.scrollTo(0, 0));
    await page.screenshot({ path: testInfo.outputPath("settings-users-disabled.png"), fullPage: true });
  } finally { await context.close(); }
});

test("forgot password resets through captured mail and ends older browser sessions", async ({ page, browser }, testInfo) => {
  const email = `recover-${testInfo.project.name}-${Date.now()}@example.test`;
  const oldPassword = "old-recovery-password-123";
  const newPassword = "new-recovery-password-456";
  expect((await page.request.get("/api/v1/dev/login?email=admin%40localhost", { maxRedirects: 0 })).status()).toBe(302);
  await page.goto("/settings/users");
  await page.getByLabel("Email address").fill(email);
  await page.getByRole("button", { name: "Send invitation", exact: true }).click();
  await expect(page.getByRole("status")).toHaveText(`Invitation sent to ${email}.`);
  await expect.poll(async () => (await messages(outbox)).find((message) => message.recipient === email && message.message_type === "invitation")?.authentication_link).toBeTruthy();
  const invite = (await messages(outbox)).find((message) => message.recipient === email && message.message_type === "invitation")!.authentication_link!;
  const contextOptions = { viewport: page.viewportSize(), isMobile: testInfo.project.name === "mobile", hasTouch: testInfo.project.name === "mobile" };
  const memberContext = await browser.newContext(contextOptions);
  const resetContext = await browser.newContext(contextOptions);
  try {
    const member = await memberContext.newPage();
    await member.goto(invite);
    await member.getByLabel("Password", { exact: true }).fill(oldPassword);
    await member.getByLabel("Confirm password", { exact: true }).fill(oldPassword);
    await member.getByRole("button", { name: "Create account", exact: true }).click();
    await expect(member.getByRole("heading", { name: "Account created" })).toBeVisible();
    // Establish an older browser session through the existing development fixture.
    expect((await member.request.get(`${baseURL}/api/v1/dev/login?email=${encodeURIComponent(email)}`, { maxRedirects: 0 })).status()).toBe(302);
    await member.goto(`${baseURL}/settings/account`);
    const before = await (await member.request.get(`${baseURL}/api/v1/auth/session`)).json() as { user: { id: number }; csrf_token: string };
    const issued = await member.request.post(`${baseURL}/api/v1/calendar-connections/apple/passwords`, { headers: { "x-csrf-token": before.csrf_token, origin: baseURL, "sec-fetch-site": "same-origin" }, data: { label: "Recovery retained calendar client" } });
    expect(issued.status()).toBe(201);
    const credential = await issued.json() as { username: string; clear_password: string };
    const davHeaders = { authorization: `Basic ${Buffer.from(`${credential.username}:${credential.clear_password}`).toString("base64")}`, depth: "0" };
    const reset = await resetContext.newPage();
    await reset.goto(`${baseURL}/login`);
    await reset.getByRole("button", { name: "Password", exact: true }).click();
    await reset.getByRole("link", { name: "Forgot password?", exact: true }).click();
    await reset.getByLabel("Email address").fill(email);
    await reset.getByRole("button", { name: "Send reset link", exact: true }).click();
    await expect(reset.getByRole("status")).toHaveText("If the account is eligible, a password reset link will be sent.");
    await reset.screenshot({ path: testInfo.outputPath("recovery-request.png"), fullPage: true });
    await expect.poll(async () => (await messages(outbox)).find((message) => message.recipient === email && message.message_type === "password_reset")?.authentication_link).toBeTruthy();
    const link = (await messages(outbox)).find((message) => message.recipient === email && message.message_type === "password_reset")!.authentication_link!;
    const resetDocument = await reset.goto(link);
    expect(resetDocument!.headers()["cache-control"]).toBe("no-store");
    expect(resetDocument!.headers()["referrer-policy"]).toBe("no-referrer");
    await expect.poll(() => new URL(reset.url()).searchParams.has("token")).toBe(false);
    expect((await member.request.get(`${baseURL}/api/v1/auth/session`)).status()).toBe(200); // Visiting a link does not consume it.
    await reset.getByLabel("New password", { exact: true }).fill(newPassword);
    await reset.getByLabel("Confirm password", { exact: true }).fill(newPassword);
    await reset.getByRole("button", { name: "Reset password", exact: true }).click();
    await expect(reset.getByRole("status")).toHaveText("Your password has been changed. Sign in with your new password.");
    await reset.screenshot({ path: testInfo.outputPath("password-reset-success.png"), fullPage: true });
    expect((await reset.request.get(`${baseURL}/api/v1/auth/session`)).status()).toBe(401); // Reset creates no session.
    expect((await member.request.get(`${baseURL}/api/v1/auth/session`)).status()).toBe(401);
    expect((await member.request.fetch(`${baseURL}/dav/`, { method: "PROPFIND", headers: davHeaders })).ok()).toBeTruthy();
    await reset.getByRole("link", { name: "Sign in", exact: true }).click();
    await reset.getByRole("button", { name: "Password", exact: true }).click();
    await reset.getByLabel("Email address").fill(email);
    await reset.getByLabel("Password", { exact: true }).fill(newPassword);
    await reset.getByRole("button", { name: "Sign in", exact: true }).click();
    await expect(reset.getByRole("button", { name: "Settings", exact: true })).toBeVisible();
    const after = await (await reset.request.get(`${baseURL}/api/v1/auth/session`)).json() as { user: { id: number } };
    expect(after.user.id).toBe(before.user.id);
    expect((await member.request.post(`${baseURL}/api/v1/auth/password-login`, { data: { email, password: oldPassword } })).status()).toBe(401);
    await reset.goto(link);
    await reset.getByLabel("New password", { exact: true }).fill(newPassword);
    await reset.getByLabel("Confirm password", { exact: true }).fill(newPassword);
    await reset.getByRole("button", { name: "Reset password", exact: true }).click();
    await expect(reset.getByRole("alert")).toHaveText("Password reset link is invalid or expired.");
    await expect(reset.getByRole("link", { name: "Request a new reset link" })).toBeVisible();
  } finally { await memberContext.close(); await resetContext.close(); }
});

test("personal email confirmation preserves identity and calendars and ends browser sessions", async ({ page, browser }, testInfo) => {
  const email = `email-${testInfo.project.name}-${Date.now()}@example.test`;
  const newEmail = `changed-${email}`;
  const password = "email-change-browser-password";
  expect((await page.request.get("/api/v1/dev/login?email=admin%40localhost", { maxRedirects: 0 })).status()).toBe(302);
  await page.goto("/settings/users");
  await page.getByLabel("Email address").fill(email);
  await page.getByRole("button", { name: "Send invitation", exact: true }).click();
  await expect(page.getByRole("status")).toHaveText(`Invitation sent to ${email}.`);
  const invite = (await messages(outbox)).find((message) => message.recipient === email && message.message_type === "invitation")!.authentication_link!;
  const options = { viewport: page.viewportSize(), isMobile: testInfo.project.name === "mobile", hasTouch: testInfo.project.name === "mobile" };
  const memberContext = await browser.newContext(options); const confirmContext = await browser.newContext(options);
  try {
    const member = await memberContext.newPage();
    await member.goto(invite);
    await member.getByLabel("Password", { exact: true }).fill(password);
    await member.getByLabel("Confirm password", { exact: true }).fill(password);
    await member.getByRole("button", { name: "Create account", exact: true }).click();
    await expect(member.getByRole("heading", { name: "Account created" })).toBeVisible();
    expect((await member.request.get(`${baseURL}/api/v1/dev/login?email=${encodeURIComponent(email)}`, { maxRedirects: 0 })).status()).toBe(302);
    await member.goto(`${baseURL}/settings/account`);
    const before = await (await member.request.get(`${baseURL}/api/v1/auth/session`)).json() as { user: { id: number }; csrf_token: string };
    const headers = { "x-csrf-token": before.csrf_token, origin: baseURL, "sec-fetch-site": "same-origin" };
    const created = await member.request.post(`${baseURL}/api/v1/calendars`, { headers, data: { name: "Email preserved calendar", color: "#2563eb", default_timezone: "UTC", default_event_visibility: "private" } });
    expect(created.status()).toBe(201); const calendar = await created.json() as { id: number };
    const issued = await member.request.post(`${baseURL}/api/v1/calendar-connections/apple/passwords`, { headers, data: { label: "Email preserved connection" } });
    expect(issued.status()).toBe(201); const credential = await issued.json() as { clear_password: string };
    const denied = await member.request.post(`${baseURL}/api/v1/account/email-changes`, { data: { email: newEmail, current_password: password } });
    expect(denied.status()).toBe(403);
    await member.getByLabel("New email address").fill(newEmail);
    await member.getByLabel("Current password").fill("incorrect-password");
    await member.getByRole("button", { name: "Send confirmation", exact: true }).click();
    await expect(member.getByRole("alert")).toHaveText("Current password is incorrect.");
    await member.getByLabel("Current password").fill(password);
    await member.getByRole("button", { name: "Send confirmation", exact: true }).click();
    await expect(member.getByRole("status")).toContainText("Your current email stays active");
    await expect(member.getByText(newEmail, { exact: true })).toBeVisible();
    await member.screenshot({ path: testInfo.outputPath("account-pending-email.png"), fullPage: true });
    if (testInfo.project.name === "desktop") {
      const oldLogin = await member.request.post(`${baseURL}/api/v1/auth/password-login`, { data: { email, password } }); expect(oldLogin.status()).toBe(200);
      expect((await member.request.post(`${baseURL}/api/v1/auth/password-login`, { data: { email: newEmail, password } })).status()).toBe(401);
    }
    const link = (await messages(outbox)).find((message) => message.recipient === newEmail && message.message_type === "email_confirmation")!.authentication_link!;
    const confirm = await confirmContext.newPage();
    const document = await confirm.goto(link);
    expect(document!.headers()["cache-control"]).toBe("no-store"); expect(document!.headers()["referrer-policy"]).toBe("no-referrer");
    await expect.poll(() => new URL(confirm.url()).searchParams.has("token")).toBe(false);
    expect((await member.request.get(`${baseURL}/api/v1/auth/session`)).status()).toBe(200);
    await confirm.getByRole("button", { name: "Confirm new email", exact: true }).click();
    await expect(confirm.getByRole("status")).toContainText("Your email has been changed.");
    await confirm.screenshot({ path: testInfo.outputPath("email-confirmed.png"), fullPage: true });
    expect((await member.request.get(`${baseURL}/api/v1/auth/session`)).status()).toBe(401);
    expect((await confirm.request.get(`${baseURL}/api/v1/auth/session`)).status()).toBe(401);
    expect((await member.request.fetch(`${baseURL}/dav/`, { method: "PROPFIND", headers: { authorization: `Basic ${Buffer.from(`${newEmail}:${credential.clear_password}`).toString("base64")}`, depth: "0" } })).ok()).toBeTruthy();
    await expect.poll(async () => (await messages(outbox)).some((message) => message.recipient === email && message.message_type === "email_changed")).toBe(true);
    await confirm.getByRole("link", { name: "Sign in", exact: true }).click();
    await confirm.getByRole("button", { name: "Password", exact: true }).click();
    await confirm.getByLabel("Email address").fill(newEmail); await confirm.getByLabel("Password", { exact: true }).fill(password);
    await confirm.getByRole("button", { name: "Sign in", exact: true }).click();
    await expect(confirm.getByRole("button", { name: "Settings", exact: true })).toBeVisible();
    const after = await (await confirm.request.get(`${baseURL}/api/v1/auth/session`)).json() as { user: { id: number; email: string } };
    expect(after.user.id).toBe(before.user.id); expect(after.user.email).toBe(newEmail);
    expect((await confirm.request.get(`${baseURL}/api/v1/calendars/${calendar.id}`)).status()).toBe(200);
    await confirm.goto(link); await confirm.getByRole("button", { name: "Confirm new email", exact: true }).click();
    await expect(confirm.getByRole("alert")).toContainText("invalid, expired, or unavailable");
  } finally { await memberContext.close(); await confirmContext.close(); }
});

test("admin replaces an invited email and only the replacement registers the same account", async ({ page, browser }, testInfo) => {
  const email = `admin-invite-${testInfo.project.name}-${Date.now()}@example.test`; const replacement = `new-${email}`; const password = "admin-invited-email-password";
  expect((await page.request.get("/api/v1/dev/login?email=admin%40localhost", { maxRedirects: 0 })).status()).toBe(302);
  await page.goto("/settings/users"); await page.getByLabel("Email address").fill(email);
  await page.getByRole("button", { name: "Send invitation", exact: true }).click(); await expect(page.getByRole("status")).toHaveText(`Invitation sent to ${email}.`);
  const oldLink = (await messages(outbox)).find((message) => message.recipient === email && message.message_type === "invitation")!.authentication_link!;
  const before = await (await page.request.get("/api/v1/admin/users")).json() as { users: { id: number; email: string }[] }; const id = before.users.find((user) => user.email === email)!.id;
  const row = page.getByRole("row").filter({ hasText: email });
  await row.getByRole("button", { name: "Change email", exact: true }).click();
  await page.getByLabel("New email address").fill(replacement);
  await page.locator('section[aria-labelledby="change-email-heading"]').screenshot({ path: testInfo.outputPath("admin-invited-email-edit.png") });
  await page.getByRole("button", { name: "Cancel email change", exact: true }).click();
  await expect(page.getByLabel("New email address")).toHaveCount(0);
  await row.getByRole("button", { name: "Change email", exact: true }).click(); await page.getByLabel("New email address").fill(replacement);
  await page.getByRole("button", { name: "Send replacement invitation", exact: true }).click();
  await expect(page.getByRole("status")).toHaveText(`Invitation sent to ${replacement}. The previous address and link no longer work.`);
  const after = await (await page.request.get("/api/v1/admin/users")).json() as { users: { id: number; email: string }[] }; expect(after.users.find((user) => user.email === replacement)!.id).toBe(id);
  const newLink = (await messages(outbox)).find((message) => message.recipient === replacement && message.message_type === "invitation")!.authentication_link!;
  const context = await browser.newContext({ viewport: page.viewportSize(), isMobile: testInfo.project.name === "mobile", hasTouch: testInfo.project.name === "mobile" });
  try {
    const recipient = await context.newPage(); await recipient.goto(oldLink); await expect(recipient.getByRole("alert")).toHaveText("Invitation is invalid or expired.");
    await recipient.goto(newLink); await expect(recipient.getByText(replacement, { exact: true })).toBeVisible();
    await recipient.getByLabel("Password", { exact: true }).fill(password); await recipient.getByLabel("Confirm password", { exact: true }).fill(password);
    await recipient.getByRole("button", { name: "Create account", exact: true }).click(); await expect(recipient.getByRole("heading", { name: "Account created" })).toBeVisible();
    await recipient.getByRole("link", { name: "Sign in", exact: true }).click(); await recipient.getByRole("button", { name: "Password", exact: true }).click();
    await recipient.getByLabel("Email address").fill(replacement); await recipient.getByLabel("Password", { exact: true }).fill(password); await recipient.getByRole("button", { name: "Sign in", exact: true }).click();
    await expect(recipient.getByRole("button", { name: "Settings", exact: true })).toBeVisible();
    const session = await (await recipient.request.get(`${baseURL}/api/v1/auth/session`)).json() as { user: { id: number } }; expect(session.user.id).toBe(id);
    await page.reload(); const registeredRow = page.getByRole("row").filter({ hasText: replacement }); await expect(registeredRow).toContainText("registered");
    await registeredRow.screenshot({ path: testInfo.outputPath("admin-invited-email-registered.png") });
  } finally { await context.close(); }
});

test("admin registered email stays pending until recipient confirms with stable identity", async ({ page, browser }, testInfo) => {
  const email = `admin-member-${testInfo.project.name}-${Date.now()}@example.test`; const replacement = `new-${email}`; const password = "admin-registered-email-password";
  expect((await page.request.get("/api/v1/dev/login?email=admin%40localhost", { maxRedirects: 0 })).status()).toBe(302);
  await page.goto("/settings/users"); await page.getByLabel("Email address").fill(email); await page.getByRole("button", { name: "Send invitation", exact: true }).click();
  await expect(page.getByRole("status")).toHaveText(`Invitation sent to ${email}.`);
  const invite = (await messages(outbox)).find((message) => message.recipient === email && message.message_type === "invitation")!.authentication_link!;
  const options = { viewport: page.viewportSize(), isMobile: testInfo.project.name === "mobile", hasTouch: testInfo.project.name === "mobile" };
  const context = await browser.newContext(options); const confirmationContext = await browser.newContext(options);
  try {
    const recipient = await context.newPage(); await recipient.goto(invite); await recipient.getByLabel("Password", { exact: true }).fill(password); await recipient.getByLabel("Confirm password", { exact: true }).fill(password);
    await recipient.getByRole("button", { name: "Create account", exact: true }).click(); await expect(recipient.getByRole("heading", { name: "Account created" })).toBeVisible();
    expect((await recipient.request.get(`${baseURL}/api/v1/dev/login?email=${encodeURIComponent(email)}`, { maxRedirects: 0 })).status()).toBe(302);
    const before = await (await recipient.request.get(`${baseURL}/api/v1/auth/session`)).json() as { user: { id: number }; csrf_token: string };
    expect((await recipient.request.post(`${baseURL}/api/v1/admin/users/${before.user.id}/email-changes`, { headers: { origin: baseURL, "sec-fetch-site": "same-origin", "x-csrf-token": before.csrf_token }, data: { email: replacement } })).status()).toBe(403);
    expect((await page.request.post(`/api/v1/admin/users/${before.user.id}/email-changes`, { data: { email: replacement } })).status()).toBe(403);
    await page.reload(); const row = page.getByRole("row").filter({ hasText: email });
    await row.getByRole("button", { name: "Change email", exact: true }).click(); await page.getByLabel("New email address").fill(replacement);
    await page.getByRole("button", { name: "Send email confirmation", exact: true }).click();
    await expect(page.getByRole("status")).toHaveText(`Confirmation sent to ${replacement}. ${email} stays active until confirmed.`);
    await expect(row).toContainText(`Awaiting confirmation: ${replacement}`);
    await row.screenshot({ path: testInfo.outputPath("admin-registered-email-pending.png") });
    const summary = await (await recipient.request.get(`${baseURL}/api/v1/account`)).json() as { email: string }; expect(summary.email).toBe(email);
    const link = (await messages(outbox)).find((message) => message.recipient === replacement && message.message_type === "email_confirmation")!.authentication_link!;
    const confirmation = await confirmationContext.newPage(); await confirmation.goto(link);
    expect((await recipient.request.get(`${baseURL}/api/v1/auth/session`)).status()).toBe(200);
    await confirmation.getByRole("button", { name: "Confirm new email", exact: true }).click(); await expect(confirmation.getByRole("status")).toContainText("Your email has been changed.");
    expect((await recipient.request.get(`${baseURL}/api/v1/auth/session`)).status()).toBe(401); expect((await confirmation.request.get(`${baseURL}/api/v1/auth/session`)).status()).toBe(401);
    await confirmation.getByRole("link", { name: "Sign in", exact: true }).click(); await confirmation.getByRole("button", { name: "Password", exact: true }).click(); await confirmation.getByLabel("Email address").fill(replacement); await confirmation.getByLabel("Password", { exact: true }).fill(password); await confirmation.getByRole("button", { name: "Sign in", exact: true }).click();
    await expect(confirmation.getByRole("button", { name: "Settings", exact: true })).toBeVisible();
    const after = await (await confirmation.request.get(`${baseURL}/api/v1/auth/session`)).json() as { user: { id: number; email: string } }; expect(after.user.id).toBe(before.user.id); expect(after.user.email).toBe(replacement);
    if (testInfo.project.name === "desktop") expect((await recipient.request.post(`${baseURL}/api/v1/auth/password-login`, { data: { email, password } })).status()).toBe(401);
    await page.reload(); const updated = page.getByRole("row").filter({ hasText: replacement }); await expect(updated).toContainText("registered"); await expect(updated).not.toContainText("Awaiting confirmation");
    await expect.poll(async () => (await messages(outbox)).some((message) => message.recipient === email && message.message_type === "email_changed")).toBe(true);
  } finally { await context.close(); await confirmationContext.close(); }
});
