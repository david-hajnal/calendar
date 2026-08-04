use chrono::{NaiveDate, TimeZone, Utc};
use commoncal_backend::ics::{
    IcsParseErrorCode, IcsParserLimits, NormalizedTiming, parse_calendar,
};

#[test]
fn parses_timed_all_day_recurring_and_escaped_events() {
    let calendar = parse_calendar(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Google Inc//Google Calendar 70.9054//EN\r\nBEGIN:VEVENT\r\nUID:timed@example.test\r\nDTSTART;TZID=Europe/Budapest:20260803T090000\r\nDTEND;TZID=Europe/Budapest:20260803T100000\r\nSUMMARY:Team\\, sync\r\nDESCRIPTION:Line one\\nLine two\\; still text\r\nLOCATION:Room\\, 1\r\nSTATUS:CONFIRMED\r\nSEQUENCE:2\r\nDTSTAMP:20260801T120000Z\r\nLAST-MODIFIED:20260802T120000Z\r\nRRULE:FREQ=WEEKLY;COUNT=3\r\nEXDATE;TZID=Europe/Budapest:20260810T090000\r\nEND:VEVENT\r\nBEGIN:VEVENT\r\nUID:all-day@example.test\r\nDTSTART;VALUE=DATE:20260804\r\nDURATION:P2D\r\nSUMMARY:All day\r\nEND:VEVENT\r\nBEGIN:VEVENT\r\nUID:timed@example.test\r\nRECURRENCE-ID;TZID=Europe/Budapest:20260817T090000\r\nDTSTART;TZID=Europe/Budapest:20260817T110000\r\nDTEND;TZID=Europe/Budapest:20260817T120000\r\nSUMMARY:Moved\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n",
        IcsParserLimits::default(),
    )
    .expect("valid ICS must parse");

    assert_eq!(calendar.events.len(), 3);
    assert_eq!(calendar.events[0].summary, "Team, sync");
    assert_eq!(
        calendar.events[0].description.as_deref(),
        Some("Line one\nLine two; still text")
    );
    assert_eq!(calendar.events[0].exdates.len(), 1);
    assert!(calendar.events[0].rrule.is_some());
    assert_eq!(
        calendar.events[0].timing,
        NormalizedTiming::Timed {
            starts_at: Utc.with_ymd_and_hms(2026, 8, 3, 7, 0, 0).unwrap(),
            ends_at: Utc.with_ymd_and_hms(2026, 8, 3, 8, 0, 0).unwrap(),
            timezone: Some("Europe/Budapest".to_owned()),
        }
    );
    assert_eq!(
        calendar.events[1].timing,
        NormalizedTiming::AllDay {
            start_date: NaiveDate::from_ymd_opt(2026, 8, 4).unwrap(),
            end_date: NaiveDate::from_ymd_opt(2026, 8, 6).unwrap(),
        }
    );
    assert!(calendar.events[2].recurrence_id.is_some());
}

#[test]
fn unfolds_lines_and_rejects_unsafe_or_structurally_invalid_input() {
    let folded = "BEGIN:VCALENDAR\nBEGIN:VEVENT\nUID:folded\nDTSTART:20260803T090000Z\nDTEND:20260803T100000Z\nSUMMARY:Safe <scr\n ipt>alert(1)</script>\nEND:VEVENT\nEND:VCALENDAR\n";
    let calendar = parse_calendar(folded, IcsParserLimits::default()).unwrap();
    assert_eq!(calendar.events[0].summary, "Safe <script>alert(1)</script>");

    let error = parse_calendar(
        "BEGIN:VCALENDAR\nBEGIN:VEVENT\nUID:broken\nDTSTART:20260803T090000Z\nDTEND:20260803T100000Z\nEND:VCALENDAR\n",
        IcsParserLimits::default(),
    )
    .unwrap_err();
    assert_eq!(error.code(), IcsParseErrorCode::Malformed);
}

#[test]
fn rejects_invalid_timing_limits_and_duplicate_event_keys() {
    let invalid = "BEGIN:VCALENDAR\nBEGIN:VEVENT\nUID:x\nDTSTART;VALUE=DATE:20260803\nDTEND:20260803T100000Z\nEND:VEVENT\nEND:VCALENDAR\n";
    assert_eq!(
        parse_calendar(invalid, IcsParserLimits::default())
            .unwrap_err()
            .code(),
        IcsParseErrorCode::InvalidEvent
    );

    let duplicate = "BEGIN:VCALENDAR\nBEGIN:VEVENT\nUID:x\nDTSTART:20260803T090000Z\nDTEND:20260803T100000Z\nEND:VEVENT\nBEGIN:VEVENT\nUID:x\nDTSTART:20260804T090000Z\nDTEND:20260804T100000Z\nEND:VEVENT\nEND:VCALENDAR\n";
    assert_eq!(
        parse_calendar(duplicate, IcsParserLimits::default())
            .unwrap_err()
            .code(),
        IcsParseErrorCode::DuplicateEvent
    );

    let oversized = "BEGIN:VCALENDAR\nBEGIN:VEVENT\nUID:x\nDTSTART:20260803T090000Z\nDTEND:20260803T100000Z\nSUMMARY:long\nEND:VEVENT\nEND:VCALENDAR\n";
    let limits = IcsParserLimits {
        max_text_bytes: 3,
        ..IcsParserLimits::default()
    };
    assert_eq!(
        parse_calendar(oversized, limits).unwrap_err().code(),
        IcsParseErrorCode::LimitExceeded
    );

    let component_limits = IcsParserLimits {
        max_component_bytes: 40,
        ..IcsParserLimits::default()
    };
    assert_eq!(
        parse_calendar(oversized, component_limits)
            .unwrap_err()
            .code(),
        IcsParseErrorCode::LimitExceeded
    );

    let recurrence_limits = IcsParserLimits {
        max_recurrence_values: 1,
        ..IcsParserLimits::default()
    };
    assert_eq!(
        parse_calendar(
            "BEGIN:VCALENDAR\nBEGIN:VEVENT\nUID:recurrence\nDTSTART:20260803T090000Z\nDTEND:20260803T100000Z\nRRULE:FREQ=DAILY;COUNT=2\nEND:VEVENT\nEND:VCALENDAR\n",
            recurrence_limits,
        )
        .unwrap_err()
        .code(),
        IcsParseErrorCode::LimitExceeded
    );
}
