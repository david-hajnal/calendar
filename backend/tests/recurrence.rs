use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Duration, TimeZone, Utc};
use chrono_tz::{America::New_York, Europe::Budapest};
use commoncal_backend::recurrence::{
    ExpansionLimits, ModifiedOccurrence, RecurrenceError, RecurrenceRule, RecurringEvent,
    TimeInterval, expand_occurrences,
};

fn utc(value: &str) -> DateTime<Utc> {
    value.parse().unwrap()
}

fn event(start: DateTime<chrono_tz::Tz>, rule: &str) -> RecurringEvent {
    RecurringEvent {
        starts_at: start,
        duration: Duration::hours(1),
        rule: RecurrenceRule::parse(rule).unwrap(),
    }
}

fn window(start: &str, end: &str) -> TimeInterval {
    TimeInterval {
        start: utc(start),
        end: utc(end),
    }
}

fn starts(
    recurring_event: &RecurringEvent,
    interval: TimeInterval,
) -> Result<Vec<DateTime<Utc>>, RecurrenceError> {
    expand_occurrences(
        recurring_event,
        interval,
        &HashSet::new(),
        &HashMap::new(),
        ExpansionLimits::default(),
    )
    .map(|occurrences| occurrences.into_iter().map(|item| item.start).collect())
}

#[test]
fn expands_daily_weekly_monthly_and_yearly_rules() {
    let cases = [
        (
            event(
                Budapest.with_ymd_and_hms(2024, 1, 1, 9, 0, 0).unwrap(),
                "FREQ=DAILY;COUNT=3",
            ),
            vec![
                utc("2024-01-01T08:00:00Z"),
                utc("2024-01-02T08:00:00Z"),
                utc("2024-01-03T08:00:00Z"),
            ],
        ),
        (
            event(
                Budapest.with_ymd_and_hms(2024, 1, 1, 9, 0, 0).unwrap(),
                "FREQ=WEEKLY;COUNT=3",
            ),
            vec![
                utc("2024-01-01T08:00:00Z"),
                utc("2024-01-08T08:00:00Z"),
                utc("2024-01-15T08:00:00Z"),
            ],
        ),
        (
            event(
                Budapest.with_ymd_and_hms(2024, 1, 15, 9, 0, 0).unwrap(),
                "FREQ=MONTHLY;COUNT=3",
            ),
            vec![
                utc("2024-01-15T08:00:00Z"),
                utc("2024-02-15T08:00:00Z"),
                utc("2024-03-15T08:00:00Z"),
            ],
        ),
        (
            event(
                Budapest.with_ymd_and_hms(2022, 6, 10, 9, 0, 0).unwrap(),
                "FREQ=YEARLY;COUNT=3",
            ),
            vec![
                utc("2022-06-10T07:00:00Z"),
                utc("2023-06-10T07:00:00Z"),
                utc("2024-06-10T07:00:00Z"),
            ],
        ),
    ];

    for (recurring_event, expected) in cases {
        assert_eq!(
            starts(
                &recurring_event,
                window("2020-01-01T00:00:00Z", "2026-01-01T00:00:00Z")
            )
            .unwrap(),
            expected
        );
    }
}

#[test]
fn count_and_until_end_the_series() {
    let start = Budapest.with_ymd_and_hms(2024, 1, 1, 9, 0, 0).unwrap();
    let count = event(start, "FREQ=DAILY;COUNT=2");
    let until = event(start, "FREQ=DAILY;UNTIL=20240102T080000Z");
    let interval = window("2024-01-01T00:00:00Z", "2024-01-10T00:00:00Z");

    assert_eq!(starts(&count, interval).unwrap().len(), 2);
    assert_eq!(starts(&until, interval).unwrap().len(), 2);
}

#[test]
fn yearly_recurrence_skips_non_leap_years_for_february_29() {
    let recurring_event = event(
        Budapest.with_ymd_and_hms(2024, 2, 29, 9, 0, 0).unwrap(),
        "FREQ=YEARLY;COUNT=3",
    );

    assert_eq!(
        starts(
            &recurring_event,
            window("2024-01-01T00:00:00Z", "2033-01-01T00:00:00Z")
        )
        .unwrap(),
        vec![
            utc("2024-02-29T08:00:00Z"),
            utc("2028-02-29T08:00:00Z"),
            utc("2032-02-29T08:00:00Z"),
        ]
    );
}

#[test]
fn monthly_recurrence_skips_months_without_the_start_day() {
    let recurring_event = event(
        Budapest.with_ymd_and_hms(2024, 1, 31, 9, 0, 0).unwrap(),
        "FREQ=MONTHLY;COUNT=4",
    );

    assert_eq!(
        starts(
            &recurring_event,
            window("2024-01-01T00:00:00Z", "2024-08-01T00:00:00Z")
        )
        .unwrap(),
        vec![
            utc("2024-01-31T08:00:00Z"),
            utc("2024-03-31T07:00:00Z"),
            utc("2024-05-31T07:00:00Z"),
            utc("2024-07-31T07:00:00Z"),
        ]
    );
}

