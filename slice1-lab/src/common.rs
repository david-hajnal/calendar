//! Shared configuration and constants for the Slice 1 disposable lab.
//!
//! All values are loopback test values. Nothing here is a real secret or a
//! production URL. The scope catalog mirrors the CommonCal scope catalog that
//! the real consent adapter (Slice 2) will enforce.

use std::net::SocketAddr;

/// The fixed CommonCal scope catalog. Consent may only grant scopes in this set.
pub const SCOPE_CATALOG: &[&str] = &[
    "commoncal.calendar.metadata.read",
    "commoncal.availability.read",
    "commoncal.event.read.basic",
    "commoncal.event.read.details",
    "commoncal.event.create",
    "commoncal.event.update",
    "commoncal.event.delete",
    "commoncal.reminder.read",
    "commoncal.reminder.write",
];

/// A scope deliberately outside the catalog, used to prove consent fails closed.
pub const EVIL_SCOPE: &str = "evil.unknown.scope";

/// The fixed test subject (numeric, matches the future CommonCal user ID contract).
pub const FIXED_SUBJECT: &str = "1";

/// The exact MCP resource / audience the consent adapter is allowed to grant.
pub const RESOURCE_URL: &str = "http://127.0.0.1:3001/mcp";

/// Candidate authorization service issuer (loopback only).
pub const AUTH_ISSUER: &str = "http://127.0.0.1:4000";

/// The MCP echo endpoint base URL (loopback).
pub const MCP_ECHO: &str = "http://127.0.0.1:3001";

/// The exact loopback redirect URI registered for the DCR client.
pub const LOOPBACK_REDIRECT: &str = "http://127.0.0.1:8321/callback";

/// The fixed test approval: the scopes the one fixed test subject approves.
/// For the lab this is the full catalog. In production this is the user's choice.
pub fn fixed_approval() -> Vec<String> {
    SCOPE_CATALOG.iter().map(|s| s.to_string()).collect()
}

/// Compute `requested ∩ catalog ∩ fixed-approval`. This is the ONLY set of
/// scopes the consent adapter may grant. Unknown scopes are dropped.
pub fn granted_scopes(requested: &[String]) -> Vec<String> {
    let approval_vec = fixed_approval();
    let approval: std::collections::BTreeSet<&str> =
        approval_vec.iter().map(|s| s.as_str()).collect();
    let catalog: std::collections::BTreeSet<&str> = SCOPE_CATALOG.iter().copied().collect();
    let mut out = Vec::new();
    for scope in requested {
        let s: &str = scope.as_str();
        if catalog.contains(s) && approval.contains(s) {
            out.push(scope.clone());
        }
    }
    out.sort();
    out.dedup();
    out
}

/// Lab configuration resolved from environment (with loopback defaults).
#[derive(Clone, Debug)]
pub struct LabConfig {
    pub auth_issuer: String,
    pub resource_url: String,
    pub issuer: String,
    pub mcp_echo: String,
    pub loopback_redirect: String,
}

impl LabConfig {
    pub fn from_env() -> Self {
        Self {
            auth_issuer: env_or("LAB_ISSUER", AUTH_ISSUER),
            resource_url: env_or("LAB_RESOURCE_URL", RESOURCE_URL),
            issuer: env_or("LAB_ISSUER", AUTH_ISSUER),
            mcp_echo: env_or("LAB_MCP_ECHO", MCP_ECHO),
            loopback_redirect: env_or("LAB_LOOPBACK_REDIRECT", LOOPBACK_REDIRECT),
        }
    }
}

fn env_or(key: &str, default: impl AsRef<str>) -> String {
    std::env::var(key).unwrap_or_else(|_| default.as_ref().to_string())
}

/// Parse a socket address from env or default.
pub fn bind_addr(env_key: &str, default: SocketAddr) -> SocketAddr {
    std::env::var(env_key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}
