import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import { createApiClient } from "../auth/api";
import { UserManagement } from "./UserManagement";
afterEach(cleanup);

it("sends a real invitation with CSRF and refreshes the visible invited account", async () => {
  let invited = false;
  const fetcher = vi.fn(async (_input: RequestInfo | URL, init?: RequestInit) => {
    if (init?.method === "POST") { invited = true; return new Response("{}", { status: 201 }); }
    return new Response(JSON.stringify({ users: invited ? [{ id: 2, email: "sam@example.test", status: "invited" }] : [], page: 1, per_page: 20, total: invited ? 1 : 0 }), { status: 200 });
  });
  const api = createApiClient(fetcher); api.setCsrfToken("test-csrf");
  render(<UserManagement api={api} currentUserId={1} />);
  expect(await screen.findByText("No users on this page.")).toBeInTheDocument();
  fireEvent.change(screen.getByLabelText("Email address"), { target: { value: "sam@example.test" } });
  fireEvent.click(screen.getByRole("button", { name: "Send invitation" }));
  expect(await screen.findByText("sam@example.test")).toBeInTheDocument();
  const post = fetcher.mock.calls.find(([, init]) => init?.method === "POST")!;
  expect(new Headers(post[1]?.headers).get("x-csrf-token")).toBe("test-csrf");
  expect(screen.getByRole("status")).toHaveTextContent("Invitation sent to sam@example.test.");
});

it("does not show sent confirmation when delivery fails", async () => {
  const api = createApiClient(vi.fn(async (_input, init) => init?.method === "POST"
    ? new Response(null, { status: 503 }) : new Response(JSON.stringify({ users: [], page: 1, per_page: 20, total: 0 }))));
  render(<UserManagement api={api} currentUserId={1} />);
  await screen.findByText("No users on this page.");
  fireEvent.change(screen.getByLabelText("Email address"), { target: { value: "sam@example.test" } });
  fireEvent.click(screen.getByRole("button", { name: "Send invitation" }));
  expect(await screen.findByRole("alert")).toHaveTextContent("The invitation could not be sent.");
  expect(screen.queryByText(/Invitation sent/)).not.toBeInTheDocument();
});

it("shows resend only for invited accounts and retries after delivery failure", async () => {
  let posts = 0;
  const fetcher = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
    if (init?.method === "POST") { expect(String(input)).toBe("/api/v1/admin/invitations/8/resend"); posts++; return new Response(null, { status: posts === 1 ? 500 : 204 }); }
    return new Response(JSON.stringify({ users: [
      { id: 2, email: "retry@example.test", status: "invited", invitation_id: 8 },
      { id: 3, email: "member@example.test", status: "registered", invitation_id: null },
    ], page: 1, per_page: 20, total: 2 }));
  });
  const api = createApiClient(fetcher); api.setCsrfToken("test-csrf");
  render(<UserManagement api={api} currentUserId={1} />);
  fireEvent.click(await screen.findByRole("button", { name: "Resend invitation" }));
  expect(await screen.findByRole("alert")).toHaveTextContent("Use Resend invitation to retry.");
  fireEvent.click(screen.getByRole("button", { name: "Resend invitation" }));
  expect(await screen.findByRole("status")).toHaveTextContent("The previous link no longer works.");
  expect(screen.getAllByRole("button", { name: "Resend invitation" })).toHaveLength(1);
  const post = fetcher.mock.calls.find(([, init]) => init?.method === "POST")!;
  expect(new Headers(post[1]?.headers).get("x-csrf-token")).toBe("test-csrf");
});

it("requires explicit confirmation, allows cancel, and disables a user with CSRF", async () => {
  let disabled = false;
  const fetcher = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
    if (init?.method === "POST") { expect(String(input)).toBe("/api/v1/admin/users/2/suspend"); disabled = true; return new Response(null, { status: 204 }); }
    return new Response(JSON.stringify({ users: [
      { id: 1, email: "admin@example.test", status: "registered", invitation_id: null },
      { id: 2, email: "member@example.test", status: disabled ? "inactive" : "registered", invitation_id: null },
    ], page: 1, per_page: 20, total: 2 }));
  });
  const api = createApiClient(fetcher); api.setCsrfToken("test-csrf");
  render(<UserManagement api={api} currentUserId={1} />);
  fireEvent.click(await screen.findByRole("button", { name: "Disable user" }));
  expect(screen.getByRole("group", { name: "Disable user?" })).toHaveTextContent("member@example.test");
  expect(screen.getByRole("button", { name: "Cancel" })).toHaveFocus();
  expect(fetcher.mock.calls.filter(([, init]) => init?.method === "POST")).toHaveLength(0);
  fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
  expect(screen.queryByRole("group", { name: "Disable user?" })).not.toBeInTheDocument();
  expect(screen.getByRole("button", { name: "Disable user" })).toHaveFocus();
  fireEvent.click(screen.getByRole("button", { name: "Disable user" }));
  fireEvent.click(screen.getByRole("button", { name: "Confirm disable" }));
  expect(await screen.findByText("Access disabled for member@example.test.")).toBeInTheDocument();
  expect(await screen.findByText("inactive")).toBeInTheDocument();
  expect(screen.queryByRole("button", { name: "Disable user" })).not.toBeInTheDocument();
  const post = fetcher.mock.calls.find(([, init]) => init?.method === "POST")!;
  expect(new Headers(post[1]?.headers).get("x-csrf-token")).toBe("test-csrf");
});

