use commoncal_backend::{
    authorization::CalendarRole,
    calendar::{CalendarRepository, NewCalendar},
    config::{AppConfig, Environment},
    database::connect_and_migrate,
    event::{
        AllDayOccurrenceChange, EventChange, EventMutation, EventRange, EventService,
        EventServiceError, EventStatus, EventTiming, OccurrenceChange,
    },
    http::Readiness,
};
use std::sync::{Arc, Mutex};
use tempfile::TempDir;

const NOW: i64 = 1_750_000_000;

#[tokio::test]
async fn creates_and_expands_a_recurring_series_in_a_bounded_range() {
    let (_dir, pool, owner, calendar_id) = setup().await;
    let service = EventService::new_at(pool, NOW);

    let series = service
        .create_recurring(
            owner,
            false,
            calendar_id,
            mutation("Standup", 1_750_000_100, 1_750_000_700),
            "FREQ=DAILY;COUNT=3".to_owned(),
        )
        .await
        .unwrap();

    let occurrences = service
        .list(
            owner,
            false,
            calendar_id,
            EventRange {
                start_utc: 1_750_000_000,
                end_utc: 1_750_259_300,
                start_date: "2025-06-15".to_owned(),
                end_date: "2025-06-19".to_owned(),
            },
        )
        .await
        .unwrap();

    assert_eq!(
        series.recurrence_rule.as_deref(),
        Some("FREQ=DAILY;COUNT=3")
    );
    assert_eq!(occurrences.len(), 3);
    assert!(
        occurrences
            .iter()
            .all(|event| event.series_id == Some(series.id))
    );
}

#[tokio::test]
async fn creates_and_expands_an_all_day_series_with_exclusive_end_dates() {
    let (_dir, pool, owner, calendar_id) = setup().await;
    let service = EventService::new_at(pool, NOW);

    let series = service
        .create_recurring(
            owner,
            false,
            calendar_id,
            all_day_mutation("Conference", "2025-06-15", "2025-06-17"),
            "FREQ=DAILY;COUNT=3".to_owned(),
        )
        .await
        .unwrap();
    let occurrences = list_all_day_series(&service, owner, calendar_id).await;

    assert_eq!(
        series.recurrence_rule.as_deref(),
        Some("FREQ=DAILY;COUNT=3")
    );
    assert_eq!(
        occurrences
            .iter()
            .map(|event| (
                event.recurrence_date.as_deref(),
                event.start_date.as_deref(),
                event.end_date.as_deref(),
            ))
            .collect::<Vec<_>>(),
        vec![
            (Some("2025-06-15"), Some("2025-06-15"), Some("2025-06-17")),
            (Some("2025-06-16"), Some("2025-06-16"), Some("2025-06-18")),
            (Some("2025-06-17"), Some("2025-06-17"), Some("2025-06-19")),
        ]
    );
}

#[tokio::test]
async fn updates_and_deletes_single_all_day_occurrences() {
    let (_dir, pool, owner, calendar_id) = setup().await;
    let service = EventService::new_at(pool, NOW);
    let series = create_all_day_series(&service, owner, calendar_id).await;

    service
        .update_all_day_occurrence(
            owner,
            false,
            calendar_id,
            series.id,
            AllDayOccurrenceChange {
                recurrence_date: "2025-06-16".to_owned(),
                expected_version: 1,
                event: all_day_mutation("Moved conference", "2025-06-20", "2025-06-22"),
            },
        )
        .await
        .unwrap();
    service
        .delete_all_day_occurrence(owner, false, calendar_id, series.id, "2025-06-17", 2)
        .await
        .unwrap();

    let occurrences = list_all_day_series(&service, owner, calendar_id).await;
    assert_eq!(occurrences.len(), 2);
    let moved = occurrences
        .iter()
        .find(|event| event.recurrence_date.as_deref() == Some("2025-06-16"))
        .unwrap();
    assert_eq!(moved.title.as_deref(), Some("Moved conference"));
    assert_eq!(moved.start_date.as_deref(), Some("2025-06-20"));
    assert_eq!(moved.end_date.as_deref(), Some("2025-06-22"));
    assert!(
        occurrences
            .iter()
            .all(|event| event.recurrence_date.as_deref() != Some("2025-06-17"))
    );
}

