use std::fmt::{self, Display, Formatter};

use chrono::{NaiveDate, NaiveDateTime, TimeZone, Utc};
use quick_xml::Reader;
use quick_xml::events::Event;

pub const MAX_BODY_BYTES: usize = 64 * 1024;
pub const MAX_RESULTS: usize = 1000;
pub const MAX_RANGE_SECONDS: i64 = 366 * 24 * 60 * 60;
pub const MAX_MULTIGET_HREFS: usize = 1000;

#[derive(Debug, PartialEq, Eq)]
pub enum QueryError {
    MalformedXml,
    UnsupportedRoot,
    MissingFilter,
    UnsupportedFilter,
    InvalidDateTime,
    InvalidRange,
    RangeTooLarge,
    MissingHref,
    TooManyHrefs,
    BadSyncLevel,
}

impl Display for QueryError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::MalformedXml => write!(f, "malformed XML"),
            Self::UnsupportedRoot => write!(f, "unsupported report type"),
            Self::MissingFilter => write!(f, "missing filter"),
            Self::UnsupportedFilter => write!(f, "unsupported filter"),
            Self::InvalidDateTime => write!(f, "invalid date-time value"),
            Self::InvalidRange => write!(f, "invalid time range"),
            Self::RangeTooLarge => write!(f, "time range too large"),
            Self::MissingHref => write!(f, "missing href"),
            Self::TooManyHrefs => write!(f, "too many hrefs"),
            Self::BadSyncLevel => write!(f, "sync-level must be 1"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CalendarQuery {
    pub start_utc: i64,
    pub end_utc: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CalendarMultiget {
    pub hrefs: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncCollection {
    /// Opaque token from the client; `None` (or empty) requests an initial
    /// snapshot.
    pub sync_token: Option<String>,
    /// `sync-level` must be 1 for calendar sync; other values are rejected.
    pub sync_level: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CalendarReport {
    Query(CalendarQuery),
    Multiget(CalendarMultiget),
    SyncCollection(SyncCollection),
}

fn local_name(tag: &str) -> &str {
    match tag.rsplit_once(':') {
        Some((_, name)) => name,
        None => tag,
    }
}

fn parse_ical_datetime(value: &str) -> Result<i64, QueryError> {
    let value = value.trim();
    if value.is_empty() {
        return Err(QueryError::InvalidDateTime);
    }
    if value.ends_with('Z') || value.ends_with('z') {
        let naive = parse_ical_naive(&value[..value.len() - 1])?;
        Ok(Utc.from_utc_datetime(&naive).timestamp())
    } else if value.len() == 8 {
        let date =
            NaiveDate::parse_from_str(value, "%Y%m%d").map_err(|_| QueryError::InvalidDateTime)?;
        Ok(Utc
            .from_utc_datetime(
                &date
                    .and_hms_opt(0, 0, 0)
                    .ok_or(QueryError::InvalidDateTime)?,
            )
            .timestamp())
    } else {
        Err(QueryError::InvalidDateTime)
    }
}

fn parse_ical_naive(value: &str) -> Result<NaiveDateTime, QueryError> {
    if value.len() != 15 {
        return Err(QueryError::InvalidDateTime);
    }
    let naive = NaiveDateTime::parse_from_str(value, "%Y%m%dT%H%M%S")
        .map_err(|_| QueryError::InvalidDateTime)?;
    Ok(naive)
}

fn name_to_str(name: &[u8]) -> &str {
    std::str::from_utf8(name).unwrap_or("")
}

pub fn parse_calendar_query(body: &[u8]) -> Result<CalendarQuery, QueryError> {
    if body.len() > MAX_BODY_BYTES {
        return Err(QueryError::MalformedXml);
    }
    let text = std::str::from_utf8(body).map_err(|_| QueryError::MalformedXml)?;
    let mut reader = Reader::from_str(text);
    reader.config_mut().trim_text(true);

    let mut buf = Vec::new();
    let mut root_seen = false;
    let mut in_filter = false;
    let mut filter_depth: u32 = 0;
    let mut found_time_range = false;
    let mut start_utc: Option<i64> = None;
    let mut end_utc: Option<i64> = None;
    let mut unsupported_in_filter = false;
    let mut depth: u32 = 0;

    fn extract_time_range(
        attrs: quick_xml::events::attributes::Attributes,
    ) -> Result<(Option<i64>, Option<i64>), QueryError> {
        let mut s: Option<i64> = None;
        let mut e: Option<i64> = None;
        for attr_result in attrs {
            let Ok(attr) = attr_result else {
                return Err(QueryError::MalformedXml);
            };
            let key = name_to_str(attr.key.as_ref());
            let k = local_name(key);
            let val = attr
                .unescape_value()
                .map(|v| v.into_owned())
                .unwrap_or_default();
            if k == "start" {
                s = Some(parse_ical_datetime(&val)?);
            } else if k == "end" {
                e = Some(parse_ical_datetime(&val)?);
            }
        }
        Ok((s, e))
    }

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref start)) => {
                let local_name_ref = start.local_name();
                let local = name_to_str(local_name_ref.as_ref());
                if !root_seen {
                    if local != "calendar-query" {
                        return Err(QueryError::UnsupportedRoot);
                    }
                    root_seen = true;
                    depth = 1;
                    continue;
                }
                depth += 1;
                if in_filter {
                    filter_depth += 1;
                    if local == "time-range" {
                        found_time_range = true;
                        let (s, e) = extract_time_range(start.attributes())?;
                        start_utc = start_utc.or(s);
                        end_utc = end_utc.or(e);
                    } else if local != "filter" {
                        unsupported_in_filter = true;
                    }
                } else if local == "filter" {
                    in_filter = true;
                    filter_depth = 1;
                }
            }
            Ok(Event::Empty(ref empty)) => {
                let local_name_ref = empty.local_name();
                let local = name_to_str(local_name_ref.as_ref());
                if !root_seen {
                    if local != "calendar-query" {
                        return Err(QueryError::UnsupportedRoot);
                    }
                    root_seen = true;
                    continue;
                }
                if in_filter {
                    if local == "time-range" {
                        found_time_range = true;
                        let (s, e) = extract_time_range(empty.attributes())?;
                        start_utc = start_utc.or(s);
                        end_utc = end_utc.or(e);
                    } else if local != "filter" {
                        unsupported_in_filter = true;
                    }
                }
            }
            Ok(Event::End(_)) => {
                depth = depth.saturating_sub(1);
                if in_filter {
                    filter_depth = filter_depth.saturating_sub(1);
                    if filter_depth == 0 {
                        in_filter = false;
                    }
                }
            }
            Ok(Event::Eof) => {
                if depth > 0 {
                    return Err(QueryError::MalformedXml);
                }
                break;
            }
            Ok(_) => {}
            Err(_) => return Err(QueryError::MalformedXml),
        }
        buf.clear();
    }

    if !root_seen {
        return Err(QueryError::UnsupportedRoot);
    }
    if unsupported_in_filter {
        return Err(QueryError::UnsupportedFilter);
    }
    if !found_time_range {
        return Err(QueryError::MissingFilter);
    }
    let start = start_utc.ok_or(QueryError::InvalidDateTime)?;
    let end = end_utc.ok_or(QueryError::InvalidDateTime)?;
    if start >= end {
        return Err(QueryError::InvalidRange);
    }
    if end - start > MAX_RANGE_SECONDS {
        return Err(QueryError::RangeTooLarge);
    }
    Ok(CalendarQuery {
        start_utc: start,
        end_utc: end,
    })
}

