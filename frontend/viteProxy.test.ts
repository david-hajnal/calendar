// @vitest-environment node
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

describe("Vite development server", () => {
  it("proxies API requests to the local backend", async () => {
    const { default: config } = await import("./vite.config.js");
    const devConfig = typeof config === "function" ? await Promise.resolve(config({ mode: "development", command: "serve" })) : config;
    expect(devConfig.server?.proxy?.["/api"]).toMatchObject({
      target: "http://127.0.0.1:3000",
    });
  });

  it("uses the frontend directory as its root when launched from the repository root", async () => {
    const frontendDirectory = dirname(fileURLToPath(import.meta.url));
    const originalWorkingDirectory = process.cwd();

    try {
      process.chdir(resolve(frontendDirectory, ".."));
      const { default: rootLaunchConfig } = await import("./vite.config.js");
      const resolved = typeof rootLaunchConfig === "function"
        ? await Promise.resolve(rootLaunchConfig({ mode: "development", command: "serve" }))
        : rootLaunchConfig;
      expect(resolved.root).toBe(frontendDirectory);
    } finally {
      process.chdir(originalWorkingDirectory);
    }
  });
});
