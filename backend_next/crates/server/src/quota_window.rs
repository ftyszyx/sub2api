use chrono::{
    DateTime, Datelike, Duration, FixedOffset, LocalResult, NaiveDate, TimeZone, Timelike, Utc,
};
use repository::UserPlatformQuotaRecord;
use serde_json::{json, Value};

const DEFAULT_TZ_OFFSET_SECONDS: i32 = 8 * 60 * 60;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlatformQuotaWindowStarts {
    pub daily: String,
    pub weekly: String,
    pub monthly: String,
}

#[derive(Debug, Clone, PartialEq)]
struct WindowSlice {
    usage: f64,
    limit: Option<f64>,
    resets_at: Option<String>,
    window_start: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PlatformQuotaExhaustion {
    pub window: &'static str,
    pub usage_usd: f64,
    pub limit_usd: f64,
    pub reset_at: String,
    pub retry_after_seconds: u64,
}

pub fn current_window_starts() -> PlatformQuotaWindowStarts {
    window_starts_for(now_utc())
}

pub fn window_starts_for(now: DateTime<Utc>) -> PlatformQuotaWindowStarts {
    let day_start = start_of_day(now);
    let week_start = start_of_week(now);
    PlatformQuotaWindowStarts {
        daily: format_rfc3339(day_start),
        weekly: format_rfc3339(week_start),
        monthly: format_rfc3339(now),
    }
}

pub fn platform_quota_json(
    record: &UserPlatformQuotaRecord,
    include_window_start: bool,
    now: DateTime<Utc>,
) -> Value {
    let daily = build_window_slice(
        record.daily_usage_usd,
        record.daily_limit_usd,
        record.daily_window_start.as_deref(),
        |start| needs_daily_reset(start, now),
        |_| next_daily_reset_time(now),
        include_window_start,
    );
    let weekly = build_window_slice(
        record.weekly_usage_usd,
        record.weekly_limit_usd,
        record.weekly_window_start.as_deref(),
        |start| needs_weekly_reset(start, now),
        |_| next_weekly_reset_time(now),
        include_window_start,
    );
    let monthly = build_window_slice(
        record.monthly_usage_usd,
        record.monthly_limit_usd,
        record.monthly_window_start.as_deref(),
        |start| needs_monthly_reset(start, now),
        |start| next_monthly_reset_time_from(start, now),
        include_window_start,
    );

    let mut item = json!({
        "platform": record.platform,
        "daily_usage_usd": daily.usage,
        "daily_limit_usd": daily.limit,
        "daily_window_resets_at": daily.resets_at,
        "weekly_usage_usd": weekly.usage,
        "weekly_limit_usd": weekly.limit,
        "weekly_window_resets_at": weekly.resets_at,
        "monthly_usage_usd": monthly.usage,
        "monthly_limit_usd": monthly.limit,
        "monthly_window_resets_at": monthly.resets_at
    });
    if include_window_start {
        item["daily_window_start"] = json!(daily.window_start);
        item["weekly_window_start"] = json!(weekly.window_start);
        item["monthly_window_start"] = json!(monthly.window_start);
    }
    item
}

pub fn reset_quota_window(
    record: &mut UserPlatformQuotaRecord,
    window: &str,
    now: DateTime<Utc>,
) -> Result<(), &'static str> {
    match window {
        "daily" => {
            record.daily_usage_usd = 0.0;
            record.daily_window_start = Some(format_rfc3339(start_of_day(now)));
        }
        "weekly" => {
            record.weekly_usage_usd = 0.0;
            record.weekly_window_start = Some(format_rfc3339(start_of_week(now)));
        }
        "monthly" => {
            record.monthly_usage_usd = 0.0;
            record.monthly_window_start = Some(format_rfc3339(now));
        }
        _ => return Err("window must be daily, weekly, or monthly"),
    }
    Ok(())
}

