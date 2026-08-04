use std::{
    collections::{HashMap, HashSet},
    fmt::{self, Display, Formatter},
};

use chrono::{DateTime, Duration, LocalResult, NaiveDate, NaiveDateTime, TimeZone, Utc};
use chrono_tz::Tz;

use crate::recurrence::RecurrenceRule;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IcsParserLimits {
    pub max_components: usize,
    pub max_events: usize,
    pub max_component_bytes: usize,
    pub max_text_bytes: usize,
    pub max_recurrence_values: usize,
}

impl Default for IcsParserLimits {
    fn default() -> Self {
        Self {
            max_components: 1_000,
            max_events: 500,
            max_component_bytes: 256 * 1024,
            max_text_bytes: 16 * 1024,
            max_recurrence_values: 1_000,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NormalizedCalendar {
    pub events: Vec<NormalizedEvent>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NormalizedEvent {
    pub uid: String,
    pub timing: NormalizedTiming,
    pub summary: String,
    pub description: Option<String>,
    pub location: Option<String>,
    pub status: Option<String>,
    pub rrule: Option<String>,
    pub exdates: Vec<NormalizedDateValue>,
    pub recurrence_id: Option<NormalizedDateValue>,
    pub sequence: u64,
    pub dtstamp: Option<DateTime<Utc>>,
    pub last_modified: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum NormalizedTiming {
    Timed {
        starts_at: DateTime<Utc>,
        ends_at: DateTime<Utc>,
        timezone: Option<String>,
    },
    AllDay {
        start_date: NaiveDate,
        end_date: NaiveDate,
    },
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum NormalizedDateValue {
    Timed(DateTime<Utc>),
    AllDay(NaiveDate),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IcsParseErrorCode {
    Malformed,
    InvalidEvent,
    LimitExceeded,
    DuplicateEvent,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IcsParseError {
    code: IcsParseErrorCode,
}
impl IcsParseError {
    fn new(code: IcsParseErrorCode) -> Self {
        Self { code }
    }
    pub fn code(&self) -> IcsParseErrorCode {
        self.code
    }
}
impl Display for IcsParseError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str("invalid calendar data")
    }
}
impl std::error::Error for IcsParseError {}

pub fn parse_calendar(
    input: &str,
    limits: IcsParserLimits,
) -> Result<NormalizedCalendar, IcsParseError> {
    if limits.max_components == 0
        || limits.max_events == 0
        || limits.max_component_bytes == 0
        || limits.max_text_bytes == 0
        || limits.max_recurrence_values == 0
    {
        return Err(IcsParseError::new(IcsParseErrorCode::LimitExceeded));
    }
    let lines = unfold(input)?;
    let mut stack = Vec::new();
    let mut components = 0_usize;
    let mut current: Option<Vec<Property>> = None;
    let mut events = Vec::new();
    let mut keys = HashSet::new();
    for line in lines {
        let property = property(&line)?;
        match (property.name.as_str(), property.value.as_str()) {
            ("BEGIN", name) => {
                components += 1;
                if components > limits.max_components {
                    return Err(IcsParseError::new(IcsParseErrorCode::LimitExceeded));
                }
                if name != "VCALENDAR"
                    && name != "VEVENT"
                    && name != "VTIMEZONE"
                    && name != "STANDARD"
                    && name != "DAYLIGHT"
                {
                    return Err(IcsParseError::new(IcsParseErrorCode::Malformed));
                }
                if name == "VCALENDAR" && !stack.is_empty() {
                    return Err(IcsParseError::new(IcsParseErrorCode::Malformed));
                }
                if name == "VEVENT" && stack.last().map(String::as_str) != Some("VCALENDAR") {
                    return Err(IcsParseError::new(IcsParseErrorCode::Malformed));
                }
                if name == "VEVENT" {
                    current = Some(Vec::new());
                }
                stack.push(name.to_owned());
            }
            ("END", name) => {
                if stack.pop().as_deref() != Some(name) {
                    return Err(IcsParseError::new(IcsParseErrorCode::Malformed));
                }
                if name == "VEVENT" {
                    let event = normalize_event(
                        current
                            .take()
                            .ok_or(IcsParseError::new(IcsParseErrorCode::Malformed))?,
                        limits,
                    )?;
                    let key = (event.uid.clone(), event.recurrence_id.clone());
                    if !keys.insert(key) {
                        return Err(IcsParseError::new(IcsParseErrorCode::DuplicateEvent));
                    }
                    events.push(event);
                    if events.len() > limits.max_events {
                        return Err(IcsParseError::new(IcsParseErrorCode::LimitExceeded));
                    }
                }
            }
            _ => {
                if stack.last().map(String::as_str) == Some("VEVENT") {
                    let target = current
                        .as_mut()
                        .ok_or(IcsParseError::new(IcsParseErrorCode::Malformed))?;
                    let bytes = target.iter().map(|p| p.raw_len).sum::<usize>() + property.raw_len;
                    if bytes > limits.max_component_bytes {
                        return Err(IcsParseError::new(IcsParseErrorCode::LimitExceeded));
                    }
                    target.push(property);
                }
            }
        }
    }
    if !stack.is_empty() || events.is_empty() {
        return Err(IcsParseError::new(IcsParseErrorCode::Malformed));
    }
    Ok(NormalizedCalendar { events })
}

#[derive(Clone)]
struct Property {
    name: String,
    params: HashMap<String, String>,
    value: String,
    raw_len: usize,
}
fn unfold(input: &str) -> Result<Vec<String>, IcsParseError> {
    let mut lines: Vec<String> = Vec::new();
    for raw in input.lines() {
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        if line.starts_with(' ') || line.starts_with('\t') {
            lines
                .last_mut()
                .ok_or(IcsParseError::new(IcsParseErrorCode::Malformed))?
                .push_str(&line[1..]);
        } else if !line.is_empty() {
            lines.push(line.to_owned());
        }
    }
    if lines.is_empty() {
        return Err(IcsParseError::new(IcsParseErrorCode::Malformed));
    }
    Ok(lines)
}
fn property(line: &str) -> Result<Property, IcsParseError> {
    let (left, value) = line
        .split_once(':')
        .ok_or(IcsParseError::new(IcsParseErrorCode::Malformed))?;
    let mut parts = left.split(';');
    let name = parts.next().unwrap().to_ascii_uppercase();
    if name.is_empty() || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
        return Err(IcsParseError::new(IcsParseErrorCode::Malformed));
    }
    let mut params = HashMap::new();
    for part in parts {
        let (key, value) = part
            .split_once('=')
            .ok_or(IcsParseError::new(IcsParseErrorCode::Malformed))?;
        if params
            .insert(key.to_ascii_uppercase(), value.trim_matches('"').to_owned())
            .is_some()
        {
            return Err(IcsParseError::new(IcsParseErrorCode::Malformed));
        }
    }
    Ok(Property {
        name,
        params,
        value: value.to_owned(),
        raw_len: line.len(),
    })
}

fn normalize_event(
    properties: Vec<Property>,
    limits: IcsParserLimits,
) -> Result<NormalizedEvent, IcsParseError> {
    let mut fields: HashMap<String, Vec<Property>> = HashMap::new();
    for p in properties {
        fields.entry(p.name.clone()).or_default().push(p);
    }
    for name in [
        "UID",
        "DTSTART",
        "DTEND",
        "DURATION",
        "SUMMARY",
        "DESCRIPTION",
        "LOCATION",
        "STATUS",
        "RRULE",
        "RECURRENCE-ID",
        "SEQUENCE",
        "DTSTAMP",
        "LAST-MODIFIED",
    ] {
        if fields.get(name).is_some_and(|values| values.len() > 1) {
            return Err(IcsParseError::new(IcsParseErrorCode::InvalidEvent));
        }
    }
    let one = |name: &str| fields.get(name).and_then(|v| v.first());
    let uid = text(
        one("UID").ok_or(IcsParseError::new(IcsParseErrorCode::InvalidEvent))?,
        limits,
    )?;
    if uid.is_empty() {
        return Err(IcsParseError::new(IcsParseErrorCode::InvalidEvent));
    }
    let start =
        date_value(one("DTSTART").ok_or(IcsParseError::new(IcsParseErrorCode::InvalidEvent))?)?;
    let end = one("DTEND").map(date_value).transpose()?;
    let duration = one("DURATION")
        .map(|p| parse_duration(&p.value))
        .transpose()?;
    if end.is_some() == duration.is_some() {
        return Err(IcsParseError::new(IcsParseErrorCode::InvalidEvent));
    }
    let timezone = one("DTSTART").and_then(|p| p.params.get("TZID")).cloned();
    if let Some(end_property) = one("DTEND")
        && end_property.params.get("TZID").cloned() != timezone
    {
        return Err(IcsParseError::new(IcsParseErrorCode::InvalidEvent));
    }
    let timing = timing(start, end, duration, timezone)?;
    let recurrence_id = one("RECURRENCE-ID").map(date_value).transpose()?;
    if recurrence_id
        .as_ref()
        .is_some_and(|v| !same_kind(v, &timing))
    {
        return Err(IcsParseError::new(IcsParseErrorCode::InvalidEvent));
    }
    let mut exdates = Vec::new();
    for p in fields.get("EXDATE").into_iter().flatten() {
        for value in p.value.split(',') {
            let copied = Property {
                value: value.to_owned(),
                ..p.clone()
            };
            let value = date_value(&copied)?;
            if !same_kind(&value, &timing) {
                return Err(IcsParseError::new(IcsParseErrorCode::InvalidEvent));
            }
            exdates.push(value);
            if exdates.len() > limits.max_recurrence_values {
                return Err(IcsParseError::new(IcsParseErrorCode::LimitExceeded));
            }
        }
    }
    let rrule = one("RRULE").map(|p| p.value.clone());
    if let Some(rule) = &rrule {
        if rule.split(';').count() > limits.max_recurrence_values {
            return Err(IcsParseError::new(IcsParseErrorCode::LimitExceeded));
        }
        if RecurrenceRule::parse(rule).is_err() {
            return Err(IcsParseError::new(IcsParseErrorCode::InvalidEvent));
        }
    }
    let status = one("STATUS").map(|p| p.value.clone());
    if status
        .as_deref()
        .is_some_and(|s| !matches!(s, "CONFIRMED" | "TENTATIVE" | "CANCELLED"))
    {
        return Err(IcsParseError::new(IcsParseErrorCode::InvalidEvent));
    }
    Ok(NormalizedEvent {
        uid,
        timing,
        summary: one("SUMMARY")
            .map(|p| text(p, limits))
            .transpose()?
            .unwrap_or_default(),
        description: one("DESCRIPTION").map(|p| text(p, limits)).transpose()?,
        location: one("LOCATION").map(|p| text(p, limits)).transpose()?,
        status,
        rrule,
        exdates,
        recurrence_id,
        sequence: one("SEQUENCE")
            .map(|p| p.value.parse())
            .transpose()
            .map_err(|_| IcsParseError::new(IcsParseErrorCode::InvalidEvent))?
            .unwrap_or(0),
        dtstamp: one("DTSTAMP").map(utc_datetime).transpose()?,
        last_modified: one("LAST-MODIFIED").map(utc_datetime).transpose()?,
    })
}
fn text(p: &Property, limits: IcsParserLimits) -> Result<String, IcsParseError> {
    if p.value.len() > limits.max_text_bytes {
        return Err(IcsParseError::new(IcsParseErrorCode::LimitExceeded));
    }
    let mut out = String::new();
    let mut escaped = false;
    for c in p.value.chars() {
        if escaped {
            out.push(match c {
                'n' | 'N' => '\n',
                '\\' => '\\',
                ',' => ',',
                ';' => ';',
                _ => return Err(IcsParseError::new(IcsParseErrorCode::Malformed)),
            });
            escaped = false;
        } else if c == '\\' {
            escaped = true;
        } else {
            out.push(c);
        }
    }
    if escaped || out.len() > limits.max_text_bytes {
        return Err(IcsParseError::new(IcsParseErrorCode::LimitExceeded));
    }
    Ok(out)
}
fn date_value(p: &Property) -> Result<NormalizedDateValue, IcsParseError> {
    if p.params.get("VALUE").is_some_and(|v| v != "DATE") {
        return Err(IcsParseError::new(IcsParseErrorCode::InvalidEvent));
    }
    if p.params.get("VALUE").is_some_and(|v| v == "DATE") || p.value.len() == 8 {
        return NaiveDate::parse_from_str(&p.value, "%Y%m%d")
            .map(NormalizedDateValue::AllDay)
            .map_err(|_| IcsParseError::new(IcsParseErrorCode::InvalidEvent));
    }
    let naive = NaiveDateTime::parse_from_str(p.value.trim_end_matches('Z'), "%Y%m%dT%H%M%S")
        .map_err(|_| IcsParseError::new(IcsParseErrorCode::InvalidEvent))?;
    let value = if p.value.ends_with('Z') {
        naive.and_utc()
    } else if let Some(tz) = p.params.get("TZID") {
        let tz: Tz = tz
            .parse()
            .map_err(|_| IcsParseError::new(IcsParseErrorCode::InvalidEvent))?;
        match tz.from_local_datetime(&naive) {
            LocalResult::Single(v) => v.with_timezone(&Utc),
            _ => return Err(IcsParseError::new(IcsParseErrorCode::InvalidEvent)),
        }
    } else {
        naive.and_utc()
    };
    Ok(NormalizedDateValue::Timed(value))
}
fn timing(
    start: NormalizedDateValue,
    end: Option<NormalizedDateValue>,
    duration: Option<Duration>,
    timezone: Option<String>,
) -> Result<NormalizedTiming, IcsParseError> {
    match (start, end, duration) {
        (NormalizedDateValue::Timed(s), Some(NormalizedDateValue::Timed(e)), None) if e > s => {
            Ok(NormalizedTiming::Timed {
                starts_at: s,
                ends_at: e,
                timezone,
            })
        }
        (NormalizedDateValue::Timed(s), None, Some(d)) if d > Duration::zero() => {
            Ok(NormalizedTiming::Timed {
                starts_at: s,
                ends_at: s + d,
                timezone,
            })
        }
        (NormalizedDateValue::AllDay(s), Some(NormalizedDateValue::AllDay(e)), None)
            if e > s && timezone.is_none() =>
        {
            Ok(NormalizedTiming::AllDay {
                start_date: s,
                end_date: e,
            })
        }
        (NormalizedDateValue::AllDay(s), None, Some(d))
            if d > Duration::zero() && d.num_seconds() % 86_400 == 0 && timezone.is_none() =>
        {
            Ok(NormalizedTiming::AllDay {
                start_date: s,
                end_date: s
                    .checked_add_signed(d)
                    .ok_or(IcsParseError::new(IcsParseErrorCode::InvalidEvent))?,
            })
        }
        _ => Err(IcsParseError::new(IcsParseErrorCode::InvalidEvent)),
    }
}
fn parse_duration(value: &str) -> Result<Duration, IcsParseError> {
    if !value.starts_with('P') || value.contains('-') {
        return Err(IcsParseError::new(IcsParseErrorCode::InvalidEvent));
    }
    let mut seconds = 0_i64;
    let mut n = String::new();
    for ch in value[1..].chars() {
        if ch.is_ascii_digit() {
            n.push(ch);
            continue;
        }
        if ch == 'T' {
            continue;
        }
        let number: i64 = n
            .parse()
            .map_err(|_| IcsParseError::new(IcsParseErrorCode::InvalidEvent))?;
        n.clear();
        seconds = seconds
            .checked_add(match ch {
                'W' => number * 604_800,
                'D' => number * 86_400,
                'H' => number * 3_600,
                'M' => number * 60,
                'S' => number,
                _ => return Err(IcsParseError::new(IcsParseErrorCode::InvalidEvent)),
            })
            .ok_or(IcsParseError::new(IcsParseErrorCode::InvalidEvent))?;
    }
    if !n.is_empty() || seconds <= 0 {
        return Err(IcsParseError::new(IcsParseErrorCode::InvalidEvent));
    }
    Ok(Duration::seconds(seconds))
}
fn utc_datetime(p: &Property) -> Result<DateTime<Utc>, IcsParseError> {
    if !p.value.ends_with('Z') || p.params.contains_key("TZID") {
        return Err(IcsParseError::new(IcsParseErrorCode::InvalidEvent));
    }
    match date_value(p)? {
        NormalizedDateValue::Timed(v) => Ok(v),
        _ => Err(IcsParseError::new(IcsParseErrorCode::InvalidEvent)),
    }
}
fn same_kind(value: &NormalizedDateValue, timing: &NormalizedTiming) -> bool {
    matches!(
        (value, timing),
        (
            NormalizedDateValue::Timed(_),
            NormalizedTiming::Timed { .. }
        ) | (
            NormalizedDateValue::AllDay(_),
            NormalizedTiming::AllDay { .. }
        )
    )
}
