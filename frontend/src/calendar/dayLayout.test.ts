import { describe, expect, it } from "vitest";
import { assignColumns, layoutDay, dayGestureTimes, dayResizeTimes } from "./dayLayout";
import type { EventProjection } from "./api";

function makeEvent(overrides: Partial<EventProjection> = {}): EventProjection {
  return {
    id: 1,
    calendar_id: 1,
    access: "details",
    status: "confirmed",
    event_kind: "timed",
    title: "Test Event",
    start_utc: Math.floor(new Date("2025-06-16T10:00:00").getTime() / 1000),
    end_utc: Math.floor(new Date("2025-06-16T11:00:00").getTime() / 1000),
    timezone: "UTC",
    version: 1,
    start_date: "2025-06-16",
    ...overrides,
  };
}

describe("layoutDay", () => {
  it("clips an overnight event to the first day", () => {
    const startUtc = Math.floor(new Date("2025-06-16T23:00:00").getTime() / 1000);
    const endUtc = Math.floor(new Date("2025-06-17T02:00:00").getTime() / 1000);
    const event = makeEvent({ start_utc: startUtc, end_utc: endUtc });

    const segments = layoutDay([event], new Date("2025-06-16T12:00:00"));

    expect(segments).toHaveLength(1);
    expect(segments[0].top).toBe(23 * 60);
    expect(segments[0].height).toBe(60);
    expect(segments[0].isClippedStart).toBe(false);
    expect(segments[0].isClippedEnd).toBe(true);
  });

  it("clips an overnight event to the second day", () => {
    const startUtc = Math.floor(new Date("2025-06-16T23:00:00").getTime() / 1000);
    const endUtc = Math.floor(new Date("2025-06-17T02:00:00").getTime() / 1000);
    const event = makeEvent({ start_utc: startUtc, end_utc: endUtc });

    const segments = layoutDay([event], new Date("2025-06-17T12:00:00"));

    expect(segments).toHaveLength(1);
    expect(segments[0].top).toBe(0);
    expect(segments[0].height).toBe(2 * 60);
    expect(segments[0].isClippedStart).toBe(true);
    expect(segments[0].isClippedEnd).toBe(false);
  });

  it("excludes an event ending at midnight from the next day", () => {
    const startUtc = Math.floor(new Date("2025-06-16T22:00:00").getTime() / 1000);
    const endUtc = Math.floor(new Date("2025-06-17T00:00:00").getTime() / 1000);
    const event = makeEvent({ start_utc: startUtc, end_utc: endUtc });

    const segments = layoutDay([event], new Date("2025-06-17T12:00:00"));

    expect(segments).toHaveLength(0);
  });

  it("skips all-day events", () => {
    const event = makeEvent({ event_kind: "all_day", start_utc: undefined, end_utc: undefined });

    const segments = layoutDay([event], new Date("2025-06-16T12:00:00"));

    expect(segments).toHaveLength(0);
  });

  it("retains recurring event identity", () => {
    const event = makeEvent({ recurrence_id: 1, recurrence_date: "2025-06-16" });

    const segments = layoutDay([event], new Date("2025-06-16T12:00:00"));

    expect(segments).toHaveLength(1);
    expect(segments[0].event.recurrence_id).toBe(1);
    expect(segments[0].event.recurrence_date).toBe("2025-06-16");
  });

  it("applies minimum height", () => {
    const startUtc = Math.floor(new Date("2025-06-16T10:00:00").getTime() / 1000);
    const endUtc = Math.floor(new Date("2025-06-16T10:01:00").getTime() / 1000);
    const event = makeEvent({ start_utc: startUtc, end_utc: endUtc });

    const segments = layoutDay([event], new Date("2025-06-16T12:00:00"));

    expect(segments).toHaveLength(1);
    expect(segments[0].height).toBeGreaterThanOrEqual(15);
  });

  it("returns the original event object for detail lookup", () => {
    const event = makeEvent({ title: "Original" });

    const segments = layoutDay([event], new Date("2025-06-16T12:00:00"));

    expect(segments[0].event).toBe(event);
  });

  it("assigns separate columns to simultaneous overlaps", () => {
    const a = makeEvent({ id: 1, start_utc: Math.floor(new Date("2025-06-16T10:00:00").getTime() / 1000), end_utc: Math.floor(new Date("2025-06-16T11:00:00").getTime() / 1000) });
    const b = makeEvent({ id: 2, start_utc: Math.floor(new Date("2025-06-16T10:30:00").getTime() / 1000), end_utc: Math.floor(new Date("2025-06-16T11:30:00").getTime() / 1000) });

    const segments = layoutDay([a, b], new Date("2025-06-16T12:00:00"));

    expect(segments).toHaveLength(2);
    const cols = segments.map((s) => s.column).sort();
    expect(cols).toEqual([0, 1]);
    expect(segments.every((s) => s.columnCount === 2)).toBe(true);
  });

  it("keeps adjacent non-overlapping events at full width", () => {
    const a = makeEvent({ id: 1, start_utc: Math.floor(new Date("2025-06-16T10:00:00").getTime() / 1000), end_utc: Math.floor(new Date("2025-06-16T11:00:00").getTime() / 1000) });
    const b = makeEvent({ id: 2, start_utc: Math.floor(new Date("2025-06-16T11:00:00").getTime() / 1000), end_utc: Math.floor(new Date("2025-06-16T12:00:00").getTime() / 1000) });

    const segments = layoutDay([a, b], new Date("2025-06-16T12:00:00"));

    expect(segments).toHaveLength(2);
    expect(segments.every((s) => s.column === 0 && s.columnCount === 1)).toBe(true);
  });
});

