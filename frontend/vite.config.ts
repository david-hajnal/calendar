import { dirname } from "node:path";
import { fileURLToPath } from "node:url";
import react from "@vitejs/plugin-react";
import { defineConfig } from "vitest/config";
import { readFileSync } from "node:fs";

export default defineConfig({
  root: dirname(fileURLToPath(import.meta.url)),
  plugins: [react()],
  server: {
    https: {
      key: readFileSync("./localhost-key.pem"),
      cert: readFileSync("./localhost.pem"),
    },
    fs: { allow: [".."] },
    watch: {
      usePolling: true,
    },
    proxy: {
      "/api": {
        target: process.env.VITE_PROXY_TARGET || "http://127.0.0.1:3000",
      },
    },
  },
  test: {
    environment: "jsdom",
    setupFiles: "./src/test/setup.ts",
  },
});
