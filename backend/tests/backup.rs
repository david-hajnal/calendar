use std::{fs, path::Path, process::Command, sync::Arc};

use commoncal_backend::{
    backup::{
        Aes256GcmEncryptor, BackupCommand, BackupEncryptor, BackupService, BackupUploader,
        RestoreCommand, RestoreService, UploadError, verify_snapshot,
    },
    config::{AppConfig, Environment},
    database::connect_and_migrate,
    http::Readiness,
};
use flate2::read::GzDecoder;
use sha2::{Digest, Sha256};
use sqlx::{
    Row, SqlitePool,
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
};
use tempfile::tempdir;

struct FailingUploader;

impl BackupUploader for FailingUploader {
    fn upload(&self, _artifact_path: &Path, _backup_id: &str) -> Result<(), UploadError> {
        Err(UploadError::new(
            "https://access-token:secret@example.test/backups",
        ))
    }
}

async fn database() -> (tempfile::TempDir, SqlitePool) {
    let directory = tempdir().unwrap();
    let config = AppConfig::with_database_path(
        Environment::Development,
        "127.0.0.1:0",
        None,
        directory.path().join("live.sqlite"),
    )
    .unwrap();
    let database = connect_and_migrate(&config, Readiness::new())
        .await
        .unwrap();
    (directory, database)
}

#[tokio::test]
async fn creates_verified_compressed_snapshot_during_controlled_writes() {
    let (directory, database) = database().await;
    sqlx::query("INSERT INTO users (normalized_email, status, created_at) VALUES ('backup@example.test', 'active', 1)")
        .execute(&database)
        .await
        .unwrap();

    let writes = Arc::new(database.clone());
    let writer = tokio::spawn(async move {
        for number in 0..20 {
            sqlx::query(
                "INSERT INTO users (normalized_email, status, created_at) VALUES (?, 'active', 1)",
            )
            .bind(format!("writer-{number}@example.test"))
            .execute(&*writes)
            .await
            .unwrap();
        }
    });

    let result = BackupService::new(database.clone())
        .create(directory.path().join("backups"), 123)
        .await
        .unwrap();
    writer.await.unwrap();

    assert!(result.artifact_path.exists());
    assert!(result.compressed_bytes > 0);
    let restored = directory.path().join("restored.sqlite");
    let compressed = fs::File::open(&result.artifact_path).unwrap();
    let mut decoder = GzDecoder::new(compressed);
    let mut output = fs::File::create(&restored).unwrap();
    std::io::copy(&mut decoder, &mut output).unwrap();
    verify_snapshot(&restored).await.unwrap();

    let restored_database = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(
            SqliteConnectOptions::new()
                .filename(&restored)
                .read_only(true),
        )
        .await
        .unwrap();
    let record = sqlx::query(
        "SELECT normalized_email FROM users WHERE normalized_email = 'backup@example.test'",
    )
    .fetch_one(&restored_database)
    .await
    .unwrap();
    assert_eq!(record.get::<String, _>(0), "backup@example.test");

    let metadata = sqlx::query("SELECT artifact_path, snapshot_sha256 FROM backup_metadata")
        .fetch_one(&database)
        .await
        .unwrap();
    assert_eq!(
        metadata.get::<String, _>(0),
        result.artifact_path.display().to_string()
    );
    assert_eq!(metadata.get::<String, _>(1), result.snapshot_sha256);
}

#[tokio::test]
async fn rejects_corrupt_snapshot() {
    let directory = tempdir().unwrap();
    let snapshot = directory.path().join("corrupt.sqlite");
    fs::write(&snapshot, b"not a sqlite database").unwrap();

    assert!(verify_snapshot(&snapshot).await.is_err());
}

#[test]
fn backup_command_requires_a_destination_directory() {
    assert_eq!(
        BackupCommand::from_arguments(&["/safe/backups".into()]).unwrap(),
        BackupCommand {
            destination_directory: "/safe/backups".into(),
        }
    );
    assert!(BackupCommand::from_arguments(&[]).is_err());
}

#[test]
fn encryptor_round_trip_recovers_the_compressed_artifact() {
    let encryptor = Aes256GcmEncryptor::new([7; 32]);
    let plaintext = b"compressed sqlite backup";

    let ciphertext = encryptor.encrypt(plaintext).unwrap();

    assert_ne!(ciphertext, plaintext);
    assert_eq!(encryptor.decrypt(&ciphertext).unwrap(), plaintext);
}

