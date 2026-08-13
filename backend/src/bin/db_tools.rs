use std::{
    env, fs,
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

use chrono::Datelike;
use sqlx::sqlite::SqlitePoolOptions;
use sqlx::{SqlitePool, migrate::Migrator, sqlite::SqliteConnectOptions};

const DEFAULT_DB_PATH: &str = "commoncal.sqlite";

static MIGRATOR: Migrator = sqlx::migrate!("./migrations");

#[tokio::main]
async fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        eprintln!("usage: db-tools <command> [args]");
        eprintln!("commands:");
        eprintln!("  status [db_path]              Show migration status");
        eprintln!("  reset [db_path] --force       Reset database and run migrations");
        eprintln!("  seed [db_path]                Seed test data");
        eprintln!("  new <description>             Create new migration file");
        std::process::exit(1);
    }

    let command = args[1].as_str();

    match command {
        "status" => {
            let db_path = args.get(2).map(|s| s.as_str()).unwrap_or(DEFAULT_DB_PATH);
            if let Err(e) = cmd_status(db_path).await {
                eprintln!("error: {e}");
                std::process::exit(1);
            }
        }
        "reset" => {
            let force = args.contains(&"--force".to_string());
            let db_path = args
                .iter()
                .skip(2)
                .find(|s| !s.starts_with("--"))
                .map(|s| s.as_str())
                .unwrap_or(DEFAULT_DB_PATH);
            if !force {
                eprintln!("error: use --force to confirm database reset");
                std::process::exit(1);
            }
            if let Err(e) = cmd_reset(db_path).await {
                eprintln!("error: {e}");
                std::process::exit(1);
            }
        }
        "seed" => {
            let db_path = args.get(2).map(|s| s.as_str()).unwrap_or(DEFAULT_DB_PATH);
            if let Err(e) = cmd_seed(db_path).await {
                eprintln!("error: {e}");
                std::process::exit(1);
            }
        }
        "new" => {
            if args.len() < 3 {
                eprintln!("error: usage: db-tools new <description>");
                std::process::exit(1);
            }
            let description = &args[2];
            if let Err(e) = cmd_new(description).await {
                eprintln!("error: {e}");
                std::process::exit(1);
            }
        }
        _ => {
            eprintln!("error: unknown command '{command}'");
            eprintln!("commands:");
            eprintln!("  status [db_path]              Show migration status");
            eprintln!("  reset [db_path] --force       Reset database and run migrations");
            eprintln!("  seed [db_path]                Seed test data");
            eprintln!("  new <description>             Create new migration file");
            std::process::exit(1);
        }
    }
}

async fn cmd_status(db_path: &str) -> Result<(), String> {
    let pool = connect(db_path).await?;

    // Check if migrations table exists
    let exists: Option<i64> = sqlx::query_scalar(
        "SELECT 1 FROM sqlite_master WHERE type='table' AND name='_sqlx_migrations'",
    )
    .fetch_optional(&pool)
    .await
    .map_err(|e| format!("failed to query migrations table: {e}"))?;

    if exists.is_none() {
        println!("no migrations applied");
        return Ok(());
    }

    // Get applied migrations
    let rows: Vec<(i64, String, String, bool, Vec<u8>, i64)> = sqlx::query_as(
        "SELECT version, description, installed_on, success, checksum, execution_time FROM _sqlx_migrations WHERE success = 1 ORDER BY installed_on"
    )
    .fetch_all(&pool)
    .await
    .map_err(|e| format!("failed to query migrations: {e}"))?;

    // Get migration count from compiled migrator
    let migration_count = MIGRATOR.migrations.len();

    println!("Applied migrations:");
    println!(
        "{:>3}  {:<10}  {:<35}  applied_at",
        "#", "status", "name"
    );
    println!("---  ----------  -----------------------------------  ----------");

    for (i, (version, description, installed_on, _success, _checksum, _execution_time)) in
        rows.iter().enumerate()
    {
        let ts = chrono::NaiveDateTime::parse_from_str(installed_on, "%Y-%m-%d %H:%M:%S")
            .ok()
            .map(|dt| dt.and_utc().timestamp())
            .and_then(|ts| chrono::DateTime::<chrono::Utc>::from_timestamp(ts, 0));
        let timestamp = ts
            .map(|dt| dt.format("%Y-%m-%d %H:%M").to_string())
            .unwrap_or_else(|| "unknown".to_string());
        println!(
            "{:>3}  {:<10}  {:<35}  {}",
            i + 1,
            "Applied",
            format!("{:04}_{}", version, description),
            timestamp
        );
    }

    let pending = migration_count.saturating_sub(rows.len());
    println!();
    println!("Total migrations: {migration_count}");
    println!("Applied: {}", rows.len());
    if pending > 0 {
        println!("Pending: {pending}");
    }

    Ok(())
}

