import type { ApiClient } from "../auth/api";
import { CalendarConnections } from "./CalendarConnections";
import { UserManagement } from "./UserManagement";
import { AccountSettings } from "./AccountSettings";
import "./Settings.css";

export function Settings({ api, isAdmin, currentUserId, path, navigate, calendarHref = "/dashboard" }: {
  api: ApiClient; isAdmin: boolean; currentUserId: number; path: string; navigate(target: string): void; calendarHref?: string;
}) {
  const users = path === "/settings/users";
  const connections = path === "/settings/calendar-connections";
  return <div className="settings-page">
    <button className="app-button" type="button" onClick={() => navigate(calendarHref)}>Return to calendar</button>
    <h1>Settings</h1>
    <nav className="settings-nav" aria-label="Settings">
      <button type="button" aria-current={!users && !connections ? "page" : undefined} onClick={() => navigate("/settings/account")}>Account</button>
      <button type="button" aria-current={connections ? "page" : undefined} onClick={() => navigate("/settings/calendar-connections")}>Calendar connections</button>
      {isAdmin && <button type="button" aria-current={users ? "page" : undefined} onClick={() => navigate("/settings/users")}>Users</button>}
    </nav>
    {users ? isAdmin ? <UserManagement api={api} currentUserId={currentUserId} /> : <p role="alert">Only admins can manage users.</p>
      : connections ? <CalendarConnections api={api} />
      : <AccountSettings api={api} />}
  </div>;
}
