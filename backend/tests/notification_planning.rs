use commoncal_backend::{
    authorization::CalendarRole,
    calendar::{CalendarRepository, NewCalendar},
    config::{AppConfig, Environment},
    database::connect_and_migrate,
    event::{EventMutation, EventService, EventStatus, EventTiming},
    http::Readiness,
    notification::{NotificationPreference, NotificationService, PreferenceScope},
};
use sqlx::Row;
use tempfile::TempDir;

const NOW: i64 = 1_750_000_000;

#[tokio::test]
async fn event_preferences_take_precedence_and_plan_independent_user_jobs() {
    let (_dir, pool, owner, calendar) = setup().await;
    let other = user(&pool, "other@example.com").await;
    CalendarRepository::new(pool.clone())
        .add_acl(calendar, other, CalendarRole::Viewer, NOW)
        .await
        .unwrap();
    let events = EventService::new_at(pool.clone(), NOW);
    let event = events
        .create(owner, false, calendar, timed_event(NOW + 86_400))
        .await
        .unwrap();
    let notifications = NotificationService::new_at(pool.clone(), NOW, 14 * 86_400);

    notifications
        .set_preference(
            owner,
            PreferenceScope::Account,
            NotificationPreference::new(60, "UTC"),
        )
        .await
        .unwrap();
    notifications
        .set_preference(
            other,
            PreferenceScope::Account,
            NotificationPreference::new(30, "UTC"),
        )
        .await
        .unwrap();
    notifications
        .set_preference(
            owner,
            PreferenceScope::Calendar(calendar),
            NotificationPreference::new(45, "UTC"),
        )
        .await
        .unwrap();
    notifications
        .set_preference(
            owner,
            PreferenceScope::Event(event.id),
            NotificationPreference::new(15, "UTC"),
        )
        .await
        .unwrap();
    assert_eq!(
        notifications
            .effective_preference(owner, calendar, event.id)
            .await
            .unwrap()
            .reminder_minutes,
        15
    );

    notifications.replan_event(event.id).await.unwrap();
    let jobs: Vec<(i64, i64)> = sqlx::query("SELECT user_id, scheduled_at FROM notification_jobs WHERE event_id = ? AND state = 'pending' ORDER BY user_id")
        .bind(event.id).map(|row: sqlx::sqlite::SqliteRow| (row.get(0), row.get(1))).fetch_all(&pool).await.unwrap();
    assert_eq!(
        jobs,
        vec![(owner, NOW + 86_400 - 900), (other, NOW + 86_400 - 1_800)]
    );
}

#[tokio::test]
async fn recurring_jobs_are_deduplicated_and_event_edits_cancel_obsolete_pending_jobs() {
    let (_dir, pool, owner, calendar) = setup().await;
    let events = EventService::new_at(pool.clone(), NOW);
    let event = events
        .create_recurring(
            owner,
            false,
            calendar,
            timed_event(NOW + 86_400),
            "FREQ=DAILY;COUNT=2".into(),
        )
        .await
        .unwrap();
    let notifications = NotificationService::new_at(pool.clone(), NOW, 14 * 86_400);
    notifications
        .set_preference(
            owner,
            PreferenceScope::Account,
            NotificationPreference::new(30, "UTC"),
        )
        .await
        .unwrap();
    notifications.replan_event(event.id).await.unwrap();
    notifications.replan_event(event.id).await.unwrap();
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM notification_jobs WHERE event_id = ? AND state = 'pending'",
    )
    .bind(event.id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(count, 2);
    sqlx::query("UPDATE events SET timed_start_utc = ?, timed_end_utc = ? WHERE id = ?")
        .bind(NOW + 172_800)
        .bind(NOW + 173_400)
        .bind(event.id)
        .execute(&pool)
        .await
        .unwrap();
    notifications.replan_event(event.id).await.unwrap();
    let cancelled: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM notification_jobs WHERE event_id = ? AND state = 'cancelled'",
    )
    .bind(event.id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(cancelled, 1);
}

