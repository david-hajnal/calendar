//! Deterministic calendar-local request-time masking policy for work-life balance.
//!
//! When protection is enabled on a calendar, the calendar is masked (hidden)
//! whenever the *request* time — converted to the calendar's `default_timezone`
//! IANA zone — falls on a Saturday, a Sunday, or a weekday at or after 17:00
//! local time. Weekdays before 17:00 (including before 09:00) remain visible.
//!
//! The helper is pure: it accepts an explicit request instant and never reads
//! a wall clock. Event start/end times are not part of the filter.

use chrono::{DateTime, Datelike, Timelike, Utc, Weekday};
use chrono_tz::Tz;

/// Decide whether a calendar with work-life-balance protection should be
/// masked for the given request time.
///
/// - Protection disabled: never masks, regardless of zone or time.
/// - Protection enabled with a valid IANA zone: masks when the request time
///   in that zone is Saturday/Sunday, or a weekday at/after 17:00 local.
/// - Protection enabled with an unparseable legacy zone: fails closed
///   (masks) without panicking.
pub fn should_mask(
    protection_enabled: bool,
    default_timezone: &str,
    request_time: DateTime<Utc>,
) -> bool {
    if !protection_enabled {
        return false;
    }
    match default_timezone.parse::<Tz>() {
        Ok(tz) => is_masked_window_in_zone(tz, request_time),
        Err(_) => true,
    }
}

/// Returns true when the given instant falls in a masked window
/// (Saturday/Sunday, or a weekday at/after 17:00) in the given IANA zone.
fn is_masked_window_in_zone(tz: Tz, request_time: DateTime<Utc>) -> bool {
    let local = request_time.with_timezone(&tz);
    match local.weekday() {
        Weekday::Sat | Weekday::Sun => true,
        _ => local.hour() >= 17,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(rfc3339: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(rfc3339)
            .unwrap()
            .with_timezone(&Utc)
    }

    // --- protection disabled never masks ---

    #[test]
    fn disabled_protection_never_masks_weekday_evening() {
        // Wednesday 2026-01-14, 17:00 Europe/Budapest local.
        assert!(!should_mask(
            false,
            "Europe/Budapest",
            at("2026-01-14T16:00:00Z")
        ));
    }

    #[test]
    fn disabled_protection_never_masks_saturday() {
        // Saturday 2026-07-18 local (Friday 2026-07-17 UTC evening).
        assert!(!should_mask(
            false,
            "Europe/Budapest",
            at("2026-07-17T22:00:00Z")
        ));
    }

    #[test]
    fn disabled_protection_never_masks_invalid_zone() {
        assert!(!should_mask(
            false,
            "Mars/Olympus",
            at("2026-01-14T16:00:00Z")
        ));
    }

    // --- weekday 16:59:59 visible, exactly 17:00:00 masked ---

    #[test]
    fn weekday_16_59_59_visible_and_17_00_00_masked_winter_offset() {
        // Europe/Budapest winter time (UTC+1), Wednesday 2026-01-14.
        assert!(!should_mask(
            true,
            "Europe/Budapest",
            at("2026-01-14T15:59:59Z")
        ));
        assert!(should_mask(
            true,
            "Europe/Budapest",
            at("2026-01-14T16:00:00Z")
        ));
    }

    #[test]
    fn weekday_16_59_59_visible_and_17_00_00_masked_summer_offset() {
        // Europe/Budapest summer time (UTC+2), Wednesday 2026-07-15.
        assert!(!should_mask(
            true,
            "Europe/Budapest",
            at("2026-07-15T14:59:59Z")
        ));
        assert!(should_mask(
            true,
            "Europe/Budapest",
            at("2026-07-15T15:00:00Z")
        ));
    }

    // --- Saturday and Sunday masked; Monday 00:00 visible ---

    #[test]
    fn saturday_local_midnight_masked() {
        // 2026-07-17T22:00:00Z is Saturday 2026-07-18 00:00 Europe/Budapest.
        assert!(should_mask(
            true,
            "Europe/Budapest",
            at("2026-07-17T22:00:00Z")
        ));
    }

    #[test]
    fn sunday_local_afternoon_masked() {
        // 2026-07-19T12:00:00Z is Sunday 2026-07-19 14:00 Europe/Budapest.
        assert!(should_mask(
            true,
            "Europe/Budapest",
            at("2026-07-19T12:00:00Z")
        ));
    }

    #[test]
    fn monday_local_midnight_visible() {
        // 2026-07-19T22:00:00Z is Monday 2026-07-20 00:00 Europe/Budapest.
        assert!(!should_mask(
            true,
            "Europe/Budapest",
            at("2026-07-19T22:00:00Z")
        ));
    }

    // --- UTC timezone and UTC/local date boundaries ---

    #[test]
    fn utc_weekday_boundary() {
        // Wednesday 2026-01-14 in UTC.
        assert!(!should_mask(true, "UTC", at("2026-01-14T16:59:59Z")));
        assert!(should_mask(true, "UTC", at("2026-01-14T17:00:00Z")));
    }

    #[test]
    fn utc_saturday_sunday_masked_and_monday_visible() {
        assert!(should_mask(true, "UTC", at("2026-01-17T12:00:00Z"))); // Saturday
        assert!(should_mask(true, "UTC", at("2026-01-18T12:00:00Z"))); // Sunday
        assert!(!should_mask(true, "UTC", at("2026-01-19T00:00:00Z"))); // Monday 00:00
    }

    // --- invalid legacy timezone fails closed ---

    #[test]
    fn invalid_zone_masks_when_enabled_without_panic() {
        assert!(should_mask(
            true,
            "Mars/Olympus",
            at("2026-01-14T16:00:00Z")
        ));
    }

    #[test]
    fn invalid_zone_stays_unmasked_when_disabled() {
        assert!(!should_mask(
            false,
            "Mars/Olympus",
            at("2026-01-14T16:00:00Z")
        ));
    }
}
