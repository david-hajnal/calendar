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

#[test]
fn parses_event_with_valarm_subcomponents() {
    let calendar = parse_calendar(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Google Inc//Google Calendar 70.9054//EN\r\nMETHOD:REQUEST\r\nBEGIN:VTIMEZONE\r\nTZID:Europe/Budapest\r\nX-LIC-LOCATION:Europe/Budapest\r\nBEGIN:DAYLIGHT\r\nTZOFFSETFROM:+0100\r\nTZOFFSETTO:+0200\r\nTZNAME:GMT+2\r\nDTSTART:19700329T020000\r\nRRULE:FREQ=YEARLY;BYMONTH=3;BYDAY=-1SU\r\nEND:DAYLIGHT\r\nBEGIN:STANDARD\r\nTZOFFSETFROM:+0200\r\nTZOFFSETTO:+0100\r\nTZNAME:GMT+1\r\nDTSTART:19701025T030000\r\nRRULE:FREQ=YEARLY;BYMONTH=10;BYDAY=-1SU\r\nEND:STANDARD\r\nEND:VTIMEZONE\r\nBEGIN:VEVENT\r\nDTSTART;TZID=Europe/Budapest:20261215T170000\r\nDTEND;TZID=Europe/Budapest:20261215T190000\r\nDTSTAMP:20260924T143219Z\r\nORGANIZER:mailto:plajerdora@gmail.com\r\nUID:ue0eli00o39blktt2id8rfkir0@google.com\r\nATTENDEE;CUTYPE=INDIVIDUAL;ROLE=REQ-PARTICIPANT;PARTSTAT=ACCEPTED;RSVP=TRUE\r\n ;X-NUM-GUESTS=0:mailto:plajerdora@gmail.com\r\nATTENDEE;CUTYPE=INDIVIDUAL;ROLE=REQ-PARTICIPANT;PARTSTAT=NEEDS-ACTION;RSVP=\r\n TRUE;CN=Bilimbo Szilva 2026/2027;X-NUM-GUESTS=0:mailto:bilimbo-szilva-2026@\r\n googlegroups.com\r\nX-MICROSOFT-CDO-OWNERAPPTID:-23788780\r\nCREATED:20260924T143207Z\r\nDESCRIPTION:Részletek később frissítésre kerülnek\r\nLAST-MODIFIED:20260924T143208Z\r\nLOCATION:\r\nSEQUENCE:0\r\nSTATUS:CONFIRMED\r\nSUMMARY:Bilimbo: Karácsonyi buli (szilva csoport)\r\nTRANSP:OPAQUE\r\nBEGIN:VALARM\r\nACTION:EMAIL\r\nDESCRIPTION:This is an event reminder\r\nSUMMARY:Alarm notification\r\nATTENDEE:mailto:david@hajnal.space\r\nTRIGGER:-P0DT0H10M0S\r\nEND:VALARM\r\nBEGIN:VALARM\r\nACTION:DISPLAY\r\nDESCRIPTION:This is an event reminder\r\nTRIGGER:-P0DT0H30M0S\r\nEND:VALARM\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n",
        IcsParserLimits::default(),
    )
    .expect("ICS with VALARM subcomponents must parse");

    assert_eq!(calendar.events.len(), 1);
    assert_eq!(
        calendar.events[0].uid,
        "ue0eli00o39blktt2id8rfkir0@google.com"
    );
    assert!(matches!(
        calendar.events[0].timing,
        NormalizedTiming::Timed { .. }
    ));
}

#[test]
fn normalizes_allowlisted_client_properties() {
    let calendar = parse_calendar(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:props@example.test\r\nDTSTART:20260803T090000Z\r\nDTEND:20260803T100000Z\r\nSUMMARY:Props\r\nCATEGORIES:Work,Personal\r\nURL:https://example.test/event\r\nTRANSP:OPAQUE\r\nX-APPLE-CEVENT-CATEGORY:TYPE:WORK\r\nX-APPLE-FALLBACK-ALARM-UID:alarm-1\r\nX-UNALLOWED-PROP:should-be-dropped\r\nBEGIN:VALARM\r\nACTION:DISPLAY\r\nDESCRIPTION:Reminder\r\nTRIGGER:-PT10M\r\nEND:VALARM\r\nBEGIN:VALARM\r\nACTION:EMAIL\r\nDESCRIPTION:Email\r\nTRIGGER:-PT5M\r\nEND:VALARM\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n",
        IcsParserLimits::default(),
    )
    .expect("event with allowlisted properties must parse");

    let event = &calendar.events[0];
    assert_eq!(
        event.categories,
        vec!["Work".to_owned(), "Personal".to_owned()]
    );
    assert_eq!(event.url.as_deref(), Some("https://example.test/event"));
    assert_eq!(event.transp.as_deref(), Some("OPAQUE"));

    // Only the allowlisted X-properties survive; X-UNALLOWED-PROP is dropped.
    assert_eq!(event.x_properties.len(), 2);
    assert!(
        event
            .x_properties
            .iter()
            .any(|x| x.name == "X-APPLE-CEVENT-CATEGORY" && x.value == "TYPE:WORK")
    );
    assert!(
        event
            .x_properties
            .iter()
            .any(|x| x.name == "X-APPLE-FALLBACK-ALARM-UID" && x.value == "alarm-1")
    );
    assert!(
        !event
            .x_properties
            .iter()
            .any(|x| x.name == "X-UNALLOWED-PROP")
    );

    // Only DISPLAY/AUDIO alarms with a TRIGGER are preserved; EMAIL is dropped.
    assert_eq!(event.alarms.len(), 1);
    assert_eq!(event.alarms[0].action, "DISPLAY");
    assert_eq!(event.alarms[0].trigger, "-PT10M");
    assert_eq!(event.alarms[0].description.as_deref(), Some("Reminder"));
}

#[test]
fn rejects_vtodo_and_vjournal_components() {
    let vtodo = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VTODO\r\nUID:todo@example.test\r\nSUMMARY:Task\r\nEND:VTODO\r\nEND:VCALENDAR\r\n";
    assert_eq!(
        parse_calendar(vtodo, IcsParserLimits::default())
            .unwrap_err()
            .code(),
        IcsParseErrorCode::Malformed
    );

    let vjournal = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VJOURNAL\r\nUID:journal@example.test\r\nSUMMARY:Journal\r\nEND:VJOURNAL\r\nEND:VCALENDAR\r\n";
    assert_eq!(
        parse_calendar(vjournal, IcsParserLimits::default())
            .unwrap_err()
            .code(),
        IcsParseErrorCode::Malformed
    );
}

#[test]
fn records_scheduling_properties_and_method() {
    let calendar = parse_calendar(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nMETHOD:REQUEST\r\nBEGIN:VEVENT\r\nUID:sched@example.test\r\nDTSTART:20260803T090000Z\r\nDTEND:20260803T100000Z\r\nSUMMARY:Sched\r\nORGANIZER:mailto:org@example.test\r\nATTENDEE:mailto:user@example.test\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n",
        IcsParserLimits::default(),
    )
    .expect("scheduling event must parse (rejection is the DAV layer's job)");

    assert!(calendar.has_method, "METHOD must be recorded");
    let event = &calendar.events[0];
    assert!(
        event.scheduling.iter().any(|s| s == "ORGANIZER"),
        "ORGANIZER must be recorded as a scheduling property"
    );
    assert!(
        event.scheduling.iter().any(|s| s == "ATTENDEE"),
        "ATTENDEE must be recorded as a scheduling property"
    );
}
