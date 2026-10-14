import type { EventProjection } from "./api";

export interface DaySegment {
  event: EventProjection;
  top: number;
  height: number;
  isClippedStart: boolean;
  isClippedEnd: boolean;
  column: number;
  columnCount: number;
}

const MIN_HEIGHT = 15;

export function assignColumns(segments: DaySegment[]): DaySegment[] {
  if (segments.length === 0) return [];

  const sorted = [...segments].sort((a, b) => {
    if (a.top !== b.top) return a.top - b.top;
    return (a.top + a.height) - (b.top + b.height);
  });

  // Find connected groups: segments that transitively overlap in time.
  const groups: DaySegment[][] = [];
  let currentGroup: DaySegment[] = [sorted[0]];
  let groupMaxEnd = sorted[0].top + sorted[0].height;

  for (let i = 1; i < sorted.length; i++) {
    const seg = sorted[i];
    if (seg.top >= groupMaxEnd) {
      groups.push(currentGroup);
      currentGroup = [seg];
      groupMaxEnd = seg.top + seg.height;
    } else {
      currentGroup.push(seg);
      groupMaxEnd = Math.max(groupMaxEnd, seg.top + seg.height);
    }
  }
  groups.push(currentGroup);

  // Assign columns within each group using greedy interval partitioning.
  const result: DaySegment[] = [];
  for (const group of groups) {
    const columnEnds: number[] = [];
    const assigned: DaySegment[] = [];

    for (const seg of group) {
      const segEnd = seg.top + seg.height;
      let col = -1;
      for (let c = 0; c < columnEnds.length; c++) {
        if (columnEnds[c] <= seg.top) {
          col = c;
          break;
        }
      }
      if (col === -1) {
        col = columnEnds.length;
        columnEnds.push(segEnd);
      } else {
        columnEnds[col] = segEnd;
      }
      assigned.push({ ...seg, column: col });
    }

    const columnCount = columnEnds.length;
    for (const seg of assigned) {
      result.push({ ...seg, columnCount });
    }
  }

  return result;
}

export function layoutDay(events: EventProjection[], date: Date): DaySegment[] {
  const dayStart = new Date(date);
  dayStart.setHours(0, 0, 0, 0);
  const dayEnd = new Date(dayStart);
  dayEnd.setDate(dayEnd.getDate() + 1);

  const segments: DaySegment[] = [];

  for (const event of events) {
    if (event.event_kind === "all_day") continue;
    if (event.start_utc == null || event.end_utc == null) continue;

    const eventStart = new Date(event.start_utc * 1000);
    const eventEnd = new Date(event.end_utc * 1000);

    const clippedStart = Math.max(eventStart.getTime(), dayStart.getTime());
    const clippedEnd = Math.min(eventEnd.getTime(), dayEnd.getTime());

    if (clippedStart >= clippedEnd) continue;

    const top = (clippedStart - dayStart.getTime()) / 60000;
    const height = (clippedEnd - clippedStart) / 60000;

    segments.push({
      event,
      top,
      height: Math.max(height, MIN_HEIGHT),
      isClippedStart: eventStart.getTime() < dayStart.getTime(),
      isClippedEnd: eventEnd.getTime() > dayEnd.getTime(),
      column: 0,
      columnCount: 1,
    });
  }

  return assignColumns(segments);
}

export function dayGestureTimes(
  event: EventProjection,
  deltaMinutes: number,
): { start_utc: number; end_utc: number } {
  if (event.start_utc == null || event.end_utc == null) {
    throw new Error("Event has no timestamps");
  }
  return {
    start_utc: event.start_utc + deltaMinutes * 60,
    end_utc: event.end_utc + deltaMinutes * 60,
  };
}

const MIN_DURATION_SECONDS = 15 * 60;

export function dayResizeTimes(
  event: EventProjection,
  direction: "top" | "bottom",
  deltaMinutes: number,
): { start_utc: number; end_utc: number } {
  if (event.start_utc == null || event.end_utc == null) {
    throw new Error("Event has no timestamps");
  }
  if (direction === "top") {
    // Preserve end, change start. Start cannot go past end - 15 min.
    const newStart = event.start_utc + deltaMinutes * 60;
    const maxStart = event.end_utc - MIN_DURATION_SECONDS;
    return { start_utc: Math.min(newStart, maxStart), end_utc: event.end_utc };
  }
  // Preserve start, change end. End cannot go before start + 15 min.
  const newEnd = event.end_utc + deltaMinutes * 60;
  const minEnd = event.start_utc + MIN_DURATION_SECONDS;
  return { start_utc: event.start_utc, end_utc: Math.max(newEnd, minEnd) };
}

export function initialDayMinute(events: EventProjection[], date: Date): number {
  const now = new Date();
  const isToday = date.toDateString() === now.toDateString();
  if (isToday) {
    return now.getHours() * 60 + now.getMinutes();
  }
  const segments = layoutDay(events, date);
  if (segments.length > 0) {
    const first = segments.reduce((min, seg) => seg.top < min.top ? seg : min, segments[0]);
    return Math.round(first.top);
  }
  return 8 * 60;
}
