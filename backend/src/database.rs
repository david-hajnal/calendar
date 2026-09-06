use std::{
    error::Error,
    fmt::{self, Display, Formatter},
    time::Duration,
};

use sqlx::{
    SqlitePool,
    migrate::MigrateError,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions},
};

use crate::{config::AppConfig, http::Readiness};

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

pub async fn connect_and_migrate(
    config: &AppConfig,
    readiness: Readiness,
) -> Result<SqlitePool, DatabaseError> {
    readiness.mark_not_ready();

    let options = SqliteConnectOptions::new()
        .filename(config.database_path())
        .create_if_missing(true)
        .foreign_keys(true)
        .journal_mode(SqliteJournalMode::Wal)
        .busy_timeout(Duration::from_secs(5));
    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect_with(options)
        .await
        .map_err(DatabaseError::Connect)?;

    MIGRATOR
        .run(&pool)
        .await
        .map_err(DatabaseError::Migration)?;
    readiness.mark_ready();

    Ok(pool)
}

/// Opens an existing database for operations that must not mutate the source.
///
/// In particular, backup uses SQLite's `VACUUM INTO`, which can write the
/// destination while the source connection itself remains read-only. Keep this
/// path free of migrations and write-oriented connection pragmas.
pub async fn connect_read_only(config: &AppConfig) -> Result<SqlitePool, DatabaseError> {
    let options = SqliteConnectOptions::new()
        .filename(config.database_path())
        .read_only(true)
        .busy_timeout(Duration::from_secs(5));
    SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .map_err(DatabaseError::Connect)
}

#[derive(Debug)]
pub enum DatabaseError {
    Connect(sqlx::Error),
    Migration(MigrateError),
}

impl Display for DatabaseError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Connect(error) => write!(formatter, "database connection failed: {error}"),
            Self::Migration(error) => write!(formatter, "database migration failed: {error}"),
        }
    }
}

impl Error for DatabaseError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Connect(error) => Some(error),
            Self::Migration(error) => Some(error),
        }
    }
}
