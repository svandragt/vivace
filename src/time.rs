//! Civil-date/epoch-day helpers shared by `show` (release-age math) and
//! `lock_writer` (normalising a package's `time` field to Composer's
//! `DATE_RFC3339`). Split out so neither module copies Howard Hinnant's
//! `days_from_civil`/`civil_from_days` algorithm.

/// A calendar date, no time-of-day: `show`'s age buckets never need finer
/// than day precision, and `lock_writer` folds the time-of-day back in
/// itself around the epoch-day conversion below.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Ymd {
    pub(crate) y: i64,
    pub(crate) m: u32,
    pub(crate) d: u32,
}

/// Composer's own `time` field shape: `Y-m-d\TH:i:sP` (`DATE_ATOM`). Only
/// the date part is kept (see [`Ymd`]).
pub(crate) fn parse_time(value: &str) -> Option<Ymd> {
    let date = value.split('T').next()?;
    let mut parts = date.splitn(3, '-');
    Some(Ymd {
        y: parts.next()?.parse().ok()?,
        m: parts.next()?.parse().ok()?,
        d: parts.next()?.parse().ok()?,
    })
}

/// Howard Hinnant's `days_from_civil`: days since the Unix epoch for a
/// proleptic-Gregorian calendar date.
pub(crate) fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (i64::from(m) + 9) % 12;
    let doy = (153 * mp + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The inverse of [`days_from_civil`].
pub(crate) fn civil_from_days(z: i64) -> Ymd {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    Ymd {
        y: y + i64::from(m <= 2),
        #[allow(
            clippy::cast_sign_loss,
            clippy::cast_possible_truncation,
            reason = "m is 1..=12 by construction"
        )]
        m: m as u32,
        #[allow(
            clippy::cast_sign_loss,
            clippy::cast_possible_truncation,
            reason = "d is 1..=31 by construction"
        )]
        d: d as u32,
    }
}
