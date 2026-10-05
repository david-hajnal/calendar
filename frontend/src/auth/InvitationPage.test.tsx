import { StrictMode } from "react";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import { AuthProvider } from "./session";
import { InvitationPage } from "./InvitationPage";
afterEach(() => { cleanup(); window.history.replaceState({}, "", "/"); });

it("keeps the token in StrictMode, strips it from the URL, and accepts only after matching passwords", async () => {
  window.history.replaceState({}, "", "/invitations/accept?token=secret");
  const fetcher = vi.fn(async (_input: RequestInfo | URL, init?: RequestInit) => new Response(JSON.stringify(init?.method === "POST" ? { user: { id: 1 } } : { email: "sam@example.test" })));
  render(<StrictMode><AuthProvider fetcher={fetcher} loadSession={false}><InvitationPage /></AuthProvider></StrictMode>);
  await screen.findByText("sam@example.test");
  expect(window.location.search).toBe("");
  expect(fetcher.mock.calls.some(([, init]) => init?.method === "POST")).toBe(false);
  fireEvent.change(screen.getByLabelText("Password"), { target: { value: "secure-invite-password" } });
  fireEvent.change(screen.getByLabelText("Confirm password"), { target: { value: "different-password" } });
  fireEvent.click(screen.getByRole("button", { name: "Create account" }));
  expect(await screen.findByRole("alert")).toHaveTextContent("Passwords must match.");
  fireEvent.change(screen.getByLabelText("Confirm password"), { target: { value: "secure-invite-password" } });
  fireEvent.click(screen.getByRole("button", { name: "Create account" }));
  expect(await screen.findByRole("heading", { name: "Account created" })).toBeInTheDocument();
  const posts = fetcher.mock.calls.filter(([, init]) => init?.method === "POST");
  expect(posts).toHaveLength(1);
  expect(JSON.parse(posts[0][1]!.body as string)).toEqual({ token: "secret", password: "secure-invite-password", password_confirmation: "secure-invite-password" });
  expect(screen.getByRole("link", { name: "Sign in" })).toHaveAttribute("href", "/login");
});
