import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import { DevLoginPage } from "./dev-login";

const { completeAuthentication, navigate } = vi.hoisted(() => ({ completeAuthentication: vi.fn().mockResolvedValue(undefined), navigate: vi.fn() }));
vi.mock("./auth/session", () => ({ useAuth: () => ({ completeAuthentication }) }));
vi.mock("./navigation", () => ({ navigate }));
afterEach(() => { cleanup(); vi.clearAllMocks(); vi.unstubAllGlobals(); window.history.replaceState({}, "", "/"); });

it("completes sign-in after fetch follows the backend redirect without a Location header", async () => {
  window.history.replaceState({}, "", "/dev-login");
  const response = new Response("<html></html>", { status: 200 });
  Object.defineProperty(response, "url", { value: `${window.location.origin}/dev-login?csrf_token=dev-csrf` });
  vi.stubGlobal("fetch", vi.fn().mockResolvedValue(response));
  render(<DevLoginPage />);
  fireEvent.change(screen.getByLabelText("Email address"), { target: { value: "person@example.test" } });
  fireEvent.click(screen.getByRole("button", { name: "Sign in" }));
  await waitFor(() => expect(completeAuthentication).toHaveBeenCalledWith("dev-csrf"));
  expect(navigate).toHaveBeenCalledWith("/", { mode: "replace" });
});

it("leaves the developer callback page after loading the session", async () => {
  window.history.replaceState({}, "", "/dev-login?csrf_token=dev-csrf");
  render(<DevLoginPage />);
  await waitFor(() => expect(navigate).toHaveBeenCalledWith("/", { mode: "replace" }));
  expect(completeAuthentication).toHaveBeenCalledWith("dev-csrf");
});
