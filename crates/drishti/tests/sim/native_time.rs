//! Time sims: the civil-date codec is walked day by day against a calendar
//! kept in this file, round-tripped over seeded instants, and fed corrupted
//! stamps; no input may panic or be read past its end.

use drishti::native_host::time::{format_millis, parse_millis};

use crate::motion_kit::ensure;
use crate::rng::SimRng;

/// Milliseconds in one day.
const DAY_MS: u64 = 86_400_000;
/// First millisecond after the last representable year (10000-01-01).
const END_OF_9999_MS: u64 = 253_402_300_800_000;
/// Decimal digits.
const DIGITS: &[u8] = b"0123456789";

/// Days in a month, Gregorian, kept independent of the crate.
fn days_in(year: u64, month: u64) -> u64 {
    let leap =
        (year.wrapping_rem(4) == 0 && year.wrapping_rem(100) != 0) || year.wrapping_rem(400) == 0;
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ => {
            if leap {
                29
            } else {
                28
            }
        }
    }
}

/// Bug caught: a civil-date conversion that is off by one around century
/// and 400-year leap rules, or drifts in any single month, so a stamp names
/// the wrong day.
#[test]
fn sim_time_every_day_from_1970_to_2400_has_the_right_date() -> Result<(), String> {
    let (mut year, mut month, mut day) = (1_970_u64, 1_u64, 1_u64);
    for counter in 0..157_100_u64 {
        let text = format_millis(counter.saturating_mul(DAY_MS));
        let want = format!("{year:04}-{month:02}-{day:02}T00:00:00Z");
        ensure(
            text == want,
            &format!("day {counter}: formatted {text}, calendar says {want}"),
        )?;
        day = day.saturating_add(1);
        if day > days_in(year, month) {
            day = 1;
            month = month.saturating_add(1);
            if month > 12 {
                month = 1;
                year = year.saturating_add(1);
            }
        }
    }
    ensure(year > 2_399, "the walk must cover four centuries")
}

/// Bug caught: a codec that drops or invents a millisecond, or cannot read
/// back what it writes (the parser must invert the formatter exactly, with
/// the sub-second part truncated).
#[test]
fn sim_time_format_then_parse_inverts_over_seeded_instants() -> Result<(), String> {
    for seed in 0..2_000_u64 {
        let mut random = SimRng::seed(seed.wrapping_add(100_000));
        let instant = random.below(END_OF_9999_MS);
        let text = format_millis(instant);
        let whole_second = instant.saturating_sub(instant.wrapping_rem(1_000));
        ensure(
            text.len() == 20,
            &format!("{instant}: {text} is not 20 bytes"),
        )?;
        ensure(
            parse_millis(&text) == Ok(whole_second),
            &format!("{instant}: {text} parsed as {:?}", parse_millis(&text)),
        )?;
        let later = random.below(END_OF_9999_MS);
        let (first, second) = if instant <= later {
            (instant, later)
        } else {
            (later, instant)
        };
        ensure(
            format_millis(first) <= format_millis(second),
            &format!("{first} and {second}: the stamps do not sort like the instants"),
        )?;
    }
    ensure(
        parse_millis("1970-01-01T00:00:00Z") == Ok(0)
            && parse_millis("9999-12-31T23:59:59Z") == Ok(END_OF_9999_MS.saturating_sub(1_000))
            && parse_millis("2000-02-29T00:00:00Z").is_ok()
            && parse_millis("2100-02-29T00:00:00Z").is_err()
            && parse_millis("1900-02-28T00:00:00Z").is_err(),
        "the epoch, the last second and the leap rules must hold",
    )
}

