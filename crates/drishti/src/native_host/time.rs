//! Millisecond time helpers for the native host.
//!
//! The broker stamps envelopes with whole-second `YYYY-MM-DDTHH:MM:SSZ`
//! timestamps (matching the platform ISO-8601 formatter's default, which
//! carries no fractional seconds) and the governance store parses them back.
//! Both directions are integer civil-date arithmetic — no time dependency.

use lgwks_macros::Error;

use super::ensure;

/// Millisecond timestamp failures.
///
/// # Examples
///
/// ```
/// use drishti::native_host::time::{TimeError, parse_millis};
///
/// assert_eq!(parse_millis("not a timestamp"), Err(TimeError::Malformed));
/// assert_eq!(parse_millis("1969-12-31T23:59:59Z"), Err(TimeError::OutOfRange));
/// ```
#[derive(Clone, Debug, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum TimeError {
    /// The text is not a strict ISO-8601 timestamp.
    #[error("malformed timestamp")]
    Malformed,
    /// The timestamp is out of the supported range (years 1970-9999).
    #[error("timestamp out of range")]
    OutOfRange,
    /// An integer conversion overflowed its target.
    #[error("integer conversion out of range")]
    IntegerConversion(#[from] core::num::TryFromIntError),
}

/// Milliseconds in one day.
const MILLIS_PER_DAY: u64 = 86_400_000;

/// Maximum governance window: 24 hours in milliseconds.
///
/// # Examples
///
/// ```
/// use drishti::native_host::time::MAX_WINDOW_MILLIS;
///
/// assert_eq!(MAX_WINDOW_MILLIS, 24 * 60 * 60 * 1_000);
/// ```
pub const MAX_WINDOW_MILLIS: u64 = 86_400_000;

/// Format millisecond time as `YYYY-MM-DDTHH:MM:SSZ`, truncating sub-second
/// precision like the platform formatter's default.
///
/// # Examples
///
/// ```
/// use drishti::native_host::time::format_millis;
///
/// assert_eq!(format_millis(0), "1970-01-01T00:00:00Z");
/// assert_eq!(format_millis(1_000_000_000_999), "2001-09-09T01:46:40Z");
/// ```
#[must_use]
pub fn format_millis(timestamp_ms: u64) -> String {
    let days = timestamp_ms.wrapping_div(MILLIS_PER_DAY);
    let day_millis = timestamp_ms.wrapping_rem(MILLIS_PER_DAY);
    let (year, month, day) = civil_from_days(days);
    let hours = day_millis.wrapping_div(3_600_000);
    let minutes = day_millis.wrapping_rem(3_600_000).wrapping_div(60_000);
    let seconds = day_millis.wrapping_rem(60_000).wrapping_div(1_000);
    format!("{year:04}-{month:02}-{day:02}T{hours:02}:{minutes:02}:{seconds:02}Z")
}

/// Days since 1970-01-01 to civil year/month/day (Howard Hinnant's
/// algorithm, integer only).
fn civil_from_days(days: u64) -> (u64, u64, u64) {
    let shifted = days.wrapping_add(719_468);
    let era = shifted.wrapping_div(146_097);
    let day_of_era = shifted.wrapping_sub(era.wrapping_mul(146_097));
    let year_of_era = day_of_era
        .wrapping_sub(day_of_era.wrapping_div(1_460))
        .wrapping_add(day_of_era.wrapping_div(36_524))
        .wrapping_sub(day_of_era.wrapping_div(146_096))
        .wrapping_div(365);
    let year = year_of_era.wrapping_add(era.wrapping_mul(400));
    let day_of_year = day_of_era.wrapping_sub(
        year_of_era
            .wrapping_mul(365)
            .wrapping_add(year_of_era.wrapping_div(4))
            .wrapping_sub(year_of_era.wrapping_div(100)),
    );
    let month_prime = day_of_year
        .wrapping_mul(5)
        .wrapping_add(2)
        .wrapping_div(153);
    let day = day_of_year
        .wrapping_sub(
            month_prime
                .wrapping_mul(153)
                .wrapping_add(2)
                .wrapping_div(5),
        )
        .wrapping_add(1);
    let month = if month_prime < 10 {
        month_prime.wrapping_add(3)
    } else {
        month_prime.wrapping_sub(9)
    };
    let full_year = if month <= 2 {
        year.wrapping_add(1)
    } else {
        year
    };
    (full_year, month, day)
}

