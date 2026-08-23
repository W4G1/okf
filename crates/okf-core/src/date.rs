//! Calendar dates and ISO-8601 datetimes for the trust and lifecycle families
//! (§5).
//!
//! OKF v0.2 puts timestamp fields in frontmatter and asks consumers to answer
//! questions about them: which verification is the most recent, and is
//! `now >= stale_after`.
//!
//! Every timestamp-valued key in OKF frontmatter is an ISO-8601 datetime with
//! an explicit UTC offset (e.g. `2026-06-30T14:00:00Z`), across `generated.at`,
//! `verified[].at`, `stale_after`, `sources[].last_modified`, and `usage_window`.
//! Plain `YYYY-MM-DD` dates are used only in `log.md` section headings (§9).
//!
//! Answering those needs real date arithmetic, so this module implements the
//! small amount required on the standard library alone: a proleptic Gregorian
//! [`Date`], an offset-aware [`DateTime`] that orders correctly across time
//! zones, and the [`DateField`] / [`DateTimeField`] wrappers that keep the raw
//! scalar around so a validator can report *what* failed to parse.

use std::fmt;
use std::time::{SystemTime, UNIX_EPOCH};

/// A proleptic-Gregorian calendar date (`YYYY-MM-DD`).
///
/// Ordering is chronological: the derived field order (year, month, day) is
/// exactly calendar order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Date {
    /// Calendar year.
    pub year: i32,
    /// Month, 1 to 12.
    pub month: u32,
    /// Day of month, 1 to 31 (validated against the month and leap year).
    pub day: u32,
}

impl Date {
    /// Builds a date, returning `None` for a day that does not exist in that
    /// month (`2026-02-30`, `2026-13-01`, …).
    #[must_use]
    pub fn new(year: i32, month: u32, day: u32) -> Option<Self> {
        if !(1..=12).contains(&month) || day < 1 || day > days_in_month(year, month) {
            return None;
        }
        Some(Self { year, month, day })
    }

    /// Parses a strict `YYYY-MM-DD` date. Returns `None` for any other shape,
    /// including datetimes; use [`DateTime::parse`] for those.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        let b = s.as_bytes();
        if b.len() != 10 || b[4] != b'-' || b[7] != b'-' {
            return None;
        }
        if !b
            .iter()
            .enumerate()
            .all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit())
        {
            return None;
        }
        Self::new(
            s[0..4].parse().ok()?,
            s[5..7].parse().ok()?,
            s[8..10].parse().ok()?,
        )
    }

    /// Today's date in UTC, from the system clock.
    ///
    /// Returns `None` only if the clock reports a time before the Unix epoch.
    #[must_use]
    pub fn today_utc() -> Option<Self> {
        let secs = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs();
        // `secs` is `u64`; saturate at `i64::MAX` (far past any representable
        // date) rather than `as i64`, which would silently wrap near the limit.
        let secs = i64::try_from(secs).unwrap_or(i64::MAX);
        Some(Self::from_days_since_epoch(secs.div_euclid(86_400)))
    }

    /// Days since 1970-01-01 (negative before it).
    #[must_use]
    pub fn days_since_epoch(&self) -> i64 {
        days_from_civil(self.year, self.month, self.day)
    }

    /// The date `days` after 1970-01-01.
    #[must_use]
    pub fn from_days_since_epoch(days: i64) -> Self {
        let (year, month, day) = civil_from_days(days);
        Self { year, month, day }
    }

    /// Returns a UTC datetime at midnight (`00:00:00Z`) on this date.
    #[must_use]
    pub const fn to_utc_datetime(&self) -> DateTime {
        DateTime::from_date_utc(*self)
    }
}

impl fmt::Display for Date {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:04}-{:02}-{:02}", self.year, self.month, self.day)
    }
}

impl std::str::FromStr for Date {
    type Err = ParseDateError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s).ok_or_else(|| ParseDateError(s.to_string()))
    }
}

/// Error returned when a string is not a valid date or datetime.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseDateError(pub String);

impl fmt::Display for ParseDateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "not an ISO-8601 date/datetime: {:?}", self.0)
    }
}

impl std::error::Error for ParseDateError {}