#[tokio::test]
async fn updates_the_entire_all_day_series_and_clears_its_exceptions() {
    let (_dir, pool, owner, calendar_id) = setup().await;
    let service = EventService::new_at(pool.clone(), NOW);
    let series = create_all_day_series(&service, owner, calendar_id).await;
    service
        .delete_all_day_occurrence(owner, false, calendar_id, series.id, "2025-06-16", 1)
        .await
        .unwrap();

    let updated = service
        .update(
            owner,
            false,
            calendar_id,
            series.id,
            EventChange {
                expected_version: 2,
                target_calendar_id: calendar_id,
                event: all_day_mutation("Updated conference", "2025-06-15", "2025-06-17"),
            },
        )
        .await
        .unwrap();

    assert_eq!(
        updated.recurrence_rule.as_deref(),
        Some("FREQ=DAILY;COUNT=3")
    );
    assert_eq!(
        list_all_day_series(&service, owner, calendar_id)
            .await
            .len(),
        3
    );
    let exception_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM event_recurrence_exceptions WHERE series_id = ?")
            .bind(series.id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(exception_count, 0);
}

#[tokio::test]
async fn all_day_occurrence_mutations_preserve_auth_conflicts_and_replanning() {
    let (_dir, pool, owner, calendar_id) = setup().await;
    let viewer = insert_user(&pool, "all-day-viewer@example.com").await;
    CalendarRepository::new(pool.clone())
        .add_acl(calendar_id, viewer, CalendarRole::Viewer, NOW)
        .await
        .unwrap();
    let replanned = Arc::new(Mutex::new(Vec::new()));
    let captured = replanned.clone();
    let service = EventService::new_at_with_notification_replanner(
        pool,
        NOW,
        Arc::new(move |series_id| captured.lock().unwrap().push(series_id)),
    );
    let series = create_all_day_series(&service, owner, calendar_id).await;

    assert!(matches!(
        service
            .update_all_day_occurrence(
                viewer,
                false,
                calendar_id,
                series.id,
                AllDayOccurrenceChange {
                    recurrence_date: "2025-06-16".to_owned(),
                    expected_version: 1,
                    event: all_day_mutation("Denied", "2025-06-20", "2025-06-22"),
                },
            )
            .await,
        Err(EventServiceError::NotFound)
    ));
    service
        .delete_all_day_occurrence(owner, false, calendar_id, series.id, "2025-06-16", 1)
        .await
        .unwrap();
    let stale = service
        .update_all_day_occurrence(
            owner,
            false,
            calendar_id,
            series.id,
            AllDayOccurrenceChange {
                recurrence_date: "2025-06-17".to_owned(),
                expected_version: 1,
                event: all_day_mutation("Stale", "2025-06-20", "2025-06-22"),
            },
        )
        .await
        .unwrap_err();

    assert!(matches!(
        stale,
        EventServiceError::Conflict { current_version: 2 }
    ));
    assert_eq!(*replanned.lock().unwrap(), vec![series.id; 2]);
}

#[tokio::test]
async fn updates_one_occurrence_without_changing_the_series_template() {
    let (_dir, pool, owner, calendar_id) = setup().await;
    let service = EventService::new_at(pool, NOW);
    let series = create_series(&service, owner, calendar_id).await;
    let recurrence_id = 1_750_086_500;

    service
        .update_occurrence(
            owner,
            false,
            calendar_id,
            series.id,
            OccurrenceChange {
                recurrence_id,
                expected_version: series.version.unwrap(),
                event: mutation(
                    "Moved standup",
                    recurrence_id + 3_600,
                    recurrence_id + 4_200,
                ),
            },
        )
        .await
        .unwrap();

    let occurrences = list_series(&service, owner, calendar_id).await;
    let moved = occurrences
        .iter()
        .find(|event| event.recurrence_id == Some(recurrence_id))
        .unwrap();
    assert_eq!(moved.title.as_deref(), Some("Moved standup"));
    assert_eq!(moved.start_utc, Some(recurrence_id + 3_600));
    assert_eq!(
        occurrences
            .iter()
            .find(|event| event.recurrence_id == Some(1_750_000_100))
            .unwrap()
            .title
            .as_deref(),
        Some("Standup")
    );
}

#[tokio::test]
async fn deletes_one_occurrence_without_deleting_the_series() {
    let (_dir, pool, owner, calendar_id) = setup().await;
    let service = EventService::new_at(pool, NOW);
    let series = create_series(&service, owner, calendar_id).await;

    service
        .delete_occurrence(
            owner,
            false,
            calendar_id,
            series.id,
            1_750_086_500,
            series.version.unwrap(),
        )
        .await
        .unwrap();

    let occurrences = list_series(&service, owner, calendar_id).await;
    assert_eq!(occurrences.len(), 2);
    assert!(
        occurrences
            .iter()
            .all(|event| event.recurrence_id != Some(1_750_086_500))
    );
}

#[tokio::test]
async fn updates_the_entire_series_and_preserves_its_rule() {
    let (_dir, pool, owner, calendar_id) = setup().await;
    let service = EventService::new_at(pool, NOW);
    let series = create_series(&service, owner, calendar_id).await;

    let updated = service
        .update(
            owner,
            false,
            calendar_id,
            series.id,
            EventChange {
                expected_version: series.version.unwrap(),
                target_calendar_id: calendar_id,
                event: mutation("Company standup", 1_750_000_100, 1_750_000_700),
            },
        )
        .await
        .unwrap();

    assert_eq!(
        updated.recurrence_rule.as_deref(),
        Some("FREQ=DAILY;COUNT=3")
    );
    assert!(
        list_series(&service, owner, calendar_id)
            .await
            .iter()
            .all(|event| event.title.as_deref() == Some("Company standup"))
    );
}

#[tokio::test]
async fn rejects_concurrent_series_and_exception_edits_with_current_version() {
    let (_dir, pool, owner, calendar_id) = setup().await;
    let service = EventService::new_at(pool, NOW);
    let series = create_series(&service, owner, calendar_id).await;
    let version = series.version.unwrap();
    service
        .delete_occurrence(owner, false, calendar_id, series.id, 1_750_086_500, version)
        .await
        .unwrap();

    let error = service
        .update_occurrence(
            owner,
            false,
            calendar_id,
            series.id,
            OccurrenceChange {
                recurrence_id: 1_750_172_900,
                expected_version: version,
                event: mutation("Stale edit", 1_750_172_900, 1_750_173_500),
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        EventServiceError::Conflict { current_version: 2 }
    ));
}

#[tokio::test]
async fn authorization_applies_to_every_recurring_operation() {
    let (_dir, pool, owner, calendar_id) = setup().await;
    let viewer = insert_user(&pool, "viewer@example.com").await;
    CalendarRepository::new(pool.clone())
        .add_acl(calendar_id, viewer, CalendarRole::Viewer, NOW)
        .await
        .unwrap();
    let service = EventService::new_at(pool, NOW);
    assert!(matches!(
        service
            .create_recurring(
                viewer,
                false,
                calendar_id,
                mutation("Denied", 1_750_000_100, 1_750_000_700),
                "FREQ=DAILY;COUNT=3".to_owned(),
            )
            .await,
        Err(EventServiceError::NotFound)
    ));
    let series = create_series(&service, owner, calendar_id).await;
    assert!(matches!(
        service
            .update_occurrence(
                viewer,
                false,
                calendar_id,
                series.id,
                OccurrenceChange {
                    recurrence_id: 1_750_086_500,
                    expected_version: series.version.unwrap(),
                    event: mutation("Denied", 1_750_086_500, 1_750_087_100),
                },
            )
            .await,
        Err(EventServiceError::NotFound)
    ));
    assert!(matches!(
        service
            .delete_occurrence(
                viewer,
                false,
                calendar_id,
                series.id,
                1_750_086_500,
                series.version.unwrap(),
            )
            .await,
        Err(EventServiceError::NotFound)
    ));
    assert!(matches!(
        service
            .update(
                viewer,
                false,
                calendar_id,
                series.id,
                EventChange {
                    expected_version: series.version.unwrap(),
                    target_calendar_id: calendar_id,
                    event: mutation("Denied", 1_750_000_100, 1_750_000_700),
                },
            )
            .await,
        Err(EventServiceError::NotFound)
    ));
}

#[tokio::test]
async fn recurring_mutations_trigger_the_notification_replanning_seam() {
    let (_dir, pool, owner, calendar_id) = setup().await;
    let replanned = Arc::new(Mutex::new(Vec::new()));
    let captured = replanned.clone();
    let service = EventService::new_at_with_notification_replanner(
        pool.clone(),
        NOW,
        Arc::new(move |series_id| captured.lock().unwrap().push(series_id)),
    );
    let series = create_series(&service, owner, calendar_id).await;
    service
        .update_occurrence(
            owner,
            false,
            calendar_id,
            series.id,
            OccurrenceChange {
                recurrence_id: 1_750_086_500,
                expected_version: 1,
                event: mutation("Moved", 1_750_086_600, 1_750_087_200),
            },
        )
        .await
        .unwrap();
    service
        .delete_occurrence(owner, false, calendar_id, series.id, 1_750_172_900, 2)
        .await
        .unwrap();
    service
        .update(
            owner,
            false,
            calendar_id,
            series.id,
            EventChange {
                expected_version: 3,
                target_calendar_id: calendar_id,
                event: mutation("Updated series", 1_750_000_100, 1_750_000_700),
            },
        )
        .await
        .unwrap();

    assert_eq!(*replanned.lock().unwrap(), vec![series.id; 4]);
    let exception_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM event_recurrence_exceptions WHERE series_id = ?")
            .bind(series.id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(exception_count, 0);
}

async fn setup() -> (TempDir, sqlx::SqlitePool, i64, i64) {
    let dir = TempDir::new().unwrap();
    let config = AppConfig::with_database_path(
        Environment::Development,
        "127.0.0.1:3000",
        None,
        dir.path().join("commoncal.sqlite"),
    )
    .unwrap();
    let pool = connect_and_migrate(&config, Readiness::new())
        .await
        .unwrap();
    let owner = insert_user(&pool, "owner@example.com").await;
    let calendar_id = CalendarRepository::new(pool.clone())
        .create_calendar(
            owner,
            NewCalendar {
                name: "Team".to_owned(),
                description: None,
                color: "#3367d6".to_owned(),
                default_timezone: "Europe/Budapest".to_owned(),
                default_event_visibility: "private".to_owned(),
                default_notification_rules_json: None,
                created_at: NOW - 50,
            },
        )
        .await
        .unwrap()
        .id;
    (dir, pool, owner, calendar_id)
}

async fn insert_user(pool: &sqlx::SqlitePool, email: &str) -> i64 {
    sqlx::query(
        "INSERT INTO users (
            normalized_email, display_name, status, is_superadmin, created_at
         ) VALUES (?, NULL, 'active', 0, ?)",
    )
    .bind(email)
    .bind(NOW - 100)
    .execute(pool)
    .await
    .unwrap()
    .last_insert_rowid()
}

async fn create_series(
    service: &EventService,
    owner: i64,
    calendar_id: i64,
) -> commoncal_backend::event::EventProjection {
    service
        .create_recurring(
            owner,
            false,
            calendar_id,
            mutation("Standup", 1_750_000_100, 1_750_000_700),
            "FREQ=DAILY;COUNT=3".to_owned(),
        )
        .await
        .unwrap()
}

async fn create_all_day_series(
    service: &EventService,
    owner: i64,
    calendar_id: i64,
) -> commoncal_backend::event::EventProjection {
    service
        .create_recurring(
            owner,
            false,
            calendar_id,
            all_day_mutation("Conference", "2025-06-15", "2025-06-17"),
            "FREQ=DAILY;COUNT=3".to_owned(),
        )
        .await
        .unwrap()
}

async fn list_series(
    service: &EventService,
    owner: i64,
    calendar_id: i64,
) -> Vec<commoncal_backend::event::EventProjection> {
    service
        .list(
            owner,
            false,
            calendar_id,
            EventRange {
                start_utc: 1_750_000_000,
                end_utc: 1_750_259_300,
                start_date: "2025-06-15".to_owned(),
                end_date: "2025-06-19".to_owned(),
            },
        )
        .await
        .unwrap()
}

fn mutation(title: &str, start_utc: i64, end_utc: i64) -> EventMutation {
    EventMutation {
        title: title.to_owned(),
        description: None,
        location: None,
        status: EventStatus::Confirmed,
        timing: EventTiming::Timed {
            start_utc,
            end_utc,
            timezone: "UTC".to_owned(),
        },
    }
}

fn all_day_mutation(title: &str, start_date: &str, end_date: &str) -> EventMutation {
    EventMutation {
        title: title.to_owned(),
        description: None,
        location: None,
        status: EventStatus::Confirmed,
        timing: EventTiming::AllDay {
            start_date: start_date.to_owned(),
            end_date: end_date.to_owned(),
        },
    }
}

async fn list_all_day_series(
    service: &EventService,
    owner: i64,
    calendar_id: i64,
) -> Vec<commoncal_backend::event::EventProjection> {
    service
        .list(
            owner,
            false,
            calendar_id,
            EventRange {
                start_utc: 1_750_000_000,
                end_utc: 1_750_604_800,
                start_date: "2025-06-15".to_owned(),
                end_date: "2025-06-22".to_owned(),
            },
        )
        .await
        .unwrap()
}
