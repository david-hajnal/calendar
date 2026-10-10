import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import { CalendarNavigationProvider, parseCalendarQuery, useCalendarNavigation } from "./calendarNavigation";

function Controls() {
  const nav = useCalendarNavigation();
  return <><span data-testid="surface">{nav.surface?.kind ?? "missing"}</span>
    <button onClick={() => nav.openDetail({ calendarId: 1, eventId: 10 })}>Open detail</button>
    <button onClick={() => nav.closeSurface()}>Return</button>
    <output>{JSON.stringify(nav.snapshot)}</output>
    <button onClick={() => nav.setVisibleCalendars([])}>Hide all</button>
    <button onClick={() => nav.setVisibleCalendars([1, 2])}>Choose calendars</button>
    <button onClick={() => nav.reconcileCalendars([2, 3])}>Reconcile</button>
    <button onClick={() => nav.setView("week")}>Week</button>
  </>;
}
afterEach(() => { cleanup(); vi.unstubAllGlobals(); window.history.replaceState({}, "", "/"); });

it("rejects impossible date/view query values and preserves explicit valid choices", () => {
  expect(parseCalendarQuery("?view=nonsense&date=2026-02-30", true, "2026-10-09")).toEqual({ view: "agenda", anchorDate: "2026-10-09", selectedDay: "2026-10-09" });
  expect(parseCalendarQuery("?view=week&date=2024-02-29", true, "2026-10-09")).toEqual({ view: "week", anchorDate: "2024-02-29", selectedDay: "2024-02-29" });
  expect(parseCalendarQuery("", false, "2026-10-09").view).toBe("month");
});

it("restores only minimal valid current-user history and reconciles revoked filters", () => {
  window.history.replaceState({ otherOwner: "preserve", commoncalCalendar: { version: 1, userId: 7, snapshot: { description: "must not enter navigation state", view: "agenda", anchorDate: "2026-10-09", selectedDay: "2026-10-09", visibleCalendarIds: [1, 2], scroll: { page: 50, timelineTop: 0, timelineLeft: 0 } } } }, "", "/dashboard?view=agenda&date=2026-10-09&source=bookmark");
  render(<CalendarNavigationProvider userId={7}><Controls /></CalendarNavigationProvider>);
  fireEvent.click(screen.getByRole("button", { name: "Reconcile" }));
  expect(JSON.parse(screen.getByRole("status").textContent!).visibleCalendarIds).toEqual([2]);
  fireEvent.click(screen.getByRole("button", { name: "Hide all" }));
  expect(JSON.parse(screen.getByRole("status").textContent!).visibleCalendarIds).toEqual([]);
  expect(window.history.state.commoncalCalendar.snapshot).not.toHaveProperty("description");
  expect(window.history.state.otherOwner).toBe("preserve");
  expect(window.history.state.commoncalCalendar.snapshot.scroll.page).toBe(50);
  expect(new URLSearchParams(window.location.search).get("source")).toBe("bookmark");
});

it("does not resurrect another user's filters or invalid scroll values", () => {
  const state = { commoncalCalendar: { version: 1, userId: 99, snapshot: { view: "week", anchorDate: "2026-10-09", selectedDay: "2026-10-09", visibleCalendarIds: [], scroll: { page: 500, timelineTop: 0, timelineLeft: 0 } } } };
  window.history.replaceState(state, "", "/dashboard?view=agenda&date=2026-10-09");
  render(<CalendarNavigationProvider userId={7}><Controls /></CalendarNavigationProvider>);
  expect(JSON.parse(screen.getByRole("status").textContent!).visibleCalendarIds).toBeNull();
  cleanup();
  state.commoncalCalendar.userId = 7;
  state.commoncalCalendar.snapshot.scroll.page = -1;
  window.history.replaceState(state, "", "/dashboard?view=agenda&date=2026-10-09");
  render(<CalendarNavigationProvider userId={7}><Controls /></CalendarNavigationProvider>);
  expect(JSON.parse(screen.getByRole("status").textContent!).scroll.page).toBe(0);
});


it("pushes identity-only detail state and discards surfaces on a fresh chain", () => {
  window.history.replaceState({}, "", "/dashboard?view=agenda&date=2025-06-16");
  render(<CalendarNavigationProvider userId={7}><Controls /></CalendarNavigationProvider>);
  const before = window.history.length;
  fireEvent.click(screen.getByRole("button", { name: "Open detail" }));
  expect(screen.getByTestId("surface")).toHaveTextContent("detail");
  expect(window.history.length).toBe(before + 1);
  expect(window.history.state.commoncalCalendar.surface).toEqual({ kind: "detail", identity: { calendarId: 1, eventId: 10 } });
  cleanup();
  render(<CalendarNavigationProvider userId={7}><Controls /></CalendarNavigationProvider>);
  expect(screen.getByTestId("surface")).toHaveTextContent("calendar");
});