/// An ISO-8601 datetime: a [`Date`], an optional time of day, and an optional
/// UTC offset.
///
/// A value with no offset is treated as UTC for comparison, which is what
/// consumers need in order to answer "which verification is most recent" (§5.2)
/// across producers that write `Z`, `+02:00`, or nothing at all.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DateTime {
    /// The calendar date.
    pub date: Date,
    /// Hour, 0 to 23 (0 when the value is date-only).
    pub hour: u32,
    /// Minute, 0 to 59.
    pub minute: u32,
    /// Second, 0 to 60 (60 admits a leap second).
    pub second: u32,
    /// Fractional second in nanoseconds.
    pub nanosecond: u32,
    /// Minutes east of UTC, or `None` when the value carries no zone.
    pub offset_minutes: Option<i32>,
    /// `false` when only a date was written (`2026-09-23`).
    pub has_time: bool,
}

impl DateTime {
    /// Parses an ISO-8601 datetime.
    ///
    /// Accepts `YYYY-MM-DD`, `YYYY-MM-DDTHH:MM[:SS[.fraction]]` (with `T`, `t`,
    /// or a space as the separator), and an optional `Z` / `±HH:MM` / `±HHMM` /
    /// `±HH` zone.
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim();
        if !s.is_ascii() || s.len() < 10 {
            return None;
        }
        let date = Date::parse(&s[..10])?;
        let rest = &s[10..];
        if rest.is_empty() {
            return Some(Self {
                date,
                hour: 0,
                minute: 0,
                second: 0,
                nanosecond: 0,
                offset_minutes: None,
                has_time: false,
            });
        }

        let sep = rest.as_bytes()[0];
        if sep != b'T' && sep != b't' && sep != b' ' {
            return None;
        }
        let mut rest = &rest[1..];

        let hour = take_u32(&mut rest, 2)?;
        expect(&mut rest, ':')?;
        let minute = take_u32(&mut rest, 2)?;
        let mut second = 0;
        let mut nanosecond = 0;
        if rest.starts_with(':') {
            rest = &rest[1..];
            second = take_u32(&mut rest, 2)?;
            if rest.starts_with('.') || rest.starts_with(',') {
                rest = &rest[1..];
                let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
                if digits.is_empty() {
                    return None;
                }
                rest = &rest[digits.len()..];
                // Left-align to nanosecond precision, truncating beyond 9 digits.
                let mut nanos = digits;
                nanos.truncate(9);
                while nanos.len() < 9 {
                    nanos.push('0');
                }
                nanosecond = nanos.parse().ok()?;
            }
        }
        if hour > 23 || minute > 59 || second > 60 {
            return None;
        }

        let offset_minutes = parse_offset(rest)?;
        Some(Self {
            date,
            hour,
            minute,
            second,
            nanosecond,
            offset_minutes,
            has_time: true,
        })
    }

    /// Seconds since the Unix epoch, normalizing the zone (a missing offset is
    /// read as UTC).
    #[must_use]
    pub fn to_utc_seconds(&self) -> i64 {
        self.date.days_since_epoch() * 86_400
            + i64::from(self.hour) * 3600
            + i64::from(self.minute) * 60
            + i64::from(self.second)
            - i64::from(self.offset_minutes.unwrap_or(0)) * 60
    }

    /// The calendar date in UTC, which differs from [`DateTime::date`] when the
    /// value carries an offset that crosses midnight.
    #[must_use]
    pub fn utc_date(&self) -> Date {
        Date::from_days_since_epoch(self.to_utc_seconds().div_euclid(86_400))
    }

    /// `true` if the parsed datetime carries an explicit UTC offset.
    #[must_use]
    pub const fn has_offset(&self) -> bool {
        self.offset_minutes.is_some()
    }

    /// The current instant in UTC from the system clock.
    #[must_use]
    pub fn now_utc() -> Option<Self> {
        let duration = SystemTime::now().duration_since(UNIX_EPOCH).ok()?;
        let secs = i64::try_from(duration.as_secs()).unwrap_or(i64::MAX);
        let nanos = duration.subsec_nanos();
        let days = secs.div_euclid(86_400);
        let rem_secs = secs.rem_euclid(86_400);
        let hour = u32::try_from(rem_secs / 3600).ok()?;
        let minute = u32::try_from((rem_secs % 3600) / 60).ok()?;
        let second = u32::try_from(rem_secs % 60).ok()?;
        Some(Self {
            date: Date::from_days_since_epoch(days),
            hour,
            minute,
            second,
            nanosecond: nanos,
            offset_minutes: Some(0),
            has_time: true,
        })
    }

    /// Creates a UTC datetime at midnight (`00:00:00Z`) for a given [`Date`].
    #[must_use]
    pub const fn from_date_utc(date: Date) -> Self {
        Self {
            date,
            hour: 0,
            minute: 0,
            second: 0,
            nanosecond: 0,
            offset_minutes: Some(0),
            has_time: true,
        }
    }
}