it("keeps confirmation available after a disable failure", async () => {
  const api = createApiClient(vi.fn(async (_input, init) => init?.method === "POST"
    ? new Response(null, { status: 500 }) : new Response(JSON.stringify({ users: [{ id: 2, email: "member@example.test", status: "registered", invitation_id: null }], page: 1, per_page: 20, total: 1 }))));
  render(<UserManagement api={api} currentUserId={1} />);
  fireEvent.click(await screen.findByRole("button", { name: "Disable user" }));
  fireEvent.click(screen.getByRole("button", { name: "Confirm disable" }));
  expect(await screen.findByRole("alert")).toHaveTextContent("The account could not be disabled.");
  expect(screen.getByRole("button", { name: "Confirm disable" })).toBeEnabled();
  expect(screen.queryByText(/Access disabled for/)).not.toBeInTheDocument();
});

it("offers email editing only for invited and registered users and cancel does not mutate", async () => {
  const fetcher = vi.fn(async () => new Response(JSON.stringify({ users: [
    { id: 1, email: "admin@example.test", status: "registered" },
    { id: 2, email: "invite@example.test", status: "invited", invitation_id: 8 },
    { id: 3, email: "pending@example.test", status: "pending" },
    { id: 4, email: "inactive@example.test", status: "inactive" },
  ], page: 1, per_page: 20, total: 4 })));
  render(<UserManagement api={createApiClient(fetcher)} currentUserId={1} />);
  const changes = await screen.findAllByRole("button", { name: "Change email" }); expect(changes).toHaveLength(2);
  fireEvent.click(changes[1]);
  expect(screen.getByLabelText("New email address")).toHaveFocus();
  expect(screen.getByText(/previous invitation stops working immediately/)).toBeInTheDocument();
  fireEvent.click(screen.getByRole("button", { name: "Cancel email change" }));
  expect(screen.queryByLabelText("New email address")).not.toBeInTheDocument(); expect(changes[1]).toHaveFocus();
  expect(fetcher).toHaveBeenCalledTimes(1);
});
it("requests admin confirmation with CSRF and displays pending email without replacing the current address", async () => {
  let pending = false;
  const fetcher = vi.fn(async (_input: RequestInfo | URL, init?: RequestInit) => {
    if (init?.method === "POST") { pending = true; return new Response(null, { status: 204 }); }
    return new Response(JSON.stringify({ users: [{ id: 2, email: "old@example.test", status: "registered", pending_email_change: pending ? { email: "new@example.test", expires_at: 2000000000 } : null }], page: 1, per_page: 20, total: 1 }));
  });
  const api = createApiClient(fetcher); api.setCsrfToken("test-csrf");
  render(<UserManagement api={api} currentUserId={1} />);
  fireEvent.click(await screen.findByRole("button", { name: "Change email" }));
  fireEvent.change(screen.getByLabelText("New email address"), { target: { value: "new@example.test" } });
  fireEvent.click(screen.getByRole("button", { name: "Send email confirmation" }));
  expect(await screen.findByRole("status")).toHaveTextContent("old@example.test stays active until confirmed.");
  await screen.findByText("new@example.test"); expect(screen.getByText("old@example.test")).toBeInTheDocument();
  const post = fetcher.mock.calls.find(([,init]) => init?.method === "POST")!;
  expect(post[0]).toBe("/api/v1/admin/users/2/email-changes"); expect(new Headers(post[1]?.headers).get("x-csrf-token")).toBe("test-csrf"); expect(JSON.parse(post[1]!.body as string)).toEqual({ email: "new@example.test" });
});
it("refreshes the changed invited address and offers resend after delivery failure", async () => {
  let changed = false;
  const api = createApiClient(async (_input, init) => {
    if (init?.method === "POST") { changed = true; return new Response(JSON.stringify({ error: { code: "invitation_delivery_failed", message: "The address changed, but the invitation could not be sent. Use Resend invitation to retry." } }), { status: 503 }); }
    return new Response(JSON.stringify({ users: [{ id: 2, email: changed ? "new@example.test" : "old@example.test", status: "invited", invitation_id: changed ? 9 : 8 }], page: 1, per_page: 20, total: 1 }));
  });
  render(<UserManagement api={api} currentUserId={1} />);
  fireEvent.click(await screen.findByRole("button", { name: "Change email" }));
  fireEvent.change(screen.getByLabelText("New email address"), { target: { value: "new@example.test" } });
  fireEvent.click(screen.getByRole("button", { name: "Send replacement invitation" }));
  expect(await screen.findByRole("alert")).toHaveTextContent("Use Resend invitation to retry.");
  await screen.findByText("new@example.test"); expect(screen.queryByText("old@example.test")).not.toBeInTheDocument();
  expect(screen.getByRole("button", { name: "Resend invitation" })).toBeEnabled();
});
