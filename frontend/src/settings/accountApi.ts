import type { ApiClient } from "../auth/api";
export type AccountStatus = "invited" | "registered" | "pending" | "inactive";
export interface ManagedUser { id: number; email: string; status: AccountStatus; is_superadmin: boolean; invitation_id: number | null; pending_email_change?: { email: string; expires_at: number } | null }
export interface UserPage { users: ManagedUser[]; page: number; per_page: number; total: number }
export async function listUsers(api: ApiClient, page = 1): Promise<UserPage> {
  const response = await api.request(`/api/v1/admin/users?page=${page}&per_page=20`);
  if (!response.ok) throw new Error("We could not load users. Please try again.");
  return response.json() as Promise<UserPage>;
}
export async function inviteUser(api: ApiClient, email: string): Promise<void> {
  const response = await api.request("/api/v1/admin/invitations", {
    method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ email }),
  });
  if (response.status === 409) {
    const body = await response.json() as { error?: { message?: string } };
    throw new Error(body.error?.message ?? "This email is already used or reserved. For an invited account, use Resend invitation.");
  }
  if (response.status === 429) throw new Error("Too many invitations. Please wait and try again.");
  if (!response.ok) throw new Error("The invitation could not be sent. Use Resend invitation in Users to retry.");
}

export async function resendInvitation(api: ApiClient, invitationId: number): Promise<void> {
  const response = await api.request(`/api/v1/admin/invitations/${invitationId}/resend`, { method: "POST" });
  if (response.status === 429) throw new Error("Too many invitations. Please wait and try again.");
  if (response.status === 404) throw new Error("This invitation has changed or the account is no longer invited. Refresh and try again.");
  if (response.status === 409) throw new Error("This email is reserved for another account.");
  if (!response.ok) throw new Error("The invitation could not be sent. Use Resend invitation to retry.");
}

export async function disableUser(api: ApiClient, userId: number): Promise<void> {
  const response = await api.request(`/api/v1/admin/users/${userId}/suspend`, { method: "POST" });
  if (response.status === 409) {
    const body = await response.json() as { error?: { message?: string } };
    throw new Error(body.error?.message ?? "This account cannot be disabled.");
  }
  if (response.status === 404) throw new Error("This account has changed or is already inactive. Refresh and try again.");
  if (!response.ok) throw new Error("The account could not be disabled. Please try again.");
}

export interface AccountSummary { email: string; has_password: boolean; pending_email_change: { email: string; expires_at: number } | null }
export async function getAccount(api: ApiClient): Promise<AccountSummary> {
  const response = await api.request("/api/v1/account");
  if (!response.ok) throw new Error("Could not load account. Please try again.");
  return response.json() as Promise<AccountSummary>;
}
export async function changeOwnEmail(api: ApiClient, email: string, currentPassword: string): Promise<void> {
  const response = await api.request("/api/v1/account/email-changes", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ email, current_password: currentPassword }) });
  if (response.status === 429) throw new Error("Too many email changes. Please wait and try again.");
  if (!response.ok) {
    const body = await response.json() as { error?: { message?: string } };
    throw new Error(body.error?.message ?? "Confirmation could not be sent. Please try again.");
  }
}

export class AdminEmailChangeError extends Error {
  constructor(message: string, readonly invitedAddressChanged: boolean) { super(message); }
}
export async function changeUserEmail(api: ApiClient, userId: number, email: string): Promise<void> {
  const response = await api.request(`/api/v1/admin/users/${userId}/email-changes`, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ email }) });
  if (response.status === 429) throw new Error("Too many email changes. Please wait and try again.");
  if (!response.ok) {
    const body = await response.json() as { error?: { code?: string; message?: string } };
    throw new AdminEmailChangeError(body.error?.message ?? "The email change could not be requested. Please try again.", body.error?.code === "invitation_delivery_failed");
  }
}