/// Parse `YYYY-MM-DDTHH:MM:SS[.fraction](Z|±HH:MM|±HHMM|±HH)` to millisecond
/// time. Fractional seconds truncate toward zero; offsets shift to UTC.
///
/// # Errors
///
/// Returns [`TimeError::Malformed`] for a shape violation or out-of-range
/// field, and [`TimeError::OutOfRange`] for years before 1970 or arithmetic
/// overflow.
///
/// # Examples
///
/// ```
/// use drishti::native_host::time::parse_millis;
///
/// let utc = parse_millis("2026-11-03T12:00:00Z")?;
/// // An offset shifts to the same UTC instant; a fraction truncates to millis.
/// assert_eq!(parse_millis("2026-11-03T14:00:00+02:00")?, utc);
/// assert_eq!(parse_millis("2026-11-03T12:00:00.789123Z")?, utc + 789);
/// assert!(parse_millis("2026-02-30T00:00:00Z").is_err());
/// # Ok::<(), drishti::native_host::time::TimeError>(())
/// ```
pub fn parse_millis(text: &str) -> Result<u64, TimeError> {
    // The fixed 19-byte `YYYY-MM-DDTHH:MM:SS` head, then the suffix. A suffix
    // is at least `Z`, so anything shorter is malformed (an empty one fails in
    // `parse_fraction`).
    let &[
        year_1,
        year_2,
        year_3,
        year_4,
        dash_1,
        month_1,
        month_2,
        dash_2,
        day_1,
        day_2,
        separator,
        hour_1,
        hour_2,
        colon_1,
        minute_1,
        minute_2,
        colon_2,
        second_1,
        second_2,
        ref suffix @ ..,
    ] = text.as_bytes()
    else {
        lgwks_std::trace::debug!("parse_millis: returning an error to the caller");
        return Err(TimeError::Malformed);
    };
    ensure(
        dash_1 == b'-' && dash_2 == b'-' && separator == b'T',
        || TimeError::Malformed,
    )?;
    ensure(colon_1 == b':' && colon_2 == b':', || TimeError::Malformed)?;
    let year = parse_digits(&[year_1, year_2, year_3, year_4])?;
    let month = parse_digits(&[month_1, month_2])?;
    let day = parse_digits(&[day_1, day_2])?;
    let hour = parse_digits(&[hour_1, hour_2])?;
    let minute = parse_digits(&[minute_1, minute_2])?;
    let second = parse_digits(&[second_1, second_2])?;
    let (fraction_millis, rest) = parse_fraction(suffix)?;
    let offset_millis = parse_offset(rest)?;
    ensure(
        (1..=12).contains(&month) && hour <= 23 && minute <= 59 && second <= 59,
        || TimeError::Malformed,
    )?;
    ensure((1970..=9999).contains(&year), || TimeError::OutOfRange)?;
    ensure(day >= 1 && day <= days_in_month(year, month), || {
        TimeError::Malformed
    })?;
    let days = days_from_civil(year, month, day)?;
    let time_millis = hour
        .checked_mul(3_600_000)
        .and_then(|value| value.checked_add(minute.checked_mul(60_000)?))
        .and_then(|value| value.checked_add(second.checked_mul(1_000)?))
        .and_then(|value| value.checked_add(fraction_millis))
        .ok_or(TimeError::OutOfRange)?;
    let absolute = days
        .checked_mul(MILLIS_PER_DAY)
        .and_then(|value| value.checked_add(time_millis))
        .ok_or(TimeError::OutOfRange)?;
    let absolute_signed = i64::try_from(absolute)?;
    let shifted = absolute_signed
        .checked_sub(offset_millis)
        .ok_or(TimeError::OutOfRange)?;
    u64::try_from(shifted).map_err(TimeError::from)
}

/// Parse ASCII digits to `u64`.
fn parse_digits(bytes: &[u8]) -> Result<u64, TimeError> {
    let mut value: u64 = 0;
    ensure(!bytes.is_empty(), || TimeError::Malformed)?;
    for byte in bytes {
        ensure(byte.is_ascii_digit(), || TimeError::Malformed)?;
        value = value
            .checked_mul(10)
            .and_then(|shifted| shifted.checked_add(u64::from(byte.wrapping_sub(b'0'))))
            .ok_or(TimeError::OutOfRange)?;
    }
    Ok(value)
}

/// Parse an optional fractional-seconds suffix, returning truncated
/// milliseconds and the remaining bytes.
fn parse_fraction(bytes: &[u8]) -> Result<(u64, &[u8]), TimeError> {
    let (head, tail) = bytes.split_first().ok_or(TimeError::Malformed)?;
    if *head != b'.' {
        return Ok((0, bytes));
    }
    let mut digits = 0_usize;
    while tail.get(digits).is_some_and(|byte| byte.is_ascii_digit()) {
        digits = digits.saturating_add(1);
    }
    ensure((1..=9).contains(&digits), || TimeError::Malformed)?;
    let mut millis: u64 = 0;
    for (position, byte) in tail.iter().take(digits).enumerate() {
        if position < 3 {
            millis = millis
                .checked_mul(10)
                .and_then(|shifted| shifted.checked_add(u64::from(byte.wrapping_sub(b'0'))))
                .ok_or(TimeError::OutOfRange)?;
        }
    }
    let scale = 3_usize.saturating_sub(digits.min(3));
    for _ in 0..scale {
        millis = millis.checked_mul(10).ok_or(TimeError::OutOfRange)?;
    }
    let rest = tail.get(digits..).ok_or(TimeError::Malformed)?;
    Ok((millis, rest))
}

