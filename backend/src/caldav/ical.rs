use std::str::FromStr;

use chrono::{DateTime, TimeZone};
use chrono_tz::Tz;
use sha2::{Digest, Sha256};

use crate::ics::{NormalizedAlarm, NormalizedXProperty};

/// The timing of one canonical event as exposed through CalDAV.
///
/// `Timed` carries UTC instants plus the IANA zone used to render local wall
/// time. `AllDay` carries calendar dates (ISO `YYYY-MM-DD`) and is rendered as
/// `VALUE=DATE` properties, which are timezone-independent per RFC 5545.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CaldavIcalTiming {
    Timed {
        start_utc: i64,
        end_utc: i64,
        timezone: String,
    },
    AllDay {
        start_date: String,
        end_date: String,
    },
}

/// One canonical event as exposed through CalDAV.
///
/// For a recurring series the master event carries `rrule` and `exdates`;
/// each modified occurrence carries `recurrence_id`. All VEVENTs in a series
/// share the same `uid`.
pub struct CaldavIcalEvent {
    pub uid: String,
    pub summary: String,
    pub description: Option<String>,
    pub location: Option<String>,
    pub status: Option<String>,
    pub timing: CaldavIcalTiming,
    pub dtstamp: i64,
    pub sequence: u64,
    /// When true, serialize a free/busy placeholder: only the time window is
    /// exposed and no private fields (summary, description, location, status).
    pub free_busy: bool,
    /// RRULE value (without the "RRULE:" prefix), present on the series master.
    pub rrule: Option<String>,
    /// EXDATE values as UTC timestamps (timed) or ISO dates (all-day), for
    /// deleted occurrences on the series master.
    pub exdates: Vec<CaldavIcalDateValue>,
    /// RECURRENCE-ID value, present on a modified occurrence.
    pub recurrence_id: Option<CaldavIcalDateValue>,
    /// Allowlisted client-owned metadata (T16): categories, URL, transparency,
    /// alarms, and selected X-properties. Serialized deterministically so the
    /// ETag is a stable function of the content.
    pub categories: Vec<String>,
    pub url: Option<String>,
    pub transp: Option<String>,
    pub alarms: Vec<NormalizedAlarm>,
    pub x_properties: Vec<NormalizedXProperty>,
}

/// A date value for EXDATE / RECURRENCE-ID, matching the timing kind.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CaldavIcalDateValue {
    /// UTC timestamp for timed events.
    Timed(i64),
    /// ISO calendar date (`YYYY-MM-DD`) for all-day events.
    AllDay(String),
}

/// Serialize one canonical VCALENDAR resource for a single event (timed or
/// all-day, recurring or not).
///
/// The output is deterministic for a given input: property order is fixed,
/// text is escaped per RFC 5545, and long lines are folded.
pub fn serialize_event_resource(event: &CaldavIcalEvent) -> String {
    let mut out = String::new();
    out.push_str("BEGIN:VCALENDAR\r\n");
    out.push_str("VERSION:2.0\r\n");
    out.push_str("PRODID:-//Happening//CalDAV 1.0//EN\r\n");
    out.push_str("CALSCALE:GREGORIAN\r\n");
    out.push_str(&serialize_vevent(event));
    out.push_str("END:VCALENDAR\r\n");
    out
}

/// Serialize a recurring series as one VCALENDAR containing the master VEVENT
/// (with RRULE and EXDATE) followed by one VEVENT per modified occurrence
/// (with RECURRENCE-ID). All VEVENTs share the same UID.
pub fn serialize_recurring_series(
    master: &CaldavIcalEvent,
    exceptions: &[CaldavIcalEvent],
) -> String {
    let mut out = String::new();
    out.push_str("BEGIN:VCALENDAR\r\n");
    out.push_str("VERSION:2.0\r\n");
    out.push_str("PRODID:-//Happening//CalDAV 1.0//EN\r\n");
    out.push_str("CALSCALE:GREGORIAN\r\n");
    out.push_str(&serialize_vevent(master));
    for exception in exceptions {
        out.push_str(&serialize_vevent(exception));
    }
    out.push_str("END:VCALENDAR\r\n");
    out
}

