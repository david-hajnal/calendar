/**
 * Pure builder / parser / describer for RRULE strings that the backend
 * recurrence parser (backend/src/recurrence.rs) accepts.
 *
 * The builder only emits FREQ, INTERVAL, COUNT, or UNTIL — never BYDAY or
 * any other part the backend rejects.
 */

export type RepeatFrequency = "none" | "daily" | "weekly" | "monthly" | "yearly";

export type RepeatEnd =
  | { kind: "never" }
  | { kind: "after"; count: number }
  | { kind: "on"; date: string };

export interface RepeatSettings {
  frequency: RepeatFrequency;
  interval: number;
  end: RepeatEnd;
}

const FREQ_TO_RRULE: Record<Exclude<RepeatFrequency, "none">, string> = {
  daily: "DAILY",
  weekly: "WEEKLY",
  monthly: "MONTHLY",
  yearly: "YEARLY",
};

const RRULE_TO_FREQ: Record<string, Exclude<RepeatFrequency, "none">> = {
  DAILY: "daily",
  WEEKLY: "weekly",
  MONTHLY: "monthly",
  YEARLY: "yearly",
};

const FREQ_LABELS: Record<Exclude<RepeatFrequency, "none">, string> = {
  daily: "Daily",
  weekly: "Weekly",
  monthly: "Monthly",
  yearly: "Yearly",
};

const FREQ_PLURALS: Record<Exclude<RepeatFrequency, "none">, string> = {
  daily: "days",
  weekly: "weeks",
  monthly: "months",
  yearly: "years",
};

/**
 * Convert an ISO date string (YYYY-MM-DD) to the RFC 5545 UTC date-time
 * format the backend expects: YYYYMMDDTHHMMSSZ.
 * Uses end-of-day (23:59:59) so the entire date is inclusive.
 */
function isoDateToRfc5545(isoDate: string): string {
  const [year, month, day] = isoDate.split("-");
  return `${year}${month}${day}T235959Z`;
}

/**
 * Convert an RFC 5545 UTC date-time (YYYYMMDDTHHMMSSZ) back to an ISO
 * date string (YYYY-MM-DD).
 */
function rfc5545ToIsoDate(rfc: string): string {
  return `${rfc.slice(0, 4)}-${rfc.slice(4, 6)}-${rfc.slice(6, 8)}`;
}

/**
 * Build an RRULE string from RepeatSettings.
 * Returns null when frequency is "none".
 *
 * Only emits FREQ, INTERVAL (when > 1), COUNT, or UNTIL.
 * Never emits BYDAY or any part the backend parser rejects.
 */
export function buildRecurrenceRule(settings: RepeatSettings): string | null {
  if (settings.frequency === "none") return null;

  const parts: string[] = [`FREQ=${FREQ_TO_RRULE[settings.frequency]}`];

  if (settings.interval > 1) {
    parts.push(`INTERVAL=${settings.interval}`);
  }

  if (settings.end.kind === "after") {
    parts.push(`COUNT=${settings.end.count}`);
  } else if (settings.end.kind === "on") {
    parts.push(`UNTIL=${isoDateToRfc5545(settings.end.date)}`);
  }

  return parts.join(";");
}

/**
 * Parse an RRULE string back into RepeatSettings.
 * Returns null when the rule is empty, missing FREQ, or contains an
 * unsupported part.
 */
export function parseRecurrenceRule(rule: string): RepeatSettings | null {
  const value = rule.startsWith("RRULE:") ? rule.slice(6) : rule;
  if (!value) return null;

  let frequency: Exclude<RepeatFrequency, "none"> | null = null;
  let interval = 1;
  let count: number | null = null;
  let until: string | null = null;

  for (const part of value.split(";")) {
    const eqIndex = part.indexOf("=");
    if (eqIndex === -1) return null;

    const name = part.slice(0, eqIndex);
    const valuePart = part.slice(eqIndex + 1);

    switch (name) {
      case "FREQ": {
        const freq = RRULE_TO_FREQ[valuePart];
        if (!freq) return null;
        frequency = freq;
        break;
      }
      case "INTERVAL": {
        const parsed = Number(valuePart);
        if (!Number.isInteger(parsed) || parsed < 1) return null;
        interval = parsed;
        break;
      }
      case "COUNT": {
        const parsed = Number(valuePart);
        if (!Number.isInteger(parsed) || parsed < 1) return null;
        count = parsed;
        break;
      }
      case "UNTIL": {
        // Must match YYYYMMDDTHHMMSSZ
        if (!/^\d{8}T\d{6}Z$/.test(valuePart)) return null;
        until = valuePart;
        break;
      }
      default:
        return null;
    }
  }

  if (!frequency) return null;
  if (count !== null && until !== null) return null;

  let end: RepeatEnd;
  if (count !== null) {
    end = { kind: "after", count };
  } else if (until !== null) {
    end = { kind: "on", date: rfc5545ToIsoDate(until) };
  } else {
    end = { kind: "never" };
  }

  return { frequency, interval, end };
}

/**
 * Return a human-readable label for an RRULE string, or null when the
 * rule is empty or unparseable.
 *
 * Examples:
 *   "FREQ=WEEKLY"                          → "Weekly"
 *   "FREQ=WEEKLY;INTERVAL=2"              → "Every 2 weeks"
 *   "FREQ=MONTHLY;COUNT=5"                → "Monthly, ends after 5 events"
 *   "FREQ=DAILY;UNTIL=20261225T235959Z"   → "Daily, ends on Dec 25, 2026"
 */
export function describeRecurrenceRule(rule: string): string | null {
  const settings = parseRecurrenceRule(rule);
  if (!settings || settings.frequency === "none") return null;

  const base = FREQ_LABELS[settings.frequency];
  const frequencyLabel =
    settings.interval > 1
      ? `Every ${settings.interval} ${FREQ_PLURALS[settings.frequency]}`
      : base;

  if (settings.end.kind === "after") {
    return `${frequencyLabel}, ends after ${settings.end.count} events`;
  }

  if (settings.end.kind === "on") {
    const [year, month, day] = settings.end.date.split("-").map(Number);
    const dateLabel = new Date(year, month - 1, day).toLocaleDateString(undefined, {
      month: "short",
      day: "numeric",
      year: "numeric",
    });
    return `${frequencyLabel}, ends on ${dateLabel}`;
  }

  return frequencyLabel;
}