/// Parse a `Z` or numeric offset suffix to signed milliseconds east of UTC.
///
/// Negative (western) offsets come back negative; the caller subtracts the
/// result from the absolute time.
fn parse_offset(bytes: &[u8]) -> Result<i64, TimeError> {
    if bytes == *b"Z" {
        return Ok(0);
    }
    let (&sign, digits) = bytes.split_first().ok_or(TimeError::Malformed)?;
    ensure(sign == b'+' || sign == b'-', || TimeError::Malformed)?;
    // `±HH`, `±HHMM` and `±HH:MM`: two hour digits, then nothing, two minute
    // digits, or a colon and two minute digits.
    let (hour_digits, minute_digits) = match digits.len() {
        2 | 4 => digits.split_at_checked(2),
        5 => digits
            .split_at_checked(2)
            .and_then(|(hours, tail)| tail.strip_prefix(b":").map(|minutes| (hours, minutes))),
        _ => None,
    }
    .ok_or(TimeError::Malformed)?;
    let hours = parse_digits(hour_digits)?;
    let minutes = if minute_digits.is_empty() {
        0
    } else {
        parse_digits(minute_digits)?
    };
    ensure(hours <= 23 && minutes <= 59, || TimeError::Malformed)?;
    let magnitude = hours
        .checked_mul(3_600_000)
        .and_then(|value| value.checked_add(minutes.checked_mul(60_000)?))
        .ok_or(TimeError::OutOfRange)?;
    let signed = i64::try_from(magnitude)?;
    if sign == b'+' {
        Ok(signed)
    } else {
        signed.checked_neg().ok_or(TimeError::OutOfRange)
    }
}

/// Days in a month, with leap years.
fn days_in_month(year: u64, month: u64) -> u64 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ => {
            if year.wrapping_rem(4) == 0
                && (year.wrapping_rem(100) != 0 || year.wrapping_rem(400) == 0)
            {
                29
            } else {
                28
            }
        }
    }
}

/// Days since 1970-01-01 for a civil date (inverse of [`civil_from_days`]).
fn days_from_civil(year: u64, month: u64, day: u64) -> Result<u64, TimeError> {
    let adjusted_year = if month <= 2 {
        year.checked_sub(1).ok_or(TimeError::OutOfRange)?
    } else {
        year
    };
    let era = adjusted_year
        .checked_div(400)
        .ok_or(TimeError::OutOfRange)?;
    let year_of_era = adjusted_year.wrapping_sub(era.wrapping_mul(400));
    let month_prime = if month > 2 {
        month.wrapping_sub(3)
    } else {
        month.wrapping_add(9)
    };
    let day_of_year = month_prime
        .wrapping_mul(153)
        .wrapping_add(2)
        .wrapping_div(5)
        .wrapping_add(day)
        .wrapping_sub(1);
    let day_of_era = year_of_era
        .wrapping_mul(365)
        .wrapping_add(year_of_era.wrapping_div(4))
        .wrapping_sub(year_of_era.wrapping_div(100))
        .wrapping_add(day_of_year);
    Ok(era
        .wrapping_mul(146_097)
        .wrapping_add(day_of_era)
        .wrapping_sub(719_468))
}

#[cfg(test)]
mod tests {
    use super::{format_millis, parse_millis};

    /// Assert one format/parse vector.
    fn assert_vector(millis: u64, text: &str) -> Result<(), String> {
        let formatted = format_millis(millis);
        if formatted != text {
            return Err(format!("format({millis}) = {formatted}, want {text}"));
        }
        let parsed = parse_millis(text).map_err(|error| format!("parse({text}): {error}"))?;
        if parsed != millis {
            return Err(format!("parse({text}) = {parsed}, want {millis}"));
        }
        Ok(())
    }

    /// Known timestamps round-trip exactly.
    #[test]
    fn civil_timestamps_round_trip() -> Result<(), String> {
        assert_vector(0, "1970-01-01T00:00:00Z")?;
        assert_vector(1_000_000_000_000, "2001-09-09T01:46:40Z")?;
        assert_vector(1_767_225_600_000, "2026-01-01T00:00:00Z")?;
        assert_vector(951_827_696_000, "2000-02-29T12:34:56Z")?;
        Ok(())
    }

    /// Fractions truncate and offsets shift to UTC.
    #[test]
    fn fractions_and_offsets_parse() -> Result<(), String> {
        let base = parse_millis("2026-11-03T12:00:00Z").map_err(|error| format!("{error}"))?;
        let with_fraction =
            parse_millis("2026-11-03T12:00:00.789Z").map_err(|error| format!("{error}"))?;
        if with_fraction != base.wrapping_add(789) {
            return Err(String::from("fraction must add truncated millis"));
        }
        let offset =
            parse_millis("2026-11-03T14:00:00+02:00").map_err(|error| format!("{error}"))?;
        if offset != base {
            return Err(String::from("+02:00 must shift to the same instant"));
        }
        for bad in [
            "2026-11-03",
            "2026-13-01T00:00:00Z",
            "2026-02-30T00:00:00Z",
            "2026-11-03T24:00:00Z",
            "2026-11-03T12:00:00",
            "1969-12-31T23:59:59Z",
            "not a timestamp",
        ] {
            if parse_millis(bad).is_ok() {
                return Err(format!("malformed timestamp parsed: {bad}"));
            }
        }
        Ok(())
    }
}
