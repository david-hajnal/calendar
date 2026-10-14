import { Fragment, useEffect, useRef, useState } from "react";
import type { EventProjection } from "./api";
import type { Calendar } from "./CalendarManagement";
import { dateKey, eventEditable as editable, eventTitle as title, type CalendarView } from "./calendarModel";
import { allDayCovers, type DragState } from "./CalendarEventUI";
import { layoutDay, initialDayMinute } from "./dayLayout";
import "./DayCalendar.css";

function formatTime(unixSeconds: number): string {
  const d = new Date(unixSeconds * 1000);
  return `${String(d.getHours()).padStart(2, "0")}:${String(d.getMinutes()).padStart(2, "0")}`;
}

export interface DayCalendarProps {
  events: EventProjection[];
  date: Date;
  gridRef: React.RefObject<HTMLDivElement | null>;
  dragging: DragState | null;
  slotHighlight: { dayIndex: number; hour: number } | null;
  captureScroll?: (state: { page: number; timelineTop: number; timelineLeft: number }) => void;
  onGridPointerMove: (e: React.PointerEvent) => void;
  onGridPointerUp: (e: React.PointerEvent) => void;
  cancelDrag: () => void;
  renderEvent: (event: EventProjection, monthDate?: string) => React.ReactNode;
  calendarFor: (event: EventProjection) => Calendar | undefined;
  isSaving: (event: EventProjection) => boolean;
  openDetail: (event: EventProjection, control?: HTMLElement) => void;
  openEdit: (event: EventProjection) => void;
  onPointerDown: (event: EventProjection, e: React.PointerEvent, dayIndex: number) => void;
  onResizeHandleDown: (event: EventProjection, direction: "top" | "bottom", e: React.PointerEvent) => void;
  setSlotHighlight: (highlight: { dayIndex: number; hour: number } | null) => void;
  onSlotClick: (dayIndex: number, hour: number, view: CalendarView) => void;
  suppressClick: () => boolean;
  loading?: boolean;
  loadError?: string | null;
  retry?: () => void;
  hasVisibleCalendars?: boolean;
  canCreate?: boolean;
}