impl PartialOrd for DateTime {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for DateTime {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.to_utc_seconds()
            .cmp(&other.to_utc_seconds())
            .then(self.nanosecond.cmp(&other.nanosecond))
    }
}

impl fmt::Display for DateTime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.date)?;
        if !self.has_time {
            return Ok(());
        }
        write!(f, "T{:02}:{:02}:{:02}", self.hour, self.minute, self.second)?;
        if self.nanosecond > 0 {
            let frac = format!("{:09}", self.nanosecond);
            write!(f, ".{}", frac.trim_end_matches('0'))?;
        }
        match self.offset_minutes {
            None => Ok(()),
            Some(0) => f.write_str("Z"),
            Some(m) => {
                let sign = if m < 0 { '-' } else { '+' };
                write!(f, "{sign}{:02}:{:02}", m.abs() / 60, m.abs() % 60)
            }
        }
    }
}

impl std::str::FromStr for DateTime {
    type Err = ParseDateError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s).ok_or_else(|| ParseDateError(s.to_string()))
    }
}

/// A frontmatter date field: the scalar exactly as written, plus its parse.
///
/// Keeping the raw text lets a consumer round-trip the value and lets
/// [`validate`](crate::validate) report *which* scalar is malformed instead of
/// silently dropping it: the spec's permissiveness rule (§11) means an
/// unparseable date must never make a document unreadable.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DateField {
    /// The scalar as written in the frontmatter.
    pub raw: String,
    /// The parsed date, or `None` if `raw` is not a `YYYY-MM-DD` date.
    pub date: Option<Date>,
}

impl DateField {
    /// Wraps a raw scalar, parsing it if possible.
    pub fn new(raw: impl Into<String>) -> Self {
        let raw = raw.into();
        let date = Date::parse(raw.trim());
        Self { raw, date }
    }

    /// `true` if the raw scalar parsed as a date.
    #[must_use]
    pub const fn is_valid(&self) -> bool {
        self.date.is_some()
    }

    /// The date this field designates, accepting a datetime by taking its date
    /// part.
    ///
    /// [`DateField::date`] is deliberately strict, because §5.5 asks
    /// `stale_after` for "an absolute date (`YYYY-MM-DD`)" and the validator
    /// should report a datetime there as the deviation it is. A comparison still
    /// has to reach a verdict, though, and reading a malformed date as "never
    /// stale" is the dangerous direction: a concept well past its date would
    /// look current. So comparisons use this instead, which matches the
    /// reference implementation's `is_stale` (it truncates with
    /// `date.fromisoformat(str(raw)[:10])`). The written date is used as-is,
    /// without shifting by any UTC offset, exactly as that truncation does.
    #[must_use]
    pub fn effective_date(&self) -> Option<Date> {
        self.date
            .or_else(|| DateTime::parse(self.raw.trim()).map(|parsed| parsed.date))
    }
}

impl fmt::Display for DateField {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.raw)
    }
}

/// A frontmatter datetime field: the scalar exactly as written, plus its parse.
///
/// Keeping the raw text lets a consumer round-trip the value and lets
/// [`validate`](crate::validate) report *which* scalar is malformed instead of
/// silently dropping it: the spec's permissiveness rule (§11) means an
/// unparseable datetime must never make a document unreadable.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DateTimeField {
    /// The scalar as written in the frontmatter.
    pub raw: String,
    /// The parsed datetime, or `None` if `raw` is not an ISO-8601
    /// datetime. A date-only or offset-less value remains available through
    /// `datetime` so it can be diagnosed without losing the original scalar.
    pub datetime: Option<DateTime>,
}