fn serialize_vevent(event: &CaldavIcalEvent) -> String {
    let mut out = String::new();
    out.push_str("BEGIN:VEVENT\r\n");
    out.push_str(&fold_line(&format!("UID:{}", escape_text(&event.uid))));
    out.push_str(&fold_line(&format!(
        "DTSTAMP:{}Z",
        utc_timestamp(event.dtstamp)
    )));
    out.push_str(&timing_lines(&event.timing));
    if let Some(rrule) = &event.rrule {
        out.push_str(&fold_line(&format!("RRULE:{}", rrule)));
    }
    if !event.exdates.is_empty() {
        let values: Vec<String> = event
            .exdates
            .iter()
            .map(|value| format_date_value(value, &event.timing))
            .collect();
        out.push_str(&fold_line(&format!(
            "EXDATE{}:{}",
            date_value_parameters(&event.timing),
            values.join(",")
        )));
    }
    if let Some(recurrence_id) = &event.recurrence_id {
        out.push_str(&fold_line(&format!(
            "RECURRENCE-ID{}:{}",
            date_value_parameters(&event.timing),
            format_date_value(recurrence_id, &event.timing)
        )));
    }
    if !event.free_busy {
        out.push_str(&fold_line(&format!(
            "SUMMARY:{}",
            escape_text(&event.summary)
        )));
        if let Some(description) = &event.description {
            out.push_str(&fold_line(&format!(
                "DESCRIPTION:{}",
                escape_text(description)
            )));
        }
        if let Some(location) = &event.location {
            out.push_str(&fold_line(&format!("LOCATION:{}", escape_text(location))));
        }
        if let Some(status) = &event.status {
            out.push_str(&format!("STATUS:{status}\r\n"));
        }
        if !event.categories.is_empty() {
            let joined = event
                .categories
                .iter()
                .map(|c| escape_text(c))
                .collect::<Vec<_>>()
                .join(",");
            out.push_str(&fold_line(&format!("CATEGORIES:{joined}")));
        }
        if let Some(url) = &event.url {
            out.push_str(&fold_line(&format!("URL:{}", escape_text(url))));
        }
        if let Some(transp) = &event.transp {
            out.push_str(&format!("TRANSP:{transp}\r\n"));
        }
        for x in &event.x_properties {
            out.push_str(&fold_line(&format!("{}:{}", x.name, escape_text(&x.value))));
        }
        for alarm in &event.alarms {
            out.push_str("BEGIN:VALARM\r\n");
            out.push_str(&format!("ACTION:{}\r\n", alarm.action));
            out.push_str(&fold_line(&format!("TRIGGER:{}", alarm.trigger)));
            if let Some(description) = &alarm.description {
                out.push_str(&fold_line(&format!(
                    "DESCRIPTION:{}",
                    escape_text(description)
                )));
            }
            out.push_str("END:VALARM\r\n");
        }
    }
    out.push_str(&format!("SEQUENCE:{}\r\n", event.sequence));
    out.push_str("END:VEVENT\r\n");
    out
}

