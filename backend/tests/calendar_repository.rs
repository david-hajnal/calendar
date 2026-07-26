use commoncal_backend::{
    authorization::CalendarRole,
    calendar::{CalendarRepository, CalendarRepositoryError, CalendarUpdate, NewCalendar},
    config::{AppConfig, Environment},
    database::connect_and_migrate,
    http::Readiness,
};
use sqlx::SqlitePool;
use tempfile::TempDir;

async fn repository() -> (TempDir, SqlitePool, CalendarRepository) {
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

    (temp_dir, pool.clone(), CalendarRepository::new(pool))
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

fn new_calendar() -> NewCalendar {
    NewCalendar {
        name: "Team calendar".to_owned(),
        description: Some("Planning".to_owned()),
        color: "#3367d6".to_owned(),
        default_timezone: "Europe/Budapest".to_owned(),
        default_event_visibility: "default".to_owned(),
        default_notification_rules_json: None,
        created_at: 200,
    }
}

#[tokio::test]
async fn calendar_creation_creates_exactly_one_owner() {
    let (_temp_dir, pool, repository) = repository().await;
    let owner_id = create_user(&pool, "owner@example.com").await;

    let calendar = repository
        .create_calendar(owner_id, new_calendar())
        .await
        .unwrap();
    let acl = repository.acl_entries(calendar.id).await.unwrap();

    assert_eq!(calendar.owner_user_id, owner_id);
    assert_eq!(acl.len(), 1);
    assert_eq!(acl[0].user_id, owner_id);
    assert_eq!(acl[0].role, CalendarRole::Owner);
}

#[tokio::test]
async fn duplicate_acl_fails() {
    let (_temp_dir, pool, repository) = repository().await;
    let owner_id = create_user(&pool, "owner@example.com").await;
    let viewer_id = create_user(&pool, "viewer@example.com").await;
    let calendar = repository
        .create_calendar(owner_id, new_calendar())
        .await
        .unwrap();
    repository
        .add_acl(calendar.id, viewer_id, CalendarRole::Viewer, 210)
        .await
        .unwrap();

    let result = repository
        .add_acl(calendar.id, viewer_id, CalendarRole::Editor, 220)
        .await;

    assert!(result.is_err());
}

#[tokio::test]
async fn ownership_transfer_updates_owner_and_acl_atomically() {
    let (_temp_dir, pool, repository) = repository().await;
    let owner_id = create_user(&pool, "owner@example.com").await;
    let next_owner_id = create_user(&pool, "next@example.com").await;
    let calendar = repository
        .create_calendar(owner_id, new_calendar())
        .await
        .unwrap();

    let updated = repository
        .transfer_ownership(calendar.id, owner_id, next_owner_id, calendar.version, 300)
        .await
        .unwrap();
    let acl = repository.acl_entries(calendar.id).await.unwrap();

    assert_eq!(updated.owner_user_id, next_owner_id);
    assert_eq!(updated.version, calendar.version + 1);
    assert_eq!(
        acl.iter()
            .filter(|entry| entry.role == CalendarRole::Owner)
            .count(),
        1
    );
    assert_eq!(
        acl.iter()
            .find(|entry| entry.user_id == owner_id)
            .unwrap()
            .role,
        CalendarRole::Manager
    );
}

#[tokio::test]
async fn failed_transfer_rolls_back() {
    let (_temp_dir, pool, repository) = repository().await;
    let owner_id = create_user(&pool, "owner@example.com").await;
    let calendar = repository
        .create_calendar(owner_id, new_calendar())
        .await
        .unwrap();

    let result = repository
        .transfer_ownership(calendar.id, owner_id, 999_999, calendar.version, 300)
        .await;
    let unchanged = repository.calendar(calendar.id).await.unwrap().unwrap();
    let acl = repository.acl_entries(calendar.id).await.unwrap();

    assert!(result.is_err());
    assert_eq!(unchanged.owner_user_id, owner_id);
    assert_eq!(unchanged.version, calendar.version);
    assert_eq!(acl.len(), 1);
    assert_eq!(acl[0].role, CalendarRole::Owner);
}

#[tokio::test]
async fn ownership_transfer_rejects_inactive_targets_without_changes() {
    for status in ["suspended", "deleted"] {
        let (_temp_dir, pool, repository) = repository().await;
        let owner_id = create_user(&pool, "owner@example.com").await;
        let target_id = create_user(&pool, "target@example.com").await;
        sqlx::query("UPDATE users SET status = ? WHERE id = ?")
            .bind(status)
            .bind(target_id)
            .execute(&pool)
            .await
            .unwrap();
        let calendar = repository
            .create_calendar(owner_id, new_calendar())
            .await
            .unwrap();

        let result = repository
            .transfer_ownership(calendar.id, owner_id, target_id, calendar.version, 300)
            .await;
        let unchanged = repository.calendar(calendar.id).await.unwrap().unwrap();
        let acl = repository.acl_entries(calendar.id).await.unwrap();

        assert!(matches!(
            result,
            Err(CalendarRepositoryError::InactiveTarget)
        ));
        assert_eq!(unchanged.owner_user_id, owner_id);
        assert_eq!(acl.len(), 1);
        assert_eq!(acl[0].role, CalendarRole::Owner);
    }
}

#[tokio::test]
async fn stale_version_update_fails() {
    let (_temp_dir, pool, repository) = repository().await;
    let owner_id = create_user(&pool, "owner@example.com").await;
    let calendar = repository
        .create_calendar(owner_id, new_calendar())
        .await
        .unwrap();
    repository
        .update_calendar(
            calendar.id,
            calendar.version,
            CalendarUpdate {
                name: "First update".to_owned(),
                description: None,
                color: "#123456".to_owned(),
                default_timezone: "UTC".to_owned(),
                default_event_visibility: "private".to_owned(),
                default_notification_rules_json: None,
                archived: false,
                updated_at: 300,
            },
        )
        .await
        .unwrap();

    let result = repository
        .update_calendar(
            calendar.id,
            calendar.version,
            CalendarUpdate {
                name: "Stale update".to_owned(),
                description: None,
                color: "#654321".to_owned(),
                default_timezone: "UTC".to_owned(),
                default_event_visibility: "default".to_owned(),
                default_notification_rules_json: None,
                archived: false,
                updated_at: 400,
            },
        )
        .await;

    assert!(matches!(result, Err(CalendarRepositoryError::StaleVersion)));
}

#[tokio::test]
async fn foreign_key_behavior_is_correct() {
    let (_temp_dir, pool, repository) = repository().await;
    let owner_id = create_user(&pool, "owner@example.com").await;
    let viewer_id = create_user(&pool, "viewer@example.com").await;
    let calendar = repository
        .create_calendar(owner_id, new_calendar())
        .await
        .unwrap();
    repository
        .add_acl(calendar.id, viewer_id, CalendarRole::Viewer, 210)
        .await
        .unwrap();

    let owner_delete = sqlx::query("DELETE FROM users WHERE id = ?")
        .bind(owner_id)
        .execute(&pool)
        .await;
    sqlx::query("DELETE FROM calendars WHERE id = ?")
        .bind(calendar.id)
        .execute(&pool)
        .await
        .unwrap();
    let remaining_acl: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM calendar_acl WHERE calendar_id = ?")
            .bind(calendar.id)
            .fetch_one(&pool)
            .await
            .unwrap();

    assert!(owner_delete.is_err());
    assert_eq!(remaining_acl, 0);
}