#[test]
fn from_hex_key_accepts_a_32_hex_key_and_round_trips() {
    let encryptor = Aes256GcmEncryptor::from_hex_key("0123456789abcdef0123456789abcdef").unwrap();
    let plaintext = b"compressed sqlite backup";

    let ciphertext = encryptor.encrypt(plaintext).unwrap();

    assert_ne!(ciphertext, plaintext);
    assert_eq!(encryptor.decrypt(&ciphertext).unwrap(), plaintext);
}

#[test]
fn from_hex_key_accepts_a_48_hex_key_and_round_trips() {
    let encryptor =
        Aes256GcmEncryptor::from_hex_key("0123456789abcdef0123456789abcdef0123456789abcdef")
            .unwrap();
    let plaintext = b"compressed sqlite backup";

    let ciphertext = encryptor.encrypt(plaintext).unwrap();

    assert_ne!(ciphertext, plaintext);
    assert_eq!(encryptor.decrypt(&ciphertext).unwrap(), plaintext);
}

#[test]
fn from_hex_key_derives_the_same_key_for_identical_hex_input() {
    let key = "0123456789abcdef0123456789abcdef";
    let first = Aes256GcmEncryptor::from_hex_key(key).unwrap();
    let second = Aes256GcmEncryptor::from_hex_key(key).unwrap();
    let plaintext = b"compressed sqlite backup";

    let ciphertext = first.encrypt(plaintext).unwrap();

    assert_eq!(second.decrypt(&ciphertext).unwrap(), plaintext);
}

#[test]
fn from_hex_key_rejects_invalid_keys() {
    let odd_length = Aes256GcmEncryptor::from_hex_key("0123456789abcdef0123456789abcde")
        .err()
        .unwrap();
    let too_short = Aes256GcmEncryptor::from_hex_key("0123456789abcdef")
        .err()
        .unwrap();
    let non_hex = Aes256GcmEncryptor::from_hex_key("0123456789abcdefg123456789abcdef")
        .err()
        .unwrap();

    assert!(odd_length.to_string().contains("key"));
    assert!(too_short.to_string().contains("key"));
    assert!(non_hex.to_string().contains("key"));
}

#[test]
fn from_hex_key_decrypts_ciphertext_made_with_a_raw_32_byte_key() {
    let legacy = Aes256GcmEncryptor::new([7_u8; 32]);
    let plaintext = b"compressed sqlite backup";

    let ciphertext = legacy.encrypt(plaintext).unwrap();

    let hex_key = "07".repeat(32);
    let encryptor = Aes256GcmEncryptor::from_hex_key(&hex_key).unwrap();
    assert_eq!(encryptor.decrypt(&ciphertext).unwrap(), plaintext);
}

#[test]
fn from_hex_key_decrypts_ciphertext_made_with_a_nonzero_high_nibble_key() {
    // Regression: decode_hex combined nibbles with `|` instead of `high << 4 | low`,
    // so any byte with a non-zero high nibble (e.g. 0xab) decoded to the wrong value.
    // The [7_u8; 32] guard above decodes correctly by coincidence (high nibble is 0).
    let legacy = Aes256GcmEncryptor::new([0xab_u8; 32]);
    let plaintext = b"compressed sqlite backup";

    let ciphertext = legacy.encrypt(plaintext).unwrap();

    let hex_key = "ab".repeat(32);
    let encryptor = Aes256GcmEncryptor::from_hex_key(&hex_key).unwrap();
    assert_eq!(encryptor.decrypt(&ciphertext).unwrap(), plaintext);
}

#[test]
fn from_hex_key_raw_path_decodes_every_byte_value_byte_identically() {
    // Strongest check: for every possible byte value, the 64-hex raw path must
    // decode byte-identically to the legacy new([u8; 32]) constructor. This
    // exhaustively covers all 256 (high, low) nibble pairs, so any residual
    // nibble-combination defect (e.g. `|` instead of `high << 4 | low`) fails.
    for byte in 0u16..=255u16 {
        let raw = [byte as u8; 32];
        let legacy = Aes256GcmEncryptor::new(raw);
        let plaintext = b"compressed sqlite backup";
        let ciphertext = legacy.encrypt(plaintext).unwrap();

        let hex_key = format!("{byte:02x}").repeat(32);
        let encryptor = Aes256GcmEncryptor::from_hex_key(&hex_key).unwrap();
        assert_eq!(
            encryptor.decrypt(&ciphertext).unwrap(),
            plaintext,
            "byte 0x{byte:02x} did not decode byte-identically through the raw path"
        );
    }
}

