use commoncal_backend::caldav::ical::{
    CaldavIcalEvent, CaldavIcalTiming, serialize_event_resource,
};
use commoncal_backend::ics::{IcsParserLimits, parse_calendar};

#[test]
fn debug_serialize() {
    let event = CaldavIcalEvent {
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
    };
    let ics = serialize_event_resource(&event);
    eprintln!("=== ICS ===\n{ics}\n=== END ===");
    let parsed = parse_calendar(&ics, IcsParserLimits::default());
    match &parsed {
        Ok(p) => eprintln!("Parse OK: {} events", p.events.len()),
        Err(e) => eprintln!("Parse FAILED: {e:?}"),
    }
    assert!(parsed.is_ok(), "serialization must parse back");
}
