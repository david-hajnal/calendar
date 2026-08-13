import { dirname } from "node:path";
import { fileURLToPath } from "node:url";
import react from "@vitejs/plugin-react";
import { defineConfig, loadEnv } from "vite";
import { existsSync, readFileSync } from "node:fs";

export default defineConfig(({ mode }) => {
  const env = loadEnv(mode, process.cwd(), "");
  const isDev = mode === "development";
  const serverConfig: {
    https?: { key: Buffer; cert: Buffer };
    fs?: { allow: string[] };
    watch?: { usePolling: boolean };
    proxy?: Record<string, { target: string }>;
  } = {};

  if (isDev && existsSync("./localhost-key.pem") && existsSync("./localhost.pem")) {
    serverConfig.https = {
      key: readFileSync("./localhost-key.pem"),
      cert: readFileSync("./localhost.pem"),
    };
    serverConfig.fs = { allow: [".."] };
    serverConfig.watch = { usePolling: true };
    serverConfig.proxy = {
      "/api": {
        target: env.VITE_PROXY_TARGET || "http://127.0.0.1:3000",
      },
    };
  }

  return {
    root: dirname(fileURLToPath(import.meta.url)),
    plugins: [react()],
    server: serverConfig,
    test: {
      environment: "jsdom",
      setupFiles: "./src/test/setup.ts",
    },
  };
});
