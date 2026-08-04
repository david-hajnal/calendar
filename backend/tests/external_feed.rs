use std::sync::{Arc, Mutex};

use commoncal_backend::{
    config::{AppConfig, Environment},
    database::connect_and_migrate,
    external_feed::{ExternalFeedService, FeedError, FeedFetcher, FetchResponse, NewFeed},
    http::Readiness,
    security::SecretKey,
};
use sqlx::SqlitePool;
use tempfile::TempDir;

const NOW: i64 = 1_700_000_000;
const FIRST: &str = "BEGIN:VCALENDAR\nBEGIN:VEVENT\nUID:one\nDTSTART:20260803T090000Z\nDTEND:20260803T100000Z\nSUMMARY:First\nEND:VEVENT\nEND:VCALENDAR\n";
const SECOND: &str = "BEGIN:VCALENDAR\nBEGIN:VEVENT\nUID:one\nSEQUENCE:1\nDTSTART:20260803T090000Z\nDTEND:20260803T100000Z\nSUMMARY:Changed\nEND:VEVENT\nBEGIN:VEVENT\nUID:two\nDTSTART:20260804T090000Z\nDTEND:20260804T100000Z\nSUMMARY:Added\nEND:VEVENT\nEND:VCALENDAR\n";

type Validators = Vec<(Option<String>, Option<String>)>;

struct Fetcher {
    responses: Mutex<Vec<Result<FetchResponse, FeedError>>>,
    validators: Arc<Mutex<Validators>>,
}

impl Fetcher {
    fn new(responses: Vec<Result<FetchResponse, FeedError>>) -> Self {
        Self {
            responses: Mutex::new(responses),
            validators: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

impl FeedFetcher for Fetcher {
    fn fetch<'a>(
        &'a self,
        _: &'a str,
        etag: Option<&'a str>,
        last_modified: Option<&'a str>,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<FetchResponse, FeedError>> + Send + 'a>,
    > {
        self.validators
            .lock()
            .unwrap()
            .push((etag.map(str::to_owned), last_modified.map(str::to_owned)));
        let response = self.responses.lock().unwrap().remove(0);
        Box::pin(async move { response })
    }
}

fn response(status: u16, body: &str) -> Result<FetchResponse, FeedError> {
    Ok(FetchResponse {
        status,
        body: body.as_bytes().to_vec(),
        etag: Some("v1".into()),
        last_modified: Some("Mon, 03 Aug 2026 00:00:00 GMT".into()),
    })
}

async fn setup() -> (TempDir, SqlitePool, ExternalFeedService, i64, i64) {
    let directory = TempDir::new().unwrap();
    let config = AppConfig::with_database_path(
        Environment::Development,
        "127.0.0.1:0",
        None,
        directory.path().join("test.sqlite"),
    )
    .unwrap();
    let pool = connect_and_migrate(&config, Readiness::new())
        .await
        .unwrap();
    let user = sqlx::query("INSERT INTO users (normalized_email, status, created_at) VALUES ('manager@example.test', 'active', ?)").bind(NOW).execute(&pool).await.unwrap().last_insert_rowid();
    let calendar = sqlx::query("INSERT INTO calendars (owner_user_id,name,color,default_timezone,default_event_visibility,created_at,updated_at) VALUES (?, 'Test', '#123', 'UTC', 'default', ?, ?)").bind(user).bind(NOW).bind(NOW).execute(&pool).await.unwrap().last_insert_rowid();
    sqlx::query("INSERT INTO calendar_acl (calendar_id,user_id,role,created_at,updated_at) VALUES (?, ?, 'manager', ?, ?)").bind(calendar).bind(user).bind(NOW).bind(NOW).execute(&pool).await.unwrap();
    (
        directory,
        pool.clone(),
        ExternalFeedService::new_at(pool, SecretKey::new([9; 32]), NOW),
        user,
        calendar,
    )
}

async fn feed(service: &ExternalFeedService, user: i64, calendar: i64) -> i64 {
    service
        .create(
            user,
            false,
            calendar,
            NewFeed {
                source_url: "https://feeds.example.test/private.ics?token=secret".into(),
                refresh_interval_seconds: Some(60),
            },
        )
        .await
        .unwrap()
        .id
}

#[tokio::test]
async fn imports_updates_adds_and_removes_only_after_complete_parse() {
    let (_dir, pool, service, user, calendar) = setup().await;
    let id = feed(&service, user, calendar).await;
    service
        .refresh(user, false, id, &Fetcher::new(vec![response(200, FIRST)]))
        .await
        .unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM events")
            .fetch_one(&pool)
            .await
            .unwrap(),
        1
    );
    service
        .refresh(user, false, id, &Fetcher::new(vec![response(200, SECOND)]))
        .await
        .unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT title FROM events WHERE title = 'Changed'")
            .fetch_one(&pool)
            .await
            .unwrap(),
        "Changed"
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM events")
            .fetch_one(&pool)
            .await
            .unwrap(),
        2
    );
    service
        .refresh(user, false, id, &Fetcher::new(vec![response(200, FIRST)]))
        .await
        .unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM events")
            .fetch_one(&pool)
            .await
            .unwrap(),
        1
    );
    assert!(matches!(
        service
            .refresh(
                user,
                false,
                id,
                &Fetcher::new(vec![response(
                    200,
                    "BEGIN:VCALENDAR\nBEGIN:VEVENT\nUID:broken",
                )]),
            )
            .await,
        Err(FeedError::ParseFailed)
    ));
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM events")
            .fetch_one(&pool)
            .await
            .unwrap(),
        1
    );
}

#[tokio::test]
async fn sends_validators_for_304_and_never_projects_full_url() {
    let (_dir, _pool, service, user, calendar) = setup().await;
    let id = feed(&service, user, calendar).await;
    service
        .refresh(user, false, id, &Fetcher::new(vec![response(200, FIRST)]))
        .await
        .unwrap();
    let fetcher = Fetcher::new(vec![response(304, "")]);
    let projection = service.refresh(user, false, id, &fetcher).await.unwrap();
    assert_eq!(
        fetcher.validators.lock().unwrap().as_slice(),
        &[(
            Some("v1".into()),
            Some("Mon, 03 Aug 2026 00:00:00 GMT".into())
        )]
    );
    assert!(!projection.source_url_display.contains("private.ics"));
    assert!(!format!("{projection:?}").contains("token=secret"));
}

#[tokio::test]
async fn editor_cannot_create_a_feed() {
    let (_dir, pool, service, _user, calendar) = setup().await;
    let editor = sqlx::query("INSERT INTO users (normalized_email, status, created_at) VALUES ('editor@example.test', 'active', ?)").bind(NOW).execute(&pool).await.unwrap().last_insert_rowid();
    sqlx::query("INSERT INTO calendar_acl (calendar_id,user_id,role,created_at,updated_at) VALUES (?, ?, 'editor', ?, ?)").bind(calendar).bind(editor).bind(NOW).bind(NOW).execute(&pool).await.unwrap();
    assert!(matches!(
        service
            .create(
                editor,
                false,
                calendar,
                NewFeed {
                    source_url: "https://feeds.example.test/a.ics".into(),
                    refresh_interval_seconds: None
                }
            )
            .await,
        Err(FeedError::Denied)
    ));
}