export function DayCalendar({
  events,
  date,
  gridRef,
  dragging,
  slotHighlight,
  captureScroll,
  onGridPointerMove,
  onGridPointerUp,
  cancelDrag,
  renderEvent,
  calendarFor,
  isSaving,
  openDetail,
  openEdit,
  onPointerDown,
  onResizeHandleDown,
  setSlotHighlight,
  onSlotClick,
  suppressClick,
  loading,
  loadError,
  retry,
  hasVisibleCalendars = true,
  canCreate = false,
}: DayCalendarProps) {
  const anchorDateRef = useRef<string | null>(null);
  useEffect(() => {
    if (loading || loadError || !hasVisibleCalendars) return;
    const dk = dateKey(date);
    if (anchorDateRef.current === dk) return;
    anchorDateRef.current = dk;
    const minute = initialDayMinute(events, date);
    const desired = Math.max(0, minute - 120);
    const el = gridRef.current;
    if (el) {
      el.scrollTop = Math.min(desired, Math.max(0, el.scrollHeight - el.clientHeight));
    }
  }, [date, loading, loadError, hasVisibleCalendars, events, gridRef]);

  const isToday = dateKey(date) === dateKey(new Date());
  const [nowMinute, setNowMinute] = useState(() => new Date().getHours() * 60 + new Date().getMinutes());
  useEffect(() => {
    if (!isToday) return;
    const tick = () => setNowMinute(new Date().getHours() * 60 + new Date().getMinutes());
    tick();
    const id = setInterval(tick, 30_000);
    return () => clearInterval(id);
  }, [isToday]);
  if (loading) {
    return (
      <div className="event-ui__day-region" role="status">
        <div className="event-ui__loading"><div className="spinner" /> <span className="typography-body-md">Loading events…</span></div>
      </div>
    );
  }
  if (loadError) {
    return (
      <div className="event-ui__day-region" role="alert">
        <p className="app-message app-message--error">{loadError}</p>
        {retry && <button type="button" className="app-button" onClick={retry}>Retry</button>}
      </div>
    );
  }
  if (!hasVisibleCalendars) {
    return (
      <div className="event-ui__day-region event-ui__day-region--empty" role="status">
        <p className="app-message">No calendars are selected. Select a calendar to see its events.</p>
      </div>
    );
  }
  const segments = layoutDay(events, date);
  const isEmpty = segments.length === 0;
  return (
    <div ref={gridRef} onScroll={event => captureScroll?.({ page: Math.max(0, window.scrollY), timelineTop: event.currentTarget.scrollTop, timelineLeft: event.currentTarget.scrollLeft })} className="event-ui__day-region" onPointerMove={onGridPointerMove} onPointerUp={onGridPointerUp} onPointerCancel={cancelDrag}>
      <div className="event-ui__allday-row">
        <div className="event-ui__allday-label"><span className="typography-label-md">All day</span></div>
        <div className="event-ui__allday-events">{events.filter((e) => allDayCovers(e, dateKey(date))).map((event) => (
          <Fragment key={`${event.calendar_id}-${event.id}-${event.recurrence_id ?? event.recurrence_date ?? ""}`}>{renderEvent(event)}</Fragment>
        ))}</div>
      </div>
      <div className="event-ui__day-body">
        <div className="event-ui__day-timeline">
          <div className="event-ui__time-column">
            <div className="event-ui__timezone-label" aria-label="Timezone">
              <span>{Intl.DateTimeFormat().resolvedOptions().timeZone?.replace(/_/g, " ") ?? "Local"}</span>
            </div>
            {Array.from({ length: 24 }, (_, h) => (
              <div key={h} className="event-ui__time-label">
                <span>{h === 0 ? "12 AM" : h < 12 ? `${h} AM` : h === 12 ? "12 PM" : `${h - 12} PM`}</span>
              </div>
            ))}
          </div>
          <div className="event-ui__day-column" onMouseLeave={() => setSlotHighlight(null)}>
            {Array.from({ length: 24 }, (_, h) => (
              <div
                key={h}
                className="event-ui__hour-row"
                tabIndex={0}
                role="button"
                aria-label={`Create event at ${h === 0 ? "12 AM" : h < 12 ? `${h} AM` : h === 12 ? "12 PM" : `${h - 12} PM`}`}
                onClick={() => { if (suppressClick()) return; onSlotClick(0, h, "day"); }}
                onKeyDown={(e) => { if (e.key === "Enter" || e.key === " ") { e.preventDefault(); if (suppressClick()) return; onSlotClick(0, h, "day"); } }}
                onMouseEnter={() => setSlotHighlight({ dayIndex: 0, hour: h })}
              />
            ))}
            {isEmpty && (
              <div className="event-ui__empty-day" role="status">
                <p className="app-message">No events on this day.</p>
                {canCreate && <button type="button" className="app-button" onClick={() => onSlotClick(0, 9, "day")}>New event</button>}
              </div>
            )}
            {segments.map((segment) => {
              const event = segment.event;
              const top = segment.top;
              const height = segment.height;
              const left = (segment.column / segment.columnCount) * 100;
              const width = 100 / segment.columnCount;
              const cal = calendarFor(event);
              const accentColor = cal?.color || "var(--color-primary)";
              const bgColor = `${accentColor}1a`;
              const isDragging = dragging?.event.id === event.id && dragging.started;
              const canEdit = editable(event, cal);
              const showMeta = height > 50 && event.start_utc != null && event.end_utc != null;
              const ariaLabel = [
                title(event),
                showMeta ? `${formatTime(event.start_utc!)} to ${formatTime(event.end_utc!)}` : null,
                cal?.name ?? null,
                segment.isClippedStart ? "continues from previous day" : null,
                segment.isClippedEnd ? "continues to next day" : null,
              ].filter(Boolean).join(", ");
              return (
                <div
                  key={`${event.calendar_id}-${event.id}-${event.recurrence_id ?? event.recurrence_date ?? ""}`}
                  className={`event-ui__event-block ${isSaving(event) ? "event-ui__event-block--saving" : ""}`}
                  style={{ top: `${top}px`, height: `${height}px`, left: `${left}%`, width: `${width}%`, borderLeftColor: accentColor, background: bgColor, opacity: isDragging ? 0.6 : 1, cursor: isDragging ? "grabbing" : dragging && canEdit ? "grab" : "pointer", zIndex: isDragging ? 10 : 2 }}
                  aria-label={ariaLabel}
                  onClick={() => { if (suppressClick()) return; openDetail(event); }}
                  onDoubleClick={() => openEdit(event)}
                  onPointerDown={(e) => onPointerDown(event, e, 0)}
                >
                  {canEdit && height > 30 && !segment.isClippedStart && (
                    <div className="event-ui__resize-handle event-ui__resize-handle--top" onPointerDown={(e) => onResizeHandleDown(event, "top", e)} />
                  )}
                  <div className="event-ui__event-card">
                    {segment.isClippedStart && <span className="event-ui__event-continue" aria-hidden="true">↑</span>}
                    <button type="button" className="event-chip__text event-ui__event-title" draggable={false} onClick={(click) => { click.stopPropagation(); openDetail(event); }}>
                      {title(event)}
                    </button>
                    {showMeta && (
                      <small className="event-ui__event-meta">
                        {formatTime(event.start_utc!)}–{formatTime(event.end_utc!)}
                        {cal?.name ? ` · ${cal.name}` : ""}
                      </small>
                    )}
                    {segment.isClippedEnd && <span className="event-ui__event-continue" aria-hidden="true">↓</span>}
                  </div>
                  {canEdit && height > 30 && !segment.isClippedEnd && (
                    <div className="event-ui__resize-handle event-ui__resize-handle--bottom" onPointerDown={(e) => onResizeHandleDown(event, "bottom", e)} />
                  )}
                </div>
              );
            })}
            {isToday && (
              <div className="event-ui__current-time" style={{ top: `${nowMinute}px` }}>
                <span className="event-ui__current-time-label">{String(Math.floor(nowMinute / 60)).padStart(2, "0")}:{String(nowMinute % 60).padStart(2, "0")}</span>
              </div>
            )}
            {slotHighlight && <div className="event-ui__slot-highlight" style={{ top: `${slotHighlight.hour * 60}px` }} />}
          </div>
        </div>
        <div className="event-ui__day-schedule" role="complementary" aria-label="Day schedule">
          <h3 className="event-ui__schedule-heading">Schedule</h3>
          <div className="event-ui__schedule-list">
            {events.filter((e) => allDayCovers(e, dateKey(date))).map((event) => {
              const cal = calendarFor(event);
              return (
                <div
                  key={`allday-${event.calendar_id}-${event.id}-${event.recurrence_id ?? event.recurrence_date ?? ""}`}
                  className="event-ui__schedule-item event-ui__schedule-item--allday"
                  tabIndex={0}
                  role="button"
                  aria-label={`${title(event)}, All day${cal?.name ? `, ${cal.name}` : ""}`}
                  onClick={() => openDetail(event)}
                  onKeyDown={(e) => { if (e.key === "Enter" || e.key === " ") { e.preventDefault(); openDetail(event); } }}
                  onDoubleClick={() => openEdit(event)}
                >
                  <span className="event-ui__schedule-title">{title(event)}</span>
                  <span className="event-ui__schedule-time">All day</span>
                  {cal?.name && <span className="event-ui__schedule-cal">{cal.name}</span>}
                </div>
              );
            })}
            {segments.map((segment) => {
              const event = segment.event;
              const cal = calendarFor(event);
              const timeLabel = event.start_utc != null && event.end_utc != null ? `${formatTime(event.start_utc)}–${formatTime(event.end_utc)}` : null;
              return (
                <div
                  key={`timed-${event.calendar_id}-${event.id}-${event.recurrence_id ?? event.recurrence_date ?? ""}`}
                  className="event-ui__schedule-item"
                  tabIndex={0}
                  role="button"
                  aria-label={`${title(event)}${timeLabel ? `, ${timeLabel}` : ""}${cal?.name ? `, ${cal.name}` : ""}`}
                  onClick={() => openDetail(event)}
                  onKeyDown={(e) => { if (e.key === "Enter" || e.key === " ") { e.preventDefault(); openDetail(event); } }}
                  onDoubleClick={() => openEdit(event)}
                >
                  <span className="event-ui__schedule-title">{title(event)}</span>
                  {timeLabel && <span className="event-ui__schedule-time">{timeLabel}</span>}
                  {cal?.name && <span className="event-ui__schedule-cal">{cal.name}</span>}
                </div>
              );
            })}
          </div>
        </div>
      </div>
    </div>
  );
}
