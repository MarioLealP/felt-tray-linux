//! A minimal proleptic-Gregorian date type — a stand-in for Python's
//! `datetime.date`, covering exactly what the wardrobe module needs:
//! strict ISO (`YYYY-MM-DD`) parse/format and day arithmetic
//! (`(today - last_worn).days`, `today - timedelta(days=n)`).

use std::ops::Sub;

/// A calendar date in the proleptic Gregorian calendar, like Python's
/// `datetime.date`. Years are restricted to `1..=9999`, matching
/// `datetime.date`'s MINYEAR/MAXYEAR.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Date {
    year: i32,
    month: u32,
    day: u32,
}

/// A whole number of days, like `datetime.timedelta.days`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Days {
    /// Number of days in the span (Python: `timedelta.days`).
    pub days: i64,
}

impl Days {
    /// `datetime.timedelta(days = days)`.
    pub const fn new(days: i64) -> Days {
        Days { days }
    }
}

/// Errors from constructing or parsing a [`Date`] (Python raises
/// `ValueError` for all of these).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DateError {
    /// The string wasn't shaped like `YYYY-MM-DD`.
    #[error("invalid date format: {0:?}, expected YYYY-MM-DD")]
    InvalidFormat(String),
    /// The components don't name a real calendar date (or the year is
    /// outside `1..=9999`).
    #[error("invalid date: year {year}, month {month}, day {day}")]
    InvalidDate { year: i32, month: u32, day: u32 },
}

impl Date {
    /// `datetime.date(year, month, day)` — validates the calendar date.
    pub fn from_ymd(year: i32, month: u32, day: u32) -> Result<Date, DateError> {
        if !(1..=9999).contains(&year)
            || !(1..=12).contains(&month)
            || day < 1
            || day > days_in_month(year, month)
        {
            return Err(DateError::InvalidDate { year, month, day });
        }
        Ok(Date { year, month, day })
    }

    /// `datetime.date.fromisoformat` — strict `YYYY-MM-DD`.
    pub fn from_isoformat(s: &str) -> Result<Date, DateError> {
        let bytes = s.as_bytes();
        if bytes.len() != 10 || bytes[4] != b'-' || bytes[7] != b'-' {
            return Err(DateError::InvalidFormat(s.to_owned()));
        }
        let part = |a: usize, b: usize| -> Result<u32, DateError> {
            parse_digits(&bytes[a..b]).ok_or_else(|| DateError::InvalidFormat(s.to_owned()))
        };
        Date::from_ymd(part(0, 4)? as i32, part(5, 7)?, part(8, 10)?)
    }

    /// `datetime.date.isoformat` — zero-padded `YYYY-MM-DD`.
    pub fn isoformat(&self) -> String {
        format!("{:04}-{:02}-{:02}", self.year, self.month, self.day)
    }
}

/// `date - date` -> a `timedelta`-like day count (Python: `.days`).
impl Sub<Date> for Date {
    type Output = Days;

    fn sub(self, rhs: Date) -> Days {
        Days {
            days: days_from_civil(self.year, self.month, self.day)
                - days_from_civil(rhs.year, rhs.month, rhs.day),
        }
    }
}

/// `date - timedelta(days = n)` (as used by the tests to build relative
/// dates).
impl Sub<Days> for Date {
    type Output = Date;

    fn sub(self, rhs: Days) -> Date {
        let (year, month, day) =
            civil_from_days(days_from_civil(self.year, self.month, self.day) - rhs.days);
        Date { year, month, day }
    }
}

fn parse_digits(bytes: &[u8]) -> Option<u32> {
    let mut n = 0u32;
    for &c in bytes {
        if !c.is_ascii_digit() {
            return None;
        }
        n = n * 10 + u32::from(c - b'0');
    }
    Some(n)
}

fn is_leap(year: i32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

fn days_in_month(year: i32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap(year) => 29,
        2 => 28,
        _ => 0,
    }
}

/// Days since 1970-01-01 (Howard Hinnant's `days_from_civil`, proleptic
/// Gregorian — the same calendar `datetime.date` uses).
fn days_from_civil(year: i32, month: u32, day: u32) -> i64 {
    let y = i64::from(year) - if month <= 2 { 1 } else { 0 };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = i64::from(if month > 2 { month - 3 } else { month + 9 });
    let doy = (153 * mp + 2) / 5 + i64::from(day) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

/// Inverse of [`days_from_civil`] — exact for the whole range that
/// [`Date`] can represent.
fn civil_from_days(z: i64) -> (i32, u32, u32) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    (y as i32, m as u32, d as u32)
}
