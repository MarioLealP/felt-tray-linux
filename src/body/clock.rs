//! Her body's sense of time: the local wall clock (the system time zone, so summer time is handled) and a
//! monotonic clock for elapsed-time maths.

use std::sync::OnceLock;
use std::time::Instant;

/// Now, in the desktop's time zone.
pub fn local() -> jiff::Zoned {
    jiff::Zoned::now()
}

/// The hour-resolution clock felt-mood runs on.
pub fn mood_now() -> felt_mood::DateTime {
    let z = local();
    // jiff's calendar fields are small positive ints (month 1-12, day 1-31, hour 0-23)
    felt_mood::DateTime::new(
        i32::from(z.year()),
        z.month().unsigned_abs().into(),
        z.day().unsigned_abs().into(),
        z.hour().unsigned_abs().into(),
    )
}

/// Seconds since her body started.
pub fn monotonic_s() -> f64 {
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_secs_f64()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_mood_clock_is_a_real_date() {
        let d = mood_now();
        assert!(d.year >= 2026 && (1..=12).contains(&d.month));
        assert!((1..=31).contains(&d.day) && d.hour < 24);
    }

    #[test]
    fn monotonic_moves_forward() {
        let a = monotonic_s();
        assert!(monotonic_s() >= a);
    }
}