/// Parse a `calendar-multiget` REPORT body into its requested hrefs.
///
/// Only `<D:href>` elements that are direct children of the root are
/// captured. The list is bounded by `MAX_MULTIGET_HREFS`.
pub fn parse_calendar_multiget(body: &[u8]) -> Result<CalendarMultiget, QueryError> {
    if body.len() > MAX_BODY_BYTES {
        return Err(QueryError::MalformedXml);
    }
    let text = std::str::from_utf8(body).map_err(|_| QueryError::MalformedXml)?;
    let mut reader = Reader::from_str(text);
    reader.config_mut().trim_text(true);

    let mut buf = Vec::new();
    let mut root_seen = false;
    let mut depth: u32 = 0;
    let mut hrefs: Vec<String> = Vec::new();
    let mut capturing_href = false;
    let mut current_href = String::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref start)) => {
                let local_name_ref = start.local_name();
                let local = name_to_str(local_name_ref.as_ref());
                if !root_seen {
                    if local != "calendar-multiget" {
                        return Err(QueryError::UnsupportedRoot);
                    }
                    root_seen = true;
                    depth = 1;
                    continue;
                }
                depth += 1;
                if depth == 2 && local == "href" {
                    capturing_href = true;
                    current_href.clear();
                }
            }
            Ok(Event::Empty(ref empty)) => {
                let local_name_ref = empty.local_name();
                let local = name_to_str(local_name_ref.as_ref());
                if !root_seen {
                    if local != "calendar-multiget" {
                        return Err(QueryError::UnsupportedRoot);
                    }
                    root_seen = true;
                    continue;
                }
                depth += 1;
                if depth == 2 && local == "href" {
                    // Self-closing href carries no text; record it as empty.
                    if hrefs.len() >= MAX_MULTIGET_HREFS {
                        return Err(QueryError::TooManyHrefs);
                    }
                    hrefs.push(String::new());
                }
                depth = depth.saturating_sub(1);
            }
            Ok(Event::Text(ref text_event)) => {
                if capturing_href {
                    current_href.push_str(text_event.unescape().unwrap_or_default().as_ref());
                }
            }
            Ok(Event::End(_)) => {
                if capturing_href {
                    capturing_href = false;
                    if hrefs.len() >= MAX_MULTIGET_HREFS {
                        return Err(QueryError::TooManyHrefs);
                    }
                    hrefs.push(current_href.trim().to_owned());
                }
                depth = depth.saturating_sub(1);
            }
            Ok(Event::Eof) => {
                if depth > 0 {
                    return Err(QueryError::MalformedXml);
                }
                break;
            }
            Ok(_) => {}
            Err(_) => return Err(QueryError::MalformedXml),
        }
        buf.clear();
    }

    if !root_seen {
        return Err(QueryError::UnsupportedRoot);
    }
    if hrefs.is_empty() {
        return Err(QueryError::MissingHref);
    }
    Ok(CalendarMultiget { hrefs })
}

