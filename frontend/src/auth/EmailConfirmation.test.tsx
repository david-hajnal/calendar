import { StrictMode } from "react";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import type { Fetcher } from "./api";
import { AuthProvider } from "./session";
import { EmailConfirmation } from "./EmailConfirmation";
afterEach(() => { cleanup(); window.history.replaceState({}, "", "/"); });
it("strips the token in StrictMode and confirms only after explicit action", async () => {
  window.history.replaceState({}, "", "/email/confirm?token=secret");
  const fetcher = vi.fn<Fetcher>(async () => new Response(null, { status: 204 }));
  render(<StrictMode><AuthProvider fetcher={fetcher} loadSession={false}><EmailConfirmation /></AuthProvider></StrictMode>);
  expect(window.location.search).toBe(""); expect(fetcher).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole("button", { name: "Confirm new email" }));
  expect(await screen.findByRole("status")).toHaveTextContent("Sign in with your new email.");
  expect(fetcher.mock.calls).toHaveLength(1);
  expect(JSON.parse(fetcher.mock.calls[0][1]!.body as string)).toEqual({ token: "secret" });
});
it("offers settings recovery for a used or expired confirmation", async () => {
  window.history.replaceState({}, "", "/email/confirm?token=used");
  render(<AuthProvider fetcher={async () => new Response(null, { status: 400 })} loadSession={false}><EmailConfirmation /></AuthProvider>);
  fireEvent.click(screen.getByRole("button", { name: "Confirm new email" }));
  expect(await screen.findByRole("alert")).toHaveTextContent("invalid, expired, or unavailable");
  expect(screen.getByRole("link", { name: /Request a new confirmation/ })).toHaveAttribute("href", "/settings/account");
});