async fn cmd_reset(db_path: &str) -> Result<(), String> {
    let path = Path::new(db_path);
    if path.exists() {
        fs::remove_file(path).map_err(|e| format!("failed to remove database file: {e}"))?;
        println!("removed: {db_path}");
    }

    println!("running migrations...");
    let pool = connect(db_path).await?;
    MIGRATOR
        .run(&pool)
        .await
        .map_err(|e| format!("migration failed: {e}"))?;

    println!("reset complete: {db_path}");
    Ok(())
}

async fn cmd_seed(db_path: &str) -> Result<(), String> {
    // Guard: never seed if the database looks like a production database
    // Production databases are typically in /app/data/ or have specific naming
    let abs_path = fs::canonicalize(db_path).unwrap_or_else(|_| db_path.into());
    let path_str = abs_path.to_string_lossy();
    if path_str.contains("/app/data/") || path_str.contains("production") {
        return Err("seed is not allowed on production database paths".to_string());
    }

    let pool = connect(db_path).await?;

    // Ensure migrations are run first
    MIGRATOR
        .run(&pool)
        .await
        .map_err(|e| format!("migration failed: {e}"))?;

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| format!("time error: {e}"))?
        .as_secs() as i64;

    // Create user
    let user_id: i64 = sqlx::query_scalar(
        "INSERT INTO users (normalized_email, display_name, status, is_superadmin, created_at)
         VALUES ('dev@example.com', 'Dev User', 'active', 1, ?)
         RETURNING id",
    )
    .bind(now)
    .fetch_one(&pool)
    .await
    .map_err(|e| format!("failed to create user: {e}"))?;

    // Optionally set password from DEFAULT_ADMIN_PASSWORD env var.
    if let Ok(password) = std::env::var("DEFAULT_ADMIN_PASSWORD")
        && !password.is_empty()
    {
            let hash = bcrypt::hash(&password, bcrypt::DEFAULT_COST)
                .map_err(|e| format!("bcrypt hash failed: {e}"))?;
            sqlx::query("UPDATE users SET password_hash = ? WHERE id = ?")
                .bind(&hash)
                .bind(user_id)
                .execute(&pool)
                .await
                .map_err(|e| format!("failed to set password: {e}"))?;
            println!("  password:  set (from DEFAULT_ADMIN_PASSWORD)");
        }

    // Create calendar
    let calendar_id: i64 = sqlx::query_scalar(
        "INSERT INTO calendars (owner_user_id, name, color, default_timezone, default_event_visibility, created_at, updated_at)
         VALUES (?, 'My Calendar', '#3b82f6', 'UTC', 'default', ?, ?)
         RETURNING id"
    )
    .bind(user_id)
    .bind(now)
    .bind(now)
    .fetch_one(&pool)
    .await
    .map_err(|e| format!("failed to create calendar: {e}"))?;

    // Create calendar_acl
    sqlx::query(
        "INSERT INTO calendar_acl (calendar_id, user_id, role, created_at, updated_at)
         VALUES (?, ?, 'owner', ?, ?)",
    )
    .bind(calendar_id)
    .bind(user_id)
    .bind(now)
    .bind(now)
    .execute(&pool)
    .await
    .map_err(|e| format!("failed to create calendar_acl: {e}"))?;

    // Create events spread across current month
    let events = generate_events(now);
    for event in &events {
        match event {
            EventData::Timed {
                title,
                description,
                location,
                start_utc,
                end_utc,
                timezone,
            } => {
                sqlx::query(
                    "INSERT INTO events (
                        calendar_id, title, description, location, status, event_kind,
                        timed_start_utc, timed_end_utc, event_timezone,
                        created_by_user_id, last_edited_by_user_id, version, created_at, updated_at
                    ) VALUES (?, ?, ?, ?, 'confirmed', 'timed', ?, ?, ?, ?, ?, 1, ?, ?)",
                )
                .bind(calendar_id)
                .bind(title)
                .bind(description)
                .bind(location)
                .bind(*start_utc)
                .bind(*end_utc)
                .bind(timezone)
                .bind(user_id)
                .bind(user_id)
                .bind(now)
                .bind(now)
                .execute(&pool)
                .await
                .map_err(|e| format!("failed to create event '{title}': {e}"))?;
            }
            EventData::AllDay {
                title,
                description,
                location,
                start_date,
                end_date,
            } => {
                sqlx::query(
                    "INSERT INTO events (
                        calendar_id, title, description, location, status, event_kind,
                        all_day_start_date, all_day_end_date,
                        created_by_user_id, last_edited_by_user_id, version, created_at, updated_at
                    ) VALUES (?, ?, ?, ?, 'confirmed', 'all_day', ?, ?, ?, ?, 1, ?, ?)",
                )
                .bind(calendar_id)
                .bind(title)
                .bind(description)
                .bind(location)
                .bind(start_date)
                .bind(end_date)
                .bind(user_id)
                .bind(user_id)
                .bind(now)
                .bind(now)
                .execute(&pool)
                .await
                .map_err(|e| format!("failed to create event '{title}': {e}"))?;
            }
        }
    }

    println!("seeded:");
    println!("  email:     dev@example.com");
    println!("  display:   Dev User");
    println!("  superadmin: true");
    println!("  calendar:  My Calendar");
    println!("  events:    {} events created", events.len());

    Ok(())
}