/// Parse a `sync-collection` REPORT body into its sync token and level.
///
/// The `sync-token` element is optional: an absent or empty token requests an
/// initial snapshot. The `sync-level` element must be present and equal to 1
/// for calendar sync; any other value is rejected.
pub fn parse_sync_collection(body: &[u8]) -> Result<SyncCollection, QueryError> {
    if body.len() > MAX_BODY_BYTES {
        return Err(QueryError::MalformedXml);
    }
    let text = std::str::from_utf8(body).map_err(|_| QueryError::MalformedXml)?;
    let mut reader = Reader::from_str(text);
    reader.config_mut().trim_text(true);

    let mut buf = Vec::new();
    let mut root_seen = false;
    let mut depth: u32 = 0;
    let mut sync_token: Option<String> = None;
    let mut sync_level: Option<u32> = None;
    let mut capturing: Option<&'static str> = None;
    let mut current = String::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref start)) => {
                let local_name_ref = start.local_name();
                let local = name_to_str(local_name_ref.as_ref());
                if !root_seen {
                    if local != "sync-collection" {
                        return Err(QueryError::UnsupportedRoot);
                    }
                    root_seen = true;
                    depth = 1;
                    continue;
                }
                depth += 1;
                if depth == 2 {
                    if local == "sync-token" {
                        capturing = Some("token");
                        current.clear();
                    } else if local == "sync-level" {
                        capturing = Some("level");
                        current.clear();
                    }
                }
            }
            Ok(Event::Empty(ref empty)) => {
                let local_name_ref = empty.local_name();
                let local = name_to_str(local_name_ref.as_ref());
                if !root_seen {
                    if local != "sync-collection" {
                        return Err(QueryError::UnsupportedRoot);
                    }
                    root_seen = true;
                    continue;
                }
                depth += 1;
                if depth == 2 && local == "sync-token" {
                    // Self-closing sync-token carries no text: initial snapshot.
                    sync_token = Some(String::new());
                }
                depth = depth.saturating_sub(1);
            }
            Ok(Event::Text(ref text_event)) => {
                if capturing.is_some() {
                    current.push_str(text_event.unescape().unwrap_or_default().as_ref());
                }
            }
            Ok(Event::End(_)) => {
                if let Some(kind) = capturing {
                    capturing = None;
                    let value = current.trim().to_owned();
                    match kind {
                        "token" => {
                            sync_token = Some(value);
                        }
                        "level" => {
                            sync_level = value.parse::<u32>().ok();
                        }
                        _ => {}
                    }
                }
                depth = depth.saturating_sub(1);
            }
            Ok(Event::Eof) => {
                if depth > 0 {
                    return Err(QueryError::MalformedXml);
                }
                break;
            }
            Ok(_) => {}
            Err(_) => return Err(QueryError::MalformedXml),
        }
        buf.clear();
    }

    if !root_seen {
        return Err(QueryError::UnsupportedRoot);
    }
    let level = sync_level.ok_or(QueryError::BadSyncLevel)?;
    if level != 1 {
        return Err(QueryError::BadSyncLevel);
    }
    let token = sync_token.unwrap_or_default();
    Ok(SyncCollection {
        sync_token: if token.is_empty() { None } else { Some(token) },
        sync_level: level,
    })
}

