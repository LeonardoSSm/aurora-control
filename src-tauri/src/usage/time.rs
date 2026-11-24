//! Checked time arithmetic for values that come from outside the process
//! (HTTP headers, provider responses, local logs). Nothing here panics: values
//! outside their semantic range become `None`, and waits are bounded (audit B02).

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use chrono::{DateTime, TimeDelta, Utc};

/// Longest wait the scheduler honors, including `Retry-After`. Longer values
/// are clamped to it, so a broken header cannot park a provider forever.
pub const MAX_WAIT: Duration = Duration::from_secs(24 * 60 * 60);

/// Accepted distance between an observation and a window renewal. Windows
/// longer than a month plus a margin are not plausible for these providers.
pub const MAX_RESET_AHEAD: TimeDelta = TimeDelta::days(45);
/// A renewal slightly in the past is normal (clock skew, slow logs).
pub const MAX_RESET_BEHIND: TimeDelta = TimeDelta::days(1);

/// Longest window length accepted from a provider.
pub const MAX_WINDOW_MINUTES: u32 = 45 * 24 * 60;

/// `wait` clamped to [`MAX_WAIT`].
pub fn bounded(wait: Duration) -> Duration {
    wait.min(MAX_WAIT)
}

fn delta(wait: Duration) -> TimeDelta {
    // `bounded` keeps the value far below the `TimeDelta` range.
    TimeDelta::from_std(bounded(wait)).unwrap_or(TimeDelta::zero())
}

/// `now + wait`, with `wait` bounded and the sum saturated at the end of the
/// representable range.
pub fn after(now: DateTime<Utc>, wait: Duration) -> DateTime<Utc> {
    now.checked_add_signed(delta(wait))
        .unwrap_or(DateTime::<Utc>::MAX_UTC)
}

/// `now + margin`, saturated.
pub fn plus(now: DateTime<Utc>, margin: TimeDelta) -> DateTime<Utc> {
    now.checked_add_signed(margin)
        .unwrap_or(DateTime::<Utc>::MAX_UTC)
}

/// `now - margin`, saturated.
pub fn minus(now: DateTime<Utc>, margin: TimeDelta) -> DateTime<Utc> {
    now.checked_sub_signed(margin)
        .unwrap_or(DateTime::<Utc>::MIN_UTC)
}

/// Whole seconds from a JSON number, rejecting NaN, infinities and values
/// that do not fit an `i64`.
pub fn whole_seconds(value: f64) -> Option<i64> {
    // `i64::MAX as f64` rounds up to 2^63, so the upper bound is exclusive.
    (value.is_finite() && value >= i64::MIN as f64 && value < i64::MAX as f64)
        .then(|| value.trunc() as i64)
}

/// A Unix timestamp in seconds, if it is representable.
pub fn from_epoch(seconds: f64) -> Option<DateTime<Utc>> {
    DateTime::from_timestamp(whole_seconds(seconds)?, 0)
}

/// A Unix timestamp in milliseconds, if it is representable.
pub fn from_epoch_millis(milliseconds: f64) -> Option<DateTime<Utc>> {
    DateTime::from_timestamp_millis(whole_seconds(milliseconds)?)
}

/// `base + seconds`, if both the offset and the result are representable.
pub fn offset(base: DateTime<Utc>, seconds: f64) -> Option<DateTime<Utc>> {
    base.checked_add_signed(TimeDelta::try_seconds(whole_seconds(seconds)?)?)
}

/// Keeps a renewal time only when it is plausible relative to `observed`.
pub fn plausible_reset(
    reset: Option<DateTime<Utc>>,
    observed: DateTime<Utc>,
) -> Option<DateTime<Utc>> {
    reset.filter(|reset| {
        let distance = reset.signed_duration_since(observed);
        distance <= MAX_RESET_AHEAD && distance >= -MAX_RESET_BEHIND
    })
}