describe("assignColumns", () => {
  function seg(id: number, top: number, height: number) {
    return { event: makeEvent({ id }), top, height, isClippedStart: false, isClippedEnd: false, column: 0, columnCount: 1 };
  }

  it("separates three truly simultaneous overlaps into distinct columns", () => {
    const a = seg(1, 600, 100);
    const b = seg(2, 620, 100);
    const c = seg(3, 640, 100);

    const result = assignColumns([a, b, c]);

    expect(result).toHaveLength(3);
    const cols = result.map((s) => s.column).sort();
    expect(cols).toEqual([0, 1, 2]);
    expect(result.every((s) => s.columnCount === 3)).toBe(true);
  });

  it("handles chained overlaps (A∩B, B∩C, A∩C=∅) with column reuse", () => {
    const a = seg(1, 0, 60);
    const b = seg(2, 30, 60);
    const c = seg(3, 60, 60);

    const result = assignColumns([a, b, c]);

    expect(result).toHaveLength(3);
    // A and C don't overlap, so they share a column; B gets its own.
    expect(result.every((s) => s.columnCount === 2)).toBe(true);
    const byId = new Map(result.map((s) => [s.event.id, s]));
    expect(byId.get(1)!.column).toBe(0);
    expect(byId.get(2)!.column).toBe(1);
    expect(byId.get(3)!.column).toBe(0);
  });

  it("gives full width to non-overlapping events in separate groups", () => {
    const a = seg(1, 0, 60);
    const b = seg(2, 120, 60);

    const result = assignColumns([a, b]);

    expect(result).toHaveLength(2);
    expect(result.every((s) => s.column === 0 && s.columnCount === 1)).toBe(true);
  });

  it("reuses columns when a later event does not overlap an earlier one", () => {
    const a = seg(1, 0, 60);
    const b = seg(2, 30, 60);
    const c = seg(3, 90, 60);

    const result = assignColumns([a, b, c]);

    expect(result).toHaveLength(3);
    // A and B overlap (same group, 2 columns); C is in a separate group (full width).
    const byId = new Map(result.map((s) => [s.event.id, s]));
    expect(byId.get(1)!.column).toBe(0);
    expect(byId.get(1)!.columnCount).toBe(2);
    expect(byId.get(2)!.column).toBe(1);
    expect(byId.get(2)!.columnCount).toBe(2);
    expect(byId.get(3)!.column).toBe(0);
    expect(byId.get(3)!.columnCount).toBe(1);
  });

  it("returns empty array for empty input", () => {
    expect(assignColumns([])).toEqual([]);
  });

  it("does not let minimum-height short cards conceal neighbors", () => {
    const a = seg(1, 600, 15);
    const b = seg(2, 600, 15);

    const result = assignColumns([a, b]);

    expect(result).toHaveLength(2);
    expect(result.every((s) => s.columnCount === 2)).toBe(true);
    const cols = result.map((s) => s.column).sort();
    expect(cols).toEqual([0, 1]);
  });
});

