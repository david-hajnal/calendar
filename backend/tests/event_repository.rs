use commoncal_backend::{
    calendar::{CalendarRepository, NewCalendar},
    config::{AppConfig, Environment},
    database::connect_and_migrate,
    event::{
        EventRange, EventRepository, EventRepositoryError, EventStatus, EventTiming, EventUpdate,
        NewEvent,
    },
    http::Readiness,
};
use sqlx::SqlitePool;
use tempfile::TempDir;

async fn repository() -> (TempDir, SqlitePool, EventRepository) {
    let temp_dir = TempDir::new().unwrap();
    let config = AppConfig::with_database_path(
        Environment::Development,
        "127.0.0.1:3000",
        None,
        temp_dir.path().join("commoncal.sqlite"),
    )
    .unwrap();
    let pool = connect_and_migrate(&config, Readiness::new())
        .await
        .unwrap();

    (temp_dir, pool.clone(), EventRepository::new(pool))
}

async fn create_user(pool: &SqlitePool, email: &str) -> i64 {
    sqlx::query(
        "INSERT INTO users (normalized_email, display_name, status, created_at)
         VALUES (?, ?, 'active', ?)",
    )
    .bind(email)
    .bind(email)
    .bind(100_i64)
    .execute(pool)
    .await
    .unwrap()
    .last_insert_rowid()
}

async fn create_calendar(pool: &SqlitePool, owner_user_id: i64, name: &str) -> i64 {
    CalendarRepository::new(pool.clone())
        .create_calendar(
            owner_user_id,
            NewCalendar {
                name: name.to_owned(),
                description: None,
                color: "#3367d6".to_owned(),
                default_timezone: "Europe/Budapest".to_owned(),
                default_event_visibility: "default".to_owned(),
                default_notification_rules_json: None,
                created_at: 200,
            },
        )
        .await
        .unwrap()
        .id
}

fn timed_event(calendar_id: i64, user_id: i64, start_utc: i64, end_utc: i64) -> NewEvent {
    NewEvent {
        calendar_id,
        title: "Planning".to_owned(),
        description: Some("Quarterly planning".to_owned()),
        location: Some("Room 1".to_owned()),
        status: EventStatus::Confirmed,
        timing: EventTiming::Timed {
            start_utc,
            end_utc,
            timezone: "Europe/Budapest".to_owned(),
        },
        created_by_user_id: user_id,
        last_edited_by_user_id: user_id,
        created_at: 300,
    }
}

fn all_day_event(calendar_id: i64, user_id: i64, start: &str, end: &str) -> NewEvent {
    NewEvent {
        calendar_id,
        title: "Conference".to_owned(),
        description: None,
        location: None,
        status: EventStatus::Tentative,
        timing: EventTiming::AllDay {
            start_date: start.to_owned(),
            end_date: end.to_owned(),
        },
        created_by_user_id: user_id,
        last_edited_by_user_id: user_id,
        created_at: 300,
    }
}

#[tokio::test]
async fn timed_event_round_trip_preserves_utc_instants_and_timezone() {
    let (_temp_dir, pool, repository) = repository().await;
    let owner_id = create_user(&pool, "owner@example.com").await;
    let calendar_id = create_calendar(&pool, owner_id, "Work").await;
    let new_event = timed_event(calendar_id, owner_id, 1_700_000_000, 1_700_003_600);

    let created = repository.create(new_event.clone()).await.unwrap();
    let loaded = repository
        .event(calendar_id, created.id)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(loaded.calendar_id, calendar_id);
    assert_eq!(loaded.title, new_event.title);
    assert_eq!(loaded.description, new_event.description);
    assert_eq!(loaded.location, new_event.location);
    assert_eq!(loaded.status, EventStatus::Confirmed);
    assert_eq!(loaded.timing, new_event.timing);
    assert_eq!(loaded.version, 1);
}

#[tokio::test]
async fn all_day_event_round_trip_preserves_exclusive_end_date() {
    let (_temp_dir, pool, repository) = repository().await;
    let owner_id = create_user(&pool, "owner@example.com").await;
    let calendar_id = create_calendar(&pool, owner_id, "Work").await;
    let new_event = all_day_event(calendar_id, owner_id, "2026-07-23", "2026-07-25");

    let created = repository.create(new_event.clone()).await.unwrap();
    let loaded = repository
        .event(calendar_id, created.id)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(loaded.timing, new_event.timing);
    assert_eq!(loaded.status, EventStatus::Tentative);
}