#[test]
fn from_hex_key_raw_path_decodes_a_mixed_byte_key_byte_identically() {
    // Mixed-byte regression: a realistic key spanning the full nibble space
    // (0x00, 0x0f, 0x10, 0xab, 0xf0, 0xff, ...) must decode byte-identically
    // to the legacy constructor, not just a single repeated byte.
    let raw: [u8; 32] = [
        0x00, 0x0f, 0x10, 0xab, 0xf0, 0xff, 0x5a, 0xa5, 0x01, 0x0e, 0x11, 0xba, 0xf1, 0xfe, 0x6b,
        0xb6, 0x02, 0x0d, 0x12, 0xaa, 0xf2, 0xfd, 0x7c, 0xc7, 0x03, 0x0c, 0x13, 0xab, 0xf3, 0xfc,
        0x8d, 0xd8,
    ];
    let legacy = Aes256GcmEncryptor::new(raw);
    let plaintext = b"compressed sqlite backup";
    let ciphertext = legacy.encrypt(plaintext).unwrap();

    let hex_key: String = raw.iter().map(|b| format!("{b:02x}")).collect();
    let encryptor = Aes256GcmEncryptor::from_hex_key(&hex_key).unwrap();
    assert_eq!(
        encryptor.decrypt(&ciphertext).unwrap(),
        plaintext,
        "mixed-byte key did not decode byte-identically through the raw path"
    );
}

#[test]
fn from_hex_key_derived_path_matches_independently_computed_kdf_output() {
    // Pin the derived-key KDF output: the 48-hex path must produce a byte-identical
    // AES key to Sha256(b"commoncal/backup-key/v1\0" || decoded). A change to the
    // domain separator, the domain || material order, or the hash would let this
    // cross-decrypt fail, exposing backups that would otherwise silently stop decrypting.
    let hex_key = "0123456789abcdef0123456789abcdef0123456789abcdef";
    let decoded: Vec<u8> = hex_key
        .as_bytes()
        .chunks(2)
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect();
    let mut hasher = Sha256::new();
    hasher.update(b"commoncal/backup-key/v1\0");
    hasher.update(&decoded);
    let expected_key: [u8; 32] = hasher.finalize().into();

    let reference = Aes256GcmEncryptor::new(expected_key);
    let plaintext = b"compressed sqlite backup";
    let ciphertext = reference.encrypt(plaintext).unwrap();

    let production = Aes256GcmEncryptor::from_hex_key(hex_key).unwrap();
    assert_eq!(
        production.decrypt(&ciphertext).unwrap(),
        plaintext,
        "derived path did not produce the pinned KDF key output"
    );
}

#[test]
fn from_hex_key_boundary_lengths_case_and_edges() {
    let hex = "0123456789abcdef";
    let key = |n: usize| {
        hex.repeat(n.div_ceil(16))
            .chars()
            .take(n)
            .collect::<String>()
    };

    // Odd lengths are always rejected, regardless of size.
    for n in [1usize, 31, 33, 63, 65] {
        assert!(
            Aes256GcmEncryptor::from_hex_key(&key(n)).is_err(),
            "len {n} should be odd-rejected"
        );
    }
    // Even lengths below 32 are rejected; 32 and above are accepted.
    for n in [0usize, 16, 30] {
        assert!(
            Aes256GcmEncryptor::from_hex_key(&key(n)).is_err(),
            "len {n} should be short-rejected"
        );
    }
    for n in [32usize, 34, 64, 66, 68] {
        assert!(
            Aes256GcmEncryptor::from_hex_key(&key(n)).is_ok(),
            "len {n} should be accepted"
        );
    }

    // Uppercase hex is accepted.
    assert!(Aes256GcmEncryptor::from_hex_key("0123456789ABCDEF0123456789ABCDEF").is_ok());
    // Non-hex at the first and last position is rejected.
    let bad = "g123456789abcdef0123456789abcdef";
    let error = Aes256GcmEncryptor::from_hex_key(bad).err().unwrap();
    assert!(error.to_string().contains("key"));
    // The error must not echo the (invalid) key material.
    assert!(!error.to_string().contains("g123456789abcdef"));
    assert!(Aes256GcmEncryptor::from_hex_key("0123456789abcdef0123456789abcdefg").is_err());
}

