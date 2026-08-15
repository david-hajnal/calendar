import { useEffect, useRef, useState, useCallback } from "react";

import type { ApiClient } from "../auth/api";

import { listNotifications, markAsRead, markAllAsRead, unreadCount } from "./api";

import "./NotificationDropdown.css";

interface NotificationItem {
  id: number;
  event_id: number;
  event_title: string;
  created_at: number;
  read_at: number | null;
}

function formatRelative(ts: number): string {
  const seconds = Math.floor((Date.now() / 1000) - ts);
  if (seconds < 60) return "just now";
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${minutes}m ago`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours}h ago`;
  const days = Math.floor(hours / 24);
  return `${days}d ago`;
}

export function NotificationDropdown({ api, onPermissionRequest }: { api: ApiClient; onPermissionRequest?: () => void }) {
  const [notifications, setNotifications] = useState<NotificationItem[] | null>(null);
  const [unreadCountNum, setUnreadCountNum] = useState(0);
  const [open, setOpen] = useState(false);
  const [loading, setLoading] = useState(true);
  const dropdownRef = useRef<HTMLDivElement>(null);

  const refresh = useCallback(async () => {
    try {
      const [notifs, count] = await Promise.all([
        listNotifications(api),
        unreadCount(api),
      ]);
      setNotifications(notifs);
      setUnreadCountNum(count.unread_count);
    } catch {
      setNotifications([]);
      setUnreadCountNum(0);
    } finally {
      setLoading(false);
    }
  }, [api]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  useEffect(() => {
    if (!open) return;
    const handler = (e: MouseEvent) => {
      if (dropdownRef.current && !dropdownRef.current.contains(e.target as Node)) {
        setOpen(false);
      }
    };
    document.addEventListener("mousedown", handler);
    return () => document.removeEventListener("mousedown", handler);
  }, [open]);

  const handleMarkAllRead = async () => {
    await markAllAsRead(api);
    await refresh();
  };

  const handleDismiss = async (id: number) => {
    await markAsRead(api, id);
    setNotifications((prev) => (prev ? prev.filter((n) => n.id !== id) : []));
    setUnreadCountNum((c) => Math.max(0, c - 1));
  };

  const handleNotificationClick = async (n: NotificationItem) => {
    if (n.read_at === null) {
      await markAsRead(api, n.id);
      await refresh();
    }
    if ("Notification" in window && onPermissionRequest) {
      const perm = await Notification.requestPermission();
      if (perm === "granted") {
        new Notification(n.event_title, { body: `Reminder: ${n.event_title}`, tag: String(n.id) });
      }
    }
  };

  const bellBadge = unreadCountNum > 0 ? (
    <span className="notif-badge">{unreadCountNum > 99 ? "99+" : unreadCountNum}</span>
  ) : null;

  return (
    <div className="notif-dropdown-wrapper" ref={dropdownRef}>
      <button
        className="notif-bell-btn"
        type="button"
        aria-label="Notifications"
        aria-expanded={open}
        onClick={() => setOpen((o) => !o)}
      >
        <span className="material-symbols-outlined notif-bell-icon">notifications</span>
        {bellBadge}
      </button>
      {open && (
        <div className="notif-dropdown">
          <div className="notif-header">
            <h3>Notifications</h3>
            {unreadCountNum > 0 && (
              <button className="notif-mark-all" type="button" onClick={handleMarkAllRead}>
                Mark all read
              </button>
            )}
          </div>
          {loading ? (
            <div className="notif-loading" role="status">
              Loading…
            </div>
          ) : !notifications || notifications.length === 0 ? (
            <div className="notif-empty">No notifications.</div>
          ) : (
            <ul className="notif-list" role="list">
              {notifications.map((n) => (
                <li
                  key={n.id}
                  className={`notif-item${n.read_at === null ? " notif-item--unread" : ""}`}
                  onClick={() => void handleNotificationClick(n)}
                >
                  <div className="notif-icon">
                    <span className="material-symbols-outlined">notifications</span>
                  </div>
                  <div className="notif-body">
                    <div className="notif-title">{n.event_title}</div>
                    <div className="notif-time">{formatRelative(n.created_at)}</div>
                  </div>
                  <button
                    className="notif-dismiss"
                    type="button"
                    aria-label="Dismiss"
                    onClick={() => handleDismiss(n.id)}
                  >
                    <span className="material-symbols-outlined" style={{ fontSize: "18px" }}>
                      close
                    </span>
                  </button>
                </li>
              ))}
            </ul>
          )}
        </div>
      )}
    </div>
  );
}