impl DateTimeField {
    /// Wraps a raw scalar, parsing it if possible.
    pub fn new(raw: impl Into<String>) -> Self {
        let raw = raw.into();
        let datetime = DateTime::parse(&raw);
        Self { raw, datetime }
    }

    /// `true` if the raw scalar parsed as an ISO-8601 datetime with a time of
    /// day and an explicit UTC offset (§5).
    #[must_use]
    pub const fn is_valid(&self) -> bool {
        self.has_time() && self.has_offset()
    }

    /// `true` if the raw scalar parsed as a datetime with a time of day.
    ///
    /// [`DateTime::parse`] intentionally also accepts date-only values for
    /// callers that need a generic ISO-8601 date/datetime parser. OKF frontmatter
    /// timestamp fields use [`DateTimeField::is_valid`].
    #[must_use]
    pub const fn has_time(&self) -> bool {
        match self.datetime {
            Some(datetime) => datetime.has_time,
            None => false,
        }
    }

    /// `true` if the raw scalar parsed with an explicit UTC offset.
    #[must_use]
    pub const fn has_offset(&self) -> bool {
        match self.datetime {
            Some(datetime) => datetime.offset_minutes.is_some(),
            None => false,
        }
    }
}

impl fmt::Display for DateTimeField {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.raw)
    }
}

fn expect<'a>(s: &mut &'a str, c: char) -> Option<()> {
    let rest: &'a str = (*s).strip_prefix(c)?;
    *s = rest;
    Some(())
}

/// Consumes exactly `n` ASCII digits from the front of `s`.
fn take_u32<'a>(s: &mut &'a str, n: usize) -> Option<u32> {
    let src: &'a str = s;
    if src.len() < n || !src.as_bytes()[..n].iter().all(u8::is_ascii_digit) {
        return None;
    }
    let value = src[..n].parse().ok()?;
    *s = &src[n..];
    Some(value)
}

/// Parses a trailing zone designator: empty (`None`), `Z`, or `±HH[[:]MM]`.
///
/// The outer `Option` distinguishes "no zone designator at all" (a bare
/// `YYYY-MM-DD` date, returned as `Some(None)`) from "a syntactically invalid
/// zone" (returned as `None`). The inner `Option<i32>` is the parsed offset in
/// minutes, with `Some(0)` for an explicit `Z` and `None` for an absent zone.
#[allow(clippy::option_option)]
fn parse_offset(s: &str) -> Option<Option<i32>> {
    if s.is_empty() {
        return Some(None);
    }
    if s.eq_ignore_ascii_case("z") {
        return Some(Some(0));
    }
    let (sign, rest) = match s.as_bytes()[0] {
        b'+' => (1, &s[1..]),
        b'-' => (-1, &s[1..]),
        _ => return None,
    };
    let mut rest = rest;
    let hours = take_u32(&mut rest, 2)?;
    let minutes = if rest.is_empty() {
        0
    } else {
        let _ = expect(&mut rest, ':');
        take_u32(&mut rest, 2)?
    };
    if !rest.is_empty() || hours > 23 || minutes > 59 {
        return None;
    }
    // `hours` (≤ 23) and `minutes` (≤ 59) fit in `i32` by construction; the
    // `try_from` keeps the cast explicit instead of relying on `as`.
    let h = i32::try_from(hours).expect("hours bounded to 23");
    let m = i32::try_from(minutes).expect("minutes bounded to 59");
    Some(Some(sign * (h * 60 + m)))
}

const fn is_leap_year(year: i32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

const fn days_in_month(year: i32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap_year(year) => 29,
        2 => 28,
        _ => 0,
    }
}

/// Days from 1970-01-01 to `y-m-d` (Howard Hinnant's `days_from_civil`).
fn days_from_civil(y: i32, m: u32, d: u32) -> i64 {
    let y = i64::from(y) - i64::from(m <= 2);
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400; // [0, 399]
    let mp = i64::from(if m > 2 { m - 3 } else { m + 9 }); // [0, 11]
    let doy = (153 * mp + 2) / 5 + i64::from(d) - 1; // [0, 365]
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
    era * 146_097 + doe - 719_468
}

