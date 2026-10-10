import { afterEach, expect, it } from "vitest";
import { navigate, safeContinuation } from "./navigation";

afterEach(() => window.history.replaceState({}, "", "/"));
it("pushes user navigation, replaces redirects, and avoids duplicate destinations", () => {
  window.history.replaceState({}, "", "/dashboard");
  const initial = window.history.length;
  navigate("/settings/account");
  expect(window.history.length).toBe(initial + 1);
  expect(window.location.pathname).toBe("/settings/account");
  navigate("/settings/account");
  expect(window.history.length).toBe(initial + 1);
  navigate("/dashboard?view=agenda&date=2026-10-09", { mode: "replace" });
  expect(window.history.length).toBe(initial + 1);
  expect(window.location.search).toBe("?view=agenda&date=2026-10-09");
});
it("rejects unsafe origins, malformed encodings, login/consumption loops, and credential URLs", () => {
  for (const value of ["https://evil.test", "//evil.test", "/\\evil.test", "/login", "/login/consume?token=x", "/%6cogin", "/%", "/invitations/consume", "/dashboard?csrf_token=x"]) expect(safeContinuation(value)).toBeNull();
  expect(safeContinuation("/dashboard?view=week&date=2026-10-09#today")).toBe("/dashboard?view=week&date=2026-10-09#today");
  expect(safeContinuation("/consent?handoff=abc")).toBe("/consent?handoff=abc");
  expect(() => navigate("//evil.test")).toThrow("local URL");
});
