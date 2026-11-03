import { describe, expect, it } from "vitest";

import {
  buildRecurrenceRule,
  describeRecurrenceRule,
  parseRecurrenceRule,
  type RepeatSettings,
} from "./recurrenceRule";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function settings(
  frequency: RepeatSettings["frequency"],
  interval = 1,
  end: RepeatSettings["end"] = { kind: "never" },
): RepeatSettings {
  return { frequency, interval, end };
}

// ---------------------------------------------------------------------------
// buildRecurrenceRule
// ---------------------------------------------------------------------------

describe("buildRecurrenceRule", () => {
  it("returns null when frequency is none", () => {
    expect(buildRecurrenceRule(settings("none"))).toBeNull();
  });

  it("builds a daily rule with no end", () => {
    expect(buildRecurrenceRule(settings("daily"))).toBe("FREQ=DAILY");
  });

  it("builds a weekly rule with no end", () => {
    expect(buildRecurrenceRule(settings("weekly"))).toBe("FREQ=WEEKLY");
  });

  it("builds a monthly rule with no end", () => {
    expect(buildRecurrenceRule(settings("monthly"))).toBe("FREQ=MONTHLY");
  });

  it("builds a yearly rule with no end", () => {
    expect(buildRecurrenceRule(settings("yearly"))).toBe("FREQ=YEARLY");
  });

  it("includes INTERVAL when greater than 1", () => {
    expect(buildRecurrenceRule(settings("daily", 2))).toBe("FREQ=DAILY;INTERVAL=2");
    expect(buildRecurrenceRule(settings("weekly", 3))).toBe("FREQ=WEEKLY;INTERVAL=3");
    expect(buildRecurrenceRule(settings("monthly", 2))).toBe("FREQ=MONTHLY;INTERVAL=2");
    expect(buildRecurrenceRule(settings("yearly", 5))).toBe("FREQ=YEARLY;INTERVAL=5");
  });

  it("omits INTERVAL when it is 1", () => {
    expect(buildRecurrenceRule(settings("daily", 1))).toBe("FREQ=DAILY");
    expect(buildRecurrenceRule(settings("weekly", 1))).toBe("FREQ=WEEKLY");
  });

  it("includes COUNT when end is after N occurrences", () => {
    expect(buildRecurrenceRule(settings("daily", 1, { kind: "after", count: 5 }))).toBe(
      "FREQ=DAILY;COUNT=5",
    );
    expect(buildRecurrenceRule(settings("weekly", 2, { kind: "after", count: 10 }))).toBe(
      "FREQ=WEEKLY;INTERVAL=2;COUNT=10",
    );
    expect(buildRecurrenceRule(settings("monthly", 1, { kind: "after", count: 12 }))).toBe(
      "FREQ=MONTHLY;COUNT=12",
    );
    expect(buildRecurrenceRule(settings("yearly", 1, { kind: "after", count: 3 }))).toBe(
      "FREQ=YEARLY;COUNT=3",
    );
  });

  it("includes UNTIL when end is on a date", () => {
    expect(buildRecurrenceRule(settings("daily", 1, { kind: "on", date: "2026-12-25" }))).toBe(
      "FREQ=DAILY;UNTIL=20261225T235959Z",
    );
    expect(buildRecurrenceRule(settings("weekly", 2, { kind: "on", date: "2027-01-01" }))).toBe(
      "FREQ=WEEKLY;INTERVAL=2;UNTIL=20270101T235959Z",
    );
    expect(buildRecurrenceRule(settings("monthly", 1, { kind: "on", date: "2026-06-15" }))).toBe(
      "FREQ=MONTHLY;UNTIL=20260615T235959Z",
    );
    expect(buildRecurrenceRule(settings("yearly", 1, { kind: "on", date: "2030-01-01" }))).toBe(
      "FREQ=YEARLY;UNTIL=20300101T235959Z",
    );
  });

  it("never emits BYDAY or unsupported parts", () => {
    const rule = buildRecurrenceRule(settings("weekly", 2, { kind: "after", count: 5 }));
    expect(rule).not.toContain("BYDAY");
    expect(rule).not.toContain("BYMONTHDAY");
    expect(rule).not.toContain("BYMONTH");
    expect(rule).not.toContain("WKST");
    expect(rule).not.toContain("RRULE:");
  });
});

// ---------------------------------------------------------------------------
// parseRecurrenceRule
// ---------------------------------------------------------------------------

