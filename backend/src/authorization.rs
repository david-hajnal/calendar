use std::{
    error::Error,
    fmt::{self, Display, Formatter},
    str::FromStr,
};

use crate::identity::UserStatus;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PlatformRole {
    User,
    Superadmin,
}

impl FromStr for PlatformRole {
    type Err = UnknownRole;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "user" => Ok(Self::User),
            "superadmin" => Ok(Self::Superadmin),
            _ => Err(UnknownRole),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CalendarRole {
    Owner,
    Manager,
    Editor,
    Viewer,
    FreeBusyViewer,
}

impl FromStr for CalendarRole {
    type Err = UnknownRole;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "owner" => Ok(Self::Owner),
            "manager" => Ok(Self::Manager),
            "editor" => Ok(Self::Editor),
            "viewer" => Ok(Self::Viewer),
            "free_busy_viewer" => Ok(Self::FreeBusyViewer),
            _ => Err(UnknownRole),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UnknownRole;

impl Display for UnknownRole {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str("unrecognized role")
    }
}

impl Error for UnknownRole {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CalendarAction {
    ReadDetails,
    ReadFreeBusy,
    CreateEvent,
    EditAnyEvent,
    ManageSettings,
    ManageAcl,
    TransferOwnership,
    DeleteCalendar,
}

/// Deliberately carries no resource metadata or denial reason.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AuthorizationDecision {
    Allow,
    Deny,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PlatformAction {
    ManageUsers,
}

pub fn authorize_platform_action(
    user_status: UserStatus,
    platform_role: Option<PlatformRole>,
    _action: PlatformAction,
) -> AuthorizationDecision {
    if user_status == UserStatus::Active && platform_role == Some(PlatformRole::Superadmin) {
        AuthorizationDecision::Allow
    } else {
        AuthorizationDecision::Deny
    }
}

pub fn authorize_calendar_action(
    user_status: UserStatus,
    platform_role: Option<PlatformRole>,
    calendar_role: Option<CalendarRole>,
    action: CalendarAction,
) -> AuthorizationDecision {
    if user_status != UserStatus::Active || platform_role.is_none() {
        return AuthorizationDecision::Deny;
    }

    let Some(calendar_role) = calendar_role else {
        return AuthorizationDecision::Deny;
    };

    let allowed = matches!(
        (calendar_role, action),
        (CalendarRole::Owner, _)
            | (
                CalendarRole::Manager,
                CalendarAction::ReadDetails
                    | CalendarAction::ReadFreeBusy
                    | CalendarAction::CreateEvent
                    | CalendarAction::EditAnyEvent
                    | CalendarAction::ManageSettings
                    | CalendarAction::ManageAcl,
            )
            | (
                CalendarRole::Editor,
                CalendarAction::ReadDetails
                    | CalendarAction::ReadFreeBusy
                    | CalendarAction::CreateEvent
                    | CalendarAction::EditAnyEvent,
            )
            | (
                CalendarRole::Viewer,
                CalendarAction::ReadDetails | CalendarAction::ReadFreeBusy,
            )
            | (CalendarRole::FreeBusyViewer, CalendarAction::ReadFreeBusy)
    );

    if allowed {
        AuthorizationDecision::Allow
    } else {
        AuthorizationDecision::Deny
    }
}
