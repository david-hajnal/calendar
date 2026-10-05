import { execFile, spawn } from "node:child_process";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { promisify } from "node:util";

const execute = promisify(execFile);
const root = fileURLToPath(new URL("../../", import.meta.url));
const binary = join(root, "backend/target/debug/commoncal-backend");

/** Each journey has its own database, outbox and live rate-limit buckets. */
export async function isolatedBackend(bootstrap = false, statuses = false) {
  const directory = await mkdtemp(join(tmpdir(), "commoncal-account-e2e-"));
  const database = join(directory, "commoncal.sqlite");
  const outbox = join(directory, "outbox.ndjson");
  const env = {
    ...process.env, APP_ENV: "development", SESSION_SECRET: "isolated-account-e2e-secret",
    BIND_ADDRESS: "127.0.0.1:0", APP_ORIGIN: "http://127.0.0.1:3100",
    DATABASE_PATH: database, FRONTEND_DIR: join(root, "frontend/dist"),
    E2E_EMAIL_OUTBOX: outbox, E2E_ICS_FIXTURE: join(root, "e2e/support/controlled.ics"),
    DEFAULT_ADMIN_PASSWORD: "admin-default-password-2026", ACCESS_LOG_LEVEL: "OFF",
  };
  // APP_ORIGIN must match the chosen port for Origin checks and captured links.
  // Reserve an ephemeral loopback port briefly, then hand it to the app.
  const { createServer } = await import("node:net");
  const reservation = createServer();
  await new Promise<void>((resolve, reject) => {
    reservation.once("error", reject);
    reservation.listen(0, "127.0.0.1", resolve);
  });
  const port = (reservation.address() as { port: number }).port;
  await new Promise<void>((resolve, reject) => reservation.close(error => error ? reject(error) : resolve()));
  const origin = `http://127.0.0.1:${port}`;
  env.BIND_ADDRESS = `127.0.0.1:${port}`;
  env.APP_ORIGIN = origin;
  let bootstrapToken: string | undefined;
  try {
    if (bootstrap || statuses) {
      // Match backend bootstrap fixtures: migration 0019 seeds admin@localhost,
      // so remove only that seeded user from this disposable database.
      try { await execute(binary, ["bootstrap-superadmin", "bootstrap@example.test"], { cwd: root, env }); }
      catch (error) {
        if (!(error as { stderr?: string }).stderr?.includes("UsersExist")) throw error;
      }
      if (bootstrap) {
        await execute("python3", ["-c", "import sqlite3,sys; db=sqlite3.connect(sys.argv[1]); db.execute(\"DELETE FROM users WHERE normalized_email='admin@localhost'\"); db.commit(); db.close()", database]);
        const result = await execute(binary, ["bootstrap-superadmin", "bootstrap@example.test"], { cwd: root, env });
        bootstrapToken = /^token=(.+)$/m.exec(result.stdout)?.[1];
        if (!bootstrapToken) throw new Error("bootstrap did not issue a fixture token");
      } else {
        await execute("python3", ["-c", "import sqlite3,sys; db=sqlite3.connect(sys.argv[1]); db.executemany('INSERT INTO users(normalized_email,status,created_at) VALUES (?,?,1)', [('pending@example.test','pending'),('inactive@example.test','inactive')]); db.commit(); db.close()", database]);
      }
    }
    const child = spawn(binary, [], { cwd: root, env, stdio: ["ignore", "ignore", "pipe"] });
    let startupError = "";
    child.stderr.on("data", chunk => { startupError += String(chunk); });
    child.on("error", error => { startupError = error.message; });
    const stop = async () => {
      if (child.pid && child.exitCode === null && child.signalCode === null) {
        const exited = new Promise<void>(resolve => child.once("exit", () => resolve()));
        child.kill("SIGTERM");
        const timeout = setTimeout(() => child.kill("SIGKILL"), 5000);
        await exited;
        clearTimeout(timeout);
      }
      await rm(directory, { recursive: true, force: true });
    };
    try {
      const deadline = Date.now() + 20000;
      while (true) {
        if (child.exitCode !== null || startupError) throw new Error(`isolated backend exited: ${startupError}`);
        try { if ((await fetch(`${origin}/health/ready`)).ok) break; } catch { /* starting */ }
        if (Date.now() >= deadline) throw new Error("isolated backend did not become ready");
        await new Promise(resolve => setTimeout(resolve, 100));
      }
      return { origin, outbox, bootstrapToken, stop };
    } catch (error) { await stop(); throw error; }
  } catch (error) { await rm(directory, { recursive: true, force: true }); throw error; }
}