#[tokio::test]
async fn invalid_event_and_query_ranges_fail() {
    let (_temp_dir, pool, repository) = repository().await;
    let owner_id = create_user(&pool, "owner@example.com").await;
    let calendar_id = create_calendar(&pool, owner_id, "Work").await;

    let invalid_timed = repository
        .create(timed_event(calendar_id, owner_id, 200, 200))
        .await;
    let invalid_all_day = repository
        .create(all_day_event(
            calendar_id,
            owner_id,
            "2026-07-25",
            "2026-07-23",
        ))
        .await;
    let invalid_query = repository
        .events_in_range(
            calendar_id,
            EventRange {
                start_utc: 400,
                end_utc: 300,
                start_date: "2026-07-23".to_owned(),
                end_date: "2026-07-24".to_owned(),
            },
        )
        .await;

    assert!(matches!(
        invalid_timed,
        Err(EventRepositoryError::InvalidRange)
    ));
    assert!(matches!(
        invalid_all_day,
        Err(EventRepositoryError::InvalidRange)
    ));
    assert!(matches!(
        invalid_query,
        Err(EventRepositoryError::InvalidRange)
    ));
}

#[tokio::test]
async fn oversized_utc_and_all_day_query_ranges_fail() {
    let (_temp_dir, pool, _repository) = repository().await;
    let owner_id = create_user(&pool, "owner@example.com").await;
    let calendar_id = create_calendar(&pool, owner_id, "Work").await;
    let repository = EventRepository::new_with_max_range(pool, 100, 2);

    let oversized_utc = repository
        .events_in_range(
            calendar_id,
            EventRange {
                start_utc: 100,
                end_utc: 201,
                start_date: "2026-07-23".to_owned(),
                end_date: "2026-07-25".to_owned(),
            },
        )
        .await;
    let oversized_all_day = repository
        .events_in_range(
            calendar_id,
            EventRange {
                start_utc: 100,
                end_utc: 200,
                start_date: "2026-07-23".to_owned(),
                end_date: "2026-07-26".to_owned(),
            },
        )
        .await;

    assert!(matches!(
        oversized_utc,
        Err(EventRepositoryError::RangeTooLarge)
    ));
    assert!(matches!(
        oversized_all_day,
        Err(EventRepositoryError::RangeTooLarge)
    ));
}

#[tokio::test]
async fn range_query_excludes_non_overlapping_events() {
    let (_temp_dir, pool, repository) = repository().await;
    let owner_id = create_user(&pool, "owner@example.com").await;
    let calendar_id = create_calendar(&pool, owner_id, "Work").await;
    repository
        .create(timed_event(calendar_id, owner_id, 100, 200))
        .await
        .unwrap();
    let overlapping_timed = repository
        .create(timed_event(calendar_id, owner_id, 250, 350))
        .await
        .unwrap();
    let overlapping_all_day = repository
        .create(all_day_event(
            calendar_id,
            owner_id,
            "2026-07-23",
            "2026-07-25",
        ))
        .await
        .unwrap();
    repository
        .create(all_day_event(
            calendar_id,
            owner_id,
            "2026-07-25",
            "2026-07-26",
        ))
        .await
        .unwrap();

    let events = repository
        .events_in_range(
            calendar_id,
            EventRange {
                start_utc: 200,
                end_utc: 300,
                start_date: "2026-07-24".to_owned(),
                end_date: "2026-07-25".to_owned(),
            },
        )
        .await
        .unwrap();

    assert_eq!(
        events.iter().map(|event| event.id).collect::<Vec<_>>(),
        vec![overlapping_timed.id, overlapping_all_day.id]
    );
}

#[tokio::test]
async fn event_lookup_is_scoped_to_its_calendar() {
    let (_temp_dir, pool, repository) = repository().await;
    let owner_id = create_user(&pool, "owner@example.com").await;
    let first_calendar_id = create_calendar(&pool, owner_id, "First").await;
    let second_calendar_id = create_calendar(&pool, owner_id, "Second").await;
    let event = repository
        .create(timed_event(first_calendar_id, owner_id, 100, 200))
        .await
        .unwrap();

    let substituted = repository
        .event(second_calendar_id, event.id)
        .await
        .unwrap();

    assert_eq!(substituted, None);
}

#[tokio::test]
async fn stale_version_update_fails() {
    let (_temp_dir, pool, repository) = repository().await;
    let owner_id = create_user(&pool, "owner@example.com").await;
    let calendar_id = create_calendar(&pool, owner_id, "Work").await;
    let event = repository
        .create(timed_event(calendar_id, owner_id, 100, 200))
        .await
        .unwrap();
    let update = EventUpdate {
        title: "Updated".to_owned(),
        description: None,
        location: None,
        status: EventStatus::Cancelled,
        timing: EventTiming::Timed {
            start_utc: 150,
            end_utc: 250,
            timezone: "UTC".to_owned(),
        },
        last_edited_by_user_id: owner_id,
        updated_at: 400,
    };
    repository
        .update(calendar_id, event.id, event.version, update.clone())
        .await
        .unwrap();

    let stale = repository
        .update(calendar_id, event.id, event.version, update)
        .await;

    assert!(matches!(stale, Err(EventRepositoryError::StaleVersion)));
}
