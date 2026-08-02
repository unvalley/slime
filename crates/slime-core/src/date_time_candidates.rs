use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct LocalDateTime {
    year: i32,
    month: u32,
    day: u32,
    weekday: u32,
    hour: u32,
    minute: u32,
}

pub(crate) const SHORT_NUMERIC: u32 = 1 << 0;
pub(crate) const ISO_NUMERIC: u32 = 1 << 1;
pub(crate) const MONTH_DAY_WEEKDAY: u32 = 1 << 2;
pub(crate) const LONG_GREGORIAN: u32 = 1 << 3;
pub(crate) const LONG_REIWA: u32 = 1 << 4;
pub(crate) const SHORT_REIWA: u32 = 1 << 5;
pub(crate) const WEEKDAY: u32 = 1 << 6;
pub(crate) const ALL_FORMATS: u32 = (1 << 7) - 1;

pub(crate) fn candidates(reading: &str, date_format_mask: u32) -> Vec<String> {
    let day_offset = match reading {
        "きのう" => -1,
        "きょう" => 0,
        "あした" => 1,
        "いま" => {
            let now = local_date_time(0);
            return vec![
                format!("{:02}:{:02}", now.hour, now.minute),
                format!("{}時{}分", now.hour, now.minute),
            ];
        }
        _ => return Vec::new(),
    };
    date_candidates(local_date_time(day_offset), date_format_mask)
}

fn date_candidates(value: LocalDateTime, mask: u32) -> Vec<String> {
    let weekday_index = usize::try_from(value.weekday).expect("weekday index");
    let weekday = ["日", "月", "火", "水", "木", "金", "土"][weekday_index];
    let weekday_name = [
        "日曜日",
        "月曜日",
        "火曜日",
        "水曜日",
        "木曜日",
        "金曜日",
        "土曜日",
    ][weekday_index];
    let mut values = Vec::with_capacity(7);
    if mask & SHORT_NUMERIC != 0 {
        values.push(format!("{}/{}", value.month, value.day));
    }
    if mask & ISO_NUMERIC != 0 {
        values.push(format!(
            "{:04}/{:02}/{:02}",
            value.year, value.month, value.day
        ));
    }
    if mask & MONTH_DAY_WEEKDAY != 0 {
        values.push(format!("{}月{}日({weekday})", value.month, value.day));
    }
    if mask & LONG_GREGORIAN != 0 {
        values.push(format!("{}年{}月{}日", value.year, value.month, value.day));
    }
    if mask & LONG_REIWA != 0
        && let Some(era) = japanese_era(value.year, value.month, value.day)
    {
        values.push(era);
    }
    if mask & SHORT_REIWA != 0
        && let Some(era) = abbreviated_japanese_era(value.year, value.month, value.day)
    {
        values.push(era);
    }
    if mask & WEEKDAY != 0 {
        values.push(weekday_name.to_owned());
    }
    values
}

fn japanese_era(year: i32, month: u32, day: u32) -> Option<String> {
    if (year, month, day) >= (2019, 5, 1) {
        let era_year = year - 2018;
        let year_text = if era_year == 1 {
            "元".to_owned()
        } else {
            era_year.to_string()
        };
        return Some(format!("令和{year_text}年{month}月{day}日"));
    }
    None
}

fn abbreviated_japanese_era(year: i32, month: u32, day: u32) -> Option<String> {
    if (year, month, day) >= (2019, 5, 1) {
        return Some(format!("R{:02}/{month:02}/{day:02}", year - 2018));
    }
    None
}

#[cfg(unix)]
fn local_date_time(day_offset: i64) -> LocalDateTime {
    let epoch_seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let raw = i64::try_from(epoch_seconds).unwrap_or(i64::MAX) as libc::time_t;
    let mut local = std::mem::MaybeUninit::<libc::tm>::uninit();
    // SAFETY: both pointers remain valid for the call. A failure is handled
    // before reading the output.
    let pointer = unsafe { libc::localtime_r(&raw const raw, local.as_mut_ptr()) };
    if pointer.is_null() {
        return LocalDateTime {
            year: 1970,
            month: 1,
            day: 1,
            weekday: 4,
            hour: 0,
            minute: 0,
        };
    }
    // SAFETY: localtime_r returned a non-null pointer to initialized output.
    let mut local = unsafe { local.assume_init() };
    if day_offset != 0 {
        local.tm_mday += i32::try_from(day_offset).expect("small date offset");
        local.tm_isdst = -1;
        // SAFETY: `local` is initialized and mktime normalizes the civil date
        // in place, including daylight-saving transitions.
        unsafe { libc::mktime(&raw mut local) };
    }
    LocalDateTime {
        year: local.tm_year + 1900,
        month: u32::try_from(local.tm_mon + 1).expect("local month"),
        day: u32::try_from(local.tm_mday).expect("local day"),
        weekday: u32::try_from(local.tm_wday).expect("local weekday"),
        hour: u32::try_from(local.tm_hour).expect("local hour"),
        minute: u32::try_from(local.tm_min).expect("local minute"),
    }
}

#[cfg(not(unix))]
fn local_date_time(day_offset: i64) -> LocalDateTime {
    let days = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
        / 86_400
        + day_offset;
    civil_from_days(days)
}

#[cfg(not(unix))]
fn civil_from_days(days: i64) -> LocalDateTime {
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    LocalDateTime {
        year: year as i32,
        month: month as u32,
        day: day as u32,
        weekday: (days + 4).rem_euclid(7) as u32,
        hour: 0,
        minute: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ALL_FORMATS, ISO_NUMERIC, LocalDateTime, SHORT_REIWA, WEEKDAY, abbreviated_japanese_era,
        date_candidates, japanese_era,
    };

    #[test]
    fn formats_fixed_date_candidates() {
        let value = LocalDateTime {
            year: 2026,
            month: 8,
            day: 2,
            weekday: 0,
            hour: 13,
            minute: 5,
        };
        let candidates = date_candidates(value, ALL_FORMATS);
        assert_eq!(
            candidates,
            [
                "8/2",
                "2026/08/02",
                "8月2日(日)",
                "2026年8月2日",
                "令和8年8月2日",
                "R08/08/02",
                "日曜日",
            ]
        );
        assert_eq!(
            date_candidates(value, ISO_NUMERIC | SHORT_REIWA | WEEKDAY),
            ["2026/08/02", "R08/08/02", "日曜日"]
        );
        assert_eq!(japanese_era(2019, 5, 1).as_deref(), Some("令和元年5月1日"));
        assert_eq!(
            abbreviated_japanese_era(2019, 5, 1).as_deref(),
            Some("R01/05/01")
        );
    }
}