pub fn platform_quota_exhaustion(
    record: &UserPlatformQuotaRecord,
    now: DateTime<Utc>,
) -> Option<PlatformQuotaExhaustion> {
    let daily_usage = active_window_usage(
        record.daily_usage_usd,
        record.daily_window_start.as_deref(),
        |start| needs_daily_reset(start, now),
    );
    if let Some(limit) = record.daily_limit_usd {
        if daily_usage >= limit {
            let reset_at = next_daily_reset_time(now);
            return Some(quota_exhaustion("daily", daily_usage, limit, reset_at, now));
        }
    }

    let weekly_usage = active_window_usage(
        record.weekly_usage_usd,
        record.weekly_window_start.as_deref(),
        |start| needs_weekly_reset(start, now),
    );
    if let Some(limit) = record.weekly_limit_usd {
        if weekly_usage >= limit {
            let reset_at = next_weekly_reset_time(now);
            return Some(quota_exhaustion(
                "weekly",
                weekly_usage,
                limit,
                reset_at,
                now,
            ));
        }
    }

    let monthly_usage = active_window_usage(
        record.monthly_usage_usd,
        record.monthly_window_start.as_deref(),
        |start| needs_monthly_reset(start, now),
    );
    if let Some(limit) = record.monthly_limit_usd {
        if monthly_usage >= limit {
            let reset_at = record
                .monthly_window_start
                .as_deref()
                .and_then(parse_rfc3339)
                .filter(|start| !needs_monthly_reset(*start, now))
                .map(|start| next_monthly_reset_time_from(start, now))
                .unwrap_or_else(|| now + Duration::days(30));
            return Some(quota_exhaustion(
                "monthly",
                monthly_usage,
                limit,
                reset_at,
                now,
            ));
        }
    }

    None
}

fn active_window_usage(
    usage: f64,
    start: Option<&str>,
    expired: impl FnOnce(DateTime<Utc>) -> bool,
) -> f64 {
    match start.and_then(parse_rfc3339) {
        Some(parsed_start) if !expired(parsed_start) => usage,
        _ => 0.0,
    }
}

fn quota_exhaustion(
    window: &'static str,
    usage_usd: f64,
    limit_usd: f64,
    reset_at: DateTime<Utc>,
    now: DateTime<Utc>,
) -> PlatformQuotaExhaustion {
    let retry_after_seconds = reset_at.signed_duration_since(now).num_seconds().max(1) as u64;
    PlatformQuotaExhaustion {
        window,
        usage_usd,
        limit_usd,
        reset_at: format_rfc3339(reset_at),
        retry_after_seconds,
    }
}

fn build_window_slice(
    usage: f64,
    limit: Option<f64>,
    start: Option<&str>,
    expired: impl FnOnce(DateTime<Utc>) -> bool,
    next_reset: impl FnOnce(DateTime<Utc>) -> DateTime<Utc>,
    include_window_start: bool,
) -> WindowSlice {
    let Some(start) = start else {
        return WindowSlice {
            usage,
            limit,
            resets_at: None,
            window_start: None,
        };
    };
    let Some(parsed_start) = parse_rfc3339(start) else {
        return WindowSlice {
            usage,
            limit,
            resets_at: None,
            window_start: include_window_start.then(|| start.to_owned()),
        };
    };
    if expired(parsed_start) {
        return WindowSlice {
            usage: 0.0,
            limit,
            resets_at: None,
            window_start: include_window_start.then(|| start.to_owned()),
        };
    }
    WindowSlice {
        usage,
        limit,
        resets_at: Some(format_rfc3339(next_reset(parsed_start))),
        window_start: include_window_start.then(|| start.to_owned()),
    }
}

fn needs_daily_reset(start: DateTime<Utc>, now: DateTime<Utc>) -> bool {
    start < start_of_day(now)
}

fn needs_weekly_reset(start: DateTime<Utc>, now: DateTime<Utc>) -> bool {
    start < start_of_week(now)
}

fn needs_monthly_reset(start: DateTime<Utc>, now: DateTime<Utc>) -> bool {
    now.signed_duration_since(start) >= Duration::days(30)
}

fn next_daily_reset_time(now: DateTime<Utc>) -> DateTime<Utc> {
    start_of_day(now) + Duration::days(1)
}

fn next_weekly_reset_time(now: DateTime<Utc>) -> DateTime<Utc> {
    start_of_week(now) + Duration::days(7)
}

fn next_monthly_reset_time_from(start: DateTime<Utc>, now: DateTime<Utc>) -> DateTime<Utc> {
    if start.timestamp() == 0 {
        now + Duration::days(30)
    } else {
        start + Duration::days(30)
    }
}