describe("dayGestureTimes (Slice 16)", () => {
  it("shifts a normal event by the snapped delta", () => {
    const event = makeEvent({
      start_utc: Math.floor(new Date("2025-06-16T10:00:00").getTime() / 1000),
      end_utc: Math.floor(new Date("2025-06-16T11:00:00").getTime() / 1000),
    });

    const result = dayGestureTimes(event, 30);

    expect(result.start_utc).toBe(event.start_utc! + 30 * 60);
    expect(result.end_utc).toBe(event.end_utc! + 30 * 60);
    // Duration preserved
    expect(result.end_utc - result.start_utc).toBe(event.end_utc! - event.start_utc!);
  });

  it("shifts a normal event backwards by the snapped delta", () => {
    const event = makeEvent({
      start_utc: Math.floor(new Date("2025-06-16T10:00:00").getTime() / 1000),
      end_utc: Math.floor(new Date("2025-06-16T11:00:00").getTime() / 1000),
    });

    const result = dayGestureTimes(event, -45);

    expect(result.start_utc).toBe(event.start_utc! - 45 * 60);
    expect(result.end_utc).toBe(event.end_utc! - 45 * 60);
    expect(result.end_utc - result.start_utc).toBe(event.end_utc! - event.start_utc!);
  });

  it("moves a clipped continuation (overnight) event by the correct delta", () => {
    // Event starts at 23:00 on June 16, ends at 02:00 on June 17
    const event = makeEvent({
      start_utc: Math.floor(new Date("2025-06-16T23:00:00").getTime() / 1000),
      end_utc: Math.floor(new Date("2025-06-17T02:00:00").getTime() / 1000),
    });

    const result = dayGestureTimes(event, 60);

    // Both timestamps shift by 60 minutes
    expect(result.start_utc).toBe(event.start_utc! + 60 * 60);
    expect(result.end_utc).toBe(event.end_utc! + 60 * 60);
    // Duration preserved (3 hours)
    expect(result.end_utc - result.start_utc).toBe(3 * 3600);
  });

  it("moves a >24-hour event by the correct delta without clamping", () => {
    // Event spans 30 hours: June 16 10:00 → June 17 16:00
    const event = makeEvent({
      start_utc: Math.floor(new Date("2025-06-16T10:00:00").getTime() / 1000),
      end_utc: Math.floor(new Date("2025-06-17T16:00:00").getTime() / 1000),
    });

    const result = dayGestureTimes(event, 90);

    // Both timestamps shift by 90 minutes
    expect(result.start_utc).toBe(event.start_utc! + 90 * 60);
    expect(result.end_utc).toBe(event.end_utc! + 90 * 60);
    // Duration preserved (30 hours)
    expect(result.end_utc - result.start_utc).toBe(30 * 3600);
  });

  it("moves a >24-hour event backwards without clamping", () => {
    const event = makeEvent({
      start_utc: Math.floor(new Date("2025-06-16T10:00:00").getTime() / 1000),
      end_utc: Math.floor(new Date("2025-06-17T16:00:00").getTime() / 1000),
    });

    const result = dayGestureTimes(event, -120);

    expect(result.start_utc).toBe(event.start_utc! - 120 * 60);
    expect(result.end_utc).toBe(event.end_utc! - 120 * 60);
    expect(result.end_utc - result.start_utc).toBe(30 * 3600);
  });

  it("throws when the event has no timestamps", () => {
    const event = makeEvent({ start_utc: undefined, end_utc: undefined });
    expect(() => dayGestureTimes(event, 30)).toThrow("Event has no timestamps");
  });

  it("returns unchanged times for zero delta", () => {
    const event = makeEvent({
      start_utc: Math.floor(new Date("2025-06-16T10:00:00").getTime() / 1000),
      end_utc: Math.floor(new Date("2025-06-16T11:00:00").getTime() / 1000),
    });

    const result = dayGestureTimes(event, 0);

    expect(result.start_utc).toBe(event.start_utc!);
    expect(result.end_utc).toBe(event.end_utc!);
  });
});