/// Bug caught: an offset applied with the wrong sign, a fraction that rounds
/// up instead of truncating, or an offset that wraps below the epoch.
#[test]
fn sim_time_offsets_shift_and_fractions_truncate() -> Result<(), String> {
    for seed in 0..1_500_u64 {
        let mut random = SimRng::seed(seed.wrapping_add(110_000));
        let base = random
            .below(END_OF_9999_MS.saturating_sub(DAY_MS.saturating_mul(4)))
            .saturating_add(DAY_MS.saturating_mul(2));
        let whole = base.saturating_sub(base.wrapping_rem(1_000));
        let stamp = format_millis(base);
        let stem = stamp.trim_end_matches('Z');
        let hours = random.below(24);
        let minutes = random.below(60);
        let shift = hours
            .saturating_mul(3_600_000)
            .saturating_add(minutes.saturating_mul(60_000));
        let east = random.below(2) == 1;
        let sign = if east { '+' } else { '-' };
        let text = format!("{stem}{sign}{hours:02}:{minutes:02}");
        let expected = if east {
            whole.saturating_sub(shift)
        } else {
            whole.saturating_add(shift)
        };
        ensure(
            parse_millis(&text) == Ok(expected),
            &format!(
                "seed {seed}: {text} parsed as {:?}, expected {expected}",
                parse_millis(&text)
            ),
        )?;
        let count = random.below_len(9).saturating_add(1);
        let fraction: String = (0..count)
            .map(|_| char::from(DIGITS[random.below_len(DIGITS.len())]))
            .collect();
        let with_fraction = format!("{stem}.{fraction}Z");
        let kept: String = fraction.chars().chain("00".chars()).take(3).collect();
        let millis: u64 = kept.parse().map_err(|error| format!("{error}"))?;
        ensure(
            parse_millis(&with_fraction) == Ok(whole.saturating_add(millis)),
            &format!("seed {seed}: {with_fraction} must truncate to {millis} ms"),
        )?;
    }
    ensure(
        parse_millis("1970-01-01T00:00:00+00:01").is_err(),
        "an offset that lands before the epoch must be refused",
    )?;
    ensure(
        parse_millis("1970-01-01T00:00:00-00:01") == Ok(60_000),
        "a western offset adds time",
    )?;
    for bad in [
        "2026-05-25T12:00:00.Z",
        "2026-05-25T12:00:00.1234567890Z",
        "2026-05-25T12:00:00+24:00",
        "2026-05-25T12:00:00+05:60",
        "2026-05-25T12:00:00+5:00",
        "2026-05-25T12:00:00+05-00",
        "2026-05-25T12:00:00*05:00",
        "2026-05-25T12:00:00ZZ",
    ] {
        ensure(
            parse_millis(bad).is_err(),
            &format!("{bad:?} must be refused"),
        )?;
    }
    Ok(())
}

/// True when the parser's verdict on `text` is the reference's: the same
/// instant when the reference accepts it, a named refusal when it does not.
fn agrees(text: &str, want: Option<u64>) -> bool {
    match parse_millis(text) {
        Ok(millis) => want == Some(millis),
        Err(error) => want.is_none() && !format!("{error}").is_empty(),
    }
}

/// The documented offset rule written independently: a sign, two hour digits,
/// then nothing, two minute digits, or a colon and two minute digits; hours
/// at most 23 and minutes at most 59. Returns signed minutes east of UTC.
fn reference_offset(text: &str) -> Option<i64> {
    let sign: i64 = match text.chars().next()? {
        '+' => 1,
        '-' => -1,
        _ => return None,
    };
    let digits = text.get(1..)?;
    let (hours, minutes) = match digits.len() {
        2 => (digits, "00"),
        4 => (digits.get(..2)?, digits.get(2..)?),
        5 if digits.get(2..3)? == ":" => (digits.get(..2)?, digits.get(3..)?),
        _ => return None,
    };
    if !hours
        .bytes()
        .chain(minutes.bytes())
        .all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    let (Ok(hours), Ok(minutes)) = (hours.parse::<i64>(), minutes.parse::<i64>()) else {
        return None;
    };
    if hours > 23 || minutes > 59 {
        return None;
    }
    Some(sign.saturating_mul(hours.saturating_mul(60).saturating_add(minutes)))
}