/// Format a date value for EXDATE / RECURRENCE-ID matching the event's timing
/// kind. Timed values render as UTC `YYYYMMDDTHHMMSSZ`; all-day values render
/// as `YYYYMMDD`. Property parameters are emitted separately, before the
/// colon, by `date_value_parameters`.
fn format_date_value(value: &CaldavIcalDateValue, timing: &CaldavIcalTiming) -> String {
    match (value, timing) {
        (CaldavIcalDateValue::Timed(utc), CaldavIcalTiming::Timed { timezone, .. }) => {
            let dt = DateTime::from_timestamp(*utc, 0).unwrap_or(DateTime::UNIX_EPOCH);
            if !timezone.trim().is_empty()
                && !timezone.eq_ignore_ascii_case("utc")
                && let Ok(tz) = Tz::from_str(timezone)
            {
                let local = tz.from_utc_datetime(&dt.naive_utc());
                local.format("%Y%m%dT%H%M%S").to_string()
            } else {
                format!("{}Z", dt.format("%Y%m%dT%H%M%S"))
            }
        }
        (CaldavIcalDateValue::AllDay(iso), CaldavIcalTiming::AllDay { .. }) => iso.replace('-', ""),
        // Mismatched kinds fall back to the value's own representation.
        (CaldavIcalDateValue::Timed(utc), _) => {
            let dt = DateTime::from_timestamp(*utc, 0).unwrap_or(DateTime::UNIX_EPOCH);
            format!("{}Z", dt.format("%Y%m%dT%H%M%S"))
        }
        (CaldavIcalDateValue::AllDay(iso), _) => iso.replace('-', ""),
    }
}

fn date_value_parameters(timing: &CaldavIcalTiming) -> String {
    match timing {
        CaldavIcalTiming::Timed { timezone, .. }
            if !timezone.trim().is_empty() && !timezone.eq_ignore_ascii_case("utc") =>
        {
            format!(";TZID={timezone}")
        }
        CaldavIcalTiming::AllDay { .. } => ";VALUE=DATE".to_owned(),
        _ => String::new(),
    }
}

/// Strong ETag derived from the canonical resource content.
///
/// The tag changes exactly when the serialized content changes and never
/// reuses a value for different content.
pub fn etag_for_ical(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    let digest = hasher.finalize();
    let hex: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
    format!("\"{hex}\"")
}

fn timing_lines(timing: &CaldavIcalTiming) -> String {
    match timing {
        CaldavIcalTiming::Timed {
            start_utc,
            end_utc,
            timezone,
        } => {
            let start = DateTime::from_timestamp(*start_utc, 0).unwrap_or(DateTime::UNIX_EPOCH);
            let end = DateTime::from_timestamp(*end_utc, 0).unwrap_or(DateTime::UNIX_EPOCH);
            if !timezone.trim().is_empty()
                && !timezone.eq_ignore_ascii_case("utc")
                && let Ok(tz) = Tz::from_str(timezone)
            {
                let start_local = tz.from_utc_datetime(&start.naive_utc());
                let end_local = tz.from_utc_datetime(&end.naive_utc());
                return format!(
                    "DTSTART;TZID={}:{}\r\nDTEND;TZID={}:{}\r\n",
                    timezone,
                    start_local.format("%Y%m%dT%H%M%S"),
                    timezone,
                    end_local.format("%Y%m%dT%H%M%S")
                );
            }
            format!(
                "DTSTART:{}Z\r\nDTEND:{}Z\r\n",
                start.format("%Y%m%dT%H%M%S"),
                end.format("%Y%m%dT%H%M%S")
            )
        }
        // All-day events are timezone-independent: render as DATE values with an
        // exclusive end, per RFC 5545. The stored ISO dates map 1:1 to ICS dates.
        CaldavIcalTiming::AllDay {
            start_date,
            end_date,
        } => format!(
            "DTSTART;VALUE=DATE:{}\r\nDTEND;VALUE=DATE:{}\r\n",
            iso_to_ics_date(start_date),
            iso_to_ics_date(end_date)
        ),
    }
}

/// Convert an ISO calendar date (`YYYY-MM-DD`) to the ICS DATE form (`YYYYMMDD`).
fn iso_to_ics_date(iso: &str) -> String {
    iso.replace('-', "")
}

fn utc_timestamp(seconds: i64) -> String {
    DateTime::from_timestamp(seconds, 0)
        .map(|dt| dt.format("%Y%m%dT%H%M%S").to_string())
        .unwrap_or_default()
}

