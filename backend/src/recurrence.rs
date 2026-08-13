use std::{
    collections::{HashMap, HashSet},
    error::Error,
    fmt::{self, Display, Formatter},
};

use chrono::{
    DateTime, Datelike, Duration, LocalResult, NaiveDate, NaiveDateTime, TimeZone, Timelike, Utc,
};
use chrono_tz::Tz;

const MAX_RULE_COUNT: u32 = 1_000_000;
const MAX_RULE_INTERVAL: u32 = 100_000;
const MAX_UNTIL_SPAN_SECONDS: i64 = 5 * 365 * 86400;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Frequency {
    Daily,
    Weekly,
    Monthly,
    Yearly,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecurrenceRule {
    frequency: Frequency,
    interval: u32,
    count: Option<u32>,
    until: Option<DateTime<Utc>>,
}

impl RecurrenceRule {
    pub fn parse(value: &str) -> Result<Self, RecurrenceError> {
        let value = value.strip_prefix("RRULE:").unwrap_or(value);
        let mut frequency = None;
        let mut interval = 1;
        let mut count = None;
        let mut until = None;
        let mut seen = HashSet::new();

        for part in value.split(';') {
            let (name, value) = part.split_once('=').ok_or(RecurrenceError::InvalidRule)?;
            if !seen.insert(name) {
                return Err(RecurrenceError::DuplicatePart(name.into()));
            }
            match name {
                "FREQ" => {
                    frequency = Some(match value {
                        "DAILY" => Frequency::Daily,
                        "WEEKLY" => Frequency::Weekly,
                        "MONTHLY" => Frequency::Monthly,
                        "YEARLY" => Frequency::Yearly,
                        other => return Err(RecurrenceError::UnsupportedFrequency(other.into())),
                    });
                }
                "INTERVAL" => {
                    interval = parse_positive(value)?;
                    if interval > MAX_RULE_INTERVAL {
                        return Err(RecurrenceError::ComplexityLimitExceeded);
                    }
                }
                "COUNT" => {
                    let parsed = parse_positive(value)?;
                    if parsed > MAX_RULE_COUNT {
                        return Err(RecurrenceError::ComplexityLimitExceeded);
                    }
                    count = Some(parsed);
                }
                "UNTIL" => until = Some(parse_until(value)?),
                other => return Err(RecurrenceError::UnsupportedPart(other.into())),
            }
        }

        if count.is_some() && until.is_some() {
            return Err(RecurrenceError::InvalidRule);
        }

        Ok(Self {
            frequency: frequency.ok_or(RecurrenceError::MissingFrequency)?,
            interval,
            count,
            until,
        })
    }
}

fn parse_positive(value: &str) -> Result<u32, RecurrenceError> {
    value
        .parse()
        .ok()
        .filter(|value| *value > 0)
        .ok_or(RecurrenceError::InvalidRule)
}

fn parse_until(value: &str) -> Result<DateTime<Utc>, RecurrenceError> {
    NaiveDateTime::parse_from_str(value, "%Y%m%dT%H%M%SZ")
        .map(|value| value.and_utc())
        .map_err(|_| RecurrenceError::InvalidUntil)
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecurringEvent {
    pub starts_at: DateTime<Tz>,
    pub duration: Duration,
    pub rule: RecurrenceRule,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TimeInterval {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExpansionLimits {
    pub max_occurrences: usize,
    pub max_iterations: usize,
}

impl Default for ExpansionLimits {
    fn default() -> Self {
        Self {
            max_occurrences: 1_000,
            max_iterations: 100_000,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ModifiedOccurrence {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Occurrence {
    pub recurrence_id: DateTime<Utc>,
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    pub modified: bool,
}

pub fn expand_occurrences(
    event: &RecurringEvent,
    requested: TimeInterval,
    excluded: &HashSet<DateTime<Utc>>,
    modified: &HashMap<DateTime<Utc>, ModifiedOccurrence>,
    limits: ExpansionLimits,
) -> Result<Vec<Occurrence>, RecurrenceError> {
    if requested.start >= requested.end {
        return Err(RecurrenceError::InvalidInterval);
    }
    if event.duration <= Duration::zero() {
        return Err(RecurrenceError::InvalidDuration);
    }
    if limits.max_occurrences == 0 || limits.max_iterations == 0 {
        return Err(RecurrenceError::ComplexityLimitExceeded);
    }

    if let Some(until) = &event.rule.until {
        let span = (*until - event.starts_at.with_timezone(&Utc)).num_seconds();
        if span > MAX_UNTIL_SPAN_SECONDS {
            return Err(RecurrenceError::ComplexityLimitExceeded);
        }
    }

    let local_start = event.starts_at.naive_local();
    let timezone = event.starts_at.timezone();
    let mut occurrences = Vec::new();
    let mut generated = 0_u32;
    let scan_through = modified
        .keys()
        .copied()
        .max()
        .map_or(requested.end, |last_modified| {
            requested.end.max(last_modified)
        });
    let mut terminated = false;

    for iteration in 0..limits.max_iterations {
        let Some(local_candidate) = candidate_at(local_start, &event.rule, iteration)? else {
            continue;
        };
        let Some(candidate) = resolve_local(timezone, local_candidate) else {
            continue;
        };
        let recurrence_id = candidate.with_timezone(&Utc);

        if event.rule.until.is_some_and(|until| recurrence_id > until) {
            terminated = true;
            break;
        }

        generated = generated
            .checked_add(1)
            .ok_or(RecurrenceError::ComplexityLimitExceeded)?;
        if event.rule.count.is_some_and(|count| generated > count) {
            terminated = true;
            break;
        }

        let occurrence = if excluded.contains(&recurrence_id) {
            None
        } else if let Some(replacement) = modified.get(&recurrence_id) {
            if replacement.start >= replacement.end {
                return Err(RecurrenceError::InvalidModification);
            }
            Some(Occurrence {
                recurrence_id,
                start: replacement.start,
                end: replacement.end,
                modified: true,
            })
        } else {
            Some(Occurrence {
                recurrence_id,
                start: recurrence_id,
                end: recurrence_id + event.duration,
                modified: false,
            })
        };

        if let Some(occurrence) = occurrence
            && overlaps(&occurrence, requested)
        {
            if occurrences.len() == limits.max_occurrences {
                return Err(RecurrenceError::OccurrenceLimitExceeded);
            }
            occurrences.push(occurrence);
        }

        if event.rule.count.is_some_and(|count| generated == count) {
            terminated = true;
            break;
        }
        if recurrence_id >= scan_through {
            terminated = true;
            break;
        }
    }

    if !terminated {
        return Err(RecurrenceError::ComplexityLimitExceeded);
    }

    occurrences.sort_by_key(|occurrence| (occurrence.start, occurrence.recurrence_id));
    Ok(occurrences)
}

fn overlaps(occurrence: &Occurrence, requested: TimeInterval) -> bool {
    occurrence.start < requested.end && occurrence.end > requested.start
}

fn resolve_local(timezone: Tz, local: NaiveDateTime) -> Option<DateTime<Tz>> {
    match timezone.from_local_datetime(&local) {
        LocalResult::Single(value) => Some(value),
        LocalResult::Ambiguous(first, second) => Some(first.min(second)),
        LocalResult::None => None,
    }
}

fn candidate_at(
    start: NaiveDateTime,
    rule: &RecurrenceRule,
    iteration: usize,
) -> Result<Option<NaiveDateTime>, RecurrenceError> {
    let offset = u32::try_from(iteration)
        .ok()
        .and_then(|iteration| iteration.checked_mul(rule.interval))
        .ok_or(RecurrenceError::ComplexityLimitExceeded)?;

    match rule.frequency {
        Frequency::Daily => checked_add_days(start, i64::from(offset)).map(Some),
        Frequency::Weekly => checked_add_days(start, i64::from(offset) * 7).map(Some),
        Frequency::Monthly => month_candidate(start, offset),
        Frequency::Yearly => year_candidate(start, offset),
    }
}

fn checked_add_days(start: NaiveDateTime, days: i64) -> Result<NaiveDateTime, RecurrenceError> {
    start
        .checked_add_signed(Duration::days(days))
        .ok_or(RecurrenceError::ComplexityLimitExceeded)
}

fn month_candidate(
    start: NaiveDateTime,
    months: u32,
) -> Result<Option<NaiveDateTime>, RecurrenceError> {
    let start_month = i64::from(start.year()) * 12 + i64::from(start.month0());
    let target = start_month
        .checked_add(i64::from(months))
        .ok_or(RecurrenceError::ComplexityLimitExceeded)?;
    let year = i32::try_from(target.div_euclid(12))
        .map_err(|_| RecurrenceError::ComplexityLimitExceeded)?;
    let month = u32::try_from(target.rem_euclid(12) + 1)
        .map_err(|_| RecurrenceError::ComplexityLimitExceeded)?;
    Ok(
        NaiveDate::from_ymd_opt(year, month, start.day()).and_then(|date| {
            date.and_hms_nano_opt(
                start.hour(),
                start.minute(),
                start.second(),
                start.nanosecond(),
            )
        }),
    )
}

fn year_candidate(
    start: NaiveDateTime,
    years: u32,
) -> Result<Option<NaiveDateTime>, RecurrenceError> {
    let year = start
        .year()
        .checked_add(i32::try_from(years).map_err(|_| RecurrenceError::ComplexityLimitExceeded)?)
        .ok_or(RecurrenceError::ComplexityLimitExceeded)?;
    Ok(
        NaiveDate::from_ymd_opt(year, start.month(), start.day()).and_then(|date| {
            date.and_hms_nano_opt(
                start.hour(),
                start.minute(),
                start.second(),
                start.nanosecond(),
            )
        }),
    )
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RecurrenceError {
    MissingFrequency,
    DuplicatePart(String),
    UnsupportedFrequency(String),
    UnsupportedPart(String),
    InvalidRule,
    InvalidUntil,
    InvalidInterval,
    InvalidDuration,
    InvalidModification,
    OccurrenceLimitExceeded,
    ComplexityLimitExceeded,
}

impl Display for RecurrenceError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingFrequency => formatter.write_str("recurrence rule is missing FREQ"),
            Self::DuplicatePart(part) => write!(formatter, "duplicate recurrence part: {part}"),
            Self::UnsupportedFrequency(frequency) => {
                write!(formatter, "unsupported recurrence frequency: {frequency}")
            }
            Self::UnsupportedPart(part) => write!(formatter, "unsupported recurrence part: {part}"),
            Self::InvalidRule => formatter.write_str("invalid recurrence rule"),
            Self::InvalidUntil => formatter.write_str("UNTIL must be an RFC 5545 UTC date-time"),
            Self::InvalidInterval => formatter.write_str("requested interval must be non-empty"),
            Self::InvalidDuration => formatter.write_str("event duration must be positive"),
            Self::InvalidModification => {
                formatter.write_str("modified occurrence must have a positive duration")
            }
            Self::OccurrenceLimitExceeded => {
                formatter.write_str("recurrence occurrence limit exceeded")
            }
            Self::ComplexityLimitExceeded => {
                formatter.write_str("recurrence complexity limit exceeded")
            }
        }
    }
}

impl Error for RecurrenceError {}
