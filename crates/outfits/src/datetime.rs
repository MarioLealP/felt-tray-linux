//! Minimal civil date/time, enough for the outfit schedule rules.
//!
//! The Python source uses `datetime.datetime` (naive, local time). Only
//! month/day/hour, day arithmetic and epoch seconds are needed here, so the
//! crate carries its own tiny type instead of a calendar dependency. Epoch
//! seconds treat the naive value as UTC; every comparison in `outfits` is
//! between two values produced the same way, so the offset cancels out.

use std::time::{SystemTime, UNIX_EPOCH};

/// A calendar date (no time zone).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Date {
    pub year: i32,
    pub month: u32,
    pub day: u32,
}

impl Date {
    /// `YYYY-MM-DD`, matching `datetime.date.isoformat()`.
    pub fn isoformat(&self) -> String {
        format!("{:04}-{:02}-{:02}", self.year, self.month, self.day)
    }
}

/// A naive date + time of day (no time zone).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DateTime {
    pub year: i32,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
}

impl DateTime {
    pub const fn new(year: i32, month: u32, day: u32, hour: u32, minute: u32, second: u32) -> Self {
        Self {
            year,
            month,
            day,
            hour,
            minute,
            second,
        }
    }

    /// The current wall-clock time (production default for the clock hook).
    pub fn now() -> Self {
        let secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        Self::from_unix_secs(secs)
    }

    pub fn date(&self) -> Date {
        Date {
            year: self.year,
            month: self.month,
            day: self.day,
        }
    }

    /// Seconds since the Unix epoch (naive value read as UTC).
    pub fn timestamp(&self) -> f64 {
        let days = days_from_civil(self.year, self.month, self.day);
        (days * 86400
            + i64::from(self.hour) * 3600
            + i64::from(self.minute) * 60
            + i64::from(self.second)) as f64
    }

    /// `self - hours`, crossing midnight backwards when needed
    /// (the wardrobe "day" rolls at 05:00).
    pub fn minus_hours(&self, hours: u32) -> Self {
        let mut hour = self.hour as i64 - i64::from(hours);
        let mut day = self.day as i64;
        while hour < 0 {
            hour += 24;
            day -= 1;
        }
        let (year, month, day) = borrow_day(self.year, self.month, day);
        Self::new(year, month, day, hour as u32, self.minute, self.second)
    }

    fn from_unix_secs(secs: i64) -> Self {
        let days = secs.div_euclid(86400);
        let rem = secs.rem_euclid(86400);
        let (year, month, day) = civil_from_days(days);
        Self::new(
            year,
            month,
            day,
            (rem / 3600) as u32,
            ((rem % 3600) / 60) as u32,
            (rem % 60) as u32,
        )
    }
}

/// Step `day` back to a valid calendar day for `year`/`month` (may go to the
/// previous month).
fn borrow_day(year: i32, month: u32, day: i64) -> (i32, u32, u32) {
    let mut year = year;
    let mut month = month;
    let mut day = day;
    while day < 1 {
        if month == 1 {
            month = 12;
            year -= 1;
        } else {
            month -= 1;
        }
        day += i64::from(days_in_month(year, month));
    }
    (year, month, day as u32)
}

fn days_in_month(year: i32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap(year) => 29,
        2 => 28,
        _ => 30,
    }
}

fn is_leap(year: i32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

/// Days since 1970-01-01 (Howard Hinnant's civil calendar algorithm).
fn days_from_civil(year: i32, month: u32, day: u32) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = (y - era * 400) as i64;
    let mp = (if month > 2 { month - 3 } else { month + 9 }) as i64;
    let doy = (153 * mp + 2) / 5 + i64::from(day) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era as i64 * 146_097 + doe - 719_468
}

/// Inverse of `days_from_civil`.
fn civil_from_days(days: i64) -> (i32, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y as i32, m as u32, d as u32)
}