/// Window length in minutes from seconds, within `1..=MAX_WINDOW_MINUTES`.
pub fn window_minutes(seconds: f64) -> Option<u32> {
    let minutes = (whole_seconds(seconds.round())? + 30) / 60;
    u32::try_from(minutes)
        .ok()
        .filter(|minutes| (1..=MAX_WINDOW_MINUTES).contains(minutes))
}

/// File-system time for `time`, or `None` outside the `SystemTime` range.
pub fn system_time(time: DateTime<Utc>) -> Option<SystemTime> {
    let seconds = time.timestamp();
    let nanos = time.timestamp_subsec_nanos();
    if seconds >= 0 {
        UNIX_EPOCH.checked_add(Duration::new(seconds.unsigned_abs(), nanos))
    } else {
        UNIX_EPOCH
            .checked_sub(Duration::new(seconds.unsigned_abs(), 0))?
            .checked_add(Duration::from_nanos(u64::from(nanos)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 23, 18, 0, 0).unwrap()
    }

    #[test]
    fn waits_are_bounded_and_sums_saturate() {
        assert_eq!(
            after(now(), Duration::from_secs(90)),
            Utc.with_ymd_and_hms(2026, 11, 23, 18, 1, 30).unwrap()
        );
        assert_eq!(after(now(), Duration::MAX), plus(now(), TimeDelta::days(1)));
        assert_eq!(
            after(DateTime::<Utc>::MAX_UTC, Duration::from_secs(1)),
            DateTime::<Utc>::MAX_UTC
        );
        assert_eq!(
            plus(DateTime::<Utc>::MAX_UTC, TimeDelta::seconds(30)),
            DateTime::<Utc>::MAX_UTC
        );
        assert_eq!(
            minus(DateTime::<Utc>::MIN_UTC, TimeDelta::hours(5)),
            DateTime::<Utc>::MIN_UTC
        );
    }

    #[test]
    fn external_numbers_outside_the_range_are_rejected() {
        for value in [
            f64::NAN,
            f64::INFINITY,
            -f64::INFINITY,
            1e300,
            -1e300,
            9.3e18,
        ] {
            assert_eq!(whole_seconds(value), None, "{value}");
            assert_eq!(from_epoch(value), None, "{value}");
            assert_eq!(offset(now(), value), None, "{value}");
        }
        // Representable as i64 but outside the TimeDelta or DateTime range.
        assert_eq!(offset(now(), 9.0e18), None);
        assert_eq!(from_epoch(9.0e18), None);
        assert_eq!(from_epoch_millis(9.0e18), None);
        assert_eq!(
            from_epoch(1_795_475_125.0),
            Some(Utc.with_ymd_and_hms(2026, 11, 23, 23, 5, 25).unwrap())
        );
        assert_eq!(
            offset(now(), 60.9),
            Some(Utc.with_ymd_and_hms(2026, 11, 23, 18, 1, 0).unwrap())
        );
    }

    #[test]
    fn resets_and_window_lengths_must_be_plausible() {
        let ahead = |days| Some(plus(now(), TimeDelta::days(days)));
        assert_eq!(plausible_reset(ahead(7), now()), ahead(7));
        assert_eq!(plausible_reset(ahead(46), now()), None);
        assert_eq!(plausible_reset(ahead(-2), now()), None);
        assert_eq!(plausible_reset(ahead(0), now()), ahead(0));
        assert_eq!(window_minutes(18_000.0), Some(300));
        assert_eq!(window_minutes(604_800.0), Some(10_080));
        assert_eq!(window_minutes(0.0), None);
        assert_eq!(window_minutes(-60.0), None);
        assert_eq!(window_minutes(1e12), None);
        assert_eq!(window_minutes(f64::NAN), None);
    }

    #[test]
    fn system_time_conversion_never_panics() {
        assert!(system_time(now()).is_some());
        let _ = system_time(DateTime::<Utc>::MIN_UTC);
        let _ = system_time(DateTime::<Utc>::MAX_UTC);
    }
}
