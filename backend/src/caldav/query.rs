use std::collections::HashMap;
use std::fmt::{self, Display, Formatter};

use chrono::{NaiveDate, NaiveDateTime, TimeZone, Utc};
use quick_xml::Reader;
use quick_xml::events::Event;
use quick_xml::name::{PrefixDeclaration, QName};

pub const MAX_BODY_BYTES: usize = 64 * 1024;
pub const MAX_RESULTS: usize = 1000;
pub const MAX_RANGE_SECONDS: i64 = 366 * 24 * 60 * 60;
pub const MAX_MULTIGET_HREFS: usize = 1000;

// --- XML namespaces (RFC 4791 / RFC 6578 / Apple) ---
//
// The CalDAV namespace is `urn:ietf:params:xml:ns:caldav`. The legacy
// `urn:ietf:params:xml:ns:calendarserver/` URI is NOT used for any RFC 4791
// property or report element. Apple's `calendar-color` lives in
// `http://apple.com/ns/ical/`.
pub const NS_DAV: &str = "DAV:";
pub const NS_CALDAV: &str = "urn:ietf:params:xml:ns:caldav";
pub const NS_APPLE: &str = "http://apple.com/ns/ical/";

// --- Expanded (namespace:local) names used for namespace-aware validation ---
pub const DAV_PROP_FIND: &str = "DAV:propfind";
pub const DAV_PROP: &str = "DAV:prop";
pub const DAV_ALLPROP: &str = "DAV:allprop";
pub const DAV_PROPNAME: &str = "DAV:propname";
pub const DAV_HREF: &str = "DAV:href";
pub const DAV_SYNC_TOKEN: &str = "DAV:sync-token";
pub const DAV_SYNC_LEVEL: &str = "DAV:sync-level";
pub const CALDAV_CALENDAR_QUERY: &str = "urn:ietf:params:xml:ns:caldav:calendar-query";
pub const CALDAV_CALENDAR_MULTIGET: &str = "urn:ietf:params:xml:ns:caldav:calendar-multiget";
pub const CALDAV_SYNC_COLLECTION: &str = "urn:ietf:params:xml:ns:caldav:sync-collection";
pub const CALDAV_FILTER: &str = "urn:ietf:params:xml:ns:caldav:filter";
pub const CALDAV_COMP_FILTER: &str = "urn:ietf:params:xml:ns:caldav:comp-filter";
pub const CALDAV_TIME_RANGE: &str = "urn:ietf:params:xml:ns:caldav:time-range";

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