#[test]
fn from_hex_key_raw_path_is_exactly_at_64_hex_chars() {
    let base = [7_u8; 32];
    let hex64: String = base.iter().map(|b| format!("{b:02x}")).collect();
    let legacy = Aes256GcmEncryptor::new(base);
    let ciphertext = legacy.encrypt(b"compressed sqlite backup").unwrap();

    // 64-hex key is the raw 32-byte key: decrypts legacy ciphertext.
    let raw = Aes256GcmEncryptor::from_hex_key(&hex64).unwrap();
    assert_eq!(
        raw.decrypt(&ciphertext).unwrap(),
        b"compressed sqlite backup"
    );

    // 66-hex key is NOT raw (decoded to 33 bytes) so it derives a different key
    // and must NOT decrypt the legacy ciphertext.
    let derived = Aes256GcmEncryptor::from_hex_key(&format!("{hex64}00")).unwrap();
    assert!(derived.decrypt(&ciphertext).is_err());
}

#[tokio::test]
async fn backup_cli_round_trips_with_a_48_hex_key() {
    let (directory, database) = database().await;
    sqlx::query("INSERT INTO users (normalized_email, status, created_at) VALUES ('cli-48hex@example.test', 'active', 1)")
        .execute(&database)
        .await
        .unwrap();
    database.close().await;

    let database_path = directory.path().join("live.sqlite");
    let backups = directory.path().join("backups");
    let key = "0123456789abcdef0123456789abcdef0123456789abcdef";
    let output = Command::new(env!("CARGO_BIN_EXE_commoncal-backend"))
        .arg("backup")
        .arg(&backups)
        .env("APP_ENV", "development")
        .env("DATABASE_PATH", &database_path)
        .env("BACKUP_ENCRYPTION_KEY_HEX", key)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "backup command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let artifact = fs::read_dir(&backups)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| path.extension().is_some_and(|extension| extension == "enc"))
        .expect("backup CLI should create an encrypted artifact");
    let restored = directory.path().join("clean-environment.sqlite");
    RestoreService::restore_encrypted(
        &artifact,
        &restored,
        &Aes256GcmEncryptor::from_hex_key(key).unwrap(),
    )
    .await
    .unwrap();
    verify_snapshot(&restored).await.unwrap();

    let restored_database = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(
            SqliteConnectOptions::new()
                .filename(&restored)
                .read_only(true),
        )
        .await
        .unwrap();
    let record = sqlx::query(
        "SELECT normalized_email FROM users WHERE normalized_email = 'cli-48hex@example.test'",
    )
    .fetch_one(&restored_database)
    .await
    .unwrap();
    assert_eq!(record.get::<String, _>(0), "cli-48hex@example.test");
}

#[tokio::test]
async fn upload_failure_retains_encrypted_local_recovery_artifact_without_logging_secret() {
    let (directory, database) = database().await;
    let encryptor = Aes256GcmEncryptor::new([9; 32]);

    let error = BackupService::new(database.clone())
        .create_encrypted_and_upload(
            directory.path().join("backups"),
            123,
            &encryptor,
            Some(&FailingUploader),
        )
        .await
        .unwrap_err();

    let local_artifacts = fs::read_dir(directory.path().join("backups"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "enc"))
        .collect::<Vec<_>>();
    assert_eq!(local_artifacts.len(), 1);
    assert!(local_artifacts[0].metadata().unwrap().len() > 0);
    assert!(!error.to_string().contains("access-token"));
    assert!(!error.to_string().contains("secret"));

    let metadata = sqlx::query(
        "SELECT artifact_path, encryption_algorithm, upload_status FROM backup_metadata",
    )
    .fetch_one(&database)
    .await
    .unwrap();
    assert_eq!(
        metadata.get::<String, _>(0),
        local_artifacts[0].display().to_string()
    );
    assert_eq!(metadata.get::<String, _>(1), "AES-256-GCM");
    assert_eq!(metadata.get::<String, _>(2), "failed");
}

#[tokio::test]
async fn restore_decrypts_decompresses_and_verifies_representative_records() {
    let (directory, database) = database().await;
    sqlx::query("INSERT INTO users (normalized_email, status, created_at) VALUES ('restore@example.test', 'active', 1)")
        .execute(&database)
        .await
        .unwrap();
    let encryptor = Aes256GcmEncryptor::new([3; 32]);
    let backup = BackupService::new(database)
        .create_encrypted_and_upload(directory.path().join("backups"), 123, &encryptor, None)
        .await
        .unwrap();
    let restored = directory.path().join("clean-environment.sqlite");

    RestoreService::restore_encrypted(&backup.artifact_path, &restored, &encryptor)
        .await
        .unwrap();

    verify_snapshot(&restored).await.unwrap();
    let restored_database = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(
            SqliteConnectOptions::new()
                .filename(&restored)
                .read_only(true),
        )
        .await
        .unwrap();
    let record = sqlx::query(
        "SELECT normalized_email FROM users WHERE normalized_email = 'restore@example.test'",
    )
    .fetch_one(&restored_database)
    .await
    .unwrap();
    assert_eq!(record.get::<String, _>(0), "restore@example.test");
}

