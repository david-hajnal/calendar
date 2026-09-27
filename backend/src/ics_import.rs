use std::collections::{HashMap, HashSet};

use serde::Serialize;

use crate::{
    event::{
        EventCreateBatch, EventCreateException, EventCreateItem, EventMutation, EventRecurrenceKey,
        EventService, EventServiceError, EventStatus, EventTiming,
    },
    ics::{
        IcsParseErrorCode, IcsParserLimits, NormalizedDateValue, NormalizedEvent, NormalizedTiming,
        parse_calendar,
    },
};

pub const MAX_ICS_IMPORT_BYTES: usize = 1024 * 1024;
pub const MAX_ICS_IMPORT_EVENTS: usize = 1_000;

#[derive(Clone)]
pub struct IcsImportService {
    event_service: EventService,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct IcsImportSummary {
    pub imported_events: usize,
    pub imported_exceptions: usize,
}

#[derive(Debug)]
pub enum IcsImportError {
    InvalidCalendar,
    LimitExceeded,
    NotFound,
    Database(sqlx::Error),
}

impl IcsImportService {
    pub fn new(event_service: EventService) -> Self {
        Self { event_service }
    }

    pub async fn import(
        &self,
        actor_user_id: i64,
        is_superadmin: bool,
        calendar_id: i64,
        bytes: &[u8],
    ) -> Result<IcsImportSummary, IcsImportError> {
        if bytes.len() > MAX_ICS_IMPORT_BYTES {
            return Err(IcsImportError::LimitExceeded);
        }
        let input = std::str::from_utf8(bytes).map_err(|_| IcsImportError::InvalidCalendar)?;
        let calendar = parse_calendar(input, import_parser_limits()).map_err(map_parse_error)?;
        let batch = convert_calendar(calendar.events)?;
        let result = self
            .event_service
            .create_batch(actor_user_id, is_superadmin, calendar_id, batch)
            .await
            .map_err(map_service_error)?;
        Ok(IcsImportSummary {
            imported_events: result.event_ids.len(),
            imported_exceptions: result.exception_count,
        })
    }
}

fn import_parser_limits() -> IcsParserLimits {
    IcsParserLimits {
        max_events: MAX_ICS_IMPORT_EVENTS,
        // A calendar and its optional timezone components accompany the events.
        max_components: MAX_ICS_IMPORT_EVENTS + 100,
        ..IcsParserLimits::default()
    }
}

fn map_parse_error(error: crate::ics::IcsParseError) -> IcsImportError {
    match error.code() {
        IcsParseErrorCode::LimitExceeded => IcsImportError::LimitExceeded,
        IcsParseErrorCode::Malformed
        | IcsParseErrorCode::InvalidEvent
        | IcsParseErrorCode::DuplicateEvent => IcsImportError::InvalidCalendar,
    }
}

fn map_service_error(error: EventServiceError) -> IcsImportError {
    match error {
        EventServiceError::NotFound => IcsImportError::NotFound,
        EventServiceError::Database(error) => IcsImportError::Database(error),
        EventServiceError::ComplexityLimitExceeded
        | EventServiceError::Conflict { .. }
        | EventServiceError::InvalidInput
        | EventServiceError::NotSupported
        | EventServiceError::ReadOnly => IcsImportError::InvalidCalendar,
    }
}

fn convert_calendar(events: Vec<NormalizedEvent>) -> Result<EventCreateBatch, IcsImportError> {
    let mut masters = Vec::new();
    let mut detached_by_uid: HashMap<String, Vec<NormalizedEvent>> = HashMap::new();
    for event in events {
        if event.recurrence_id.is_some() {
            detached_by_uid
                .entry(event.uid.clone())
                .or_default()
                .push(event);
        } else {
            masters.push(event);
        }
    }

    let mut items = Vec::with_capacity(masters.len());
    for master in masters {
        let detached = detached_by_uid.remove(&master.uid).unwrap_or_default();
        if (!master.exdates.is_empty() || !detached.is_empty()) && master.rrule.is_none() {
            return Err(IcsImportError::InvalidCalendar);
        }
        let mut exceptions = Vec::with_capacity(master.exdates.len() + detached.len());
        let mut keys = HashSet::with_capacity(exceptions.capacity());
        for exdate in &master.exdates {
            let recurrence = recurrence_key(exdate);
            if !keys.insert(recurrence.clone()) {
                return Err(IcsImportError::InvalidCalendar);
            }
            exceptions.push(EventCreateException {
                recurrence,
                replacement: None,
            });
        }
        for override_event in detached {
            if override_event.rrule.is_some() || !override_event.exdates.is_empty() {
                return Err(IcsImportError::InvalidCalendar);
            }
            let recurrence = recurrence_key(
                override_event
                    .recurrence_id
                    .as_ref()
                    .ok_or(IcsImportError::InvalidCalendar)?,
            );
            if !keys.insert(recurrence.clone()) || !matches_master_kind(&master, &recurrence) {
                return Err(IcsImportError::InvalidCalendar);
            }
            exceptions.push(EventCreateException {
                recurrence,
                replacement: Some(event_mutation(&override_event)?),
            });
        }
        items.push(EventCreateItem {
            event: event_mutation(&master)?,
            recurrence_rule: master.rrule,
            exceptions,
        });
    }
    if !detached_by_uid.is_empty() || items.is_empty() {
        return Err(IcsImportError::InvalidCalendar);
    }
    Ok(EventCreateBatch { events: items })
}

fn matches_master_kind(master: &NormalizedEvent, recurrence: &EventRecurrenceKey) -> bool {
    matches!(
        (&master.timing, recurrence),
        (NormalizedTiming::Timed { .. }, EventRecurrenceKey::Timed(_))
            | (
                NormalizedTiming::AllDay { .. },
                EventRecurrenceKey::AllDay(_)
            )
    )
}

fn recurrence_key(value: &NormalizedDateValue) -> EventRecurrenceKey {
    match value {
        NormalizedDateValue::Timed(value) => EventRecurrenceKey::Timed(value.timestamp()),
        NormalizedDateValue::AllDay(value) => EventRecurrenceKey::AllDay(value.to_string()),
    }
}

fn event_mutation(event: &NormalizedEvent) -> Result<EventMutation, IcsImportError> {
    let status = match event.status.as_deref() {
        None | Some("CONFIRMED") => EventStatus::Confirmed,
        Some("TENTATIVE") => EventStatus::Tentative,
        Some("CANCELLED") => EventStatus::Cancelled,
        Some(_) => return Err(IcsImportError::InvalidCalendar),
    };
    let timing = match &event.timing {
        NormalizedTiming::Timed {
            starts_at,
            ends_at,
            timezone,
        } => EventTiming::Timed {
            start_utc: starts_at.timestamp(),
            end_utc: ends_at.timestamp(),
            timezone: timezone.clone().unwrap_or_else(|| "UTC".to_owned()),
        },
        NormalizedTiming::AllDay {
            start_date,
            end_date,
        } => EventTiming::AllDay {
            start_date: start_date.to_string(),
            end_date: end_date.to_string(),
        },
    };
    Ok(EventMutation {
        title: event.summary.clone(),
        description: event.description.clone(),
        location: event.location.clone(),
        status,
        timing,
    })
}