fn start_of_day(time: DateTime<Utc>) -> DateTime<Utc> {
    let local = time.with_timezone(&server_timezone());
    local_midnight(local.year(), local.month(), local.day())
}

fn start_of_week(time: DateTime<Utc>) -> DateTime<Utc> {
    let local = time.with_timezone(&server_timezone());
    let monday = local.date_naive() - Duration::days(local.weekday().num_days_from_monday() as i64);
    local_midnight(monday.year(), monday.month(), monday.day())
}

fn local_midnight(year: i32, month: u32, day: u32) -> DateTime<Utc> {
    let timezone = server_timezone();
    let date = NaiveDate::from_ymd_opt(year, month, day).expect("valid local date");
    match timezone.from_local_datetime(&date.and_hms_opt(0, 0, 0).expect("valid midnight")) {
        LocalResult::Single(value) => value.with_timezone(&Utc),
        LocalResult::Ambiguous(value, _) => value.with_timezone(&Utc),
        LocalResult::None => timezone
            .with_ymd_and_hms(year, month, day, 0, 0, 0)
            .single()
            .expect("fixed offset local midnight")
            .with_timezone(&Utc),
    }
}

fn server_timezone() -> FixedOffset {
    let offset = std::env::var("TZ")
        .ok()
        .and_then(|value| parse_timezone_offset(&value))
        .unwrap_or(DEFAULT_TZ_OFFSET_SECONDS);
    FixedOffset::east_opt(offset).expect("valid timezone offset")
}

fn parse_timezone_offset(value: &str) -> Option<i32> {
    match value.trim() {
        "" | "Asia/Shanghai" | "Asia/Chongqing" | "Asia/Harbin" | "Asia/Urumqi" => {
            Some(8 * 60 * 60)
        }
        "UTC" | "Etc/UTC" | "Z" => Some(0),
        raw => parse_numeric_timezone_offset(raw),
    }
}

fn parse_numeric_timezone_offset(raw: &str) -> Option<i32> {
    let sign = if let Some(rest) = raw.strip_prefix('+') {
        (1, rest)
    } else if let Some(rest) = raw.strip_prefix('-') {
        (-1, rest)
    } else {
        return None;
    };
    let (sign, value) = sign;
    let mut parts = value.split(':');
    let hours = parts.next()?.parse::<i32>().ok()?;
    let minutes = parts.next().unwrap_or("0").parse::<i32>().ok()?;
    if parts.next().is_some() || hours > 23 || minutes > 59 {
        return None;
    }
    Some(sign * (hours * 60 * 60 + minutes * 60))
}

fn parse_rfc3339(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|value| value.with_timezone(&Utc))
}

