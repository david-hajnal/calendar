import { StrictMode } from "react";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import type { Fetcher } from "./api";
import { AuthProvider } from "./session";
import { PasswordRecoveryRequest, PasswordResetPage } from "./PasswordRecovery";
afterEach(() => { cleanup(); window.history.replaceState({}, "", "/"); });

it("requests recovery and shows the generic confirmation without claiming delivery", async () => {
  const fetcher = vi.fn<Fetcher>(async () => new Response("{}", { status: 202 }));
  render(<AuthProvider fetcher={fetcher} loadSession={false}><PasswordRecoveryRequest /></AuthProvider>);
  fireEvent.change(screen.getByLabelText("Email address"), { target: { value: "unknown@example.test" } });
  fireEvent.click(screen.getByRole("button", { name: "Send reset link" }));
  expect(await screen.findByRole("status")).toHaveTextContent("If the account is eligible, a password reset link will be sent.");
  expect(fetcher.mock.calls).toHaveLength(1);
  expect(fetcher.mock.calls[0][0]).toBe("/api/v1/auth/password-resets");
  fireEvent.click(screen.getByRole("button", { name: "Request another link" }));
  expect(screen.getByLabelText("Email address")).toHaveValue("");
});

it("keeps retry available when recovery is rate limited", async () => {
  render(<AuthProvider fetcher={vi.fn(async () => new Response(null, { status: 429 }))} loadSession={false}><PasswordRecoveryRequest /></AuthProvider>);
  fireEvent.change(screen.getByLabelText("Email address"), { target: { value: "member@example.test" } });
  fireEvent.click(screen.getByRole("button", { name: "Send reset link" }));
  expect(await screen.findByRole("alert")).toHaveTextContent("Too many recovery requests.");
  expect(screen.getByRole("button", { name: "Send reset link" })).toBeEnabled();
});

it("strips reset token in StrictMode and changes password only on matching explicit submit", async () => {
  window.history.replaceState({}, "", "/password-reset?token=secret");
  const fetcher = vi.fn<Fetcher>(async () => new Response(null, { status: 204 }));
  render(<StrictMode><AuthProvider fetcher={fetcher} loadSession={false}><PasswordResetPage /></AuthProvider></StrictMode>);
  expect(window.location.search).toBe(""); expect(fetcher).not.toHaveBeenCalled();
  fireEvent.change(screen.getByLabelText("New password"), { target: { value: "a-new-password-long-enough" } });
  fireEvent.change(screen.getByLabelText("Confirm password"), { target: { value: "different-password" } });
  fireEvent.click(screen.getByRole("button", { name: "Reset password" }));
  expect(screen.getByRole("alert")).toHaveTextContent("Passwords must match."); expect(fetcher).not.toHaveBeenCalled();
  fireEvent.change(screen.getByLabelText("Confirm password"), { target: { value: "a-new-password-long-enough" } });
  fireEvent.click(screen.getByRole("button", { name: "Reset password" }));
  expect(await screen.findByRole("status")).toHaveTextContent("Your password has been changed.");
  expect(fetcher.mock.calls).toHaveLength(1);
  const init = fetcher.mock.calls[0][1] as RequestInit;
  expect(JSON.parse(init.body as string)).toEqual({ token: "secret", password: "a-new-password-long-enough", password_confirmation: "a-new-password-long-enough" });
  expect(screen.getByRole("link", { name: "Sign in" })).toHaveAttribute("href", "/login");
});

it("offers a new recovery request for a used or expired link", async () => {
  window.history.replaceState({}, "", "/password-reset?token=expired");
  render(<AuthProvider fetcher={vi.fn(async () => new Response(JSON.stringify({ error: { code: "invalid_reset" } }), { status: 400 }))} loadSession={false}><PasswordResetPage /></AuthProvider>);
  fireEvent.change(screen.getByLabelText("New password"), { target: { value: "a-new-password-long-enough" } });
  fireEvent.change(screen.getByLabelText("Confirm password"), { target: { value: "a-new-password-long-enough" } });
  fireEvent.click(screen.getByRole("button", { name: "Reset password" }));
  expect(await screen.findByRole("alert")).toHaveTextContent("Password reset link is invalid or expired.");
  expect(screen.getByRole("link", { name: "Request a new reset link" })).toHaveAttribute("href", "/forgot-password");
  expect(screen.queryByLabelText("New password")).not.toBeInTheDocument();
});