async fn cmd_new(description: &str) -> Result<(), String> {
    let migrations_dir = Path::new("backend/migrations");
    if !migrations_dir.exists() {
        return Err("migrations directory not found: backend/migrations".to_string());
    }

    // Find highest sequence number
    let mut max_seq: u32 = 0;
    let entries = fs::read_dir(migrations_dir)
        .map_err(|e| format!("failed to read migrations directory: {e}"))?;

    for entry in entries {
        let entry = entry.map_err(|e| format!("failed to read entry: {e}"))?;
        let filename = entry.file_name();
        let name = filename.to_string_lossy();

        if !name.ends_with(".sql") {
            continue;
        }

        // Parse NNNN_ prefix
        if let Some(seq_str) = name.split('_').next()
            && let Ok(seq) = seq_str.parse::<u32>()
            && seq > max_seq
        {
            max_seq = seq;
        }
    }

    let new_seq = max_seq + 1;
    let padded = format!("{:04}", new_seq);
    let slug = description
        .to_lowercase()
        .replace([' ', '-'], "_");
    let filename = format!("{padded}_{slug}.sql");
    let filepath = migrations_dir.join(&filename);

    let content = format!(
        "-- Migration: {description}\n-- Created: {}\n",
        chrono::Utc::now().format("%Y-%m-%d")
    );

    fs::write(&filepath, content).map_err(|e| format!("failed to create migration file: {e}"))?;

    println!("created: migrations/{filename}");
    Ok(())
}

#[derive(Debug)]
enum EventData {
    Timed {
        title: String,
        description: String,
        location: String,
        start_utc: i64,
        end_utc: i64,
        timezone: String,
    },
    AllDay {
        title: String,
        description: String,
        location: String,
        start_date: String,
        end_date: String,
    },
}

