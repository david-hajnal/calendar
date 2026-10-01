use chrono::TimeZone;
use commoncal_backend::caldav::ical::{
    CaldavIcalEvent, CaldavIcalTiming, serialize_event_resource,
};
use commoncal_backend::ics::{IcsParserLimits, parse_calendar};

fn mk(uid: &str, start_utc: i64, end_utc: i64) -> CaldavIcalEvent {
    CaldavIcalEvent {
        uid: uid.to_owned(),
        summary: "S".to_owned(),
        description: None,
        location: None,
        status: Some("CONFIRMED".to_owned()),
        timing: CaldavIcalTiming::Timed {
            start_utc,
            end_utc,
            timezone: "America/New_York".to_owned(),
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

#[test]
fn dst_ambiguity_check() {
    // DST fallback 2026-11-01 America/New_York: 2:00 AM EDT -> 1:00 AM EST (at 06:00 UTC)
    // 1:30 AM occurs twice: 05:30 UTC (EDT) and 06:30 UTC (EST)
    let c_start = chrono::Utc
        .with_ymd_and_hms(2026, 11, 1, 5, 30, 0)
        .single()
        .unwrap()
        .timestamp();
    let c_end = chrono::Utc
        .with_ymd_and_hms(2026, 11, 1, 6, 30, 0)
        .single()
        .unwrap()
        .timestamp();
    let a_start = chrono::Utc
        .with_ymd_and_hms(2026, 11, 1, 6, 30, 0)
        .single()
        .unwrap()
        .timestamp();
    let a_end = chrono::Utc
        .with_ymd_and_hms(2026, 11, 1, 7, 30, 0)
        .single()
        .unwrap()
        .timestamp();
    let c = mk("c", c_start, c_end); // 05:30 UTC -> 1:30 AM EDT (first)
    let a = mk("a", a_start, a_end); // 06:30 UTC -> 1:30 AM EST (second)
    let ics_c = serialize_event_resource(&c);
    let ics_a = serialize_event_resource(&a);
    let dtstart_c = ics_c.lines().find(|l| l.starts_with("DTSTART")).unwrap();
    let dtstart_a = ics_a.lines().find(|l| l.starts_with("DTSTART")).unwrap();
    eprintln!("C (05:30 UTC) DTSTART: {dtstart_c}");
    eprintln!("A (06:30 UTC) DTSTART: {dtstart_a}");
    eprintln!("Same DTSTART (lossy)? {}", dtstart_c == dtstart_a);
    eprintln!(
        "Parse C: {:?}",
        parse_calendar(&ics_c, IcsParserLimits::default()).is_ok()
    );
    eprintln!(
        "Parse A: {:?}",
        parse_calendar(&ics_a, IcsParserLimits::default()).is_ok()
    );
}