#[tokio::test]
async fn revocation_cancels_pending_jobs_and_all_day_uses_preference_timezone() {
    let (_dir, pool, owner, calendar) = setup().await;
    let viewer = user(&pool, "viewer@example.com").await;
    CalendarRepository::new(pool.clone())
        .add_acl(calendar, viewer, CalendarRole::Viewer, NOW)
        .await
        .unwrap();
    let events = EventService::new_at(pool.clone(), NOW);
    let event = events
        .create(
            owner,
            false,
            calendar,
            EventMutation {
                title: "Day".into(),
                description: None,
                location: None,
                status: EventStatus::Confirmed,
                timing: EventTiming::AllDay {
                    start_date: "2025-06-16".into(),
                    end_date: "2025-06-17".into(),
                },
            },
        )
        .await
        .unwrap();
    let notifications = NotificationService::new_at(pool.clone(), NOW, 14 * 86_400);
    notifications
        .set_preference(
            viewer,
            PreferenceScope::Account,
            NotificationPreference::new(60, "America/New_York"),
        )
        .await
        .unwrap();
    notifications.replan_event(event.id).await.unwrap();
    let scheduled: i64 = sqlx::query_scalar(
        "SELECT scheduled_at FROM notification_jobs WHERE event_id = ? AND user_id = ?",
    )
    .bind(event.id)
    .bind(viewer)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(scheduled, 1_750_042_800);
    notifications
        .cancel_pending(calendar, viewer)
        .await
        .unwrap();
    let state: String = sqlx::query_scalar(
        "SELECT state FROM notification_jobs WHERE event_id = ? AND user_id = ?",
    )
    .bind(event.id)
    .bind(viewer)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(state, "cancelled");
}

#[tokio::test]
async fn test_delivery_is_visible_only_to_the_authenticated_recipient() {
    let (_dir, pool, owner, calendar) = setup().await;
    let event = EventService::new_at(pool.clone(), NOW)
        .create(owner, false, calendar, timed_event(NOW + 3_600))
        .await
        .unwrap();
    let notifications = NotificationService::new_at(pool, NOW, 14 * 86_400);

    notifications
        .create_test_delivery(owner, event.id)
        .await
        .unwrap();

    assert_eq!(notifications.list_in_app(owner).await.unwrap().len(), 1);
    assert!(
        notifications
            .list_in_app(owner + 999)
            .await
            .unwrap()
            .is_empty()
    );
}

async fn setup() -> (TempDir, sqlx::SqlitePool, i64, i64) {
    let dir = TempDir::new().unwrap();
    let config = AppConfig::with_database_path(
        Environment::Development,
        "127.0.0.1:0",
        None,
        dir.path().join("test.sqlite"),
    )
    .unwrap();
    let pool = connect_and_migrate(&config, Readiness::new())
        .await
        .unwrap();
    let owner = user(&pool, "owner@example.com").await;
    let calendar = CalendarRepository::new(pool.clone())
        .create_calendar(
            owner,
            NewCalendar {
                name: "Calendar".into(),
                description: None,
                color: "#123456".into(),
                default_timezone: "UTC".into(),
                default_event_visibility: "default".into(),
                default_notification_rules_json: None,
                created_at: NOW,
            },
        )
        .await
        .unwrap()
        .id;
    (dir, pool, owner, calendar)
}
async fn user(pool: &sqlx::SqlitePool, email: &str) -> i64 {
    sqlx::query("INSERT INTO users (normalized_email, display_name, status, created_at) VALUES (?, ?, 'active', ?)").bind(email).bind(email).bind(NOW).execute(pool).await.unwrap().last_insert_rowid()
}
fn timed_event(start: i64) -> EventMutation {
    EventMutation {
        title: "Meeting".into(),
        description: None,
        location: None,
        status: EventStatus::Confirmed,
        timing: EventTiming::Timed {
            start_utc: start,
            end_utc: start + 600,
            timezone: "UTC".into(),
        },
    }
}