describe("parseRecurrenceRule", () => {
  it("returns null for an empty string", () => {
    expect(parseRecurrenceRule("")).toBeNull();
  });

  it("parses a simple daily rule", () => {
    expect(parseRecurrenceRule("FREQ=DAILY")).toEqual(settings("daily"));
  });

  it("parses a simple weekly rule", () => {
    expect(parseRecurrenceRule("FREQ=WEEKLY")).toEqual(settings("weekly"));
  });

  it("parses a simple monthly rule", () => {
    expect(parseRecurrenceRule("FREQ=MONTHLY")).toEqual(settings("monthly"));
  });

  it("parses a simple yearly rule", () => {
    expect(parseRecurrenceRule("FREQ=YEARLY")).toEqual(settings("yearly"));
  });

  it("parses INTERVAL", () => {
    expect(parseRecurrenceRule("FREQ=DAILY;INTERVAL=2")).toEqual(settings("daily", 2));
    expect(parseRecurrenceRule("FREQ=WEEKLY;INTERVAL=3")).toEqual(settings("weekly", 3));
  });

  it("parses COUNT", () => {
    expect(parseRecurrenceRule("FREQ=DAILY;COUNT=5")).toEqual(
      settings("daily", 1, { kind: "after", count: 5 }),
    );
    expect(parseRecurrenceRule("FREQ=WEEKLY;COUNT=10")).toEqual(
      settings("weekly", 1, { kind: "after", count: 10 }),
    );
  });

  it("parses UNTIL", () => {
    expect(parseRecurrenceRule("FREQ=DAILY;UNTIL=20261225T235959Z")).toEqual(
      settings("daily", 1, { kind: "on", date: "2026-12-25" }),
    );
    expect(parseRecurrenceRule("FREQ=WEEKLY;UNTIL=20270101T235959Z")).toEqual(
      settings("weekly", 1, { kind: "on", date: "2027-01-01" }),
    );
  });

  it("parses a rule with the RRULE: prefix", () => {
    expect(parseRecurrenceRule("RRULE:FREQ=WEEKLY;INTERVAL=2")).toEqual(settings("weekly", 2));
  });

  it("returns null for an unsupported frequency", () => {
    expect(parseRecurrenceRule("FREQ=HOURLY")).toBeNull();
    expect(parseRecurrenceRule("FREQ=MINUTELY")).toBeNull();
  });

  it("returns null for an unsupported part", () => {
    expect(parseRecurrenceRule("FREQ=WEEKLY;BYDAY=MO")).toBeNull();
    expect(parseRecurrenceRule("FREQ=DAILY;BYMONTHDAY=15")).toBeNull();
    expect(parseRecurrenceRule("FREQ=MONTHLY;WKST=MO")).toBeNull();
  });

  it("returns null when both COUNT and UNTIL are present", () => {
    expect(parseRecurrenceRule("FREQ=DAILY;COUNT=5;UNTIL=20261225T235959Z")).toBeNull();
  });

  it("returns null when FREQ is missing", () => {
    expect(parseRecurrenceRule("INTERVAL=2")).toBeNull();
    expect(parseRecurrenceRule("COUNT=5")).toBeNull();
  });

  it("returns null for invalid INTERVAL", () => {
    expect(parseRecurrenceRule("FREQ=DAILY;INTERVAL=0")).toBeNull();
    expect(parseRecurrenceRule("FREQ=DAILY;INTERVAL=-1")).toBeNull();
    expect(parseRecurrenceRule("FREQ=DAILY;INTERVAL=abc")).toBeNull();
  });

  it("returns null for invalid COUNT", () => {
    expect(parseRecurrenceRule("FREQ=DAILY;COUNT=0")).toBeNull();
    expect(parseRecurrenceRule("FREQ=DAILY;COUNT=-1")).toBeNull();
    expect(parseRecurrenceRule("FREQ=DAILY;COUNT=abc")).toBeNull();
  });

  it("returns null for invalid UNTIL format", () => {
    expect(parseRecurrenceRule("FREQ=DAILY;UNTIL=2026-12-25")).toBeNull();
    expect(parseRecurrenceRule("FREQ=DAILY;UNTIL=not-a-date")).toBeNull();
    expect(parseRecurrenceRule("FREQ=DAILY;UNTIL=20261225")).toBeNull();
  });
});

// ---------------------------------------------------------------------------
// Round-trip: build → parse → compare
// ---------------------------------------------------------------------------

