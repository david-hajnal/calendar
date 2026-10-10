import { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { useIsMobile } from "../useIsMobile";
import { dateKey, validDateKey, validView, type CalendarView, type DateKey, type EventIdentity } from "./calendarModel";

import { navigate } from "../navigation";

export type CalendarSurface = { kind: "calendar" } | { kind: "detail"; identity: EventIdentity };

export interface CalendarSnapshot {
  view: CalendarView;
  anchorDate: DateKey;
  selectedDay: DateKey;
  visibleCalendarIds: number[] | null;
  scroll: { page: number; timelineTop: number; timelineLeft: number };
}

export interface CalendarNavigation {
  snapshot: CalendarSnapshot;
  surface: CalendarSurface;
  openDetail(identity: EventIdentity): void;
  closeSurface(): void;
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
  const chainId = useRef(crypto.randomUUID());
  const index = useRef(0);
  const visited = useRef(new Set([0]));
  const [surface, setSurface] = useState<CalendarSurface>({ kind: "calendar" });
  const surfaceRef = useRef(surface);
  const origin = useRef<number | null>(null);
  const readSurface = useCallback((state: unknown): CalendarSurface => {
    const entry = (state as { commoncalCalendar?: { version?: number; userId?: number; chainId?: string; index?: number; originIndex?: number; surface?: CalendarSurface } } | null)?.commoncalCalendar;
    if (!entry || entry.version !== 1 || entry.userId !== userId || entry.chainId !== chainId.current || !Number.isSafeInteger(entry.index) || !visited.current.has(entry.index!)) return { kind: "calendar" };
    index.current = entry.index!;
    origin.current = Number.isSafeInteger(entry.originIndex) && visited.current.has(entry.originIndex!) ? entry.originIndex! : null;
    const value = entry.surface;
    if (value?.kind !== "detail") return { kind: "calendar" };
    const identity = value.identity;
    if (!identity || !Number.isSafeInteger(identity.calendarId) || identity.calendarId <= 0 || !Number.isSafeInteger(identity.eventId) || identity.eventId <= 0 || (identity.recurrenceId !== undefined && typeof identity.recurrenceId !== "string" && typeof identity.recurrenceId !== "number")) return { kind: "calendar" };
    return { kind: "detail", identity: { calendarId: identity.calendarId, eventId: identity.eventId, ...(identity.recurrenceId === undefined ? {} : { recurrenceId: identity.recurrenceId }) } };
  }, [userId]);
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
    window.history.replaceState({ ...(state && typeof state === "object" ? state : {}), commoncalCalendar: { version: 1, userId, snapshot: value, chainId: chainId.current, index: index.current, originIndex: origin.current, surface: surfaceRef.current } }, "", href(value, window.location.pathname));
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
      const nextSurface = readSurface(window.history.state);
      surfaceRef.current = nextSurface; setSurface(nextSurface);
      const saved = readSnapshot(window.history.state, userId);
      const next = { ...(saved ?? latest.current), ...parseCalendarQuery(window.location.search, mobile, dateKey(new Date())) };
      latest.current = next; persist(next); setSnapshot(next);
    };
    window.addEventListener("popstate", restore);
    return () => window.removeEventListener("popstate", restore);
  }, [mobile, persist, userId, readSurface]);

  const captureScroll = useCallback((scroll: CalendarSnapshot["scroll"]) => {
    if (!isCalendarRoute() || surfaceRef.current.kind !== "calendar") return;
    latest.current = { ...latest.current, scroll }; persist(latest.current);
  }, [persist]);

  const value = useMemo<CalendarNavigation>(() => ({
    snapshot, surface,
    openDetail(identity) {
      persist(latest.current);
      const originIndex = index.current;
      const nextIndex = originIndex + 1;
      for (const known of visited.current) if (known >= nextIndex) visited.current.delete(known);
      visited.current.add(nextIndex);
      navigate(href(latest.current), { state: { commoncalCalendar: { version: 1, userId, chainId: chainId.current, index: nextIndex, originIndex, snapshot: latest.current, surface: { kind: "detail", identity } } } });
    },
    closeSurface() {
      const target = origin.current;
      if (target !== null && visited.current.has(target) && index.current === target + 1) window.history.back();
      else { surfaceRef.current = { kind: "calendar" }; setSurface(surfaceRef.current); origin.current = null; persist(latest.current); }
    },
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
  }), [snapshot, surface, update, href, captureScroll, persist, userId]);
  return <Context.Provider value={value}>{children}</Context.Provider>;
}

export function useOptionalCalendarNavigation() { return useContext(Context); }
export function useCalendarNavigation() {
  const context = useOptionalCalendarNavigation();
  if (!context) throw new Error("Calendar navigation requires an authenticated provider");
  return context;
}
