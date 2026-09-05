use std::{
    error::Error,
    fmt::{self, Display, Formatter},
    fs::{self, File},
    io::{self, Read},
    path::{Path, PathBuf},
};

use aes_gcm::{
    Aes256Gcm, Nonce,
    aead::{Aead, KeyInit},
};
use flate2::{Compression, read::GzDecoder, write::GzEncoder};
use sha2::{Digest, Sha256};
use sqlx::{
    Row, SqlitePool,
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
};
use uuid::Uuid;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BackupMetadata {
    pub id: String,
    pub artifact_path: PathBuf,
    pub snapshot_sha256: String,
    pub compressed_sha256: String,
    pub snapshot_bytes: u64,
    pub compressed_bytes: u64,
    pub created_at: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BackupCommand {
    pub destination_directory: PathBuf,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RestoreCommand {
    pub artifact_path: PathBuf,
    pub destination_database: PathBuf,
}

impl RestoreCommand {
    pub fn from_arguments(arguments: &[String]) -> Result<Self, BackupError> {
        if arguments.len() != 2 {
            return Err(BackupError::InvalidCommand(
                "usage: commoncal-backend restore <encrypted-artifact> <clean-database-path>"
                    .into(),
            ));
        }
        Ok(Self {
            artifact_path: arguments[0].clone().into(),
            destination_database: arguments[1].clone().into(),
        })
    }

    pub fn refuse_production_target(&self, production_database: &Path) -> Result<(), BackupError> {
        if same_file_target(&self.destination_database, production_database) {
            return Err(BackupError::ProductionTarget);
        }
        Ok(())
    }
}

impl BackupCommand {
    pub fn from_arguments(arguments: &[String]) -> Result<Self, BackupError> {
        if arguments.len() != 1 {
            return Err(BackupError::InvalidCommand(
                "usage: commoncal-backend backup <destination-directory>".into(),
            ));
        }
        Ok(Self {
            destination_directory: arguments[0].clone().into(),
        })
    }
}

#[derive(Clone)]
pub struct BackupService {
    database: SqlitePool,
}

pub struct RestoreService;

/// Encrypts backup artifacts before they leave the local recovery directory.
pub trait BackupEncryptor: Send + Sync {
    fn encrypt(&self, plaintext: &[u8]) -> Result<Vec<u8>, BackupError>;
}

/// Sends an already-encrypted artifact to remote storage.
pub trait BackupUploader: Send + Sync {
    fn upload(&self, artifact_path: &Path, backup_id: &str) -> Result<(), UploadError>;
}

/// Deliberately omits remote-service details so error output cannot expose credentials.
#[derive(Debug)]
pub struct UploadError {
    _private: (),
}

impl UploadError {
    pub fn new(_detail: impl Into<String>) -> Self {
        Self { _private: () }
    }
}

/// AES-256-GCM artifact encryption with a random nonce prefixed to each ciphertext.
pub struct Aes256GcmEncryptor {
    cipher: Aes256Gcm,
}

impl Aes256GcmEncryptor {
    pub fn new(key: [u8; 32]) -> Self {
        Self {
            cipher: Aes256Gcm::new_from_slice(&key).expect("AES-256 keys are always 32 bytes"),
        }
    }

    pub fn decrypt(&self, ciphertext: &[u8]) -> Result<Vec<u8>, BackupError> {
        let (nonce, ciphertext) = ciphertext
            .split_first_chunk::<12>()
            .ok_or(BackupError::Encryption)?;
        self.cipher
            .decrypt(Nonce::from_slice(nonce), ciphertext)
            .map_err(|_| BackupError::Encryption)
    }

    pub fn from_hex_key(value: &str) -> Result<Self, BackupError> {
        if !value.len().is_multiple_of(2) {
            return Err(BackupError::InvalidKey(
                "key must contain an even number of hex characters".into(),
            ));
        }
        if value.len() < 32 {
            return Err(BackupError::InvalidKey(
                "key must be at least 32 hex characters".into(),
            ));
        }
        let decoded = decode_hex(value)?;
        if decoded.len() == 32 {
            return Ok(Self::new(decoded.try_into().expect("decoded length is 32")));
        }
        let mut hasher = Sha256::new();
        hasher.update(b"commoncal/backup-key/v1\0");
        hasher.update(&decoded);
        let key = hasher
            .finalize()
            .as_slice()
            .try_into()
            .expect("SHA-256 output is exactly 32 bytes");
        Ok(Self::new(key))
    }
}

fn decode_hex(value: &str) -> Result<Vec<u8>, BackupError> {
    let mut decoded = Vec::with_capacity(value.len() / 2);
    for chunk in value.as_bytes().as_chunks::<2>().0 {
        decoded.push((hex_nibble(chunk[0])? << 4) | hex_nibble(chunk[1])?);
    }
    Ok(decoded)
}

fn hex_nibble(byte: u8) -> Result<u8, BackupError> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        _ => Err(BackupError::InvalidKey(
            "key contains non-hex characters".into(),
        )),
    }
}

