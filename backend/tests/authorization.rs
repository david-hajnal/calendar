use std::str::FromStr;

use commoncal_backend::{
    authorization::{
        AuthorizationDecision, CalendarAction, CalendarRole, PlatformRole,
        authorize_calendar_action,
    },
    identity::UserStatus,
};

const ACTIONS: [CalendarAction; 8] = [
    CalendarAction::ReadDetails,
    CalendarAction::ReadFreeBusy,
    CalendarAction::CreateEvent,
    CalendarAction::EditAnyEvent,
    CalendarAction::ManageSettings,
    CalendarAction::ManageAcl,
    CalendarAction::TransferOwnership,
    CalendarAction::DeleteCalendar,
];

#[test]
fn every_calendar_role_action_pair_matches_the_permission_matrix() {
    let cases = [
        (
            CalendarRole::Owner,
            [true, true, true, true, true, true, true, true],
        ),
        (
            CalendarRole::Manager,
            [true, true, true, true, true, true, false, false],
        ),
        (
            CalendarRole::Editor,
            [true, true, true, true, false, false, false, false],
        ),
        (
            CalendarRole::Viewer,
            [true, true, false, false, false, false, false, false],
        ),
        (
            CalendarRole::FreeBusyViewer,
            [false, true, false, false, false, false, false, false],
        ),
    ];

    for (role, expected) in cases {
        for (action, should_allow) in ACTIONS.into_iter().zip(expected) {
            let decision = authorize_calendar_action(
                UserStatus::Active,
                Some(PlatformRole::User),
                Some(role),
                action,
            );

            assert_eq!(
                decision,
                if should_allow {
                    AuthorizationDecision::Allow
                } else {
                    AuthorizationDecision::Deny
                },
                "{role:?} should have expected decision for {action:?}"
            );
        }
    }
}

#[test]
fn missing_or_unrecognized_roles_deny() {
    let unrecognized_calendar_role = CalendarRole::from_str("administrator").ok();
    let unrecognized_platform_role = PlatformRole::from_str("root").ok();

    assert_eq!(
        authorize_calendar_action(
            UserStatus::Active,
            Some(PlatformRole::User),
            None,
            CalendarAction::ReadFreeBusy,
        ),
        AuthorizationDecision::Deny
    );
    assert_eq!(
        authorize_calendar_action(
            UserStatus::Active,
            Some(PlatformRole::User),
            unrecognized_calendar_role,
            CalendarAction::ReadFreeBusy,
        ),
        AuthorizationDecision::Deny
    );
    assert_eq!(
        authorize_calendar_action(
            UserStatus::Active,
            None,
            Some(CalendarRole::Owner),
            CalendarAction::ReadDetails,
        ),
        AuthorizationDecision::Deny
    );
    assert_eq!(
        authorize_calendar_action(
            UserStatus::Active,
            unrecognized_platform_role,
            Some(CalendarRole::Owner),
            CalendarAction::ReadDetails,
        ),
        AuthorizationDecision::Deny
    );
}

#[test]
fn superadmin_has_no_implicit_private_calendar_read_permission() {
    for action in [CalendarAction::ReadDetails, CalendarAction::ReadFreeBusy] {
        assert_eq!(
            authorize_calendar_action(
                UserStatus::Active,
                Some(PlatformRole::Superadmin),
                None,
                action,
            ),
            AuthorizationDecision::Deny
        );
    }

    assert_eq!(
        authorize_calendar_action(
            UserStatus::Active,
            Some(PlatformRole::Superadmin),
            Some(CalendarRole::Viewer),
            CalendarAction::ReadDetails,
        ),
        AuthorizationDecision::Allow
    );
}

#[test]
fn suspended_users_are_denied_every_calendar_action() {
    for role in [
        CalendarRole::Owner,
        CalendarRole::Manager,
        CalendarRole::Editor,
        CalendarRole::Viewer,
        CalendarRole::FreeBusyViewer,
    ] {
        for action in ACTIONS {
            assert_eq!(
                authorize_calendar_action(
                    UserStatus::Suspended,
                    Some(PlatformRole::Superadmin),
                    Some(role),
                    action,
                ),
                AuthorizationDecision::Deny,
                "suspended {role:?} must be denied {action:?}"
            );
        }
    }
}

#[test]
fn inactive_users_are_denied_every_calendar_action() {
    for status in [UserStatus::Invited, UserStatus::Deleted] {
        for action in ACTIONS {
            assert_eq!(
                authorize_calendar_action(
                    status,
                    Some(PlatformRole::User),
                    Some(CalendarRole::Owner),
                    action,
                ),
                AuthorizationDecision::Deny
            );
        }
    }
}

#[test]
fn ownership_transfer_and_calendar_deletion_remain_owner_only() {
    for action in [
        CalendarAction::TransferOwnership,
        CalendarAction::DeleteCalendar,
    ] {
        assert_eq!(
            authorize_calendar_action(
                UserStatus::Active,
                Some(PlatformRole::User),
                Some(CalendarRole::Owner),
                action,
            ),
            AuthorizationDecision::Allow
        );

        for role in [
            CalendarRole::Manager,
            CalendarRole::Editor,
            CalendarRole::Viewer,
            CalendarRole::FreeBusyViewer,
        ] {
            assert_eq!(
                authorize_calendar_action(
                    UserStatus::Active,
                    Some(PlatformRole::Superadmin),
                    Some(role),
                    action,
                ),
                AuthorizationDecision::Deny
            );
        }
    }
}