/// Bug caught: the documented numeric offset forms `+HH:MM`, `+HHMM` and
/// `+HH` are not all read (the compact forms are refused as malformed), so a
/// stamp a peer legitimately writes is dropped, or two spellings of one offset
/// name different instants.
#[test]
fn sim_time_every_documented_offset_form_agrees_with_the_reference() -> Result<(), String> {
    let mut accepted = 0_usize;
    for seed in 0..3_000_u64 {
        let mut random = SimRng::seed(seed.wrapping_add(115_000));
        let base = random
            .below(END_OF_9999_MS.saturating_sub(DAY_MS.saturating_mul(4)))
            .saturating_add(DAY_MS.saturating_mul(2));
        let stamp = format_millis(base);
        let stem = stamp.trim_end_matches('Z');
        let whole = base.saturating_sub(base.wrapping_rem(1_000));
        let sign = if random.below(2) == 0 { '+' } else { '-' };
        let hours = random.below(27);
        let minutes = random.below(66);
        let offset = match random.below(5) {
            0 => format!("{sign}{hours:02}:{minutes:02}"),
            1 => format!("{sign}{hours:02}{minutes:02}"),
            2 => format!("{sign}{hours:02}"),
            3 => format!("{sign}{hours}{minutes}"),
            _ => format!("{sign}{hours:02}:{minutes}"),
        };
        let text = format!("{stem}{offset}");
        let want = reference_offset(&offset).and_then(|east_minutes| {
            let shift = east_minutes.unsigned_abs().saturating_mul(60_000);
            if east_minutes >= 0 {
                whole.checked_sub(shift)
            } else {
                whole.checked_add(shift)
            }
        });
        accepted = accepted.saturating_add(usize::from(want.is_some()));
        ensure(
            agrees(&text, want),
            &format!(
                "seed {seed}: {text} read as {:?}, the offset rule says {want:?}",
                parse_millis(&text)
            ),
        )?;
    }
    ensure(accepted > 1_000, "the corpus must reach accepted forms")?;
    for (colon, compact, hour_only) in [
        ("+05:30", "+0530", None),
        ("-03:00", "-0300", Some("-03")),
        ("+00:00", "+0000", Some("+00")),
        ("+23:59", "+2359", None),
        ("-23:00", "-2300", Some("-23")),
    ] {
        let reading = parse_millis(&format!("2026-05-25T12:00:00{colon}"));
        ensure(reading.is_ok(), &format!("{colon} must be accepted"))?;
        ensure(
            parse_millis(&format!("2026-05-25T12:00:00{compact}")) == reading,
            &format!("{compact} must name the instant {colon} names"),
        )?;
        if let Some(short) = hour_only {
            ensure(
                parse_millis(&format!("2026-05-25T12:00:00{short}")) == reading,
                &format!("{short} must name the instant {colon} names"),
            )?;
        }
    }
    Ok(())
}

/// Bug caught: a parser that indexes past the end or splits a multi-byte
/// character on corrupted input, or accepts a stamp it could not itself have
/// written without changing the instant.
#[test]
fn sim_time_corrupted_stamps_never_panic_and_stay_canonical() -> Result<(), String> {
    const REPLACEMENTS: &[&str] = &[
        "",
        "x",
        "\u{e9}",
        "9",
        "-",
        "T",
        " ",
        ":",
        "\0",
        "+",
        "\u{1f600}",
    ];
    for seed in 0..1_500_u64 {
        let mut random = SimRng::seed(seed.wrapping_add(120_000));
        let instant = random.below(END_OF_9999_MS);
        let stamp = format_millis(instant);
        let mut chars: Vec<char> = stamp.chars().collect();
        let position = random.below_len(chars.len());
        if let Some(replacement) = random.pick_str(REPLACEMENTS) {
            chars.splice(position..=position, replacement.chars());
        }
        let mutated: String = chars.into_iter().collect();
        if let Ok(parsed) = parse_millis(&mutated) {
            let again = format_millis(parsed);
            ensure(
                parse_millis(&again) == Ok(parsed.saturating_sub(parsed.wrapping_rem(1_000))),
                &format!("seed {seed}: {mutated:?} parsed to {parsed} which does not re-read"),
            )?;
        }
    }
    let stamp = format_millis(1_767_225_600_000);
    for cut in 0..stamp.len() {
        ensure(
            parse_millis(&stamp[..cut]).is_err(),
            &format!("the prefix of {cut} bytes must be refused"),
        )?;
    }
    for separator in [4_usize, 7, 10, 13, 16] {
        let mut broken = stamp.clone().into_bytes();
        broken[separator] = b'x';
        let broken = String::from_utf8(broken).map_err(|error| format!("{error}"))?;
        ensure(
            parse_millis(&broken).is_err(),
            &format!("a corrupted separator at {separator} was accepted: {broken}"),
        )?;
    }
    Ok(())
}