impl BackupEncryptor for Aes256GcmEncryptor {
    fn encrypt(&self, plaintext: &[u8]) -> Result<Vec<u8>, BackupError> {
        let mut nonce = [0_u8; 12];
        getrandom::fill(&mut nonce).map_err(|_| BackupError::Encryption)?;
        let ciphertext = self
            .cipher
            .encrypt(Nonce::from_slice(&nonce), plaintext)
            .map_err(|_| BackupError::Encryption)?;
        let mut artifact = nonce.to_vec();
        artifact.extend_from_slice(&ciphertext);
        Ok(artifact)
    }
}

impl BackupService {
    pub fn new(database: SqlitePool) -> Self {
        Self { database }
    }

    /// Creates a SQLite-consistent copy using SQLite's `VACUUM INTO` mechanism.
    pub async fn create(
        &self,
        destination_directory: impl AsRef<Path>,
        created_at: i64,
    ) -> Result<BackupMetadata, BackupError> {
        let destination_directory = destination_directory.as_ref();
        fs::create_dir_all(destination_directory).map_err(BackupError::Io)?;
        let id = Uuid::new_v4().to_string();
        let snapshot_path = destination_directory.join(format!("{id}.sqlite"));
        let artifact_path = destination_directory.join(format!("{id}.sqlite.gz"));

        // `VACUUM INTO` creates a transactionally consistent snapshot while the source remains live.
        let escaped_path = snapshot_path.display().to_string().replace('\'', "''");
        sqlx::query(&format!("VACUUM INTO '{escaped_path}'"))
            .execute(&self.database)
            .await
            .map_err(BackupError::Snapshot)?;

        verify_snapshot(&snapshot_path).await?;
        let snapshot_bytes = fs::metadata(&snapshot_path).map_err(BackupError::Io)?.len();
        let snapshot_sha256 = sha256_file(&snapshot_path)?;
        compress(&snapshot_path, &artifact_path)?;
        let compressed_bytes = fs::metadata(&artifact_path).map_err(BackupError::Io)?.len();
        let compressed_sha256 = sha256_file(&artifact_path)?;

        sqlx::query(
            "INSERT INTO backup_metadata (id, artifact_path, snapshot_sha256, compressed_sha256, snapshot_bytes, compressed_bytes, integrity_check, created_at) VALUES (?, ?, ?, ?, ?, ?, 'ok', ?)",
        )
        .bind(&id)
        .bind(artifact_path.display().to_string())
        .bind(&snapshot_sha256)
        .bind(&compressed_sha256)
        .bind(snapshot_bytes as i64)
        .bind(compressed_bytes as i64)
        .bind(created_at)
        .execute(&self.database)
        .await
        .map_err(BackupError::Metadata)?;

        fs::remove_file(&snapshot_path).map_err(BackupError::Io)?;
        Ok(BackupMetadata {
            id,
            artifact_path,
            snapshot_sha256,
            compressed_sha256,
            snapshot_bytes,
            compressed_bytes,
            created_at,
        })
    }

    /// The encrypted artifact is retained locally even when remote upload fails, so recovery
    /// never depends on the remote service being available.
    pub async fn create_encrypted_and_upload(
        &self,
        destination_directory: impl AsRef<Path>,
        created_at: i64,
        encryptor: &dyn BackupEncryptor,
        uploader: Option<&dyn BackupUploader>,
    ) -> Result<BackupMetadata, BackupError> {
        let mut metadata = self.create(destination_directory, created_at).await?;
        let encrypted_path = metadata.artifact_path.with_extension("gz.enc");
        let compressed = fs::read(&metadata.artifact_path).map_err(BackupError::Io)?;
        let encrypted = encryptor.encrypt(&compressed)?;
        fs::write(&encrypted_path, encrypted).map_err(BackupError::Io)?;
        fs::remove_file(&metadata.artifact_path).map_err(BackupError::Io)?;

        let encrypted_bytes = fs::metadata(&encrypted_path)
            .map_err(BackupError::Io)?
            .len();
        let encrypted_sha256 = sha256_file(&encrypted_path)?;
        let upload_status = if let Some(uploader) = uploader {
            if uploader.upload(&encrypted_path, &metadata.id).is_ok() {
                "uploaded"
            } else {
                "failed"
            }
        } else {
            "not_requested"
        };

        sqlx::query(
            "UPDATE backup_metadata SET artifact_path = ?, encryption_algorithm = 'AES-256-GCM', encrypted_sha256 = ?, encrypted_bytes = ?, upload_status = ? WHERE id = ?",
        )
        .bind(encrypted_path.display().to_string())
        .bind(encrypted_sha256)
        .bind(encrypted_bytes as i64)
        .bind(upload_status)
        .bind(&metadata.id)
        .execute(&self.database)
        .await
        .map_err(BackupError::Metadata)?;

        metadata.artifact_path = encrypted_path;
        if upload_status == "failed" {
            return Err(BackupError::Upload);
        }
        Ok(metadata)
    }
}