fn escape_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            ';' => out.push_str("\\;"),
            ',' => out.push_str("\\,"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\n"),
            _ => out.push(ch),
        }
    }
    out
}

/// Fold a content line to at most 73 octets per RFC 5545 section 3.1.
fn fold_line(line: &str) -> String {
    const LIMIT: usize = 73;
    if line.len() <= LIMIT {
        return format!("{line}\r\n");
    }
    let chars: Vec<char> = line.chars().collect();
    let mut out = String::with_capacity(line.len() + 16);
    let mut pos = 0;
    let mut first = true;
    while pos < chars.len() {
        let width = if first { LIMIT } else { LIMIT - 1 };
        let mut end = pos;
        while end < chars.len() {
            let segment: String = chars[pos..=end].iter().collect();
            if segment.len() > width {
                break;
            }
            end += 1;
        }
        if end == pos {
            end = pos + 1;
        }
        let segment: String = chars[pos..end].iter().collect();
        if !first {
            out.push(' ');
        }
        out.push_str(&segment);
        out.push_str("\r\n");
        pos = end;
        first = false;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event() -> CaldavIcalEvent {
        CaldavIcalEvent {
            uid: "test-uid-1".to_owned(),
            summary: "Planning".to_owned(),
            description: Some("Quarterly planning".to_owned()),
            location: Some("Room 1".to_owned()),
            status: Some("CONFIRMED".to_owned()),
            timing: CaldavIcalTiming::Timed {
                start_utc: 1_768_435_200,
                end_utc: 1_768_438_800,
                timezone: "UTC".to_owned(),
            },
            dtstamp: 1_768_435_200,
            sequence: 1,
            free_busy: false,
            rrule: None,
            exdates: Vec::new(),
            recurrence_id: None,
            categories: Vec::new(),
            url: None,
            transp: None,
            alarms: Vec::new(),
            x_properties: Vec::new(),
        }
    }

    fn all_day_event() -> CaldavIcalEvent {
        let mut event = event();
        event.summary = "Conference".to_owned();
        event.timing = CaldavIcalTiming::AllDay {
            start_date: "2026-01-15".to_owned(),
            end_date: "2026-01-17".to_owned(),
        };
        event
    }

    #[test]
    fn serializes_valid_vcalendar_with_fixed_property_order() {
        let ics = serialize_event_resource(&event());
        assert!(ics.starts_with("BEGIN:VCALENDAR\r\n"));
        assert!(ics.ends_with("END:VCALENDAR\r\n"));
        assert!(ics.contains("VERSION:2.0\r\n"));
        assert!(ics.contains("PRODID:-//Happening//CalDAV 1.0//EN\r\n"));
        assert!(ics.contains("BEGIN:VEVENT\r\n"));
        assert!(ics.contains("UID:test-uid-1\r\n"));
        assert!(ics.contains("DTSTAMP:20260115T000000Z\r\n"));
        assert!(ics.contains("DTSTART:20260115T000000Z\r\n"));
        assert!(ics.contains("DTEND:20260115T010000Z\r\n"));
        assert!(ics.contains("SUMMARY:Planning\r\n"));
        assert!(ics.contains("DESCRIPTION:Quarterly planning\r\n"));
        assert!(ics.contains("LOCATION:Room 1\r\n"));
        assert!(ics.contains("STATUS:CONFIRMED\r\n"));
        assert!(ics.contains("SEQUENCE:1\r\n"));
        assert!(ics.contains("END:VEVENT\r\n"));
    }

    #[test]
    fn free_busy_placeholder_omits_private_fields() {
        let mut event = event();
        event.free_busy = true;
        let ics = serialize_event_resource(&event);
        // The time window and identity are preserved.
        assert!(ics.contains("UID:test-uid-1\r\n"));
        assert!(ics.contains("DTSTART:20260115T000000Z\r\n"));
        assert!(ics.contains("DTEND:20260115T010000Z\r\n"));
        // No private fields leak.
        assert!(
            !ics.contains("SUMMARY:"),
            "free-busy must not expose SUMMARY"
        );
        assert!(
            !ics.contains("DESCRIPTION:"),
            "free-busy must not expose DESCRIPTION"
        );
        assert!(
            !ics.contains("LOCATION:"),
            "free-busy must not expose LOCATION"
        );
        assert!(!ics.contains("STATUS:"), "free-busy must not expose STATUS");
    }

    #[test]
    fn free_busy_etag_differs_from_full_representation() {
        let full = event();
        let mut busy = event();
        busy.free_busy = true;
        assert_ne!(
            etag_for_ical(&serialize_event_resource(&full)),
            etag_for_ical(&serialize_event_resource(&busy)),
            "free-busy and full representations must have distinct ETags"
        );
    }

    #[test]
    fn serialization_is_deterministic() {
        assert_eq!(
            serialize_event_resource(&event()),
            serialize_event_resource(&event())
        );
    }

    #[test]
    fn etag_changes_only_with_content() {
        let first = etag_for_ical(&serialize_event_resource(&event()));
        let same = etag_for_ical(&serialize_event_resource(&event()));
        let mut changed = event();
        changed.summary = "Renamed".to_owned();
        let rotated = etag_for_ical(&serialize_event_resource(&changed));
        assert_eq!(first, same);
        assert_ne!(first, rotated);
        assert!(first.starts_with('"'));
        assert!(first.ends_with('"'));
    }

    #[test]
    fn timed_event_uses_tzid_local_wall_time() {
        let mut event = event();
        event.timing = CaldavIcalTiming::Timed {
            start_utc: 1_768_435_200,
            end_utc: 1_768_438_800,
            timezone: "America/New_York".to_owned(),
        };
        let ics = serialize_event_resource(&event);
        // 2026-01-15T00:00:00Z is 2026-01-14T19:00:00 in New York.
        assert!(ics.contains("DTSTART;TZID=America/New_York:20260114T190000\r\n"));
        assert!(ics.contains("DTEND;TZID=America/New_York:20260114T200000\r\n"));
    }

    #[test]
    fn recurrence_date_parameters_precede_the_colon() {
        let mut timed = event();
        timed.timing = CaldavIcalTiming::Timed {
            start_utc: 1_768_435_200,
            end_utc: 1_768_438_800,
            timezone: "America/New_York".to_owned(),
        };
        timed.exdates = vec![CaldavIcalDateValue::Timed(1_768_521_600)];
        timed.recurrence_id = Some(CaldavIcalDateValue::Timed(1_768_608_000));
        let timed_ics = serialize_event_resource(&timed);
        assert!(timed_ics.contains("EXDATE;TZID=America/New_York:"));
        assert!(timed_ics.contains("RECURRENCE-ID;TZID=America/New_York:"));
        assert!(!timed_ics.contains(";TZID=America/New_York\r\n"));

        let mut all_day = all_day_event();
        all_day.exdates = vec![CaldavIcalDateValue::AllDay("2026-01-16".to_owned())];
        all_day.recurrence_id = Some(CaldavIcalDateValue::AllDay("2026-01-17".to_owned()));
        let all_day_ics = serialize_event_resource(&all_day);
        assert!(all_day_ics.contains("EXDATE;VALUE=DATE:20260116\r\n"));
        assert!(all_day_ics.contains("RECURRENCE-ID;VALUE=DATE:20260117\r\n"));
        assert!(!all_day_ics.contains("20260116;VALUE=DATE"));
    }

    #[test]
    fn all_day_event_uses_date_values_with_exclusive_end() {
        let ics = serialize_event_resource(&all_day_event());
        // All-day events render as DATE values, timezone-independent.
        assert!(ics.contains("DTSTART;VALUE=DATE:20260115\r\n"));
        assert!(ics.contains("DTEND;VALUE=DATE:20260117\r\n"));
        // No timezone or DATE-TIME form must leak into the DTSTART/DTEND lines.
        assert!(!ics.contains("TZID="));
        let dt_lines: Vec<&str> = ics
            .lines()
            .filter(|line| line.starts_with("DTSTART") || line.starts_with("DTEND"))
            .collect();
        assert_eq!(dt_lines.len(), 2);
        for line in dt_lines {
            let line = line.trim_end_matches('\r');
            assert!(
                line.contains("VALUE=DATE"),
                "all-day DT line must be a DATE: {line}"
            );
            // The value (after the colon) must be a bare 8-digit date, no time.
            let value = line.split_once(':').unwrap().1;
            assert_eq!(value.len(), 8, "all-day DT value must be YYYYMMDD: {line}");
            assert!(
                value.bytes().all(|b| b.is_ascii_digit()),
                "all-day DT value must be digits only: {line}"
            );
        }
    }

    #[test]
    fn all_day_event_round_trips_through_parser() {
        let ics = serialize_event_resource(&all_day_event());
        let parsed = crate::ics::parse_calendar(&ics, crate::ics::IcsParserLimits::default())
            .expect("all-day event must parse");
        assert_eq!(parsed.events.len(), 1);
        assert_eq!(
            parsed.events[0].timing,
            crate::ics::NormalizedTiming::AllDay {
                start_date: chrono::NaiveDate::from_ymd_opt(2026, 1, 15).unwrap(),
                end_date: chrono::NaiveDate::from_ymd_opt(2026, 1, 17).unwrap(),
            }
        );
    }

    #[test]
    fn all_day_single_day_event_round_trips() {
        let mut event = all_day_event();
        event.timing = CaldavIcalTiming::AllDay {
            start_date: "2026-01-15".to_owned(),
            end_date: "2026-01-16".to_owned(),
        };
        let ics = serialize_event_resource(&event);
        assert!(ics.contains("DTSTART;VALUE=DATE:20260115\r\n"));
        assert!(ics.contains("DTEND;VALUE=DATE:20260116\r\n"));
        let parsed = crate::ics::parse_calendar(&ics, crate::ics::IcsParserLimits::default())
            .expect("single-day all-day event must parse");
        assert_eq!(
            parsed.events[0].timing,
            crate::ics::NormalizedTiming::AllDay {
                start_date: chrono::NaiveDate::from_ymd_opt(2026, 1, 15).unwrap(),
                end_date: chrono::NaiveDate::from_ymd_opt(2026, 1, 16).unwrap(),
            }
        );
    }

    #[test]
    fn timed_event_round_trips_through_parser() {
        let ics = serialize_event_resource(&event());
        let parsed = crate::ics::parse_calendar(&ics, crate::ics::IcsParserLimits::default())
            .expect("timed event must parse");
        assert_eq!(parsed.events.len(), 1);
    }

    #[test]
    fn text_is_escaped_per_rfc5545() {
        let mut event = event();
        event.summary = "a\\b;c,d\ne".to_owned();
        let ics = serialize_event_resource(&event);
        assert!(ics.contains("SUMMARY:a\\\\b\\;c\\,d\\ne\r\n"));
    }

    #[test]
    fn long_lines_are_folded_within_73_octets() {
        let mut event = event();
        event.summary = "x".repeat(200);
        let ics = serialize_event_resource(&event);
        for line in ics.split("\r\n") {
            assert!(line.len() <= 73, "line too long: {line:?}");
        }
        // Unfolding must recover the original value.
        let unfolded: String =
            ics.lines()
                .collect::<Vec<_>>()
                .iter()
                .fold(String::new(), |mut acc, line| {
                    if line.starts_with(' ') {
                        acc.push_str(line.strip_prefix(' ').unwrap());
                    } else {
                        if !acc.is_empty() {
                            acc.push('\n');
                        }
                        acc.push_str(line);
                    }
                    acc
                });
        assert!(unfolded.contains(&format!("SUMMARY:{}", "x".repeat(200))));
    }
}
