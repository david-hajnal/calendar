use std::collections::HashMap;
use std::fmt::{self, Display, Formatter};

use chrono::{NaiveDateTime, TimeZone, Utc};
use quick_xml::NsReader;
use quick_xml::events::Event;
use quick_xml::name::ResolveResult;

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
pub const CALDAV_SYNC_COLLECTION: &str = "DAV:sync-collection";
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
    UnsupportedCalendarData,
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
            Self::UnsupportedCalendarData => write!(f, "unsupported calendar-data transformation"),
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
    Prop(Vec<DavName>),
    AllPropInclude(Vec<DavName>),
    AllProp,
    PropName,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CalendarQuery {
    pub start_utc: i64,
    pub end_utc: i64,
    pub has_time_range: bool,
    /// Expanded names of the requested `D:prop` set. Empty means "all
    /// supported properties" (no `D:prop` element was present).
    pub props: Option<PropfindMode>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CalendarMultiget {
    pub hrefs: Vec<String>,
    pub props: Option<PropfindMode>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncCollection {
    /// Opaque token from the client; `None` (or empty) requests an initial
    /// snapshot.
    pub sync_token: Option<String>,
    /// `sync-level` must be 1 for calendar sync; other values are rejected.
    pub sync_level: u32,
    pub limit: Option<usize>,
    pub props: Option<PropfindMode>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CalendarReport {
    Query(CalendarQuery),
    Multiget(CalendarMultiget),
    SyncCollection(SyncCollection),
}

/// XML expanded name. Namespace identity never depends on delimiter splitting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DavName {
    pub namespace: String,
    pub local: String,
}

impl DavName {
    pub fn new(namespace: &str, local: &str) -> Self {
        Self {
            namespace: namespace.into(),
            local: local.into(),
        }
    }
}

fn internal_name_parts(value: &str) -> (&str, &str) {
    for (namespace, prefix) in [
        (NS_DAV, NS_DAV),
        (NS_CALDAV, "urn:ietf:params:xml:ns:caldav:"),
        (NS_APPLE, NS_APPLE),
    ] {
        if let Some(local) = value.strip_prefix(prefix) {
            return (namespace, local);
        }
    }
    if let Some(rest) = value.strip_prefix('{')
        && let Some((namespace, local)) = rest.split_once('}')
    {
        return (namespace, local);
    }
    ("", value)
}
impl From<&str> for DavName {
    fn from(value: &str) -> Self {
        let (namespace, local) = internal_name_parts(value);
        Self::new(namespace, local)
    }
}
impl From<String> for DavName {
    fn from(value: String) -> Self {
        Self::from(value.as_str())
    }
}
impl PartialEq<String> for DavName {
    fn eq(&self, value: &String) -> bool {
        <Self as PartialEq<&str>>::eq(self, &value.as_str())
    }
}
impl PartialEq<&str> for DavName {
    fn eq(&self, value: &&str) -> bool {
        let (namespace, local) = internal_name_parts(value);
        self.namespace == namespace && self.local == local
    }
}

#[derive(Debug)]
struct Element {
    name: DavName,
    attrs: HashMap<String, String>,
    children: Vec<Element>,
    text: String,
}
impl Element {
    fn is(&self, name: &str) -> bool {
        self.name == name
    }
    fn children_named(&self, name: &str) -> Vec<&Element> {
        self.children
            .iter()
            .filter(|child| child.is(name))
            .collect()
    }
    fn only_child(&self, name: &str) -> Result<&Element, QueryError> {
        let children = self.children_named(name);
        if children.len() != 1 {
            return Err(QueryError::MalformedXml);
        }
        Ok(children[0])
    }
    fn empty(&self) -> bool {
        self.children.is_empty() && self.text.trim().is_empty()
    }
}

fn xml_character(character: char) -> bool {
    matches!(character as u32, 0x9 | 0xA | 0xD | 0x20..=0xD7FF | 0xE000..=0xFFFD | 0x10000..=0x10FFFF)
}
fn ncname(value: &str) -> bool {
    let start = |c: char| matches!(c as u32, 0x41..=0x5A | 0x5F | 0x61..=0x7A | 0xC0..=0xD6 | 0xD8..=0xF6 | 0xF8..=0x2FF | 0x370..=0x37D | 0x37F..=0x1FFF | 0x200C..=0x200D | 0x2070..=0x218F | 0x2C00..=0x2FEF | 0x3001..=0xD7FF | 0xF900..=0xFDCF | 0xFDF0..=0xFFFD | 0x10000..=0xEFFFF);
    let mut chars = value.chars();
    chars.next().is_some_and(start) && chars.all(|c| start(c) || matches!(c as u32, 0x2D | 0x2E | 0x30..=0x39 | 0xB7 | 0x300..=0x36F | 0x203F..=0x2040))
}
fn qualified_name(value: &[u8]) -> Result<(), QueryError> {
    let value = std::str::from_utf8(value).map_err(|_| QueryError::MalformedXml)?;
    let parts: Vec<_> = value.split(':').collect();
    if parts.len() > 2 || parts.iter().any(|part| !ncname(part)) {
        return Err(QueryError::MalformedXml);
    }
    Ok(())
}
fn namespace_value(value: &[u8]) -> Result<String, QueryError> {
    let value = std::str::from_utf8(value).map_err(|_| QueryError::MalformedXml)?;
    let value = quick_xml::escape::unescape(value).map_err(|_| QueryError::MalformedXml)?;
    if !value.chars().all(xml_character) {
        return Err(QueryError::MalformedXml);
    }
    Ok(value.into_owned())
}

/// Parse once with scoped namespaces, bounded nesting, and a single document root.
fn document(body: &[u8]) -> Result<Element, QueryError> {
    if body.len() > MAX_BODY_BYTES {
        return Err(QueryError::MalformedXml);
    }
    let text = std::str::from_utf8(body).map_err(|_| QueryError::MalformedXml)?;
    if !text.chars().all(xml_character) {
        return Err(QueryError::MalformedXml);
    }
    let mut reader = NsReader::from_str(text);
    let mut stack: Vec<Element> = Vec::new();
    let mut root = None;
    let mut declaration = false;
    loop {
        let (resolved, event) = reader
            .read_resolved_event()
            .map_err(|_| QueryError::MalformedXml)?;
        match event {
            Event::Start(ref start) | Event::Empty(ref start) => {
                if stack.len() >= 64 || (stack.is_empty() && root.is_some()) {
                    return Err(QueryError::MalformedXml);
                }
                let namespace = match resolved {
                    ResolveResult::Bound(ns) => namespace_value(ns.as_ref())?,
                    ResolveResult::Unbound => String::new(),
                    ResolveResult::Unknown(_) => return Err(QueryError::MalformedXml),
                };
                let local = std::str::from_utf8(start.local_name().as_ref())
                    .map_err(|_| QueryError::MalformedXml)?
                    .to_owned();
                qualified_name(start.name().as_ref())?;
                if !ncname(&local) {
                    return Err(QueryError::MalformedXml);
                }
                let mut attrs = HashMap::new();
                for attr in start.attributes() {
                    let attr = attr.map_err(|_| QueryError::MalformedXml)?;
                    qualified_name(attr.key.as_ref())?;
                    let value = attr
                        .unescape_value()
                        .map_err(|_| QueryError::MalformedXml)?
                        .into_owned();
                    if !value.chars().all(xml_character) {
                        return Err(QueryError::MalformedXml);
                    }
                    if attr.key.as_namespace_binding().is_some() {
                        continue;
                    }
                    let (namespace, name) = reader.resolve_attribute(attr.key);
                    if matches!(namespace, ResolveResult::Unknown(_)) {
                        return Err(QueryError::MalformedXml);
                    }
                    let local =
                        std::str::from_utf8(name.as_ref()).map_err(|_| QueryError::MalformedXml)?;
                    let key = match namespace {
                        ResolveResult::Bound(ns) => {
                            format!("{{{}}}{local}", namespace_value(ns.as_ref())?)
                        }
                        _ => local.into(),
                    };
                    if attrs.insert(key, value).is_some() {
                        return Err(QueryError::MalformedXml);
                    }
                }
                let element = Element {
                    name: DavName { namespace, local },
                    attrs,
                    children: Vec::new(),
                    text: String::new(),
                };
                if matches!(event, Event::Empty(_)) {
                    if let Some(parent) = stack.last_mut() {
                        parent.children.push(element);
                    } else {
                        root = Some(element);
                    }
                } else {
                    stack.push(element);
                }
            }
            Event::End(_) => {
                let element = stack.pop().ok_or(QueryError::MalformedXml)?;
                if let Some(parent) = stack.last_mut() {
                    parent.children.push(element);
                } else {
                    root = Some(element);
                }
            }
            Event::Text(text) => {
                let value = text.unescape().map_err(|_| QueryError::MalformedXml)?;
                if !value.chars().all(xml_character) {
                    return Err(QueryError::MalformedXml);
                }
                if let Some(parent) = stack.last_mut() {
                    parent.text.push_str(&value);
                } else if !value.trim().is_empty() {
                    return Err(QueryError::MalformedXml);
                }
            }
            Event::CData(text) => {
                let parent = stack.last_mut().ok_or(QueryError::MalformedXml)?;
                parent.text.push_str(
                    std::str::from_utf8(text.as_ref()).map_err(|_| QueryError::MalformedXml)?,
                );
            }
            Event::Decl(_) => {
                if declaration || root.is_some() || !stack.is_empty() {
                    return Err(QueryError::MalformedXml);
                }
                declaration = true;
            }
            Event::DocType(_) => return Err(QueryError::MalformedXml),
            Event::Eof => break,
            Event::Comment(_) | Event::PI(_) => {}
        }
    }
    if !stack.is_empty() {
        return Err(QueryError::MalformedXml);
    }
    root.ok_or(QueryError::MalformedXml)
}

fn parse_ical_datetime(value: &str) -> Result<i64, QueryError> {
    if value.len() != 16 || !value.ends_with('Z') {
        return Err(QueryError::InvalidDateTime);
    }
    let naive = NaiveDateTime::parse_from_str(value, "%Y%m%dT%H%M%SZ")
        .map_err(|_| QueryError::InvalidDateTime)?;
    Ok(Utc.from_utc_datetime(&naive).timestamp())
}

fn property_names(prop: &Element, report: bool) -> Result<Vec<DavName>, QueryError> {
    if !prop.text.trim().is_empty() {
        return Err(QueryError::MalformedXml);
    }
    for child in &prop.children {
        if report
            && child.is("urn:ietf:params:xml:ns:caldav:calendar-data")
            && (!child.empty() || !child.attrs.is_empty())
        {
            // Only full text/calendar version 2.0 is implemented.
            if !child.empty()
                || child.attrs.iter().any(|(name, value)| {
                    !matches!(
                        (name.as_str(), value.as_str()),
                        ("content-type", "text/calendar") | ("version", "2.0")
                    )
                })
            {
                return Err(QueryError::UnsupportedCalendarData);
            }
        }
    }
    Ok(prop
        .children
        .iter()
        .map(|child| child.name.clone())
        .collect())
}

pub fn parse_propfind(body: &[u8]) -> Result<PropfindMode, QueryError> {
    if body.is_empty() {
        return Ok(PropfindMode::AllProp);
    }
    let root = document(body)?;
    if !root.is(DAV_PROP_FIND) {
        return Err(QueryError::UnsupportedRoot);
    }
    if !root.text.trim().is_empty() {
        return Err(QueryError::MalformedXml);
    }
    let mut selector = None;
    let mut include = None;
    for child in &root.children {
        let selected = if child.is(DAV_PROP) {
            Some(PropfindMode::Prop(property_names(child, false)?))
        } else if child.is(DAV_ALLPROP) && child.empty() {
            Some(PropfindMode::AllProp)
        } else if child.is(DAV_PROPNAME) && child.empty() {
            Some(PropfindMode::PropName)
        } else if child.is("DAV:include") {
            if include.is_some() {
                return Err(QueryError::MalformedXml);
            }
            include = Some(property_names(child, false)?);
            None
        } else if child.name.namespace == NS_DAV {
            return Err(QueryError::MalformedXml);
        } else {
            None
        }; // RFC 4918 section 17 permits unrecognized extension elements.
        if let Some(selected) = selected
            && selector.replace(selected).is_some()
        {
            return Err(QueryError::MalformedXml);
        }
    }
    match (selector, include) {
        (Some(PropfindMode::AllProp), Some(names)) => Ok(PropfindMode::AllPropInclude(names)),
        (Some(mode), None) => Ok(mode),
        _ => Err(QueryError::MalformedXml),
    }
}

fn report_props(root: &Element) -> Result<Option<PropfindMode>, QueryError> {
    let mut mode = None;
    for child in &root.children {
        let selected = if child.is(DAV_PROP) {
            Some(PropfindMode::Prop(property_names(child, true)?))
        } else if !root.is("DAV:sync-collection") && child.is(DAV_ALLPROP) && child.empty() {
            Some(PropfindMode::AllProp)
        } else if !root.is("DAV:sync-collection") && child.is(DAV_PROPNAME) && child.empty() {
            Some(PropfindMode::PropName)
        } else if child.is(DAV_ALLPROP) || child.is(DAV_PROPNAME) {
            return Err(QueryError::MalformedXml);
        } else {
            None
        };
        if let Some(selected) = selected
            && mode.replace(selected).is_some()
        {
            return Err(QueryError::MalformedXml);
        }
    }
    if root.is("DAV:sync-collection") && mode.is_none() {
        return Err(QueryError::MalformedXml);
    }
    Ok(mode)
}
fn report_children(root: &Element, allowed: &[&str]) -> Result<(), QueryError> {
    if !root.text.trim().is_empty() {
        return Err(QueryError::MalformedXml);
    }
    for child in &root.children {
        if (child.name.namespace == NS_DAV || child.name.namespace == NS_CALDAV)
            && !allowed.iter().any(|name| child.is(name))
        {
            return Err(QueryError::MalformedXml);
        }
    }
    Ok(())
}

fn query_from(root: &Element) -> Result<CalendarQuery, QueryError> {
    report_children(root, &[DAV_PROP, DAV_ALLPROP, DAV_PROPNAME, CALDAV_FILTER])?;
    let filters = root.children_named(CALDAV_FILTER);
    if filters.is_empty() {
        return Err(QueryError::MissingFilter);
    }
    let filter = root.only_child(CALDAV_FILTER)?;
    if !filter.text.trim().is_empty() || filter.children.len() != 1 {
        return Err(QueryError::UnsupportedFilter);
    }
    let calendar = &filter.children[0];
    if !calendar.is(CALDAV_COMP_FILTER)
        || calendar.attrs.get("name").map(String::as_str) != Some("VCALENDAR")
        || !calendar.text.trim().is_empty()
    {
        return Err(QueryError::UnsupportedFilter);
    }
    let mut range = None;
    if !calendar.children.is_empty() {
        if calendar.children.len() != 1 {
            return Err(QueryError::UnsupportedFilter);
        }
        let event = &calendar.children[0];
        if !event.is(CALDAV_COMP_FILTER)
            || event.attrs.get("name").map(String::as_str) != Some("VEVENT")
            || !event.text.trim().is_empty()
        {
            return Err(QueryError::UnsupportedFilter);
        }
        if !event.children.is_empty() {
            if event.children.len() != 1
                || !event.children[0].is(CALDAV_TIME_RANGE)
                || !event.children[0].empty()
            {
                return Err(QueryError::UnsupportedFilter);
            }
            range = Some(&event.children[0]);
        }
    }
    let mut start = -62135596800; // RFC 4791's unbounded start.
    let mut end = 253402300799;
    if let Some(range) = range {
        if range.attrs.is_empty()
            || range
                .attrs
                .keys()
                .any(|name| name != "start" && name != "end")
        {
            return Err(QueryError::InvalidRange);
        }
        if let Some(value) = range.attrs.get("start") {
            start = parse_ical_datetime(value)?;
        }
        if let Some(value) = range.attrs.get("end") {
            end = parse_ical_datetime(value)?;
        }
        if start >= end {
            return Err(QueryError::InvalidRange);
        }
    }
    Ok(CalendarQuery {
        start_utc: start,
        end_utc: end,
        has_time_range: range.is_some(),
        props: report_props(root)?,
    })
}
fn multiget_from(root: &Element) -> Result<CalendarMultiget, QueryError> {
    report_children(root, &[DAV_PROP, DAV_ALLPROP, DAV_PROPNAME, DAV_HREF])?;
    let mut hrefs = Vec::new();
    for href in root.children_named(DAV_HREF) {
        if !href.children.is_empty() || href.text.trim().is_empty() {
            return Err(QueryError::MissingHref);
        }
        hrefs.push(href.text.trim().to_owned());
    }
    if hrefs.is_empty() {
        return Err(QueryError::MissingHref);
    }
    if hrefs.len() > MAX_MULTIGET_HREFS {
        return Err(QueryError::TooManyHrefs);
    }
    Ok(CalendarMultiget {
        hrefs,
        props: report_props(root)?,
    })
}
fn sync_from(root: &Element) -> Result<SyncCollection, QueryError> {
    report_children(
        root,
        &[DAV_PROP, DAV_SYNC_TOKEN, DAV_SYNC_LEVEL, "DAV:limit"],
    )?;
    let token = root.only_child(DAV_SYNC_TOKEN)?;
    let level = root.only_child(DAV_SYNC_LEVEL)?;
    if !token.children.is_empty() || !level.children.is_empty() {
        return Err(QueryError::MalformedXml);
    }
    if level.text.trim() != "1" {
        return Err(QueryError::BadSyncLevel);
    }
    let limits = root.children_named("DAV:limit");
    let limit = match limits.as_slice() {
        [] => None,
        [limit] => {
            let number = limit.only_child("DAV:nresults")?;
            if limit.children.len() != 1
                || !limit.text.trim().is_empty()
                || !number.children.is_empty()
            {
                return Err(QueryError::MalformedXml);
            }
            Some(
                number
                    .text
                    .trim()
                    .parse::<usize>()
                    .ok()
                    .filter(|n| *n > 0)
                    .ok_or(QueryError::MalformedXml)?,
            )
        }
        _ => return Err(QueryError::MalformedXml),
    };
    let text = token.text.trim();
    Ok(SyncCollection {
        sync_token: (!text.is_empty()).then(|| text.to_owned()),
        sync_level: 1,
        limit,
        props: report_props(root)?,
    })
}

pub fn parse_calendar_report(body: &[u8]) -> Result<CalendarReport, QueryError> {
    let root = document(body)?;
    if root.is(CALDAV_CALENDAR_QUERY) {
        Ok(CalendarReport::Query(query_from(&root)?))
    } else if root.is(CALDAV_CALENDAR_MULTIGET) {
        Ok(CalendarReport::Multiget(multiget_from(&root)?))
    } else if root.is("DAV:sync-collection") {
        Ok(CalendarReport::SyncCollection(sync_from(&root)?))
    } else {
        Err(QueryError::UnsupportedRoot)
    }
}
pub fn parse_calendar_query(body: &[u8]) -> Result<CalendarQuery, QueryError> {
    let root = document(body)?;
    if !root.is(CALDAV_CALENDAR_QUERY) {
        return Err(QueryError::UnsupportedRoot);
    }
    query_from(&root)
}
pub fn parse_calendar_multiget(body: &[u8]) -> Result<CalendarMultiget, QueryError> {
    let root = document(body)?;
    if !root.is(CALDAV_CALENDAR_MULTIGET) {
        return Err(QueryError::UnsupportedRoot);
    }
    multiget_from(&root)
}
pub fn parse_sync_collection(body: &[u8]) -> Result<SyncCollection, QueryError> {
    let root = document(body)?;
    if !root.is("DAV:sync-collection") {
        return Err(QueryError::UnsupportedRoot);
    }
    sync_from(&root)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_property_end_tags_and_namespace_scope() {
        let body = br#"<D:propfind xmlns:D="DAV:" xmlns:X="urn:outer/"><D:prop><D:getetag></D:getetag><X:first xmlns:X="urn:inner/"/><X:second/></D:prop></D:propfind>"#;
        assert_eq!(
            parse_propfind(body).unwrap(),
            PropfindMode::Prop(vec![
                "DAV:getetag".into(),
                "{urn:inner/}first".into(),
                "{urn:outer/}second".into()
            ])
        );
    }

    #[test]
    fn rejects_invalid_document_and_selector_grammar() {
        for body in [
            r#"<D:propfind xmlns:D="DAV:"><D:prop/><D:allprop/></D:propfind>"#,
            r#"<D:propfind xmlns:D="DAV:"><D:prop><X:unknown/></D:prop></D:propfind>"#,
            r#"<D:propfind xmlns:D="DAV:"><D:prop/></D:propfind><extra/>"#,
            r#"<D:propfind xmlns:D="DAV:"><D:prop/></D:propfind>trailing"#,
        ] {
            assert!(parse_propfind(body.as_bytes()).is_err(), "{body}");
        }
    }

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
  <C:filter><C:comp-filter name="VCALENDAR"><C:comp-filter name="VEVENT">
    <C:time-range start="20260101T000000Z" end="20260131T235959Z"/>
  </C:comp-filter></C:comp-filter></C:filter>
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
                "DAV:displayname".into(),
                "http://apple.com/ns/ical/calendar-color".into()
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
            Some(PropfindMode::Prop(vec![
                "DAV:getetag".into(),
                "DAV:getcontenttype".into(),
                "urn:ietf:params:xml:ns:caldav:calendar-data".into()
            ]))
        );
    }

    #[test]
    fn query_rejects_calendarserver_namespace_root() {
        // The legacy calendarserver namespace must NOT be accepted as CalDAV.
        let body = r#"<C:calendar-query xmlns:C="urn:ietf:params:xml:ns:calendarserver/"><C:filter><C:comp-filter name="VCALENDAR"><C:comp-filter name="VEVENT"><C:time-range start="20260101T000000Z" end="20260131T235959Z"/></C:comp-filter></C:comp-filter></C:filter></C:calendar-query>"#;
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
            QueryError::MalformedXml
        );
    }

    #[test]
    fn rejects_non_xml() {
        assert_eq!(
            parse_calendar_query(b"not xml at all").unwrap_err(),
            QueryError::MalformedXml
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
            r#"<C:calendar-query xmlns:C="{CALDAV}"><C:filter><C:comp-filter name="VCALENDAR"><C:comp-filter name="VEVENT"><C:time-range start="20260131T000000Z" end="20260101T000000Z"/></C:comp-filter></C:comp-filter></C:filter></C:calendar-query>"#
        );
        assert_eq!(
            parse_calendar_query(body.as_bytes()).unwrap_err(),
            QueryError::InvalidRange
        );
    }

    #[test]
    fn rejects_equal_start_and_end() {
        let body = format!(
            r#"<C:calendar-query xmlns:C="{CALDAV}"><C:filter><C:comp-filter name="VCALENDAR"><C:comp-filter name="VEVENT"><C:time-range start="20260101T000000Z" end="20260101T000000Z"/></C:comp-filter></C:comp-filter></C:filter></C:calendar-query>"#
        );
        assert_eq!(
            parse_calendar_query(body.as_bytes()).unwrap_err(),
            QueryError::InvalidRange
        );
    }

    #[test]
    fn accepts_large_finite_range() {
        let body = format!(
            r#"<C:calendar-query xmlns:C="{CALDAV}"><C:filter><C:comp-filter name="VCALENDAR"><C:comp-filter name="VEVENT"><C:time-range start="20200101T000000Z" end="20260101T000000Z"/></C:comp-filter></C:comp-filter></C:filter></C:calendar-query>"#
        );
        assert!(parse_calendar_query(body.as_bytes()).is_ok());
    }

    #[test]
    fn rejects_invalid_date_format() {
        let body = format!(
            r#"<C:calendar-query xmlns:C="{CALDAV}"><C:filter><C:comp-filter name="VCALENDAR"><C:comp-filter name="VEVENT"><C:time-range start="notadate" end="20260101T000000Z"/></C:comp-filter></C:comp-filter></C:filter></C:calendar-query>"#
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
    fn rejects_date_only_time_range_start() {
        let body = format!(
            r#"<C:calendar-query xmlns:C="{CALDAV}"><C:filter><C:comp-filter name="VCALENDAR"><C:comp-filter name="VEVENT"><C:time-range start="20260101" end="20260131T235959Z"/></C:comp-filter></C:comp-filter></C:filter></C:calendar-query>"#
        );
        assert_eq!(
            parse_calendar_query(body.as_bytes()).unwrap_err(),
            QueryError::InvalidDateTime
        );
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
        let body = r#"<C:calendar-query xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:calendarserver/"><C:filter><C:comp-filter name="VCALENDAR"><C:comp-filter name="VEVENT"><C:time-range start="20260101T000000Z" end="20260131T235959Z"/></C:comp-filter></C:comp-filter></C:filter></C:calendar-query>"#;
        assert_eq!(
            parse_calendar_report(body.as_bytes()).unwrap_err(),
            QueryError::UnsupportedRoot
        );
    }

    #[test]
    fn report_dispatches_sync_collection() {
        let body = format!(
            r#"<D:sync-collection xmlns:D="DAV:" xmlns:C="{CALDAV}"><D:sync-token></D:sync-token><D:sync-level>1</D:sync-level><D:prop><D:getetag/></D:prop></D:sync-collection>"#
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
            r#"<D:sync-collection xmlns:D="DAV:" xmlns:C="{CALDAV}"><D:sync-token>opaque-token-123</D:sync-token><D:sync-level>1</D:sync-level><D:prop/></D:sync-collection>"#
        );
        let sync = parse_sync_collection(body.as_bytes()).unwrap();
        assert_eq!(sync.sync_token.as_deref(), Some("opaque-token-123"));
        assert_eq!(sync.sync_level, 1);
    }

    #[test]
    fn sync_collection_rejects_bad_level() {
        let body = format!(
            r#"<D:sync-collection xmlns:D="DAV:" xmlns:C="{CALDAV}"><D:sync-token></D:sync-token><D:sync-level>2</D:sync-level></D:sync-collection>"#
        );
        assert_eq!(
            parse_sync_collection(body.as_bytes()).unwrap_err(),
            QueryError::BadSyncLevel
        );
    }

    #[test]
    fn sync_collection_rejects_missing_level() {
        let body = format!(
            r#"<D:sync-collection xmlns:D="DAV:" xmlns:C="{CALDAV}"><D:sync-token></D:sync-token></D:sync-collection>"#
        );
        assert_eq!(
            parse_sync_collection(body.as_bytes()).unwrap_err(),
            QueryError::MalformedXml
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