fn generate_events(now: i64) -> Vec<EventData> {
    let dt = chrono::DateTime::<chrono::Utc>::from_timestamp(now, 0).expect("valid timestamp");
    let year = dt.year();
    let month = dt.month();

    // Find first day of current month
    let first_day = chrono::NaiveDate::from_ymd_opt(year, month, 1).expect("valid date");

    // Find last day of current month
    let next_month = if month == 12 {
        chrono::NaiveDate::from_ymd_opt(year + 1, 1, 1).expect("valid date")
    } else {
        chrono::NaiveDate::from_ymd_opt(year, month + 1, 1).expect("valid date")
    };
    let _last_day = next_month - chrono::Days::new(1);

    // Week 1: Monday
    let days_from_monday = first_day.weekday().num_days_from_monday() as u64;
    let w1_mon = first_day
        .checked_sub_days(chrono::Days::new(days_from_monday))
        .expect("valid date");
    // Week 2: Monday
    let w2_mon = w1_mon + chrono::TimeDelta::days(7);
    // Week 3: Monday
    let w3_mon = w2_mon + chrono::TimeDelta::days(7);
    // Week 4: Monday
    let w4_mon = w3_mon + chrono::TimeDelta::days(7);
    // Week 5: Monday (if in current month)
    let _w5_mon = w4_mon + chrono::Days::new(7);

    let mut events = Vec::new();

    // Week 1: Sprint Planning (timed, Mon 9-10am UTC)
    if w1_mon.month() == month {
        let start = w1_mon
            .and_hms_opt(9, 0, 0)
            .expect("valid time")
            .and_utc()
            .timestamp();
        let end = w1_mon
            .and_hms_opt(10, 0, 0)
            .expect("valid time")
            .and_utc()
            .timestamp();
        events.push(EventData::Timed {
            title: "Sprint Planning".to_string(),
            description: "Weekly sprint planning session".to_string(),
            location: "Conference Room A".to_string(),
            start_utc: start,
            end_utc: end,
            timezone: "America/New_York".to_string(),
        });
    }

    // Week 1: Team Standup (all-day, Tue)
    let w1_tue = w1_mon + chrono::TimeDelta::days(1);
    if w1_tue.month() == month {
        let start = w1_tue.format("%Y-%m-%d").to_string();
        let end = (w1_tue + chrono::TimeDelta::days(1))
            .format("%Y-%m-%d")
            .to_string();
        events.push(EventData::AllDay {
            title: "Team Standup".to_string(),
            description: "Daily standup moved to all-day for planning week".to_string(),
            location: "Virtual".to_string(),
            start_date: start,
            end_date: end,
        });
    }

    // Week 2: Code Review (timed, Wed 14:00-15:30 UTC)
    let w2_wed = w2_mon + chrono::TimeDelta::days(2);
    if w2_wed.month() == month {
        let start = w2_wed
            .and_hms_opt(14, 0, 0)
            .expect("valid time")
            .and_utc()
            .timestamp();
        let end = w2_wed
            .and_hms_opt(15, 30, 0)
            .expect("valid time")
            .and_utc()
            .timestamp();
        events.push(EventData::Timed {
            title: "Code Review".to_string(),
            description: "Weekly code review session".to_string(),
            location: "Room B".to_string(),
            start_utc: start,
            end_utc: end,
            timezone: "America/Los_Angeles".to_string(),
        });
    }

    // Week 2: Design Sync (all-day, Thu)
    let w2_thu = w2_mon + chrono::TimeDelta::days(3);
    if w2_thu.month() == month {
        let start = w2_thu.format("%Y-%m-%d").to_string();
        let end = (w2_thu + chrono::TimeDelta::days(1))
            .format("%Y-%m-%d")
            .to_string();
        events.push(EventData::AllDay {
            title: "Design Sync".to_string(),
            description: "Design system review and planning".to_string(),
            location: "Design Lab".to_string(),
            start_date: start,
            end_date: end,
        });
    }

    // Week 3: Hack Friday (timed, Fri 10:00-16:00 UTC)
    let w3_fri = w3_mon + chrono::TimeDelta::days(4);
    if w3_fri.month() == month {
        let start = w3_fri
            .and_hms_opt(10, 0, 0)
            .expect("valid time")
            .and_utc()
            .timestamp();
        let end = w3_fri
            .and_hms_opt(16, 0, 0)
            .expect("valid time")
            .and_utc()
            .timestamp();
        events.push(EventData::Timed {
            title: "Hack Friday".to_string(),
            description: "Open hacking time for experiments".to_string(),
            location: "Anywhere".to_string(),
            start_utc: start,
            end_utc: end,
            timezone: "UTC".to_string(),
        });
    }

    // Week 4: Retrospective (timed, Mon 11:00-12:00 UTC)
    if w4_mon.month() == month {
        let start = w4_mon
            .and_hms_opt(11, 0, 0)
            .expect("valid time")
            .and_utc()
            .timestamp();
        let end = w4_mon
            .and_hms_opt(12, 0, 0)
            .expect("valid time")
            .and_utc()
            .timestamp();
        events.push(EventData::Timed {
            title: "Sprint Retrospective".to_string(),
            description: "End of sprint retrospective".to_string(),
            location: "Conference Room A".to_string(),
            start_utc: start,
            end_utc: end,
            timezone: "America/New_York".to_string(),
        });
    }

    events
}

async fn connect(db_path: &str) -> Result<SqlitePool, String> {
    let options = SqliteConnectOptions::new()
        .filename(db_path)
        .create_if_missing(true);
    let pool = SqlitePoolOptions::new()
        .connect_with(options)
        .await
        .map_err(|e| format!("failed to connect to database: {e}"))?;
    Ok(pool)
}
