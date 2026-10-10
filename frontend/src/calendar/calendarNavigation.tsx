import { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { useIsMobile } from "../useIsMobile";
import { dateKey, validDateKey, validView, type CalendarView, type DateKey } from "./calendarModel";

export interface CalendarSnapshot {
  view: CalendarView;
  anchorDate: DateKey;
  selectedDay: DateKey;
  visibleCalendarIds: number[] | null;
  scroll: { page: number; timelineTop: number; timelineLeft: number };
}

export interface CalendarNavigation {
  snapshot: CalendarSnapshot;
  setView(view: CalendarView): void;
  setDate(date: DateKey): void;
  selectDay(date: DateKey): void;
  setVisibleCalendars(ids: readonly number[]): void;
  reconcileCalendars(ids: readonly number[]): void;
  captureScroll(scroll: CalendarSnapshot["scroll"]): void;
  calendarHref(): string;
}

export function parseCalendarQuery(search: string, mobile: boolean, today: DateKey) {
  const params = new URLSearchParams(search);
  const view = params.get("view");
  const date = params.get("date");
  return { view: validView(view) ? view : mobile ? "agenda" as const : "month" as const,
    anchorDate: validDateKey(date) ? date : today, selectedDay: validDateKey(date) ? date : today };
}

function isCalendarRoute() { return window.location.pathname === "/dashboard" || window.location.pathname === "/"; }
function readSnapshot(state: unknown, userId: number): CalendarSnapshot | null {
  if (!state || typeof state !== "object" || !("commoncalCalendar" in state)) return null;
  const entry = state.commoncalCalendar;
  if (!entry || typeof entry !== "object" || !("userId" in entry) || entry.userId !== userId || !("version" in entry) || entry.version !== 1 || !("snapshot" in entry)) return null;
  const value = entry.snapshot;
  if (!value || typeof value !== "object" || !("view" in value) || !validView(value.view) || !("anchorDate" in value) || !validDateKey(value.anchorDate) || !("selectedDay" in value) || !validDateKey(value.selectedDay)) return null;
  if (!("visibleCalendarIds" in value) || (value.visibleCalendarIds !== null && (!Array.isArray(value.visibleCalendarIds) || !value.visibleCalendarIds.every(id => Number.isSafeInteger(id) && id > 0)))) return null;
  if (!("scroll" in value) || !value.scroll || typeof value.scroll !== "object") return null;
  const scroll = value.scroll;
  if (!("page" in scroll) || !("timelineTop" in scroll) || !("timelineLeft" in scroll) || ![scroll.page, scroll.timelineTop, scroll.timelineLeft].every(n => typeof n === "number" && Number.isFinite(n) && n >= 0)) return null;
  return { view: value.view, anchorDate: value.anchorDate, selectedDay: value.selectedDay,
    visibleCalendarIds: value.visibleCalendarIds === null ? null : [...new Set(value.visibleCalendarIds as number[])],
    scroll: { page: scroll.page as number, timelineTop: scroll.timelineTop as number, timelineLeft: scroll.timelineLeft as number } };
}

const Context = createContext<CalendarNavigation | null>(null);

export function CalendarNavigationProvider({ userId, children }: { userId: number; children: ReactNode }) {
  const mobile = useIsMobile();
  const [snapshot, setSnapshot] = useState<CalendarSnapshot>(() => ({
    ...(readSnapshot(window.history.state, userId) ?? { visibleCalendarIds: null, scroll: { page: 0, timelineTop: 0, timelineLeft: 0 } }),
    ...parseCalendarQuery(window.location.search, mobile, dateKey(new Date())),
  }));
  const latest = useRef(snapshot);
  const query = useRef(new URLSearchParams(isCalendarRoute() ? window.location.search : ""));
  const hash = useRef(isCalendarRoute() ? window.location.hash : "");
  const href = useCallback((value: CalendarSnapshot, path = "/dashboard") => {
    const params = new URLSearchParams(query.current);
    params.set("view", value.view); params.set("date", value.anchorDate);
    return `${path}?${params}${hash.current}`;
  }, []);
  const persist = useCallback((value: CalendarSnapshot) => {
    if (!isCalendarRoute()) return;
    const state = window.history.state;
    window.history.replaceState({ ...(state && typeof state === "object" ? state : {}), commoncalCalendar: { version: 1, userId, snapshot: value } }, "", href(value, window.location.pathname));
  }, [href, userId]);
  const update = useCallback((change: Partial<CalendarSnapshot>) => {
    const next = { ...latest.current, ...change };
    latest.current = next; persist(next); setSnapshot(next);
  }, [persist]);

  useEffect(() => {
    persist(latest.current);
    const restore = () => {
      if (!isCalendarRoute()) return;
      query.current = new URLSearchParams(window.location.search);
      hash.current = window.location.hash;
      const saved = readSnapshot(window.history.state, userId);
      const next = { ...(saved ?? latest.current), ...parseCalendarQuery(window.location.search, mobile, dateKey(new Date())) };
      latest.current = next; persist(next); setSnapshot(next);
    };
    window.addEventListener("popstate", restore);
    return () => window.removeEventListener("popstate", restore);
  }, [mobile, persist, userId]);

  const captureScroll = useCallback((scroll: CalendarSnapshot["scroll"]) => {
    if (!isCalendarRoute()) return;
    latest.current = { ...latest.current, scroll }; persist(latest.current);
  }, [persist]);

  const value = useMemo<CalendarNavigation>(() => ({
    snapshot,
    setView(view) { update({ view }); },
    setDate(date) { if (validDateKey(date)) update({ anchorDate: date, selectedDay: date }); },
    selectDay(date) { if (validDateKey(date)) update({ anchorDate: date, selectedDay: date }); },
    setVisibleCalendars(ids) { update({ visibleCalendarIds: [...new Set(ids)] }); },
    reconcileCalendars(ids) {
      const current = latest.current.visibleCalendarIds;
      if (current === null) return;
      const reconciled = current.filter(id => ids.includes(id));
      if (reconciled.length !== current.length) update({ visibleCalendarIds: reconciled });
    },
    captureScroll,
    calendarHref() { return href(latest.current); },
  }), [snapshot, update, href, captureScroll]);
  return <Context.Provider value={value}>{children}</Context.Provider>;
}

export function useOptionalCalendarNavigation() { return useContext(Context); }
export function useCalendarNavigation() {
  const context = useOptionalCalendarNavigation();
  if (!context) throw new Error("Calendar navigation requires an authenticated provider");
  return context;
}
