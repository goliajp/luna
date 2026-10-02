//! Calendar arithmetic over UTC: civil dates, `gmtime`/`mktime` and ISO weeks.

/// Howard Hinnant's days-from-civil: days since 1970-01-01 of the
/// proleptic Gregorian (y, m, d), `m` in 1..=12.
pub(super) fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y / 400 } else { (y - 399) / 400 };
    let yoe = y - era * 400; // [0, 399]
    let mm = m as i64;
    let doy = (153 * (mm + if mm > 2 { -3 } else { 9 }) + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

/// Inverse of `days_from_civil`.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719468;
    let era = if z >= 0 {
        z / 146097
    } else {
        (z - 146096) / 146097
    };
    let doe = (z - era * 146097) as u64; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

fn is_leap(y: i64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

/// A `struct tm` for UTC: `month` 1-based, `wday` 0 = Sunday, `yday`
/// 0-based, as C keeps them.
pub(super) struct Tm {
    pub(super) year: i64,
    pub(super) month: u32,
    pub(super) day: u32,
    pub(super) hour: u32,
    pub(super) min: u32,
    pub(super) sec: u32,
    pub(super) wday: u32,
    pub(super) yday: u32,
}

/// `gmtime`: `None` when the year does not fit `tm_year` (an `int`).
pub(super) fn gmtime(t: i64) -> Option<Tm> {
    let days = t.div_euclid(86_400);
    let secs = t.rem_euclid(86_400) as u32;
    let (year, month, day) = civil_from_days(days);
    i32::try_from(year - 1900).ok()?;
    let yday = (days - days_from_civil(year, 1, 1)) as u32;
    Some(Tm {
        year,
        month,
        day,
        hour: secs / 3600,
        min: (secs % 3600) / 60,
        sec: secs % 60,
        // 1970-01-01 was a Thursday
        wday: (days + 4).rem_euclid(7) as u32,
        yday,
    })
}

/// `mktime` over UTC: normalise the out-of-range fields and return the
/// time, or `None` when it cannot be represented.
pub(super) fn mktime(
    year: i64,
    mon0: i64,
    mday: i64,
    hour: i64,
    min: i64,
    sec: i64,
) -> Option<i64> {
    let year = year.checked_add(mon0.div_euclid(12))?;
    let month = mon0.rem_euclid(12) as u32 + 1;
    let t = days_from_civil(year, month, 1)
        .checked_add(mday - 1)?
        .checked_mul(86_400)?
        .checked_add(hour * 3600 + min * 60 + sec)?;
    gmtime(t)?;
    Some(t)
}

/// ISO 8601 week-based year and week (for `%G`, `%g`, `%V`).
pub(super) fn iso_week(tm: &Tm) -> (i64, u32) {
    let wd = (tm.wday + 6) % 7; // Monday = 0
    let week = (tm.yday as i64 - wd as i64 + 10) / 7;
    if week < 1 {
        // the last week of the previous year
        let py = tm.year - 1;
        let pyday = tm.yday as i64 + if is_leap(py) { 366 } else { 365 };
        let pwd = wd as i64;
        return (py, ((pyday - pwd + 10) / 7) as u32);
    }
    let days_in_year = if is_leap(tm.year) { 366 } else { 365 };
    // a week of which 4+ days fall in January belongs to the next year
    if week == 53 && tm.yday as i64 - wd as i64 + 3 >= days_in_year {
        return (tm.year + 1, 1);
    }
    (tm.year, week as u32)
}
