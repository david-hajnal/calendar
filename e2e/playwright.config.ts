import { defineConfig, devices } from "@playwright/test";

const baseURL = process.env.E2E_BASE_URL ?? "http://127.0.0.1:3100";

export default defineConfig({
  testDir: "./tests",
  fullyParallel: false,
  forbidOnly: !!process.env.CI,
  retries: process.env.CI ? 1 : 0,
  reporter: process.env.CI ? [["github"], ["html", { open: "never" }]] : "list",
  use: {
    baseURL,
    trace: "retain-on-failure",
    screenshot: "only-on-failure",
    video: "off",
    ignoreHTTPSErrors: true,
  },
  projects: [
    { name: "desktop", use: { ...devices["Desktop Firefox"] } },
    { name: "mobile", use: { ...devices["iPhone 13"] } },
  ],
  webServer: process.env.E2E_BASE_URL ? undefined : {
    command: "pnpm e2e:server",
    url: `${baseURL}/health/ready`,
    reuseExistingServer: !process.env.CI,
    timeout: 120_000,
  },
});