#[test]
fn preserves_local_time_across_spring_and_autumn_dst_changes() {
    let spring = event(
        New_York.with_ymd_and_hms(2024, 3, 9, 9, 0, 0).unwrap(),
        "FREQ=DAILY;COUNT=3",
    );
    let autumn = event(
        New_York.with_ymd_and_hms(2024, 11, 2, 9, 0, 0).unwrap(),
        "FREQ=DAILY;COUNT=3",
    );

    assert_eq!(
        starts(
            &spring,
            window("2024-03-09T00:00:00Z", "2024-03-13T00:00:00Z")
        )
        .unwrap(),
        vec![
            utc("2024-03-09T14:00:00Z"),
            utc("2024-03-10T13:00:00Z"),
            utc("2024-03-11T13:00:00Z"),
        ]
    );
    assert_eq!(
        starts(
            &autumn,
            window("2024-11-02T00:00:00Z", "2024-11-06T00:00:00Z")
        )
        .unwrap(),
        vec![
            utc("2024-11-02T13:00:00Z"),
            utc("2024-11-03T14:00:00Z"),
            utc("2024-11-04T14:00:00Z"),
        ]
    );
}

#[test]
fn applies_excluded_and_modified_single_occurrences() {
    let recurring_event = event(
        Budapest.with_ymd_and_hms(2024, 1, 1, 9, 0, 0).unwrap(),
        "FREQ=DAILY;COUNT=4",
    );
    let excluded = HashSet::from([utc("2024-01-02T08:00:00Z")]);
    let modifications = HashMap::from([(
        utc("2024-01-03T08:00:00Z"),
        ModifiedOccurrence {
            start: utc("2024-01-03T12:00:00Z"),
            end: utc("2024-01-03T14:00:00Z"),
        },
    )]);

    let occurrences = expand_occurrences(
        &recurring_event,
        window("2024-01-01T00:00:00Z", "2024-01-10T00:00:00Z"),
        &excluded,
        &modifications,
        ExpansionLimits::default(),
    )
    .unwrap();

    assert_eq!(
        occurrences
            .iter()
            .map(|item| item.start)
            .collect::<Vec<_>>(),
        vec![
            utc("2024-01-01T08:00:00Z"),
            utc("2024-01-03T12:00:00Z"),
            utc("2024-01-04T08:00:00Z"),
        ]
    );
    assert!(occurrences[1].modified);
    assert_eq!(occurrences[1].end, utc("2024-01-03T14:00:00Z"));
}

#[test]
fn rejects_unsupported_or_malicious_rules_and_bounds_unending_rules() {
    assert_eq!(
        RecurrenceRule::parse("FREQ=HOURLY"),
        Err(RecurrenceError::UnsupportedFrequency("HOURLY".into()))
    );
    assert_eq!(
        RecurrenceRule::parse("FREQ=DAILY;BYSECOND=1"),
        Err(RecurrenceError::UnsupportedPart("BYSECOND".into()))
    );
    assert_eq!(
        RecurrenceRule::parse("FREQ=DAILY;COUNT=1000001"),
        Err(RecurrenceError::ComplexityLimitExceeded)
    );

    let unending = event(
        Budapest.with_ymd_and_hms(2024, 1, 1, 9, 0, 0).unwrap(),
        "FREQ=DAILY",
    );
    assert_eq!(
        starts(
            &unending,
            window("2024-01-01T00:00:00Z", "2024-01-04T00:00:00Z")
        )
        .unwrap()
        .len(),
        3
    );
    assert_eq!(
        expand_occurrences(
            &unending,
            window("2024-01-01T00:00:00Z", "2030-01-01T00:00:00Z"),
            &HashSet::new(),
            &HashMap::new(),
            ExpansionLimits {
                max_occurrences: 10,
                max_iterations: 100,
            },
        ),
        Err(RecurrenceError::OccurrenceLimitExceeded)
    );
    assert_eq!(
        expand_occurrences(
            &unending,
            window("2200-01-01T00:00:00Z", "2200-01-02T00:00:00Z"),
            &HashSet::new(),
            &HashMap::new(),
            ExpansionLimits {
                max_occurrences: 10,
                max_iterations: 10,
            },
        ),
        Err(RecurrenceError::ComplexityLimitExceeded)
    );
}

#[test]
fn rejects_until_span_exceeding_five_years() {
    let start = Budapest.with_ymd_and_hms(2024, 1, 1, 9, 0, 0).unwrap();
    let far_until = event(
        start,
        "FREQ=DAILY;UNTIL=99991231T235959Z",
    );
    let result = starts(
        &far_until,
        window("2024-01-01T00:00:00Z", "2030-01-01T00:00:00Z"),
    );
    assert_eq!(result, Err(RecurrenceError::ComplexityLimitExceeded));
}

#[test]
fn accepts_until_span_within_five_years() {
    let start = Budapest.with_ymd_and_hms(2024, 1, 1, 9, 0, 0).unwrap();
    let near_until = event(
        start,
        "FREQ=WEEKLY;UNTIL=20280101T090000Z",
    );
    let result = starts(
        &near_until,
        window("2024-01-01T00:00:00Z", "2029-01-01T00:00:00Z"),
    );
    assert!(result.is_ok());
    assert!(!result.unwrap().is_empty());
}
