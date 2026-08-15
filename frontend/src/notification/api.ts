import type { ApiClient } from "../auth/api";

export interface Notification {
  id: number;
  event_id: number;
  event_title: string;
  created_at: number;
  read_at: number | null;
}

export function listNotifications(api: ApiClient): Promise<Notification[]> {
  return api.request("/api/v1/notifications").then((res) => res.json() as Promise<Notification[]>);
}

export function markAsRead(api: ApiClient, notificationId: number): Promise<void> {
  return api.request(`/api/v1/notifications/${notificationId}/read`, { method: "POST" }).then(() => {});
}

export function markAllAsRead(api: ApiClient): Promise<void> {
  return api.request("/api/v1/notifications/mark-all-read", { method: "POST" }).then(() => {});
}

export function unreadCount(api: ApiClient): Promise<{ unread_count: number }> {
  return api.request("/api/v1/notifications/unread-count").then((res) => res.json() as Promise<{ unread_count: number }>);
}