/// The inverse of [`days_from_civil`] (Hinnant's `civil_from_days`).
///
/// The algorithm bounds `y` to a signed 32-bit range and `m`, `d` to `[1, 12]`
/// and `[1, 31]` respectively, so the narrowing casts below cannot truncate or
/// wrap for any date representable under the algorithm.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_possible_wrap
)]
fn civil_from_days(z: i64) -> (i32, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    ((y + i64::from(m <= 2)) as i32, m as u32, d as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plain_dates() {
        assert_eq!(Date::parse("2026-09-23"), Date::new(2026, 9, 23));
        assert_eq!(Date::parse("2024-02-29"), Date::new(2024, 2, 29));
        assert_eq!(Date::parse("2026-02-29"), None); // not a leap year
        assert_eq!(Date::parse("2026-13-01"), None);
        assert_eq!(Date::parse("2026-9-23"), None); // must be zero-padded
        assert_eq!(Date::parse("2026-09-23T00:00:00Z"), None);
    }

    #[test]
    fn epoch_roundtrip() {
        for days in [-40_000_i64, -1, 0, 1, 20_000, 100_000] {
            let d = Date::from_days_since_epoch(days);
            assert_eq!(d.days_since_epoch(), days, "{d}");
        }
        assert_eq!(Date::from_days_since_epoch(0).to_string(), "1970-01-01");
    }

    #[test]
    fn parses_datetimes_with_zones() {
        let z = DateTime::parse("2026-06-20T22:53:05Z").unwrap();
        assert_eq!(z.offset_minutes, Some(0));
        assert_eq!(z.to_string(), "2026-06-20T22:53:05Z");

        let offset = DateTime::parse("2026-05-28T22:53:05+00:00").unwrap();
        assert_eq!(
            offset.to_utc_seconds(),
            DateTime::parse("2026-05-28T22:53:05Z")
                .unwrap()
                .to_utc_seconds()
        );

        // Same instant, written in two zones.
        let a = DateTime::parse("2026-06-25T09:00:00+02:00").unwrap();
        let b = DateTime::parse("2026-06-25T07:00:00Z").unwrap();
        assert_eq!(a.cmp(&b), std::cmp::Ordering::Equal);

        assert!(DateTime::parse("2026-06-20 22:53:05").unwrap().has_time);
        assert!(!DateTime::parse("2026-06-20").unwrap().has_time);
        assert_eq!(DateTime::parse("2026-06-20T22:53").unwrap().second, 0);
        assert_eq!(
            DateTime::parse("2026-06-20T22:53:05.25Z")
                .unwrap()
                .nanosecond,
            250_000_000
        );
        assert_eq!(DateTime::parse("2026-06-20T25:00:00Z"), None);
        assert_eq!(DateTime::parse("not a date"), None);
    }

    #[test]
    fn offsets_order_across_midnight() {
        let late = DateTime::parse("2026-06-20T23:00:00-05:00").unwrap();
        assert_eq!(late.utc_date(), Date::new(2026, 6, 21).unwrap());
        assert!(late > DateTime::parse("2026-06-21T03:00:00Z").unwrap());
    }

    #[test]
    fn fields_keep_raw_text() {
        let bad = DateField::new("last tuesday");
        assert!(!bad.is_valid());
        assert_eq!(bad.raw, "last tuesday");

        let good = DateTimeField::new("2026-06-25T09:00:00Z");
        assert!(good.is_valid());
        assert!(good.has_time());
        assert!(good.has_offset());
        assert_eq!(good.datetime.unwrap().date, Date::new(2026, 6, 25).unwrap());

        let no_offset = DateTimeField::new("2026-06-25T09:00:00");
        assert!(!no_offset.is_valid());
        assert!(no_offset.has_time());
        assert!(!no_offset.has_offset());

        let date_only = DateTimeField::new("2026-06-25");
        assert!(!date_only.is_valid());
        assert!(!date_only.has_time());
        assert!(!date_only.has_offset());
    }
}