/// The requested property set of a PROPFIND, resolved to expanded names.
///
/// `Prop` carries the exact expanded names the client asked for (in request
/// order). `AllProp` is the RFC 4918 default (an empty body or an explicit
/// `D:allprop`). `PropName` requests the names of the supported properties.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PropfindMode {
    Prop(Vec<String>),
    AllProp,
    PropName,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CalendarQuery {
    pub start_utc: i64,
    pub end_utc: i64,
    /// Expanded names of the requested `D:prop` set. Empty means "all
    /// supported properties" (no `D:prop` element was present).
    pub props: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CalendarMultiget {
    pub hrefs: Vec<String>,
    pub props: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncCollection {
    /// Opaque token from the client; `None` (or empty) requests an initial
    /// snapshot.
    pub sync_token: Option<String>,
    /// `sync-level` must be 1 for calendar sync; other values are rejected.
    pub sync_level: u32,
    pub props: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CalendarReport {
    Query(CalendarQuery),
    Multiget(CalendarMultiget),
    SyncCollection(SyncCollection),
}

fn name_to_str(name: &[u8]) -> &str {
    std::str::from_utf8(name).unwrap_or("")
}

/// Tracks in-scope `xmlns` declarations so element names can be resolved to
/// their expanded (namespace:local) form.
#[derive(Default)]
struct Ns {
    map: HashMap<String, String>,
}

impl Ns {
    fn update(
        &mut self,
        attrs: quick_xml::events::attributes::Attributes,
    ) -> Result<(), QueryError> {
        for attr_result in attrs {
            let Ok(attr) = attr_result else {
                return Err(QueryError::MalformedXml);
            };
            if let Some(binding) = attr.key.as_namespace_binding() {
                let value = attr
                    .unescape_value()
                    .map(|v| v.into_owned())
                    .unwrap_or_default();
                let prefix = match binding {
                    PrefixDeclaration::Default => String::new(),
                    PrefixDeclaration::Named(name) => name_to_str(name).to_owned(),
                };
                self.map.insert(prefix, value);
            }
        }
        Ok(())
    }

    fn expanded(&self, name: QName) -> String {
        let prefix_str = name
            .prefix()
            .map(|p| name_to_str(p.as_ref()).to_owned())
            .unwrap_or_default();
        let local = name.local_name();
        let local_str = name_to_str(local.as_ref());
        let uri = self.map.get(&prefix_str).cloned().unwrap_or_default();
        // The DAV namespace URI is literally `DAV:` and the Apple namespace is
        // `http://apple.com/ns/ical/` — both already carry a trailing separator,
        // so do not add another. The CalDAV namespace does not, so it needs one.
        if uri.ends_with(':') || uri.ends_with('/') {
            format!("{uri}{local_str}")
        } else {
            format!("{uri}:{local_str}")
        }
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

/// Parse a PROPFIND body into its requested property mode.
///
/// An empty body is the RFC 4918 default and means `AllProp`. The root must be
/// `DAV:propfind`; its first child selects the mode (`D:prop`, `D:allprop`, or
/// `D:propname`). For `D:prop`, the expanded names of the requested properties
/// are captured in request order.
pub fn parse_propfind(body: &[u8]) -> Result<PropfindMode, QueryError> {
    if body.is_empty() {
        return Ok(PropfindMode::AllProp);
    }
    let text = std::str::from_utf8(body).map_err(|_| QueryError::MalformedXml)?;
    let mut reader = Reader::from_str(text);
    reader.config_mut().trim_text(true);

    let mut buf = Vec::new();
    let mut ns = Ns::default();
    let mut root_seen = false;
    let mut mode: Option<PropfindMode> = None;
    let mut in_prop = false;
    let mut prop_names: Vec<String> = Vec::new();
    let mut depth: u32 = 0;

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref start)) => {
                ns.update(start.attributes())?;
                let expanded = ns.expanded(start.name());
                if !root_seen {
                    if expanded != DAV_PROP_FIND {
                        return Err(QueryError::UnsupportedRoot);
                    }
                    root_seen = true;
                    depth = 1;
                    continue;
                }
                depth += 1;
                if mode.is_none() {
                    match expanded.as_str() {
                        DAV_PROP => {
                            mode = Some(PropfindMode::Prop(Vec::new()));
                            in_prop = true;
                        }
                        DAV_ALLPROP => mode = Some(PropfindMode::AllProp),
                        DAV_PROPNAME => mode = Some(PropfindMode::PropName),
                        _ => return Err(QueryError::MalformedXml),
                    }
                } else if in_prop && depth == 2 {
                    prop_names.push(expanded);
                }
            }
            Ok(Event::Empty(ref empty)) => {
                ns.update(empty.attributes())?;
                let expanded = ns.expanded(empty.name());
                if !root_seen {
                    if expanded != DAV_PROP_FIND {
                        return Err(QueryError::UnsupportedRoot);
                    }
                    root_seen = true;
                    continue;
                }
                if mode.is_none() {
                    match expanded.as_str() {
                        DAV_PROP => mode = Some(PropfindMode::Prop(Vec::new())),
                        DAV_ALLPROP => mode = Some(PropfindMode::AllProp),
                        DAV_PROPNAME => mode = Some(PropfindMode::PropName),
                        _ => return Err(QueryError::MalformedXml),
                    }
                } else if in_prop && depth == 2 {
                    prop_names.push(expanded);
                }
            }
            Ok(Event::End(_)) => {
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

    match mode {
        Some(PropfindMode::Prop(_)) => Ok(PropfindMode::Prop(prop_names)),
        other => other.ok_or(QueryError::MalformedXml),
    }
}

/// Extract the requested `D:prop` expanded names from a report body.
///
/// Returns an empty `Vec` when no `D:prop` element is present, which the
/// caller treats as "all supported properties".
fn parse_prop_set(body: &[u8]) -> Vec<String> {
    let Ok(text) = std::str::from_utf8(body) else {
        return Vec::new();
    };
    let mut reader = Reader::from_str(text);
    reader.config_mut().trim_text(true);

    let mut buf = Vec::new();
    let mut ns = Ns::default();
    let mut prop_depth: Option<u32> = None;
    let mut depth: u32 = 0;
    let mut props: Vec<String> = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref start)) => {
                let _ = ns.update(start.attributes());
                let expanded = ns.expanded(start.name());
                depth += 1;
                if expanded == DAV_PROP {
                    prop_depth = Some(depth);
                } else if prop_depth == Some(depth - 1) {
                    // Direct child of D:prop.
                    props.push(expanded);
                }
            }
            Ok(Event::Empty(ref empty)) => {
                let _ = ns.update(empty.attributes());
                let expanded = ns.expanded(empty.name());
                if expanded == DAV_PROP {
                    // Self-closing D:prop requests no properties.
                } else if prop_depth == Some(depth) {
                    // Direct child of D:prop (Empty elements do not change depth).
                    props.push(expanded);
                }
            }
            Ok(Event::End(_)) => {
                depth = depth.saturating_sub(1);
                if prop_depth.is_some_and(|pd| depth < pd) {
                    prop_depth = None;
                }
            }
            Ok(Event::Eof) => break,
            Ok(_) => {}
            Err(_) => break,
        }
        buf.clear();
    }
    props
}

