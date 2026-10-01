use std::{
    error::Error,
    fmt::{self, Display, Formatter},
};

use serde::{Deserialize, Serialize};

use crate::{
    ics::{NormalizedAlarm, NormalizedXProperty},
    security::SecretToken,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CaldavAccount {
    pub user_id: i64,
    pub principal_id: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CredentialMetadata {
    pub id: i64,
    pub label: String,
    pub created_at: i64,
    pub last_used_at: Option<i64>,
}

#[derive(Clone, Debug)]
pub struct IssuedCredential {
    pub metadata: CredentialMetadata,
    pub username: String,
    pub password: SecretToken,
    pub server_url: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ConnectionStatus {
    pub server_url: String,
    pub principal_id: Option<String>,
    pub credentials: Vec<CredentialMetadata>,
    pub last_successful_access_at: Option<i64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DavSession {
    pub user_id: i64,
    pub credential_id: i64,
    pub principal_id: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PrincipalInfo {
    pub user_id: i64,
    pub display_name: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CaldavCalendar {
    pub calendar_id: i64,
    pub owner_user_id: i64,
    pub owner_principal_id: Option<String>,
    pub name: String,
    pub description: Option<String>,
    pub color: String,
    pub role: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CaldavEventResource {
    pub event_id: i64,
    pub calendar_id: i64,
    pub uid: String,
    pub resource_name: String,
}

/// The allowlisted, client-owned metadata for one CalDAV event resource (T16).
///
/// This is the canonical, persisted form of the properties that survive
/// unrelated edits and whose explicit removals stay removed. The serialized
/// ICS (and thus the ETag) is a deterministic function of this value, so
/// identical content always yields the same ETag.
#[derive(Clone, Debug, Eq, PartialEq, Default, Serialize, Deserialize)]
pub struct CaldavClientProperties {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub categories: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transp: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub alarms: Vec<NormalizedAlarm>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub x_properties: Vec<NormalizedXProperty>,
}

impl CaldavClientProperties {
    /// True when no allowlisted property is set.
    pub fn is_empty(&self) -> bool {
        self.categories.is_empty()
            && self.url.is_none()
            && self.transp.is_none()
            && self.alarms.is_empty()
            && self.x_properties.is_empty()
    }
}

/// One ordered entry in the CalDAV change log. `id` is the monotonically
/// increasing revision used to page the log and to derive sync tokens.
///
/// `resource_name` carries the DAV resource name for deleted changes (where the
/// tombstone has cleared `event_id` and the name cannot be recovered from the
/// live mapping). For created/updated changes it is `None` and the name is
/// resolved from the live mapping.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CaldavEventChange {
    pub id: i64,
    pub calendar_id: i64,
    pub event_id: Option<i64>,
    pub change_type: String,
    pub created_at: i64,
    pub resource_name: Option<String>,
}

/// Classifies DAV request outcomes for operator monitoring.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum CaldavFailureClass {
    AuthFailure,
    RateLimited,
    MalformedRequest,
    OversizedBody,
    Unauthorized,
    NotFound,
    PreconditionFailed,
    InternalError,
}

impl CaldavFailureClass {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::AuthFailure => "auth_failure",
            Self::RateLimited => "rate_limited",
            Self::MalformedRequest => "malformed_request",
            Self::OversizedBody => "oversized_body",
            Self::Unauthorized => "unauthorized",
            Self::NotFound => "not_found",
            Self::PreconditionFailed => "precondition_failed",
            Self::InternalError => "internal_error",
        }
    }
}

/// In-memory counter for DAV failure classifications.
///
/// Thread-safe via `AtomicU64`; suitable for periodic scraping by an
/// operator monitoring agent. Share via `Arc<CaldavMetrics>`.
#[derive(Debug, Default)]
pub struct CaldavMetrics {
    auth_failures: std::sync::atomic::AtomicU64,
    rate_limited: std::sync::atomic::AtomicU64,
    malformed_requests: std::sync::atomic::AtomicU64,
    oversized_bodies: std::sync::atomic::AtomicU64,
    unauthorized: std::sync::atomic::AtomicU64,
    not_found: std::sync::atomic::AtomicU64,
    precondition_failed: std::sync::atomic::AtomicU64,
    internal_errors: std::sync::atomic::AtomicU64,
    successful_requests: std::sync::atomic::AtomicU64,
}

impl CaldavMetrics {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record_auth_failure(&self) {
        self.auth_failures
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    pub fn record_rate_limited(&self) {
        self.rate_limited
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    pub fn record_malformed_request(&self) {
        self.malformed_requests
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    pub fn record_oversized_body(&self) {
        self.oversized_bodies
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    pub fn record_unauthorized(&self) {
        self.unauthorized
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    pub fn record_not_found(&self) {
        self.not_found
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    pub fn record_precondition_failed(&self) {
        self.precondition_failed
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    pub fn record_internal_error(&self) {
        self.internal_errors
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    pub fn record_success(&self) {
        self.successful_requests
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    pub fn auth_failures(&self) -> u64 {
        self.auth_failures
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    pub fn rate_limited(&self) -> u64 {
        self.rate_limited.load(std::sync::atomic::Ordering::Relaxed)
    }

    pub fn malformed_requests(&self) -> u64 {
        self.malformed_requests
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    pub fn oversized_bodies(&self) -> u64 {
        self.oversized_bodies
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    pub fn unauthorized(&self) -> u64 {
        self.unauthorized.load(std::sync::atomic::Ordering::Relaxed)
    }

    pub fn not_found(&self) -> u64 {
        self.not_found.load(std::sync::atomic::Ordering::Relaxed)
    }

    pub fn precondition_failed(&self) -> u64 {
        self.precondition_failed
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    pub fn internal_errors(&self) -> u64 {
        self.internal_errors
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    pub fn successful_requests(&self) -> u64 {
        self.successful_requests
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    pub fn total_failures(&self) -> u64 {
        self.auth_failures()
            + self.rate_limited()
            + self.malformed_requests()
            + self.oversized_bodies()
            + self.unauthorized()
            + self.not_found()
            + self.precondition_failed()
            + self.internal_errors()
    }
}

#[derive(Debug)]
pub enum CaldavAuthError {
    InvalidCredentials,
    Revoked,
    RateLimited,
    Persistence,
    ResourceExists,
    UidConflict,
}

impl Display for CaldavAuthError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidCredentials => formatter.write_str("invalid credentials"),
            Self::Revoked => formatter.write_str("credential revoked"),
            Self::RateLimited => formatter.write_str("too many requests"),
            Self::Persistence => formatter.write_str("caldav account operation failed"),
            Self::ResourceExists => formatter.write_str("resource already exists"),
            Self::UidConflict => formatter.write_str("uid already exists in calendar"),
        }
    }
}

impl Error for CaldavAuthError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        None
    }
}