/// Determine the report type from the body and parse it accordingly.
pub fn parse_calendar_report(body: &[u8]) -> Result<CalendarReport, QueryError> {
    if body.len() > MAX_BODY_BYTES {
        return Err(QueryError::MalformedXml);
    }
    let text = std::str::from_utf8(body).map_err(|_| QueryError::MalformedXml)?;
    let root = peek_root(text).ok_or(QueryError::UnsupportedRoot)?;
    match root.as_str() {
        "calendar-query" => Ok(CalendarReport::Query(parse_calendar_query(body)?)),
        "calendar-multiget" => Ok(CalendarReport::Multiget(parse_calendar_multiget(body)?)),
        "sync-collection" => Ok(CalendarReport::SyncCollection(parse_sync_collection(body)?)),
        _ => Err(QueryError::UnsupportedRoot),
    }
}

/// Read the first element name in an XML document, if any.
fn peek_root(text: &str) -> Option<String> {
    let mut reader = Reader::from_str(text);
    reader.config_mut().trim_text(true);
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref start)) => {
                return Some(name_to_str(start.local_name().as_ref()).to_owned());
            }
            Ok(Event::Empty(ref empty)) => {
                return Some(name_to_str(empty.local_name().as_ref()).to_owned());
            }
            Ok(Event::Eof) => return None,
            Ok(_) => {}
            Err(_) => return None,
        }
        buf.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_query() -> String {
        r#"<?xml version="1.0" encoding="utf-8" ?>
<C:calendar-query xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:calendarserver/">
  <D:prop>
    <D:getetag/>
    <D:getcontenttype/>
    <C:calendar-data/>
  </D:prop>
  <C:filter>
    <C:time-range start="20260101T000000Z" end="20260131T235959Z"/>
  </C:filter>
</C:calendar-query>"#
            .to_owned()
    }

    fn valid_multiget() -> String {
        r#"<?xml version="1.0" encoding="utf-8" ?>
<C:calendar-multiget xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:calendarserver/">
  <D:prop>
    <D:getetag/>
    <D:getcontenttype/>
    <C:calendar-data/>
  </D:prop>
  <D:href>/dav/calendars/p/1/aaa.ics</D:href>
  <D:href>/dav/calendars/p/1/bbb.ics</D:href>
</C:calendar-multiget>"#
            .to_owned()
    }

    #[test]
    fn parses_valid_multiget_hrefs() {
        let m = parse_calendar_multiget(valid_multiget().as_bytes()).unwrap();
        assert_eq!(
            m.hrefs,
            vec![
                "/dav/calendars/p/1/aaa.ics".to_owned(),
                "/dav/calendars/p/1/bbb.ics".to_owned()
            ]
        );
    }

    #[test]
    fn parses_multiget_single_href() {
        let body = r#"<C:calendar-multiget xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:calendarserver/"><D:href>/dav/calendars/p/1/aaa.ics</D:href></C:calendar-multiget>"#;
        let m = parse_calendar_multiget(body.as_bytes()).unwrap();
        assert_eq!(m.hrefs, vec!["/dav/calendars/p/1/aaa.ics".to_owned()]);
    }

    #[test]
    fn rejects_multiget_missing_href() {
        let body = r#"<C:calendar-multiget xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:calendarserver/"><D:prop><D:getetag/></D:prop></C:calendar-multiget>"#;
        assert_eq!(
            parse_calendar_multiget(body.as_bytes()).unwrap_err(),
            QueryError::MissingHref
        );
    }

    #[test]
    fn rejects_multiget_wrong_root() {
        let body = r#"<C:calendar-query xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:calendarserver/"><D:href>/x/</D:href></C:calendar-query>"#;
        assert_eq!(
            parse_calendar_multiget(body.as_bytes()).unwrap_err(),
            QueryError::UnsupportedRoot
        );
    }

    #[test]
    fn rejects_multiget_malformed_xml() {
        let body = r#"<C:calendar-multiget xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:calendarserver/"><D:href>"#;
        assert_eq!(
            parse_calendar_multiget(body.as_bytes()).unwrap_err(),
            QueryError::MalformedXml
        );
    }

    #[test]
    fn rejects_multiget_too_many_hrefs() {
        let hrefs = (0..=MAX_MULTIGET_HREFS)
            .map(|i| format!("<D:href>/dav/calendars/p/1/{i}.ics</D:href>"))
            .collect::<Vec<_>>()
            .join("");
        let body = format!(
            r#"<C:calendar-multiget xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:calendarserver/">{hrefs}</C:calendar-multiget>"#
        );
        assert_eq!(
            parse_calendar_multiget(body.as_bytes()).unwrap_err(),
            QueryError::TooManyHrefs
        );
    }

    #[test]
    fn accepts_multiget_at_href_limit() {
        let hrefs = (0..MAX_MULTIGET_HREFS)
            .map(|i| format!("<D:href>/dav/calendars/p/1/{i}.ics</D:href>"))
            .collect::<Vec<_>>()
            .join("");
        let body = format!(
            r#"<C:calendar-multiget xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:calendarserver/">{hrefs}</C:calendar-multiget>"#
        );
        let m = parse_calendar_multiget(body.as_bytes()).unwrap();
        assert_eq!(m.hrefs.len(), MAX_MULTIGET_HREFS);
    }

    #[test]
    fn report_dispatches_calendar_query() {
        let report = parse_calendar_report(valid_query().as_bytes()).unwrap();
        assert!(matches!(report, CalendarReport::Query(_)));
    }

    #[test]
    fn report_dispatches_calendar_multiget() {
        let report = parse_calendar_report(valid_multiget().as_bytes()).unwrap();
        match report {
            CalendarReport::Multiget(m) => assert_eq!(m.hrefs.len(), 2),
            _ => panic!("expected multiget"),
        }
    }

    #[test]
    fn report_rejects_unknown_root() {
        let body = r#"<C:unknown-report xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:calendarserver/"><D:href>/x/</D:href></C:unknown-report>"#;
        assert_eq!(
            parse_calendar_report(body.as_bytes()).unwrap_err(),
            QueryError::UnsupportedRoot
        );
    }

    #[test]
    fn report_dispatches_sync_collection() {
        let body = r#"<C:sync-collection xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:calendarserver/"><D:sync-token></D:sync-token><D:sync-level>1</D:sync-level><D:prop><D:getetag/></D:prop></C:sync-collection>"#;
        let report = parse_calendar_report(body.as_bytes()).unwrap();
        match report {
            CalendarReport::SyncCollection(sync) => {
                assert!(sync.sync_token.is_none());
                assert_eq!(sync.sync_level, 1);
            }
            _ => panic!("expected sync-collection"),
        }
    }

    #[test]
    fn sync_collection_parses_token_and_level() {
        let body = r#"<C:sync-collection xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:calendarserver/"><D:sync-token>opaque-token-123</D:sync-token><D:sync-level>1</D:sync-level></C:sync-collection>"#;
        let sync = parse_sync_collection(body.as_bytes()).unwrap();
        assert_eq!(sync.sync_token.as_deref(), Some("opaque-token-123"));
        assert_eq!(sync.sync_level, 1);
    }

    #[test]
    fn sync_collection_rejects_bad_level() {
        let body = r#"<C:sync-collection xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:calendarserver/"><D:sync-token></D:sync-token><D:sync-level>2</D:sync-level></C:sync-collection>"#;
        assert_eq!(
            parse_sync_collection(body.as_bytes()).unwrap_err(),
            QueryError::BadSyncLevel
        );
    }

    #[test]
    fn sync_collection_rejects_missing_level() {
        let body = r#"<C:sync-collection xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:calendarserver/"><D:sync-token></D:sync-token></C:sync-collection>"#;
        assert_eq!(
            parse_sync_collection(body.as_bytes()).unwrap_err(),
            QueryError::BadSyncLevel
        );
    }

    #[test]
    fn sync_collection_rejects_wrong_root() {
        let body = r#"<C:calendar-query xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:calendarserver/"><D:sync-token></D:sync-token><D:sync-level>1</D:sync-level></C:calendar-query>"#;
        assert_eq!(
            parse_sync_collection(body.as_bytes()).unwrap_err(),
            QueryError::UnsupportedRoot
        );
    }

    #[test]
    fn parses_valid_time_range() {
        let q = parse_calendar_query(valid_query().as_bytes()).unwrap();
        assert_eq!(q.start_utc, 1767225600);
        assert_eq!(q.end_utc, 1769903999);
    }

    #[test]
    fn rejects_empty_body() {
        assert_eq!(
            parse_calendar_query(b"").unwrap_err(),
            QueryError::UnsupportedRoot
        );
    }

    #[test]
    fn rejects_non_xml() {
        assert_eq!(
            parse_calendar_query(b"not xml at all").unwrap_err(),
            QueryError::UnsupportedRoot
        );
    }

    #[test]
    fn rejects_wrong_root_element() {
        let body = r#"<C:calendar-multiget xmlns:C="urn:ietf:params:xml:ns:calendarserver/"><D:href xmlns:D="DAV:">/x/</D:href></C:calendar-multiget>"#;
        assert_eq!(
            parse_calendar_query(body.as_bytes()).unwrap_err(),
            QueryError::UnsupportedRoot
        );
    }

    #[test]
    fn rejects_missing_filter() {
        let body = r#"<C:calendar-query xmlns:C="urn:ietf:params:xml:ns:calendarserver/"><D:prop xmlns:D="DAV:"><D:getetag/></D:prop></C:calendar-query>"#;
        assert_eq!(
            parse_calendar_query(body.as_bytes()).unwrap_err(),
            QueryError::MissingFilter
        );
    }

    #[test]
    fn rejects_unsupported_filter_type() {
        let body = r#"<C:calendar-query xmlns:C="urn:ietf:params:xml:ns:calendarserver/"><C:filter><C:comp-filter name="VEVENT"><C:prop-filter name="SUMMARY"><C:is-text matches="anywhere">test</C:is-text></C:prop-filter></C:comp-filter></C:filter></C:calendar-query>"#;
        assert_eq!(
            parse_calendar_query(body.as_bytes()).unwrap_err(),
            QueryError::UnsupportedFilter
        );
    }

    #[test]
    fn rejects_invalid_start_before_end() {
        let body = r#"<C:calendar-query xmlns:C="urn:ietf:params:xml:ns:calendarserver/"><C:filter><C:time-range start="20260131T000000Z" end="20260101T000000Z"/></C:filter></C:calendar-query>"#;
        assert_eq!(
            parse_calendar_query(body.as_bytes()).unwrap_err(),
            QueryError::InvalidRange
        );
    }

    #[test]
    fn rejects_equal_start_and_end() {
        let body = r#"<C:calendar-query xmlns:C="urn:ietf:params:xml:ns:calendarserver/"><C:filter><C:time-range start="20260101T000000Z" end="20260101T000000Z"/></C:filter></C:calendar-query>"#;
        assert_eq!(
            parse_calendar_query(body.as_bytes()).unwrap_err(),
            QueryError::InvalidRange
        );
    }

    #[test]
    fn rejects_range_too_large() {
        let body = r#"<C:calendar-query xmlns:C="urn:ietf:params:xml:ns:calendarserver/"><C:filter><C:time-range start="20200101T000000Z" end="20260101T000000Z"/></C:filter></C:calendar-query>"#;
        assert_eq!(
            parse_calendar_query(body.as_bytes()).unwrap_err(),
            QueryError::RangeTooLarge
        );
    }

    #[test]
    fn rejects_invalid_date_format() {
        let body = r#"<C:calendar-query xmlns:C="urn:ietf:params:xml:ns:calendarserver/"><C:filter><C:time-range start="notadate" end="20260101T000000Z"/></C:filter></C:calendar-query>"#;
        assert_eq!(
            parse_calendar_query(body.as_bytes()).unwrap_err(),
            QueryError::InvalidDateTime
        );
    }

    #[test]
    fn rejects_oversized_body() {
        let mut body = valid_query();
        body.push_str(&format!("<!-- {} -->", "x".repeat(MAX_BODY_BYTES)));
        assert_eq!(
            parse_calendar_query(body.as_bytes()).unwrap_err(),
            QueryError::MalformedXml
        );
    }

    #[test]
    fn accepts_date_only_start() {
        let body = r#"<C:calendar-query xmlns:C="urn:ietf:params:xml:ns:calendarserver/"><C:filter><C:time-range start="20260101" end="20260131T235959Z"/></C:filter></C:calendar-query>"#;
        let q = parse_calendar_query(body.as_bytes()).unwrap();
        assert_eq!(q.start_utc, 1767225600);
    }

    #[test]
    fn rejects_malformed_xml() {
        let body =
            r#"<C:calendar-query xmlns:C="urn:ietf:params:xml:ns:calendarserver/"><C:filter>"#;
        assert_eq!(
            parse_calendar_query(body.as_bytes()).unwrap_err(),
            QueryError::MalformedXml
        );
    }
}