describe("dayResizeTimes (Slice 17)", () => {
  it("top resize preserves end and shifts start later", () => {
    const event = makeEvent({
      start_utc: Math.floor(new Date("2025-06-16T10:00:00").getTime() / 1000),
      end_utc: Math.floor(new Date("2025-06-16T11:00:00").getTime() / 1000),
    });

    const result = dayResizeTimes(event, "top", 30);

    expect(result.start_utc).toBe(event.start_utc! + 30 * 60);
    expect(result.end_utc).toBe(event.end_utc!);
  });

  it("top resize preserves end and shifts start earlier", () => {
    const event = makeEvent({
      start_utc: Math.floor(new Date("2025-06-16T10:00:00").getTime() / 1000),
      end_utc: Math.floor(new Date("2025-06-16T11:00:00").getTime() / 1000),
    });

    const result = dayResizeTimes(event, "top", -15);

    expect(result.start_utc).toBe(event.start_utc! - 15 * 60);
    expect(result.end_utc).toBe(event.end_utc!);
  });

  it("top resize clamps start to 15 minutes before end", () => {
    const event = makeEvent({
      start_utc: Math.floor(new Date("2025-06-16T10:00:00").getTime() / 1000),
      end_utc: Math.floor(new Date("2025-06-16T11:00:00").getTime() / 1000),
    });

    // Try to move start 70 minutes later (past end - 15 min)
    const result = dayResizeTimes(event, "top", 70);

    expect(result.start_utc).toBe(event.end_utc! - 15 * 60);
    expect(result.end_utc).toBe(event.end_utc!);
  });

  it("bottom resize preserves start and shifts end later", () => {
    const event = makeEvent({
      start_utc: Math.floor(new Date("2025-06-16T10:00:00").getTime() / 1000),
      end_utc: Math.floor(new Date("2025-06-16T11:00:00").getTime() / 1000),
    });

    const result = dayResizeTimes(event, "bottom", 30);

    expect(result.start_utc).toBe(event.start_utc!);
    expect(result.end_utc).toBe(event.end_utc! + 30 * 60);
  });

  it("bottom resize preserves start and shifts end earlier", () => {
    const event = makeEvent({
      start_utc: Math.floor(new Date("2025-06-16T10:00:00").getTime() / 1000),
      end_utc: Math.floor(new Date("2025-06-16T11:00:00").getTime() / 1000),
    });

    const result = dayResizeTimes(event, "bottom", -30);

    expect(result.start_utc).toBe(event.start_utc!);
    expect(result.end_utc).toBe(event.end_utc! - 30 * 60);
  });

  it("bottom resize clamps end to 15 minutes after start", () => {
    const event = makeEvent({
      start_utc: Math.floor(new Date("2025-06-16T10:00:00").getTime() / 1000),
      end_utc: Math.floor(new Date("2025-06-16T11:00:00").getTime() / 1000),
    });

    // Try to move end 70 minutes earlier (before start + 15 min)
    const result = dayResizeTimes(event, "bottom", -70);

    expect(result.start_utc).toBe(event.start_utc!);
    expect(result.end_utc).toBe(event.start_utc! + 15 * 60);
  });

  it("returns unchanged times for zero delta", () => {
    const event = makeEvent({
      start_utc: Math.floor(new Date("2025-06-16T10:00:00").getTime() / 1000),
      end_utc: Math.floor(new Date("2025-06-16T11:00:00").getTime() / 1000),
    });

    const topResult = dayResizeTimes(event, "top", 0);
    const bottomResult = dayResizeTimes(event, "bottom", 0);

    expect(topResult.start_utc).toBe(event.start_utc!);
    expect(topResult.end_utc).toBe(event.end_utc!);
    expect(bottomResult.start_utc).toBe(event.start_utc!);
    expect(bottomResult.end_utc).toBe(event.end_utc!);
  });

  it("throws when the event has no timestamps", () => {
    const event = makeEvent({ start_utc: undefined, end_utc: undefined });
    expect(() => dayResizeTimes(event, "top", 30)).toThrow("Event has no timestamps");
    expect(() => dayResizeTimes(event, "bottom", 30)).toThrow("Event has no timestamps");
  });
});
