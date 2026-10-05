import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import { createApiClient } from "../auth/api";
import { AccountSettings } from "./AccountSettings";
afterEach(cleanup);
it("keeps current email and submits the password with CSRF then displays pending confirmation", async () => {
  let pending = false;
  const fetcher = vi.fn(async (_input: RequestInfo | URL, init?: RequestInit) => {
    if (init?.method === "POST") { pending = true; return new Response(null, { status: 204 }); }
    return new Response(JSON.stringify({ email: "old@example.test", has_password: true, pending_email_change: pending ? { email: "new@example.test", expires_at: 2000000000 } : null }));
  });
  const api = createApiClient(fetcher); api.setCsrfToken("csrf-secret");
  render(<AccountSettings api={api} />);
  await screen.findByText("old@example.test");
  fireEvent.change(screen.getByLabelText("New email address"), { target: { value: "new@example.test" } });
  fireEvent.change(screen.getByLabelText("Current password"), { target: { value: "current-password" } });
  fireEvent.click(screen.getByRole("button", { name: "Send confirmation" }));
  expect(await screen.findByRole("status")).toHaveTextContent("Your current email stays active");
  await screen.findByText("new@example.test");
  expect(screen.getByText("old@example.test")).toBeInTheDocument();
  const post = fetcher.mock.calls.find(([, init]) => init?.method === "POST")!;
  expect(new Headers(post[1]?.headers).get("x-csrf-token")).toBe("csrf-secret");
  expect(JSON.parse(post[1]!.body as string)).toEqual({ email: "new@example.test", current_password: "current-password" });
  expect(screen.getByLabelText("Current password")).toHaveValue("");
});
it("shows password recovery for a passwordless account", async () => {
  render(<AccountSettings api={createApiClient(async () => new Response(JSON.stringify({ email: "old@example.test", has_password: false, pending_email_change: null })))} />);
  expect(await screen.findByRole("link", { name: "Forgot password" })).toHaveAttribute("href", "/forgot-password");
  expect(screen.queryByLabelText("Current password")).not.toBeInTheDocument();
});
it("keeps retry available after an incorrect password or delivery failure", async () => {
  render(<AccountSettings api={createApiClient(async (_input, init) => init?.method === "POST" ? new Response(JSON.stringify({ error: { message: "Current password is incorrect." } }), { status: 400 }) : new Response(JSON.stringify({ email: "old@example.test", has_password: true, pending_email_change: null })))} />);
  await screen.findByText("old@example.test");
  fireEvent.change(screen.getByLabelText("New email address"), { target: { value: "new@example.test" } });
  fireEvent.change(screen.getByLabelText("Current password"), { target: { value: "bad" } });
  fireEvent.click(screen.getByRole("button", { name: "Send confirmation" }));
  expect(await screen.findByRole("alert")).toHaveTextContent("Current password is incorrect.");
  expect(screen.queryByRole("status")).not.toBeInTheDocument();
  expect(screen.getByRole("button", { name: "Send confirmation" })).toBeEnabled();
});
