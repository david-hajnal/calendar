// @vitest-environment node
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it, vi } from "vitest";
import config from "./vite.config";

describe("Vite development server", () => {
  it("proxies API requests to the local backend", () => {
    expect(config.server?.proxy?.["/api"]).toMatchObject({
      target: "http://127.0.0.1:3000",
      changeOrigin: true,
    });
  });

  it("uses the frontend directory as its root when launched from the repository root", async () => {
    const frontendDirectory = dirname(fileURLToPath(import.meta.url));
    const originalWorkingDirectory = process.cwd();

    try {
      process.chdir(resolve(frontendDirectory, ".."));
      vi.resetModules();
      const { default: rootLaunchConfig } = await import("./vite.config");

      expect(rootLaunchConfig.root).toBe(frontendDirectory);
    } finally {
      process.chdir(originalWorkingDirectory);
      vi.resetModules();
    }
  });
});
