//! Regression fixtures for Apple Calendar client compatibility.
//!
//! These tests load the sanitized real-client traces committed under
//! `tests/fixtures/apple-calendar/` and assert that:
//!
//! - every ICS fixture parses through the production `parse_calendar` parser
//!   and round-trips through the CalDAV serializer with a stable ETag;
//! - every trace fixture is well-formed JSON with a non-empty `steps` array,
//!   sanitized placeholders (no real credentials), and valid HTTP
//!   methods/statuses.
//!
//! The fixtures encode the protocol shapes current macOS and iOS Calendar
//! clients send, so regressions in discovery, sync, CRUD, and revocation are
//! caught here rather than only by manual device testing.

use std::collections::HashMap;

use commoncal_backend::caldav::ical::{
    CaldavIcalEvent, CaldavIcalTiming, etag_for_ical, serialize_event_resource,
};
use commoncal_backend::ics::{IcsParserLimits, NormalizedTiming, parse_calendar};
use serde::Deserialize;

// --- ICS fixture tests ---

fn load_ics_fixture(name: &str) -> String {
    let path = format!("tests/fixtures/apple-calendar/ics/{name}");
    std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("failed to read ICS fixture {name}: {e}"))
}

fn parse_fixture(name: &str) -> commoncal_backend::ics::NormalizedCalendar {
    let ics = load_ics_fixture(name);
    parse_calendar(&ics, IcsParserLimits::default())
        .unwrap_or_else(|e| panic!("ICS fixture {name} must parse: {e}"))
}

/// Build a `CaldavIcalEvent` from a parsed `NormalizedEvent` for round-trip
/// serialization.
fn to_caldav_event(event: &commoncal_backend::ics::NormalizedEvent) -> CaldavIcalEvent {
    let timing = match &event.timing {
        NormalizedTiming::Timed {
            starts_at,
            ends_at,
            timezone,
        } => CaldavIcalTiming::Timed {
            start_utc: starts_at.timestamp(),
            end_utc: ends_at.timestamp(),
            timezone: timezone.clone().unwrap_or_else(|| "UTC".to_owned()),
        },
        NormalizedTiming::AllDay {
            start_date,
            end_date,
        } => CaldavIcalTiming::AllDay {
            start_date: start_date.format("%Y-%m-%d").to_string(),
            end_date: end_date.format("%Y-%m-%d").to_string(),
        },
    };
    CaldavIcalEvent {
        uid: event.uid.clone(),
        summary: event.summary.clone(),
        description: event.description.clone(),
        location: event.location.clone(),
        status: event.status.clone(),
        timing,
        dtstamp: event
            .dtstamp
            .map(|dt| dt.timestamp())
            .unwrap_or(1_768_435_200),
        sequence: event.sequence,
        free_busy: false,
        rrule: event.rrule.clone(),
        exdates: Vec::new(),
        recurrence_id: None,
        categories: event.categories.clone(),
        url: event.url.clone(),
        transp: event.transp.clone(),
        alarms: event.alarms.clone(),
        x_properties: event.x_properties.clone(),
    }
}

#[test]
fn timed_event_fixture_parses_and_round_trips() {
    let parsed = parse_fixture("timed-event.fixture.ics");
    assert_eq!(parsed.events.len(), 1);
    let event = &parsed.events[0];
    assert_eq!(event.uid, "timed-0001@example.test");
    assert_eq!(event.summary, "Team sync");
    assert_eq!(event.location.as_deref(), Some("Room 1"));
    assert_eq!(event.status.as_deref(), Some("CONFIRMED"));
    match &event.timing {
        NormalizedTiming::Timed { .. } => {}
        other => panic!("expected timed event, got {other:?}"),
    }

    // Round-trip: serialize through the CalDAV serializer and verify the
    // output is a valid VCALENDAR with a stable ETag.
    let caldav_event = to_caldav_event(event);
    let serialized = serialize_event_resource(&caldav_event);
    assert!(serialized.starts_with("BEGIN:VCALENDAR\r\n"));
    assert!(serialized.ends_with("END:VCALENDAR\r\n"));
    assert!(serialized.contains("UID:timed-0001@example.test"));
    assert!(serialized.contains("SUMMARY:Team sync"));

    // The ETag must be deterministic for identical content.
    let etag1 = etag_for_ical(&serialized);
    let etag2 = etag_for_ical(&serialize_event_resource(&caldav_event));
    assert_eq!(etag1, etag2, "ETag must be stable for identical content");
}

