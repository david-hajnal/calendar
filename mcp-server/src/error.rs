// Error types for the MCP server.

#[derive(Debug)]
pub enum TokenError {
    MissingToken,
    InvalidToken(String),
    Expired,
    InvalidAudience,
    InvalidIssuer,
    InvalidDpop,
    MissingDpop,
    Revoked,
}

impl std::fmt::Display for TokenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingToken => write!(f, "missing authorization token"),
            Self::InvalidToken(msg) => write!(f, "invalid token: {}", msg),
            Self::Expired => write!(f, "token has expired"),
            Self::InvalidAudience => write!(f, "token audience mismatch"),
            Self::InvalidIssuer => write!(f, "token issuer not trusted"),
            Self::InvalidDpop => write!(f, "invalid DPoP proof"),
            Self::MissingDpop => write!(f, "DPoP proof required"),
            Self::Revoked => write!(f, "token has been revoked"),
        }
    }
}

impl std::error::Error for TokenError {}

#[derive(Debug)]
pub enum GrantError {
    NoGrant,
    GrantExpired,
    GrantRevoked,
    CalendarNotInGrant,
    ToolPermissionDenied,
    Db(String),
}

impl std::fmt::Display for GrantError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoGrant => write!(f, "no MCP grant found"),
            Self::GrantExpired => write!(f, "MCP grant has expired"),
            Self::GrantRevoked => write!(f, "MCP grant has been revoked"),
            Self::CalendarNotInGrant => write!(f, "calendar not in grant"),
            Self::ToolPermissionDenied => write!(f, "tool permission denied"),
            Self::Db(msg) => write!(f, "database error: {}", msg),
        }
    }
}

impl std::error::Error for GrantError {}

#[derive(Debug)]
pub enum ToolError {
    Unauthorized(String),
    Forbidden(String),
    BadRequest(String),
    NotFound,
    Conflict(String),
    RateLimited,
    Internal(String),
}

impl ToolError {
    pub fn status_code(&self) -> u16 {
        match self {
            Self::Unauthorized(_) => 401,
            Self::Forbidden(_) => 403,
            Self::BadRequest(_) => 400,
            Self::NotFound => 404,
            Self::Conflict(_) => 409,
            Self::RateLimited => 429,
            Self::Internal(_) => 500,
        }
    }

    pub fn to_mcp_error(&self, id: Option<&serde_json::Value>) -> serde_json::Value {
        let (code, message) = match self {
            Self::Unauthorized(msg) => (-2000, format!("Unauthorized: {}", msg)),
            Self::Forbidden(msg) => (-2001, format!("Forbidden: {}", msg)),
            Self::BadRequest(msg) => (-32600, format!("Bad request: {}", msg)),
            Self::NotFound => (-32602, "Not found".to_string()),
            Self::Conflict(msg) => (-2003, format!("Conflict: {}", msg)),
            Self::RateLimited => (-2004, "Rate limit exceeded".to_string()),
            Self::Internal(msg) => (-32603, format!("Internal error: {}", msg)),
        };
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": {
                "code": code,
                "message": message,
            },
        })
    }
}

impl std::fmt::Display for ToolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unauthorized(msg) => write!(f, "unauthorized: {}", msg),
            Self::Forbidden(msg) => write!(f, "forbidden: {}", msg),
            Self::BadRequest(msg) => write!(f, "bad request: {}", msg),
            Self::NotFound => write!(f, "not found"),
            Self::Conflict(msg) => write!(f, "conflict: {}", msg),
            Self::RateLimited => write!(f, "rate limited"),
            Self::Internal(msg) => write!(f, "internal error: {}", msg),
        }
    }
}

impl std::error::Error for ToolError {}

#[derive(Debug)]
pub enum SecurityError {
    RateLimitExceeded,
    WeakAuthentication(String),
    AuthNotRecent(String),
}

impl std::fmt::Display for SecurityError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RateLimitExceeded => write!(f, "rate limit exceeded"),
            Self::WeakAuthentication(msg) => write!(f, "weak authentication: {}", msg),
            Self::AuthNotRecent(msg) => write!(f, "authentication not recent: {}", msg),
        }
    }
}

impl std::error::Error for SecurityError {}
