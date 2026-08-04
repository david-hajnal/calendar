import { useEffect, useState } from "react";

import type { ApiClient } from "../auth/api";

type Notification = { id: number; event_id: number; event_title: string; created_at: number; read_at: number | null };

export function NotificationSurface({ api }: { api: ApiClient }) {
  const [notifications, setNotifications] = useState<Notification[] | null>(null);
  useEffect(() => {
    let active = true;
    void api.request("/api/v1/notifications").then(async (response) => {
      if (!response.ok) throw new Error("notifications unavailable");
      if (active) setNotifications(await response.json() as Notification[]);
    }).catch(() => { if (active) setNotifications([]); });
    return () => { active = false; };
  }, [api]);
  if (notifications === null) return null;
  return <section aria-label="Notifications" role="status">
    <h2>Notifications</h2>
    {notifications.length === 0 ? <p>No notifications.</p> : <ul>{notifications.map((notification) => <li key={notification.id}>{notification.event_title}</li>)}</ul>}
  </section>;
}
