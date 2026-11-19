//! Calendar dates and RFC 3339 timestamps for the trust and lifecycle families.
//!
//! OKF puts timestamp fields in frontmatter and asks consumers to answer
//! questions about them: which verification is current, which is the most
//! recent, and is `now >= stale_after`.
//!
//! The timestamp keys are `generated.at`, `verified[].at`, `stale_after`,
//! `sources[].last_modified`, `usage_window.from`, and `usage_window.to`. Each
//! value has one of two forms from RFC 3339, and no other form is valid:
//!
//! | Form      | Example                | Meaning                                |
//! |-----------|------------------------|----------------------------------------|
//! | Date-time | `2026-06-30T14:00:00Z` | That instant (an offset is required).  |
//! | Date only | `2026-06-30`           | The start of that day in UTC.          |
//!
//! Plain `YYYY-MM-DD` dates are also what `log.md` section headings use.
//!
//! Answering those questions needs real date arithmetic, so this module
//! implements the small amount required on the standard library alone: a
//! proleptic Gregorian [`Date`], an offset-aware [`DateTime`] that orders
//! correctly across time zones, and the [`DateTimeField`] wrapper that keeps
//! the raw scalar around so a validator can report *what* failed to parse.
//!
//! Two parsers sit side by side on purpose. [`DateTime::parse_rfc3339`] is the
//! strict one that decides whether a frontmatter value is valid, and
//! [`DateTime::parse`] is a lenient ISO 8601 reader used to *repair* values
//! that are close (a missing offset, a space separator).

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
/// consumers need in order to answer "which verification is most recent"
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
    /// Parses an ISO-8601 datetime leniently.
    ///
    /// Accepts `YYYY-MM-DD`, `YYYY-MM-DDTHH:MM[:SS[.fraction]]` (with `T`, `t`,
    /// or a space as the separator), and an optional `Z` / `±HH:MM` / `±HHMM` /
    /// `±HH` zone.
    ///
    /// This is deliberately wider than what OKF accepts in frontmatter, so
    /// that a value which is merely close can be read and repaired. Use
    /// [`DateTime::parse_rfc3339`] to decide whether a value is valid.
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
                nanosecond = fraction_to_nanos(&digits)?;
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

    /// Parses an OKF timestamp: one of the two RFC 3339 forms the
    /// specification allows, and nothing else.
    ///
    /// - A `date-time`, `YYYY-MM-DDTHH:MM:SS[.fraction]` followed by `Z` or a
    ///   `±HH:MM` offset. The `T` and `Z` may be lower case. The offset is
    ///   required: a date-time without one is malformed.
    /// - A `full-date`, `YYYY-MM-DD`, which means the start of that day in
    ///   UTC. The result is midnight with a zero offset, and
    ///   [`DateTime::has_time`] stays `false` so the written form is still
    ///   distinguishable.
    ///
    /// Unlike [`DateTime::parse`], nothing is trimmed and nothing is guessed:
    /// a space separator, a missing seconds field, or a `±HHMM` offset all
    /// return `None`.
    #[must_use]
    pub fn parse_rfc3339(s: &str) -> Option<Self> {
        if !s.is_ascii() || s.len() < 10 {
            return None;
        }
        let date = Date::parse(&s[..10])?;
        let rest = &s[10..];
        if rest.is_empty() {
            return Some(Self {
                has_time: false,
                ..Self::from_date_utc(date)
            });
        }

        let mut rest = rest.strip_prefix('T').or_else(|| rest.strip_prefix('t'))?;
        let hour = take_u32(&mut rest, 2)?;
        expect(&mut rest, ':')?;
        let minute = take_u32(&mut rest, 2)?;
        expect(&mut rest, ':')?;
        let second = take_u32(&mut rest, 2)?;
        let mut nanosecond = 0;
        if let Some(after_dot) = rest.strip_prefix('.') {
            let digits = after_dot.bytes().take_while(u8::is_ascii_digit).count();
            if digits == 0 {
                return None;
            }
            nanosecond = fraction_to_nanos(&after_dot[..digits])?;
            rest = &after_dot[digits..];
        }
        if hour > 23 || minute > 59 || second > 60 {
            return None;
        }

        let offset_minutes = if rest.eq_ignore_ascii_case("z") {
            0
        } else {
            let sign = match rest.as_bytes().first() {
                Some(b'+') => 1,
                Some(b'-') => -1,
                _ => return None,
            };
            let mut zone = &rest[1..];
            let hours = take_u32(&mut zone, 2)?;
            expect(&mut zone, ':')?;
            let minutes = take_u32(&mut zone, 2)?;
            if !zone.is_empty() || hours > 23 || minutes > 59 {
                return None;
            }
            sign * (i32::try_from(hours).ok()? * 60 + i32::try_from(minutes).ok()?)
        };

        Some(Self {
            date,
            hour,
            minute,
            second,
            nanosecond,
            offset_minutes: Some(offset_minutes),
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

/// Normalizes a date or datetime string to an ISO-8601 datetime with an explicit UTC offset (`Z`).
///
/// - Bare date `YYYY-MM-DD` -> `YYYY-MM-DDT00:00:00Z`
/// - Datetime without offset `YYYY-MM-DDTHH:MM:SS` -> `YYYY-MM-DDTHH:MM:SSZ`
/// - Datetime with space separator `YYYY-MM-DD HH:MM:SS` -> `YYYY-MM-DDTHH:MM:SSZ`
/// - Already valid datetime with offset -> canonical formatted string
/// - Unparseable string -> `None`
///
/// This always produces the date-time form. A date-only value is valid on its
/// own, so prefer [`repair_timestamp`] when the aim is to fix what is broken
/// and leave what is valid untouched.
#[must_use]
pub fn normalize_iso_datetime(s: &str) -> Option<String> {
    let clean = s.trim().trim_matches(|c| c == '\'' || c == '"').trim();
    let dt = DateTime::parse(clean)?;
    if !dt.has_time {
        Some(format!("{}T00:00:00Z", dt.date))
    } else if dt.offset_minutes.is_none() {
        let mut normalized = dt;
        normalized.offset_minutes = Some(0);
        Some(normalized.to_string())
    } else {
        Some(dt.to_string())
    }
}

/// Repairs a timestamp value, returning the replacement text, or `None` when
/// the value should be left exactly as written.
///
/// A value that is already one of the two valid forms is not rewritten, with
/// one exception: with `prefer_datetime` set, a date-only value is expanded to
/// the equivalent `YYYY-MM-DDT00:00:00Z`. That is for `generated.at` and
/// `verified[].at`, which the specification asks producers to write as
/// date-times because consumers compare them.
///
/// A value that is close but not valid is rewritten to the canonical
/// date-time form: a space separator becomes `T`, a `±HHMM` offset gains its
/// colon, and a missing offset is filled in as UTC. That last repair is an
/// assumption, so callers should say so when they report it. A value that
/// cannot be read as a date at all returns `None`.
#[must_use]
pub fn repair_timestamp(s: &str, prefer_datetime: bool) -> Option<String> {
    if let Some(valid) = DateTime::parse_rfc3339(s) {
        return (prefer_datetime && !valid.has_time).then(|| format!("{}T00:00:00Z", valid.date));
    }
    let dt = DateTime::parse(s)?;
    if !dt.has_time {
        // A date with stray whitespace around it.
        return Some(if prefer_datetime {
            format!("{}T00:00:00Z", dt.date)
        } else {
            dt.date.to_string()
        });
    }
    let mut repaired = dt;
    if repaired.offset_minutes.is_none() {
        repaired.offset_minutes = Some(0);
    }
    Some(repaired.to_string())
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

/// A date-only field: the scalar exactly as written, plus its parse.
///
/// OKF's own timestamp keys are read through [`DateTimeField`], which accepts
/// a date as well as a date-time. This stricter wrapper is for values that are
/// dates and nothing else, such as a `log.md` date heading.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DateField {
    /// The scalar as written.
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
    /// part as written, without shifting by any UTC offset.
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

/// A frontmatter timestamp field: the scalar exactly as written, plus its
/// parse.
///
/// Keeping the raw text lets a consumer round-trip the value and lets
/// the okf-validator crate report *which* scalar is malformed instead of
/// silently dropping it: the spec's permissiveness rule means an
/// unparseable timestamp must never make a document unreadable.
///
/// Read the value through [`DateTimeField::instant`], which is `Some` only for
/// the two forms the specification allows. [`DateTimeField::datetime`] is the
/// lenient reading kept for diagnostics and repair, and it is also set for
/// values that are *not* valid, so do not compare with it directly.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DateTimeField {
    /// The scalar as written in the frontmatter.
    pub raw: String,
    /// A lenient ISO-8601 reading of `raw`, or `None` if it is not a date at
    /// all. An offset-less or space-separated value still parses here, so
    /// that it can be diagnosed and repaired without losing the original
    /// scalar. Use [`DateTimeField::instant`] for the valid reading.
    pub datetime: Option<DateTime>,
    /// The strict reading: `Some` only for a valid OKF timestamp.
    instant: Option<DateTime>,
}

impl DateTimeField {
    /// Wraps a raw scalar, parsing it if possible.
    pub fn new(raw: impl Into<String>) -> Self {
        let raw = raw.into();
        let instant = DateTime::parse_rfc3339(&raw);
        let datetime = instant.or_else(|| DateTime::parse(&raw));
        Self {
            raw,
            datetime,
            instant,
        }
    }

    /// `true` if the raw scalar is a valid OKF timestamp: an RFC 3339
    /// date-time with an explicit offset, or a date only.
    #[must_use]
    pub const fn is_valid(&self) -> bool {
        self.instant.is_some()
    }

    /// The instant this field names, or `None` when the value is malformed.
    ///
    /// A date-only value is the start of that day in UTC, so the two forms
    /// compare with each other directly.
    #[must_use]
    pub const fn instant(&self) -> Option<DateTime> {
        self.instant
    }

    /// `true` if the value is valid and written in the date-only form.
    #[must_use]
    pub const fn is_date_only(&self) -> bool {
        match self.instant {
            Some(instant) => !instant.has_time,
            None => false,
        }
    }

    /// `true` if the raw scalar reads as a datetime with a time of day,
    /// whether or not it is valid.
    #[must_use]
    pub const fn has_time(&self) -> bool {
        match self.datetime {
            Some(datetime) => datetime.has_time,
            None => false,
        }
    }

    /// `true` if the raw scalar was written with an explicit UTC offset.
    #[must_use]
    pub const fn has_offset(&self) -> bool {
        match self.datetime {
            Some(datetime) => datetime.has_time && datetime.offset_minutes.is_some(),
            None => false,
        }
    }

    /// `true` if the value can be read leniently, so that
    /// [`repair_timestamp`] has something to work with.
    #[must_use]
    pub const fn is_repairable(&self) -> bool {
        self.instant.is_none() && self.datetime.is_some()
    }
}

impl fmt::Display for DateTimeField {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.raw)
    }
}

/// Left-aligns a run of fraction digits to nanosecond precision, truncating
/// beyond nine digits.
fn fraction_to_nanos(digits: &str) -> Option<u32> {
    let mut nanos: String = digits.chars().take(9).collect();
    while nanos.len() < 9 {
        nanos.push('0');
    }
    nanos.parse().ok()
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
    fn rfc3339_accepts_exactly_the_two_forms() {
        let instant = DateTime::parse_rfc3339("2026-06-30T14:00:00Z").unwrap();
        assert!(instant.has_time);
        assert_eq!(instant.offset_minutes, Some(0));
        assert_eq!(
            DateTime::parse_rfc3339("2026-06-30t14:00:00.5z")
                .unwrap()
                .nanosecond,
            500_000_000
        );
        assert_eq!(
            DateTime::parse_rfc3339("2026-09-23T14:00:00+02:00")
                .unwrap()
                .offset_minutes,
            Some(120)
        );

        // A date only is the start of that day in UTC.
        let day = DateTime::parse_rfc3339("2026-06-30").unwrap();
        assert!(!day.has_time);
        assert_eq!(
            day.cmp(&DateTime::parse_rfc3339("2026-06-30T00:00:00Z").unwrap()),
            std::cmp::Ordering::Equal
        );
        assert_eq!(day.to_string(), "2026-06-30");

        for malformed in [
            "2026-06-30T14:00:00",      // no offset
            "2026-06-30 14:00:00Z",     // space separator
            "2026-06-30T14:00Z",        // no seconds
            "2026-06-30T14:00:00+0200", // offset without a colon
            "2026-06-30T14:00:00+02",   // offset without minutes
            "2026-06-30T14:00:00.Z",    // empty fraction
            "2026-06-30T24:00:00Z",     // hour out of range
            "2026-02-30",               // no such day
            "20260630",
            " 2026-06-30",
            "soon",
            "",
        ] {
            assert_eq!(DateTime::parse_rfc3339(malformed), None, "{malformed:?}");
        }
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
        assert!(!good.is_date_only());
        assert_eq!(
            good.instant().unwrap().date,
            Date::new(2026, 6, 25).unwrap()
        );

        // No offset: readable for repair, but not a valid timestamp.
        let no_offset = DateTimeField::new("2026-06-25T09:00:00");
        assert!(!no_offset.is_valid());
        assert!(no_offset.instant().is_none());
        assert!(no_offset.has_time());
        assert!(!no_offset.has_offset());
        assert!(no_offset.is_repairable());

        // A date only is valid, and names midnight UTC.
        let date_only = DateTimeField::new("2026-06-25");
        assert!(date_only.is_valid());
        assert!(date_only.is_date_only());
        assert!(!date_only.has_time());
        assert_eq!(
            date_only.instant().unwrap().to_utc_seconds(),
            DateTime::parse("2026-06-25T00:00:00Z")
                .unwrap()
                .to_utc_seconds()
        );

        let garbage = DateTimeField::new("soon");
        assert!(!garbage.is_valid());
        assert!(!garbage.is_repairable());
    }

    #[test]
    fn repair_leaves_valid_values_alone() {
        // Valid values are never rewritten.
        assert_eq!(repair_timestamp("2026-06-30T14:20:00Z", false), None);
        assert_eq!(repair_timestamp("2026-06-30T14:20:00+02:00", true), None);
        assert_eq!(repair_timestamp("2026-06-30", false), None);
        // Except a date where a date-time is preferred.
        assert_eq!(
            repair_timestamp("2026-06-30", true).as_deref(),
            Some("2026-06-30T00:00:00Z")
        );
        // Close-but-invalid values are repaired.
        assert_eq!(
            repair_timestamp("2026-06-30T14:20:00", false).as_deref(),
            Some("2026-06-30T14:20:00Z")
        );
        assert_eq!(
            repair_timestamp("2026-06-30 14:20:00+0200", false).as_deref(),
            Some("2026-06-30T14:20:00+02:00")
        );
        assert_eq!(repair_timestamp("last tuesday", true), None);
    }

    #[test]
    fn test_normalize_iso_datetime() {
        assert_eq!(
            normalize_iso_datetime("2026-06-30"),
            Some("2026-06-30T00:00:00Z".to_string())
        );
        assert_eq!(
            normalize_iso_datetime("'2026-06-30'"),
            Some("2026-06-30T00:00:00Z".to_string())
        );
        assert_eq!(
            normalize_iso_datetime("\"2026-06-30\""),
            Some("2026-06-30T00:00:00Z".to_string())
        );
        assert_eq!(
            normalize_iso_datetime("2026-06-30T14:20:00"),
            Some("2026-06-30T14:20:00Z".to_string())
        );
        assert_eq!(
            normalize_iso_datetime("2026-06-30 14:20:00"),
            Some("2026-06-30T14:20:00Z".to_string())
        );
        assert_eq!(
            normalize_iso_datetime("2026-06-30T14:20:00Z"),
            Some("2026-06-30T14:20:00Z".to_string())
        );
        assert_eq!(
            normalize_iso_datetime("2026-06-30T14:20:00+02:00"),
            Some("2026-06-30T14:20:00+02:00".to_string())
        );
        assert_eq!(normalize_iso_datetime("last tuesday"), None);
    }
}
