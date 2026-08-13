// Output schema types for structured MCP responses.
//
// All output is JSON-schema-valid structured data.
// Event descriptions are tagged as untrusted user-supplied data.

use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct ToolOutput {
    pub content: Vec<ContentBlock>,
}

#[derive(Debug, Serialize)]
pub enum ContentBlock {
    Text { text: String },
    Image { data: String, mime_type: String },
}

#[derive(Debug, Serialize)]
pub struct CalendarListOutput {
    pub calendars: Vec<CalendarSummary>,
}

#[derive(Debug, Serialize)]
pub struct CalendarSummary {
    pub id: i64,
    pub name: String,
    pub color: String,
    pub access: String,
}

#[derive(Debug, Serialize)]
pub struct AvailabilityOutput {
    pub slots: Vec<AvailabilitySlot>,
}

#[derive(Debug, Serialize)]
pub struct AvailabilitySlot {
    pub start: String,
    pub end: String,
    pub status: String, // "free", "busy", "tbd"
}

#[derive(Debug, Serialize)]
pub struct EventOutput {
    pub event: EventSummary,
    pub access: String,
}

#[derive(Debug, Serialize)]
pub struct EventSummary {
    pub id: i64,
    pub calendar_id: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<EventDescription>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
    pub status: String,
    pub event_kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_utc: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_utc: Option<String>,
    pub version: i64,
}

/// Event description tagged as untrusted user-supplied data.
/// Prevents prompt injection via event descriptions.
#[derive(Debug, Serialize)]
pub struct EventDescription {
    pub value: String,
    pub trust: &'static str, // "user_supplied_untrusted"
}

#[derive(Debug, Serialize)]
pub struct EventSearchOutput {
    pub events: Vec<EventSummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_page: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ReminderOutput {
    pub reminder_id: String,
    pub event_id: i64,
}

#[derive(Debug, Serialize)]
pub struct DeletePrepareOutput {
    pub intent_id: String,
    pub event_summary: EventSummary,
    pub expires_at: i64,
    pub confirmation_required: bool,
    pub confirmation_url: String,
}

#[derive(Debug, Serialize)]
pub struct DeleteCommitOutput {
    pub deleted: bool,
}

#[derive(Debug, Serialize)]
pub struct ConfirmationUrl {
    pub url: String,
}