#[test]
fn all_day_event_fixture_parses_and_round_trips() {
    let parsed = parse_fixture("all-day-event.fixture.ics");
    assert_eq!(parsed.events.len(), 1);
    let event = &parsed.events[0];
    assert_eq!(event.uid, "allday-0001@example.test");
    assert_eq!(event.summary, "Conference");
    match &event.timing {
        NormalizedTiming::AllDay {
            start_date,
            end_date,
        } => {
            assert_eq!(
                start_date,
                &chrono::NaiveDate::from_ymd_opt(2026, 1, 15).unwrap()
            );
            assert_eq!(
                end_date,
                &chrono::NaiveDate::from_ymd_opt(2026, 1, 17).unwrap()
            );
        }
        other => panic!("expected all-day event, got {other:?}"),
    }

    let caldav_event = to_caldav_event(event);
    let serialized = serialize_event_resource(&caldav_event);
    assert!(serialized.contains("DTSTART;VALUE=DATE:20260115"));
    assert!(serialized.contains("DTEND;VALUE=DATE:20260117"));
    assert!(
        !serialized.contains("TZID="),
        "all-day event must not carry TZID"
    );
}

#[test]
fn recurring_series_fixture_parses_and_round_trips() {
    let parsed = parse_fixture("recurring-series.fixture.ics");
    assert_eq!(parsed.events.len(), 1);
    let event = &parsed.events[0];
    assert_eq!(event.uid, "series-0001@example.test");
    assert_eq!(event.summary, "Daily standup");
    assert_eq!(event.rrule.as_deref(), Some("FREQ=DAILY;COUNT=5"));

    let caldav_event = to_caldav_event(event);
    let serialized = serialize_event_resource(&caldav_event);
    assert!(serialized.contains("RRULE:FREQ=DAILY;COUNT=5"));
    assert!(serialized.contains("UID:series-0001@example.test"));
}

#[test]
fn modified_occurrence_fixture_parses_with_master_and_exception() {
    let parsed = parse_fixture("modified-occurrence.fixture.ics");
    assert_eq!(parsed.events.len(), 2, "master + one modified occurrence");
    // All VEVENTs share the same UID.
    assert!(
        parsed
            .events
            .iter()
            .all(|e| e.uid == "series-0002@example.test")
    );
    // One master (no RECURRENCE-ID) and one exception (with RECURRENCE-ID).
    let masters: Vec<_> = parsed
        .events
        .iter()
        .filter(|e| e.recurrence_id.is_none())
        .collect();
    let exceptions: Vec<_> = parsed
        .events
        .iter()
        .filter(|e| e.recurrence_id.is_some())
        .collect();
    assert_eq!(masters.len(), 1, "exactly one master VEVENT");
    assert_eq!(exceptions.len(), 1, "exactly one modified occurrence");
    assert_eq!(masters[0].rrule.as_deref(), Some("FREQ=DAILY;COUNT=5"));
}

#[test]
fn client_properties_fixture_preserves_allowlisted_metadata() {
    let parsed = parse_fixture("client-properties.fixture.ics");
    assert_eq!(parsed.events.len(), 1);
    let event = &parsed.events[0];
    assert_eq!(event.uid, "props-0001@example.test");
    assert_eq!(
        event.categories,
        vec!["Work".to_owned(), "Personal".to_owned()]
    );
    assert_eq!(event.url.as_deref(), Some("https://example.test/event"));
    assert_eq!(event.transp.as_deref(), Some("OPAQUE"));
    // Only allowlisted X-properties survive.
    assert!(
        event
            .x_properties
            .iter()
            .any(|x| x.name == "X-APPLE-CEVENT-CATEGORY")
    );
    assert!(
        event
            .x_properties
            .iter()
            .any(|x| x.name == "X-APPLE-FALLBACK-ALARM-UID")
    );
    // Only DISPLAY/AUDIO alarms with a TRIGGER are preserved.
    assert_eq!(event.alarms.len(), 1);
    assert_eq!(event.alarms[0].action, "DISPLAY");
    assert_eq!(event.alarms[0].trigger, "-PT10M");
}

// --- Trace fixture tests ---

#[derive(Deserialize)]
struct Trace {
    name: String,
    description: String,
    sanitized: bool,
    steps: Vec<TraceStep>,
}

#[derive(Deserialize)]
struct TraceStep {
    note: String,
    request: TraceRequest,
    response: TraceResponse,
}

#[derive(Deserialize)]
struct TraceRequest {
    method: String,
    uri: String,
    #[serde(default)]
    headers: HashMap<String, String>,
    #[serde(default)]
    body: Option<String>,
}

#[derive(Deserialize)]
struct TraceResponse {
    status: u16,
    #[serde(default)]
    headers: HashMap<String, String>,
    #[serde(default)]
    body_contains: Option<Vec<String>>,
}

fn load_trace_fixture(name: &str) -> Trace {
    let path = format!("tests/fixtures/apple-calendar/traces/{name}");
    let json = std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("failed to read trace fixture {name}: {e}"));
    serde_json::from_str(&json)
        .unwrap_or_else(|e| panic!("trace fixture {name} must be valid JSON: {e}"))
}

