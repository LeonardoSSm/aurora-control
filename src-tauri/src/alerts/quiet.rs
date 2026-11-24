//! The daily quiet period.
//!
//! The period is stored as local minutes since midnight and compared with the
//! local time read at each evaluation, so a clock adjustment, a daylight
//! saving change or a new time zone takes effect immediately. `start > end`
//! crosses midnight. The start minute is inside the period, the end minute is
//! not.

use crate::preferences::QuietHours;

pub fn is_quiet(quiet: &QuietHours, minute: u16) -> bool {
    if !quiet.enabled || quiet.start == quiet.end {
        return false;
    }
    if quiet.start < quiet.end {
        (quiet.start..quiet.end).contains(&minute)
    } else {
        minute >= quiet.start || minute < quiet.end
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quiet(start: u16, end: u16) -> QuietHours {
        QuietHours {
            enabled: true,
            start,
            end,
        }
    }

    #[test]
    fn a_daytime_period() {
        let lunch = quiet(12 * 60, 13 * 60);
        assert!(!is_quiet(&lunch, 11 * 60 + 59));
        assert!(is_quiet(&lunch, 12 * 60));
        assert!(is_quiet(&lunch, 12 * 60 + 59));
        assert!(!is_quiet(&lunch, 13 * 60));
    }

    #[test]
    fn a_period_across_midnight() {
        let night = quiet(22 * 60, 7 * 60);
        assert!(is_quiet(&night, 22 * 60));
        assert!(is_quiet(&night, 23 * 60 + 59));
        assert!(is_quiet(&night, 0));
        assert!(is_quiet(&night, 6 * 60 + 59));
        assert!(!is_quiet(&night, 7 * 60));
        assert!(!is_quiet(&night, 12 * 60));
        assert!(!is_quiet(&night, 21 * 60 + 59));
    }

    #[test]
    fn disabled_or_empty_periods_never_mute() {
        let mut night = quiet(22 * 60, 7 * 60);
        night.enabled = false;
        assert!(!is_quiet(&night, 23 * 60));
        assert!(!is_quiet(&quiet(600, 600), 600));
    }
}