fn format_rfc3339(time: DateTime<Utc>) -> String {
    time.with_nanosecond(0)
        .expect("valid second precision timestamp")
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

fn now_utc() -> DateTime<Utc> {
    Utc::now()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dt(value: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(value)
            .unwrap()
            .with_timezone(&Utc)
    }

    fn quota() -> UserPlatformQuotaRecord {
        UserPlatformQuotaRecord {
            id: 1,
            user_id: 1,
            platform: "openai".to_owned(),
            daily_limit_usd: Some(1.0),
            weekly_limit_usd: Some(3.0),
            monthly_limit_usd: Some(9.0),
            daily_usage_usd: 0.7,
            weekly_usage_usd: 1.7,
            monthly_usage_usd: 2.7,
            daily_window_start: Some("2026-06-06T16:00:00Z".to_owned()),
            weekly_window_start: Some("2026-05-31T16:00:00Z".to_owned()),
            monthly_window_start: Some("2026-05-20T00:00:00Z".to_owned()),
        }
    }

    #[test]
    fn computes_asia_shanghai_day_and_week_windows() {
        let starts = window_starts_for(dt("2026-06-07T10:30:00Z"));

        assert_eq!(starts.daily, "2026-06-06T16:00:00Z");
        assert_eq!(starts.weekly, "2026-05-31T16:00:00Z");
        assert_eq!(starts.monthly, "2026-06-07T10:30:00Z");
    }

    #[test]
    fn lazy_zero_resets_expired_windows_without_rewriting_start() {
        let mut record = quota();
        record.daily_window_start = Some("2026-06-05T16:00:00Z".to_owned());
        record.weekly_window_start = Some("2026-05-24T16:00:00Z".to_owned());
        record.monthly_window_start = Some("2026-05-01T00:00:00Z".to_owned());

        let item = platform_quota_json(&record, false, dt("2026-06-07T10:30:00Z"));

        assert_eq!(item["daily_usage_usd"], 0.0);
        assert_eq!(item["weekly_usage_usd"], 0.0);
        assert_eq!(item["monthly_usage_usd"], 0.0);
        assert_eq!(item["daily_window_resets_at"], Value::Null);
        assert_eq!(item["weekly_window_resets_at"], Value::Null);
        assert_eq!(item["monthly_window_resets_at"], Value::Null);
        assert!(item.get("daily_window_start").is_none());
    }

    #[test]
    fn active_windows_return_next_reset_times_and_admin_starts() {
        let item = platform_quota_json(&quota(), true, dt("2026-06-07T10:30:00Z"));

        assert_eq!(item["daily_usage_usd"], 0.7);
        assert_eq!(item["daily_window_resets_at"], "2026-06-07T16:00:00Z");
        assert_eq!(item["weekly_window_resets_at"], "2026-06-07T16:00:00Z");
        assert_eq!(item["monthly_window_resets_at"], "2026-06-19T00:00:00Z");
        assert_eq!(item["daily_window_start"], "2026-06-06T16:00:00Z");
    }

    #[test]
    fn reset_quota_window_uses_current_window_start() {
        let mut record = quota();

        reset_quota_window(&mut record, "daily", dt("2026-06-07T10:30:00Z")).unwrap();
        reset_quota_window(&mut record, "weekly", dt("2026-06-07T10:30:00Z")).unwrap();
        reset_quota_window(&mut record, "monthly", dt("2026-06-07T10:30:00Z")).unwrap();

        assert_eq!(record.daily_usage_usd, 0.0);
        assert_eq!(
            record.daily_window_start.as_deref(),
            Some("2026-06-06T16:00:00Z")
        );
        assert_eq!(
            record.weekly_window_start.as_deref(),
            Some("2026-05-31T16:00:00Z")
        );
        assert_eq!(
            record.monthly_window_start.as_deref(),
            Some("2026-06-07T10:30:00Z")
        );
    }

    #[test]
    fn platform_quota_exhaustion_blocks_active_window_at_limit() {
        let mut record = quota();
        record.daily_usage_usd = 1.0;
        let exhaustion = platform_quota_exhaustion(&record, dt("2026-06-07T10:30:00Z")).unwrap();

        assert_eq!(exhaustion.window, "daily");
        assert_eq!(exhaustion.usage_usd, 1.0);
        assert_eq!(exhaustion.limit_usd, 1.0);
        assert_eq!(exhaustion.reset_at, "2026-06-07T16:00:00Z");
        assert_eq!(exhaustion.retry_after_seconds, 19_800);
    }

    #[test]
    fn platform_quota_exhaustion_ignores_expired_usage() {
        let mut record = quota();
        record.daily_usage_usd = 10.0;
        record.weekly_usage_usd = 10.0;
        record.monthly_usage_usd = 10.0;
        record.daily_window_start = Some("2026-06-05T16:00:00Z".to_owned());
        record.weekly_window_start = Some("2026-05-24T16:00:00Z".to_owned());
        record.monthly_window_start = Some("2026-05-01T00:00:00Z".to_owned());

        assert!(platform_quota_exhaustion(&record, dt("2026-06-07T10:30:00Z")).is_none());
    }

    #[test]
    fn platform_quota_exhaustion_treats_zero_limit_as_exhausted() {
        let mut record = quota();
        record.daily_limit_usd = Some(0.0);
        record.daily_usage_usd = 0.0;
        record.daily_window_start = None;
        record.weekly_limit_usd = None;
        record.monthly_limit_usd = None;

        let exhaustion = platform_quota_exhaustion(&record, dt("2026-06-07T10:30:00Z")).unwrap();

        assert_eq!(exhaustion.window, "daily");
        assert_eq!(exhaustion.usage_usd, 0.0);
        assert_eq!(exhaustion.limit_usd, 0.0);
    }
}