describe("round-trip (build → parse)", () => {
  const frequencies: Array<Exclude<RepeatSettings["frequency"], "none">> = [
    "daily",
    "weekly",
    "monthly",
    "yearly",
  ];

  for (const frequency of frequencies) {
    it(`round-trips ${frequency} with no end`, () => {
      const original = settings(frequency);
      const rule = buildRecurrenceRule(original)!;
      expect(parseRecurrenceRule(rule)).toEqual(original);
    });

    it(`round-trips ${frequency} with interval 2 and no end`, () => {
      const original = settings(frequency, 2);
      const rule = buildRecurrenceRule(original)!;
      expect(parseRecurrenceRule(rule)).toEqual(original);
    });

    it(`round-trips ${frequency} with count end`, () => {
      const original = settings(frequency, 1, { kind: "after", count: 5 });
      const rule = buildRecurrenceRule(original)!;
      expect(parseRecurrenceRule(rule)).toEqual(original);
    });

    it(`round-trips ${frequency} with interval 3 and count end`, () => {
      const original = settings(frequency, 3, { kind: "after", count: 10 });
      const rule = buildRecurrenceRule(original)!;
      expect(parseRecurrenceRule(rule)).toEqual(original);
    });

    it(`round-trips ${frequency} with until-date end`, () => {
      const original = settings(frequency, 1, { kind: "on", date: "2026-12-25" });
      const rule = buildRecurrenceRule(original)!;
      expect(parseRecurrenceRule(rule)).toEqual(original);
    });

    it(`round-trips ${frequency} with interval 2 and until-date end`, () => {
      const original = settings(frequency, 2, { kind: "on", date: "2027-06-15" });
      const rule = buildRecurrenceRule(original)!;
      expect(parseRecurrenceRule(rule)).toEqual(original);
    });
  }

  it("round-trips with the RRULE: prefix", () => {
    const original = settings("weekly", 2, { kind: "after", count: 5 });
    const rule = buildRecurrenceRule(original)!;
    expect(parseRecurrenceRule(`RRULE:${rule}`)).toEqual(original);
  });
});

// ---------------------------------------------------------------------------
// describeRecurrenceRule
// ---------------------------------------------------------------------------

describe("describeRecurrenceRule", () => {
  it("returns null for an empty string", () => {
    expect(describeRecurrenceRule("")).toBeNull();
  });

  it("returns null for an unparseable rule", () => {
    expect(describeRecurrenceRule("FREQ=HOURLY")).toBeNull();
    expect(describeRecurrenceRule("FREQ=WEEKLY;BYDAY=MO")).toBeNull();
  });

  it("describes a simple weekly rule", () => {
    expect(describeRecurrenceRule("FREQ=WEEKLY")).toBe("Weekly");
  });

  it("describes a simple daily rule", () => {
    expect(describeRecurrenceRule("FREQ=DAILY")).toBe("Daily");
  });

  it("describes a simple monthly rule", () => {
    expect(describeRecurrenceRule("FREQ=MONTHLY")).toBe("Monthly");
  });

  it("describes a simple yearly rule", () => {
    expect(describeRecurrenceRule("FREQ=YEARLY")).toBe("Yearly");
  });

  it("describes a rule with interval", () => {
    expect(describeRecurrenceRule("FREQ=WEEKLY;INTERVAL=2")).toBe("Every 2 weeks");
    expect(describeRecurrenceRule("FREQ=DAILY;INTERVAL=3")).toBe("Every 3 days");
    expect(describeRecurrenceRule("FREQ=MONTHLY;INTERVAL=2")).toBe("Every 2 months");
    expect(describeRecurrenceRule("FREQ=YEARLY;INTERVAL=2")).toBe("Every 2 years");
  });

  it("describes a rule with count end", () => {
    expect(describeRecurrenceRule("FREQ=MONTHLY;COUNT=5")).toBe("Monthly, ends after 5 events");
    expect(describeRecurrenceRule("FREQ=WEEKLY;COUNT=10")).toBe("Weekly, ends after 10 events");
    expect(describeRecurrenceRule("FREQ=DAILY;COUNT=3")).toBe("Daily, ends after 3 events");
    expect(describeRecurrenceRule("FREQ=YEARLY;COUNT=2")).toBe("Yearly, ends after 2 events");
  });

  it("describes a rule with interval and count end", () => {
    expect(describeRecurrenceRule("FREQ=WEEKLY;INTERVAL=2;COUNT=5")).toBe(
      "Every 2 weeks, ends after 5 events",
    );
  });

  it("describes a rule with until-date end", () => {
    const label = describeRecurrenceRule("FREQ=DAILY;UNTIL=20261225T235959Z");
    expect(label).not.toBeNull();
    expect(label).toMatch(/^Daily, ends on /);
    expect(label).toContain("2026");
  });

  it("describes a rule with interval and until-date end", () => {
    const label = describeRecurrenceRule("FREQ=WEEKLY;INTERVAL=2;UNTIL=20270101T235959Z");
    expect(label).not.toBeNull();
    expect(label).toMatch(/^Every 2 weeks, ends on /);
    expect(label).toContain("2027");
  });
});