impl RestoreService {
    /// Restores only after authenticated decryption, decompression, and SQLite verification.
    /// The destination is replaced atomically only after all checks succeed.
    pub async fn restore_encrypted(
        artifact_path: impl AsRef<Path>,
        destination_database: impl AsRef<Path>,
        encryptor: &Aes256GcmEncryptor,
    ) -> Result<(), BackupError> {
        let artifact_path = artifact_path.as_ref();
        let destination_database = destination_database.as_ref();
        let encrypted = fs::read(artifact_path).map_err(BackupError::Io)?;
        let compressed = encryptor
            .decrypt(&encrypted)
            .map_err(|_| BackupError::RestoreArtifact)?;
        let temporary =
            destination_database.with_extension(format!("restore-{}.tmp", Uuid::new_v4()));

        let restore_result = (|| -> Result<(), BackupError> {
            let mut decoder = GzDecoder::new(compressed.as_slice());
            let mut output = File::create(&temporary).map_err(BackupError::Io)?;
            io::copy(&mut decoder, &mut output).map_err(|_| BackupError::RestoreArtifact)?;
            output.sync_all().map_err(BackupError::Io)?;
            Ok(())
        })();
        if let Err(error) = restore_result {
            let _ = fs::remove_file(&temporary);
            return Err(error);
        }

        if let Err(error) = verify_snapshot(&temporary).await {
            let _ = fs::remove_file(&temporary);
            return Err(error);
        }
        fs::rename(&temporary, destination_database).map_err(BackupError::Io)
    }
}

pub async fn verify_snapshot(path: impl AsRef<Path>) -> Result<(), BackupError> {
    let database = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(
            SqliteConnectOptions::new()
                .filename(path.as_ref())
                .read_only(true),
        )
        .await
        .map_err(BackupError::Integrity)?;
    let result: String = sqlx::query("PRAGMA integrity_check")
        .fetch_one(&database)
        .await
        .map_err(BackupError::Integrity)?
        .try_get(0)
        .map_err(BackupError::Integrity)?;
    database.close().await;
    if result == "ok" {
        Ok(())
    } else {
        Err(BackupError::InvalidIntegrity(result))
    }
}

fn compress(snapshot_path: &Path, artifact_path: &Path) -> Result<(), BackupError> {
    let source = File::open(snapshot_path).map_err(BackupError::Io)?;
    let artifact = File::create(artifact_path).map_err(BackupError::Io)?;
    let mut encoder = GzEncoder::new(artifact, Compression::default());
    io::copy(&mut io::BufReader::new(source), &mut encoder).map_err(BackupError::Io)?;
    encoder.finish().map_err(BackupError::Io)?;
    Ok(())
}

fn sha256_file(path: &Path) -> Result<String, BackupError> {
    let mut file = File::open(path).map_err(BackupError::Io)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 8192];
    loop {
        let count = file.read(&mut buffer).map_err(BackupError::Io)?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn same_file_target(left: &Path, right: &Path) -> bool {
    if left == right {
        return true;
    }
    match (fs::canonicalize(left), fs::canonicalize(right)) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

pub enum BackupError {
    Io(io::Error),
    Snapshot(sqlx::Error),
    Integrity(sqlx::Error),
    InvalidIntegrity(String),
    Metadata(sqlx::Error),
    Encryption,
    InvalidKey(String),
    Upload,
    RestoreArtifact,
    ProductionTarget,
    InvalidCommand(String),
}

impl fmt::Debug for BackupError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        Display::fmt(self, formatter)
    }
}

impl Display for BackupError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "backup file operation failed: {error}"),
            Self::Snapshot(error) => write!(formatter, "snapshot creation failed: {error}"),
            Self::Integrity(error) => write!(formatter, "snapshot integrity check failed: {error}"),
            Self::InvalidIntegrity(result) => {
                write!(formatter, "snapshot integrity check returned: {result}")
            }
            Self::Metadata(error) => write!(formatter, "backup metadata recording failed: {error}"),
            Self::Encryption => formatter.write_str("backup artifact encryption failed"),
            Self::InvalidKey(message) => {
                write!(formatter, "backup encryption key is invalid: {message}")
            }
            Self::Upload => formatter
                .write_str("backup upload failed; encrypted local recovery artifact retained"),
            Self::RestoreArtifact => formatter.write_str("restore artifact is invalid or corrupted"),
            Self::ProductionTarget => formatter.write_str(
                "restore refuses to overwrite the configured production database; restore into a clean environment instead",
            ),
            Self::InvalidCommand(message) => formatter.write_str(message),
        }
    }
}

impl Error for BackupError {}