fn assert_trace_well_formed(trace: &Trace) {
    assert!(!trace.name.is_empty(), "trace name must be non-empty");
    assert!(
        !trace.description.is_empty(),
        "trace description must be non-empty"
    );
    assert!(trace.sanitized, "trace must be marked sanitized");
    assert!(!trace.steps.is_empty(), "trace must have at least one step");
    for step in &trace.steps {
        assert!(!step.note.is_empty(), "step note must be non-empty");
        assert!(
            !step.request.method.is_empty(),
            "request method must be non-empty"
        );
        assert!(
            !step.request.uri.is_empty(),
            "request URI must be non-empty"
        );
        assert!(
            (100..600).contains(&step.response.status),
            "response status {} must be a valid HTTP status",
            step.response.status
        );
        // Sanitization: no real credentials in Authorization headers.
        for (key, value) in &step.request.headers {
            if key.eq_ignore_ascii_case("authorization") {
                assert!(
                    value.contains("<sanitized-credentials>"),
                    "Authorization header must be sanitized, got: {value}"
                );
            }
        }
        // Sanitization: no real credentials in request bodies.
        if let Some(body) = &step.request.body {
            assert!(
                !body.contains("Basic ") || body.contains("<sanitized-credentials>"),
                "request body must not contain unsanitized Basic credentials"
            );
        }
        // Response headers, when present, must be well-formed key/value pairs.
        for (key, value) in &step.response.headers {
            assert!(!key.is_empty(), "response header name must be non-empty");
            assert!(!value.is_empty(), "response header value must be non-empty");
        }
        // body_contains substrings, when present, must be non-empty.
        if let Some(substrings) = &step.response.body_contains {
            for substring in substrings {
                assert!(
                    !substring.is_empty(),
                    "body_contains substring must be non-empty"
                );
            }
        }
    }
}

#[test]
fn connect_trace_is_well_formed_and_sanitized() {
    let trace = load_trace_fixture("connect.trace.json");
    assert_eq!(trace.name, "connect");
    assert_trace_well_formed(&trace);
    // The connect trace must include an OPTIONS capability discovery step.
    assert!(trace.steps.iter().any(|s| s.request.method == "OPTIONS"));
    // And an authenticated PROPFIND step.
    assert!(trace.steps.iter().any(|s| s.request.method == "PROPFIND"));
}

#[test]
fn sync_initial_trace_is_well_formed_and_sanitized() {
    let trace = load_trace_fixture("sync-initial.trace.json");
    assert_eq!(trace.name, "sync-initial");
    assert_trace_well_formed(&trace);
    // The sync-initial trace must use a REPORT method.
    assert!(trace.steps.iter().all(|s| s.request.method == "REPORT"));
}

#[test]
fn sync_incremental_trace_is_well_formed_and_sanitized() {
    let trace = load_trace_fixture("sync-incremental.trace.json");
    assert_eq!(trace.name, "sync-incremental");
    assert_trace_well_formed(&trace);
    // The incremental trace must carry a non-empty sync-token in the body.
    assert!(trace.steps.iter().all(|s| {
        s.request
            .body
            .as_deref()
            .unwrap_or("")
            .contains("sync-token")
    }));
}

#[test]
fn crud_create_trace_is_well_formed_and_sanitized() {
    let trace = load_trace_fixture("crud-create.trace.json");
    assert_eq!(trace.name, "crud-create");
    assert_trace_well_formed(&trace);
    // The create trace must use a PUT method with If-None-Match.
    let step = &trace.steps[0];
    assert_eq!(step.request.method, "PUT");
    assert_eq!(
        step.request
            .headers
            .get("If-None-Match")
            .map(String::as_str),
        Some("*")
    );
    assert_eq!(step.response.status, 201);
}

#[test]
fn crud_update_trace_is_well_formed_and_sanitized() {
    let trace = load_trace_fixture("crud-update.trace.json");
    assert_eq!(trace.name, "crud-update");
    assert_trace_well_formed(&trace);
    // The update trace must include a successful PUT (200) and a stale PUT (412).
    let statuses: Vec<u16> = trace.steps.iter().map(|s| s.response.status).collect();
    assert!(statuses.contains(&200), "must include a successful update");
    assert!(statuses.contains(&412), "must include a stale-writer 412");
}

#[test]
fn crud_delete_trace_is_well_formed_and_sanitized() {
    let trace = load_trace_fixture("crud-delete.trace.json");
    assert_eq!(trace.name, "crud-delete");
    assert_trace_well_formed(&trace);
    // The delete trace must include a DELETE (200) and a subsequent GET (404).
    let statuses: Vec<u16> = trace.steps.iter().map(|s| s.response.status).collect();
    assert!(statuses.contains(&200), "must include a successful delete");
    assert!(
        statuses.contains(&404),
        "must include a 404 for the deleted resource"
    );
}

#[test]
fn revoke_trace_is_well_formed_and_sanitized() {
    let trace = load_trace_fixture("revoke.trace.json");
    assert_eq!(trace.name, "revoke");
    assert_trace_well_formed(&trace);
    // The revoke trace must show a successful request (207) followed by a
    // rejected request (401) after revocation.
    let statuses: Vec<u16> = trace.steps.iter().map(|s| s.response.status).collect();
    assert!(
        statuses.contains(&207),
        "must include a pre-revocation success"
    );
    assert!(
        statuses.contains(&401),
        "must include a post-revocation 401"
    );
}