#[tokio::test]
async fn backup_cli_creates_an_encrypted_artifact_that_restores_a_representative_record() {
    let (directory, database) = database().await;
    sqlx::query("INSERT INTO users (normalized_email, status, created_at) VALUES ('cli-restore@example.test', 'active', 1)")
        .execute(&database)
        .await
        .unwrap();
    database.close().await;

    let database_path = directory.path().join("live.sqlite");
    let backups = directory.path().join("backups");
    let key = "0303030303030303030303030303030303030303030303030303030303030303";
    let output = Command::new(env!("CARGO_BIN_EXE_commoncal-backend"))
        .arg("backup")
        .arg(&backups)
        .env("APP_ENV", "development")
        .env("DATABASE_PATH", &database_path)
        .env("BACKUP_ENCRYPTION_KEY_HEX", key)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "backup command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let artifact = fs::read_dir(&backups)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| path.extension().is_some_and(|extension| extension == "enc"))
        .expect("backup CLI should create an encrypted artifact");
    let restored = directory.path().join("clean-environment.sqlite");
    RestoreService::restore_encrypted(
        &artifact,
        &restored,
        &Aes256GcmEncryptor::from_hex_key(key).unwrap(),
    )
    .await
    .unwrap();
    verify_snapshot(&restored).await.unwrap();

    let restored_database = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(
            SqliteConnectOptions::new()
                .filename(&restored)
                .read_only(true),
        )
        .await
        .unwrap();
    let record = sqlx::query(
        "SELECT normalized_email FROM users WHERE normalized_email = 'cli-restore@example.test'",
    )
    .fetch_one(&restored_database)
    .await
    .unwrap();
    assert_eq!(record.get::<String, _>(0), "cli-restore@example.test");
}

#[tokio::test]
async fn restore_rejects_corrupt_encrypted_artifacts_without_replacing_destination() {
    let directory = tempdir().unwrap();
    let artifact = directory.path().join("corrupt.sqlite.gz.enc");
    let destination = directory.path().join("destination.sqlite");
    fs::write(&artifact, b"not an encrypted backup").unwrap();
    fs::write(&destination, b"preserve this database").unwrap();

    let error = RestoreService::restore_encrypted(
        &artifact,
        &destination,
        &Aes256GcmEncryptor::new([4; 32]),
    )
    .await
    .unwrap_err();

    assert!(error.to_string().contains("restore artifact"));
    assert_eq!(fs::read(&destination).unwrap(), b"preserve this database");
}

#[test]
fn restore_command_requires_artifact_and_clean_destination() {
    assert_eq!(
        RestoreCommand::from_arguments(&["backup.sqlite.gz.enc".into(), "clean.sqlite".into()])
            .unwrap(),
        RestoreCommand {
            artifact_path: "backup.sqlite.gz.enc".into(),
            destination_database: "clean.sqlite".into(),
        }
    );
    assert!(RestoreCommand::from_arguments(&["backup.sqlite.gz.enc".into()]).is_err());
}

#[test]
fn restore_refuses_a_production_database_target_without_exposing_key_material() {
    let command =
        RestoreCommand::from_arguments(&["backup.sqlite.gz.enc".into(), "live.sqlite".into()])
            .unwrap();
    let error = command
        .refuse_production_target(Path::new("live.sqlite"))
        .unwrap_err();

    assert!(error.to_string().contains("refuses"));
    assert!(!error.to_string().contains("0123456789abcdef"));
}

#[test]
fn restore_cli_refuses_the_configured_production_database_before_reading_key_material() {
    let directory = tempdir().unwrap();
    let database_path = directory.path().join("live.sqlite");
    let output = Command::new(env!("CARGO_BIN_EXE_commoncal-backend"))
        .arg("restore")
        .arg("backup.sqlite.gz.enc")
        .arg(&database_path)
        .env("APP_ENV", "production")
        .env("SESSION_SECRET", "not-a-real-production-secret")
        .env("DATABASE_PATH", &database_path)
        .env("BACKUP_ENCRYPTION_KEY_HEX", "0123456789abcdef")
        .output()
        .unwrap();
    let output = String::from_utf8_lossy(&output.stderr);

    assert!(!output.contains("0123456789abcdef"));
    assert!(output.contains("restore refuses to overwrite"));
}
