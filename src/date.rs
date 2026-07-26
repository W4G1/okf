//! Calendar dates and ISO-8601 datetimes for the trust and lifecycle families
//! (§5).
//!
//! OKF v0.2 puts date-shaped fields in frontmatter and asks consumers to answer
//! questions about them: which verification is the most recent, and is
//! `today >= stale_after`. Two shapes occur:
//!
//! - ISO-8601 datetimes in `generated.at` and `verified[].at` (§5.2).
//! - Plain `YYYY-MM-DD` dates in `stale_after` (§5.5),
//!   `sources[].last_modified`, and `usage_window` (§5.1).
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
    pub fn new(year: i32, month: u32, day: u32) -> Option<Date> {
        if !(1..=12).contains(&month) || day < 1 || day > days_in_month(year, month) {
            return None;
        }
        Some(Date { year, month, day })
    }

    /// Parses a strict `YYYY-MM-DD` date. Returns `None` for any other shape,
    /// including datetimes; use [`DateTime::parse`] for those.
    pub fn parse(s: &str) -> Option<Date> {
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
        Date::new(
            s[0..4].parse().ok()?,
            s[5..7].parse().ok()?,
            s[8..10].parse().ok()?,
        )
    }

    /// Today's date in UTC, from the system clock.
    ///
    /// Returns `None` only if the clock reports a time before the Unix epoch.
    pub fn today_utc() -> Option<Date> {
        let secs = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs() as i64;
        Some(Date::from_days_since_epoch(secs.div_euclid(86_400)))
    }

    /// Days since 1970-01-01 (negative before it).
    pub fn days_since_epoch(&self) -> i64 {
        days_from_civil(self.year, self.month, self.day)
    }

    /// The date `days` after 1970-01-01.
    pub fn from_days_since_epoch(days: i64) -> Date {
        let (year, month, day) = civil_from_days(days);
        Date { year, month, day }
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
        Date::parse(s).ok_or_else(|| ParseDateError(s.to_string()))
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
    pub fn parse(s: &str) -> Option<DateTime> {
        let s = s.trim();
        if !s.is_ascii() || s.len() < 10 {
            return None;
        }
        let date = Date::parse(&s[..10])?;
        let rest = &s[10..];
        if rest.is_empty() {
            return Some(DateTime {
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
                let mut nanos = digits.clone();
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
        Some(DateTime {
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
    pub fn to_utc_seconds(&self) -> i64 {
        self.date.days_since_epoch() * 86_400
            + i64::from(self.hour) * 3600
            + i64::from(self.minute) * 60
            + i64::from(self.second)
            - i64::from(self.offset_minutes.unwrap_or(0)) * 60
    }

    /// The calendar date in UTC, which differs from [`DateTime::date`] when the
    /// value carries an offset that crosses midnight.
    pub fn utc_date(&self) -> Date {
        Date::from_days_since_epoch(self.to_utc_seconds().div_euclid(86_400))
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
        DateTime::parse(s).ok_or_else(|| ParseDateError(s.to_string()))
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
    pub fn new(raw: impl Into<String>) -> DateField {
        let raw = raw.into();
        let date = Date::parse(raw.trim());
        DateField { raw, date }
    }

    /// `true` if the raw scalar parsed as a date.
    pub fn is_valid(&self) -> bool {
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
/// See [`DateField`] for why the raw text is retained.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DateTimeField {
    /// The scalar as written in the frontmatter.
    pub raw: String,
    /// The parsed datetime, or `None` if `raw` is not ISO-8601.
    pub datetime: Option<DateTime>,
}

impl DateTimeField {
    /// Wraps a raw scalar, parsing it if possible.
    pub fn new(raw: impl Into<String>) -> DateTimeField {
        let raw = raw.into();
        let datetime = DateTime::parse(&raw);
        DateTimeField { raw, datetime }
    }

    /// `true` if the raw scalar parsed as a datetime.
    pub fn is_valid(&self) -> bool {
        self.datetime.is_some()
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
    Some(Some(sign * (hours as i32 * 60 + minutes as i32)))
}

fn is_leap_year(year: i32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

fn days_in_month(year: i32, month: u32) -> u32 {
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
        assert_eq!(good.datetime.unwrap().date, Date::new(2026, 6, 25).unwrap());
    }
}