pub fn parse_calendar_query(body: &[u8]) -> Result<CalendarQuery, QueryError> {
    if body.len() > MAX_BODY_BYTES {
        return Err(QueryError::MalformedXml);
    }
    let text = std::str::from_utf8(body).map_err(|_| QueryError::MalformedXml)?;
    let mut reader = Reader::from_str(text);
    reader.config_mut().trim_text(true);

    let mut buf = Vec::new();
    let mut ns = Ns::default();
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
            let local = attr.key.local_name();
            let k = name_to_str(local.as_ref());
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
                ns.update(start.attributes())?;
                let expanded = ns.expanded(start.name());
                if !root_seen {
                    if expanded != CALDAV_CALENDAR_QUERY {
                        return Err(QueryError::UnsupportedRoot);
                    }
                    root_seen = true;
                    depth = 1;
                    continue;
                }
                depth += 1;
                if in_filter {
                    filter_depth += 1;
                    if expanded == CALDAV_TIME_RANGE {
                        found_time_range = true;
                        let (s, e) = extract_time_range(start.attributes())?;
                        start_utc = start_utc.or(s);
                        end_utc = end_utc.or(e);
                    } else if expanded != CALDAV_FILTER && expanded != CALDAV_COMP_FILTER {
                        unsupported_in_filter = true;
                    }
                } else if expanded == CALDAV_FILTER {
                    in_filter = true;
                    filter_depth = 1;
                }
            }
            Ok(Event::Empty(ref empty)) => {
                ns.update(empty.attributes())?;
                let expanded = ns.expanded(empty.name());
                if !root_seen {
                    if expanded != CALDAV_CALENDAR_QUERY {
                        return Err(QueryError::UnsupportedRoot);
                    }
                    root_seen = true;
                    continue;
                }
                if in_filter {
                    if expanded == CALDAV_TIME_RANGE {
                        found_time_range = true;
                        let (s, e) = extract_time_range(empty.attributes())?;
                        start_utc = start_utc.or(s);
                        end_utc = end_utc.or(e);
                    } else if expanded != CALDAV_FILTER && expanded != CALDAV_COMP_FILTER {
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
        props: parse_prop_set(body),
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
    let mut ns = Ns::default();
    let mut root_seen = false;
    let mut depth: u32 = 0;
    let mut hrefs: Vec<String> = Vec::new();
    let mut capturing_href = false;
    let mut current_href = String::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref start)) => {
                ns.update(start.attributes())?;
                let expanded = ns.expanded(start.name());
                if !root_seen {
                    if expanded != CALDAV_CALENDAR_MULTIGET {
                        return Err(QueryError::UnsupportedRoot);
                    }
                    root_seen = true;
                    depth = 1;
                    continue;
                }
                depth += 1;
                if depth == 2 && expanded == DAV_HREF {
                    capturing_href = true;
                    current_href.clear();
                }
            }
            Ok(Event::Empty(ref empty)) => {
                ns.update(empty.attributes())?;
                let expanded = ns.expanded(empty.name());
                if !root_seen {
                    if expanded != CALDAV_CALENDAR_MULTIGET {
                        return Err(QueryError::UnsupportedRoot);
                    }
                    root_seen = true;
                    continue;
                }
                depth += 1;
                if depth == 2 && expanded == DAV_HREF {
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
    Ok(CalendarMultiget {
        hrefs,
        props: parse_prop_set(body),
    })
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
    let mut ns = Ns::default();
    let mut root_seen = false;
    let mut depth: u32 = 0;
    let mut sync_token: Option<String> = None;
    let mut sync_level: Option<u32> = None;
    let mut capturing: Option<&'static str> = None;
    let mut current = String::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref start)) => {
                ns.update(start.attributes())?;
                let expanded = ns.expanded(start.name());
                if !root_seen {
                    if expanded != CALDAV_SYNC_COLLECTION {
                        return Err(QueryError::UnsupportedRoot);
                    }
                    root_seen = true;
                    depth = 1;
                    continue;
                }
                depth += 1;
                if depth == 2 {
                    if expanded == DAV_SYNC_TOKEN {
                        capturing = Some("token");
                        current.clear();
                    } else if expanded == DAV_SYNC_LEVEL {
                        capturing = Some("level");
                        current.clear();
                    }
                }
            }
            Ok(Event::Empty(ref empty)) => {
                ns.update(empty.attributes())?;
                let expanded = ns.expanded(empty.name());
                if !root_seen {
                    if expanded != CALDAV_SYNC_COLLECTION {
                        return Err(QueryError::UnsupportedRoot);
                    }
                    root_seen = true;
                    continue;
                }
                depth += 1;
                if depth == 2 && expanded == DAV_SYNC_TOKEN {
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
        props: parse_prop_set(body),
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
        CALDAV_CALENDAR_QUERY => Ok(CalendarReport::Query(parse_calendar_query(body)?)),
        CALDAV_CALENDAR_MULTIGET => Ok(CalendarReport::Multiget(parse_calendar_multiget(body)?)),
        CALDAV_SYNC_COLLECTION => Ok(CalendarReport::SyncCollection(parse_sync_collection(body)?)),
        _ => Err(QueryError::UnsupportedRoot),
    }
}

/// Read the first element's expanded name in an XML document, if any.
fn peek_root(text: &str) -> Option<String> {
    let mut reader = Reader::from_str(text);
    reader.config_mut().trim_text(true);
    let mut buf = Vec::new();
    let mut ns = Ns::default();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref start)) => {
                let _ = ns.update(start.attributes());
                return Some(ns.expanded(start.name()));
            }
            Ok(Event::Empty(ref empty)) => {
                let _ = ns.update(empty.attributes());
                return Some(ns.expanded(empty.name()));
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

    const CALDAV: &str = "urn:ietf:params:xml:ns:caldav";

    fn valid_query() -> String {
        format!(
            r#"<?xml version="1.0" encoding="utf-8" ?>
<C:calendar-query xmlns:D="DAV:" xmlns:C="{CALDAV}">
  <D:prop>
    <D:getetag/>
    <D:getcontenttype/>
    <C:calendar-data/>
  </D:prop>
  <C:filter>
    <C:time-range start="20260101T000000Z" end="20260131T235959Z"/>
  </C:filter>
</C:calendar-query>"#
        )
    }

    fn valid_multiget() -> String {
        format!(
            r#"<?xml version="1.0" encoding="utf-8" ?>
<C:calendar-multiget xmlns:D="DAV:" xmlns:C="{CALDAV}">
  <D:prop>
    <D:getetag/>
    <D:getcontenttype/>
    <C:calendar-data/>
  </D:prop>
  <D:href>/dav/calendars/p/1/aaa.ics</D:href>
  <D:href>/dav/calendars/p/1/bbb.ics</D:href>
</C:calendar-multiget>"#
        )
    }

    // --- PROPFIND parsing ---

    #[test]
    fn propfind_empty_body_defaults_to_allprop() {
        assert_eq!(parse_propfind(b"").unwrap(), PropfindMode::AllProp);
    }

    #[test]
    fn propfind_explicit_allprop() {
        let body = r#"<D:propfind xmlns:D="DAV:"><D:allprop/></D:propfind>"#;
        assert_eq!(
            parse_propfind(body.as_bytes()).unwrap(),
            PropfindMode::AllProp
        );
    }

    #[test]
    fn propfind_propname() {
        let body = r#"<D:propfind xmlns:D="DAV:"><D:propname/></D:propfind>"#;
        assert_eq!(
            parse_propfind(body.as_bytes()).unwrap(),
            PropfindMode::PropName
        );
    }

    #[test]
    fn propfind_explicit_prop_captures_expanded_names() {
        let body = format!(
            r#"<D:propfind xmlns:D="DAV:" xmlns:C="{CALDAV}" xmlns:A="http://apple.com/ns/ical/">
  <D:prop>
    <D:displayname/>
    <A:calendar-color/>
  </D:prop>
</D:propfind>"#
        );
        let mode = parse_propfind(body.as_bytes()).unwrap();
        assert_eq!(
            mode,
            PropfindMode::Prop(vec![
                "DAV:displayname".to_owned(),
                "http://apple.com/ns/ical/calendar-color".to_owned()
            ])
        );
    }

    #[test]
    fn propfind_rejects_wrong_root_namespace() {
        // Same local name, wrong namespace: must be rejected.
        let body = r#"<C:propfind xmlns:C="urn:ietf:params:xml:ns:caldav"><D:allprop xmlns:D="DAV:"/></C:propfind>"#;
        assert_eq!(
            parse_propfind(body.as_bytes()).unwrap_err(),
            QueryError::UnsupportedRoot
        );
    }

    #[test]
    fn propfind_rejects_malformed_xml() {
        let body = r#"<D:propfind xmlns:D="DAV:"><D:prop>"#;
        assert_eq!(
            parse_propfind(body.as_bytes()).unwrap_err(),
            QueryError::MalformedXml
        );
    }

    // --- calendar-query ---

    #[test]
    fn parses_valid_time_range() {
        let q = parse_calendar_query(valid_query().as_bytes()).unwrap();
        assert_eq!(q.start_utc, 1767225600);
        assert_eq!(q.end_utc, 1769903999);
        assert_eq!(
            q.props,
            vec![
                "DAV:getetag".to_owned(),
                "DAV:getcontenttype".to_owned(),
                "urn:ietf:params:xml:ns:caldav:calendar-data".to_owned()
            ]
        );
    }

    #[test]
    fn query_rejects_calendarserver_namespace_root() {
        // The legacy calendarserver namespace must NOT be accepted as CalDAV.
        let body = r#"<C:calendar-query xmlns:C="urn:ietf:params:xml:ns:calendarserver/"><C:filter><C:time-range start="20260101T000000Z" end="20260131T235959Z"/></C:filter></C:calendar-query>"#;
        assert_eq!(
            parse_calendar_query(body.as_bytes()).unwrap_err(),
            QueryError::UnsupportedRoot
        );
    }

    #[test]
    fn query_rejects_comp_filter_in_wrong_namespace() {
        let body = format!(
            r#"<C:calendar-query xmlns:C="{CALDAV}"><C:filter><X:comp-filter xmlns:X="urn:ietf:params:xml:ns:calendarserver/" name="VEVENT"><C:time-range start="20260101T000000Z" end="20260131T235959Z"/></X:comp-filter></C:filter></C:calendar-query>"#
        );
        assert_eq!(
            parse_calendar_query(body.as_bytes()).unwrap_err(),
            QueryError::UnsupportedFilter
        );
    }

    #[test]
    fn query_accepts_comp_filter_tree_with_time_range() {
        let body = format!(
            r#"<C:calendar-query xmlns:C="{CALDAV}">
  <C:filter>
    <C:comp-filter name="VCALENDAR">
      <C:comp-filter name="VEVENT">
        <C:time-range start="20260101T000000Z" end="20260131T235959Z"/>
      </C:comp-filter>
    </C:comp-filter>
  </C:filter>
</C:calendar-query>"#
        );
        let q = parse_calendar_query(body.as_bytes()).unwrap();
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
        let body = format!(
            r#"<C:calendar-multiget xmlns:C="{CALDAV}"><D:href xmlns:D="DAV:">/x/</D:href></C:calendar-multiget>"#
        );
        assert_eq!(
            parse_calendar_query(body.as_bytes()).unwrap_err(),
            QueryError::UnsupportedRoot
        );
    }

    #[test]
    fn rejects_missing_filter() {
        let body = format!(
            r#"<C:calendar-query xmlns:C="{CALDAV}"><D:prop xmlns:D="DAV:"><D:getetag/></D:prop></C:calendar-query>"#
        );
        assert_eq!(
            parse_calendar_query(body.as_bytes()).unwrap_err(),
            QueryError::MissingFilter
        );
    }

    #[test]
    fn rejects_unsupported_filter_type() {
        let body = format!(
            r#"<C:calendar-query xmlns:C="{CALDAV}"><C:filter><C:comp-filter name="VEVENT"><C:prop-filter name="SUMMARY"><C:is-text matches="anywhere">test</C:is-text></C:prop-filter></C:comp-filter></C:filter></C:calendar-query>"#
        );
        assert_eq!(
            parse_calendar_query(body.as_bytes()).unwrap_err(),
            QueryError::UnsupportedFilter
        );
    }

    #[test]
    fn rejects_invalid_start_before_end() {
        let body = format!(
            r#"<C:calendar-query xmlns:C="{CALDAV}"><C:filter><C:time-range start="20260131T000000Z" end="20260101T000000Z"/></C:filter></C:calendar-query>"#
        );
        assert_eq!(
            parse_calendar_query(body.as_bytes()).unwrap_err(),
            QueryError::InvalidRange
        );
    }

    #[test]
    fn rejects_equal_start_and_end() {
        let body = format!(
            r#"<C:calendar-query xmlns:C="{CALDAV}"><C:filter><C:time-range start="20260101T000000Z" end="20260101T000000Z"/></C:filter></C:calendar-query>"#
        );
        assert_eq!(
            parse_calendar_query(body.as_bytes()).unwrap_err(),
            QueryError::InvalidRange
        );
    }

    #[test]
    fn rejects_range_too_large() {
        let body = format!(
            r#"<C:calendar-query xmlns:C="{CALDAV}"><C:filter><C:time-range start="20200101T000000Z" end="20260101T000000Z"/></C:filter></C:calendar-query>"#
        );
        assert_eq!(
            parse_calendar_query(body.as_bytes()).unwrap_err(),
            QueryError::RangeTooLarge
        );
    }

    #[test]
    fn rejects_invalid_date_format() {
        let body = format!(
            r#"<C:calendar-query xmlns:C="{CALDAV}"><C:filter><C:time-range start="notadate" end="20260101T000000Z"/></C:filter></C:calendar-query>"#
        );
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
        let body = format!(
            r#"<C:calendar-query xmlns:C="{CALDAV}"><C:filter><C:time-range start="20260101" end="20260131T235959Z"/></C:filter></C:calendar-query>"#
        );
        let q = parse_calendar_query(body.as_bytes()).unwrap();
        assert_eq!(q.start_utc, 1767225600);
    }

    #[test]
    fn rejects_malformed_xml() {
        let body = format!(r#"<C:calendar-query xmlns:C="{CALDAV}"><C:filter>"#);
        assert_eq!(
            parse_calendar_query(body.as_bytes()).unwrap_err(),
            QueryError::MalformedXml
        );
    }

    // --- calendar-multiget ---

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
        let body = format!(
            r#"<C:calendar-multiget xmlns:D="DAV:" xmlns:C="{CALDAV}"><D:href>/dav/calendars/p/1/aaa.ics</D:href></C:calendar-multiget>"#
        );
        let m = parse_calendar_multiget(body.as_bytes()).unwrap();
        assert_eq!(m.hrefs, vec!["/dav/calendars/p/1/aaa.ics".to_owned()]);
    }

    #[test]
    fn rejects_multiget_missing_href() {
        let body = format!(
            r#"<C:calendar-multiget xmlns:D="DAV:" xmlns:C="{CALDAV}"><D:prop><D:getetag/></D:prop></C:calendar-multiget>"#
        );
        assert_eq!(
            parse_calendar_multiget(body.as_bytes()).unwrap_err(),
            QueryError::MissingHref
        );
    }

    #[test]
    fn rejects_multiget_wrong_root() {
        let body = format!(
            r#"<C:calendar-query xmlns:D="DAV:" xmlns:C="{CALDAV}"><D:href>/x/</D:href></C:calendar-query>"#
        );
        assert_eq!(
            parse_calendar_multiget(body.as_bytes()).unwrap_err(),
            QueryError::UnsupportedRoot
        );
    }

    #[test]
    fn rejects_multiget_malformed_xml() {
        let body = format!(r#"<C:calendar-multiget xmlns:D="DAV:" xmlns:C="{CALDAV}"><D:href>"#);
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
            r#"<C:calendar-multiget xmlns:D="DAV:" xmlns:C="{CALDAV}">{hrefs}</C:calendar-multiget>"#
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
            r#"<C:calendar-multiget xmlns:D="DAV:" xmlns:C="{CALDAV}">{hrefs}</C:calendar-multiget>"#
        );
        let m = parse_calendar_multiget(body.as_bytes()).unwrap();
        assert_eq!(m.hrefs.len(), MAX_MULTIGET_HREFS);
    }

    // --- report dispatch ---

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
        let body = format!(
            r#"<C:unknown-report xmlns:D="DAV:" xmlns:C="{CALDAV}"><D:href>/x/</D:href></C:unknown-report>"#
        );
        assert_eq!(
            parse_calendar_report(body.as_bytes()).unwrap_err(),
            QueryError::UnsupportedRoot
        );
    }

    #[test]
    fn report_rejects_calendarserver_namespace_root() {
        let body = r#"<C:calendar-query xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:calendarserver/"><C:filter><C:time-range start="20260101T000000Z" end="20260131T235959Z"/></C:filter></C:calendar-query>"#;
        assert_eq!(
            parse_calendar_report(body.as_bytes()).unwrap_err(),
            QueryError::UnsupportedRoot
        );
    }

    #[test]
    fn report_dispatches_sync_collection() {
        let body = format!(
            r#"<C:sync-collection xmlns:D="DAV:" xmlns:C="{CALDAV}"><D:sync-token></D:sync-token><D:sync-level>1</D:sync-level><D:prop><D:getetag/></D:prop></C:sync-collection>"#
        );
        let report = parse_calendar_report(body.as_bytes()).unwrap();
        match report {
            CalendarReport::SyncCollection(sync) => {
                assert!(sync.sync_token.is_none());
                assert_eq!(sync.sync_level, 1);
            }
            _ => panic!("expected sync-collection"),
        }
    }

    // --- sync-collection ---

    #[test]
    fn sync_collection_parses_token_and_level() {
        let body = format!(
            r#"<C:sync-collection xmlns:D="DAV:" xmlns:C="{CALDAV}"><D:sync-token>opaque-token-123</D:sync-token><D:sync-level>1</D:sync-level></C:sync-collection>"#
        );
        let sync = parse_sync_collection(body.as_bytes()).unwrap();
        assert_eq!(sync.sync_token.as_deref(), Some("opaque-token-123"));
        assert_eq!(sync.sync_level, 1);
    }

    #[test]
    fn sync_collection_rejects_bad_level() {
        let body = format!(
            r#"<C:sync-collection xmlns:D="DAV:" xmlns:C="{CALDAV}"><D:sync-token></D:sync-token><D:sync-level>2</D:sync-level></C:sync-collection>"#
        );
        assert_eq!(
            parse_sync_collection(body.as_bytes()).unwrap_err(),
            QueryError::BadSyncLevel
        );
    }

    #[test]
    fn sync_collection_rejects_missing_level() {
        let body = format!(
            r#"<C:sync-collection xmlns:D="DAV:" xmlns:C="{CALDAV}"><D:sync-token></D:sync-token></C:sync-collection>"#
        );
        assert_eq!(
            parse_sync_collection(body.as_bytes()).unwrap_err(),
            QueryError::BadSyncLevel
        );
    }

    #[test]
    fn sync_collection_rejects_wrong_root() {
        let body = format!(
            r#"<C:calendar-query xmlns:D="DAV:" xmlns:C="{CALDAV}"><D:sync-token></D:sync-token><D:sync-level>1</D:sync-level></C:calendar-query>"#
        );
        assert_eq!(
            parse_sync_collection(body.as_bytes()).unwrap_err(),
            QueryError::UnsupportedRoot
        );
    }
}
