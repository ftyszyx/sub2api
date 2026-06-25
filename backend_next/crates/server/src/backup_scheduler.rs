use crate::backup_executor::{
    execute_postgres_s3_backup, execute_repository_manifest_backup, PostgresS3BackupConfig,
};
use crate::response::ApiError;
use chrono::{Duration, LocalResult, NaiveTime, TimeZone, Utc};
use serde_json::{json, Value};

const DEFAULT_LOCK_TTL_SECONDS: i64 = 600;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackupSchedule {
    pub enabled: bool,
    pub cron_expr: String,
    pub retain_days: i64,
    pub retain_count: usize,
    pub last_run_at_unix: Option<i64>,
    pub next_run_at_unix: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackupScheduleDecision {
    Disabled,
    NotDue { next_run_at_unix: i64 },
    Due { scheduled_for_unix: i64 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackupScheduleRunStatus {
    Disabled,
    NotDue,
    Locked,
    Created,
}

#[derive(Debug, Clone)]
pub struct BackupScheduleRunResult {
    pub status: BackupScheduleRunStatus,
    pub schedule: Value,
    pub backup: Option<Value>,
}

#[derive(Debug, Clone)]
pub struct BackupScheduleRunInput {
    pub schedule: Value,
    pub now_unix: i64,
    pub owner: String,
    pub base_backup: Value,
    pub manifest: Value,
    pub postgres_s3_config: Option<PostgresS3BackupConfig>,
}

impl BackupSchedule {
    pub fn from_value(value: &Value) -> Self {
        let cron_expr = value
            .get("cron")
            .or_else(|| value.get("cron_expr"))
            .and_then(Value::as_str)
            .unwrap_or("0 3 * * *")
            .to_owned();
        Self {
            enabled: value
                .get("enabled")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            cron_expr,
            retain_days: value
                .get("retain_days")
                .or_else(|| value.get("expire_days"))
                .and_then(Value::as_i64)
                .unwrap_or(7)
                .max(0),
            retain_count: value
                .get("retain_count")
                .or_else(|| value.get("keep_last"))
                .and_then(Value::as_u64)
                .and_then(|value| usize::try_from(value).ok())
                .unwrap_or(10),
            last_run_at_unix: value.get("last_run_at_unix").and_then(Value::as_i64),
            next_run_at_unix: value.get("next_run_at_unix").and_then(Value::as_i64),
        }
    }

    pub fn executor(&self, value: &Value) -> String {
        value
            .get("executor")
            .or_else(|| value.get("backup_executor"))
            .and_then(Value::as_str)
            .unwrap_or("repository_manifest")
            .to_owned()
    }

    pub fn decision(&self, now_unix: i64) -> Result<BackupScheduleDecision, ApiError> {
        if !self.enabled {
            return Ok(BackupScheduleDecision::Disabled);
        }
        let next = self
            .next_run_at_unix
            .unwrap_or_else(|| next_run_after(&self.cron_expr, self.last_run_at_unix, now_unix));
        if next <= now_unix {
            Ok(BackupScheduleDecision::Due {
                scheduled_for_unix: next,
            })
        } else {
            Ok(BackupScheduleDecision::NotDue {
                next_run_at_unix: next,
            })
        }
    }

    pub fn with_run_result(&self, schedule: &Value, now_unix: i64) -> Value {
        let mut updated = schedule.clone();
        updated["last_run_at_unix"] = json!(now_unix);
        updated["last_run_at"] = json!(unix_to_rfc3339(now_unix));
        let next = next_run_after(&self.cron_expr, Some(now_unix), now_unix);
        updated["next_run_at_unix"] = json!(next);
        updated["next_run_at"] = json!(unix_to_rfc3339(next));
        updated
    }
}

pub async fn run_due_backup_schedule<R>(
    repository: &R,
    input: BackupScheduleRunInput,
) -> Result<BackupScheduleRunResult, ApiError>
where
    R: repository::AppRepository + ?Sized,
{
    let schedule = BackupSchedule::from_value(&input.schedule);
    let decision = schedule.decision(input.now_unix)?;
    let BackupScheduleDecision::Due { scheduled_for_unix } = decision else {
        return Ok(BackupScheduleRunResult {
            status: match decision {
                BackupScheduleDecision::Disabled => BackupScheduleRunStatus::Disabled,
                BackupScheduleDecision::NotDue { .. } => BackupScheduleRunStatus::NotDue,
                BackupScheduleDecision::Due { .. } => unreachable!(),
            },
            schedule: input.schedule,
            backup: None,
        });
    };

    let lock_name = format!("backup:schedule:{scheduled_for_unix}");
    let Some(lock) = repository
        .try_acquire_lock(
            &lock_name,
            &input.owner,
            DEFAULT_LOCK_TTL_SECONDS,
            json!({ "scheduled_for_unix": scheduled_for_unix }),
        )
        .await
        .map_err(repository_error)?
    else {
        return Ok(BackupScheduleRunResult {
            status: BackupScheduleRunStatus::Locked,
            schedule: input.schedule,
            backup: None,
        });
    };

    let backup = execute_scheduled_backup(&schedule, &input).await?;
    let updated_schedule = schedule.with_run_result(&input.schedule, input.now_unix);
    repository
        .release_lock(&lock_name, &input.owner, lock.fencing_token)
        .await
        .map_err(repository_error)?;
    Ok(BackupScheduleRunResult {
        status: BackupScheduleRunStatus::Created,
        schedule: updated_schedule,
        backup: Some(backup),
    })
}

async fn execute_scheduled_backup(
    schedule: &BackupSchedule,
    input: &BackupScheduleRunInput,
) -> Result<Value, ApiError> {
    match schedule.executor(&input.schedule).as_str() {
        "repository_manifest" => {
            execute_repository_manifest_backup(input.base_backup.clone(), input.manifest.clone())
                .await
        }
        "postgres_s3" => {
            let config = input
                .postgres_s3_config
                .clone()
                .ok_or_else(|| ApiError::bad_request("postgres_s3 schedule requires config"))?;
            execute_postgres_s3_backup(input.base_backup.clone(), input.manifest.clone(), config)
                .await
        }
        _ => Err(ApiError::bad_request(
            "backup executor must be repository_manifest or postgres_s3",
        )),
    }
}

pub fn next_run_after(cron_expr: &str, last_run_at_unix: Option<i64>, now_unix: i64) -> i64 {
    let minute_hour = parse_daily_cron(cron_expr).unwrap_or((0, 3));
    let base_unix = last_run_at_unix.unwrap_or(now_unix);
    let base = Utc
        .timestamp_opt(base_unix, 0)
        .single()
        .unwrap_or_else(Utc::now);
    let today = base.date_naive();
    let target_time = NaiveTime::from_hms_opt(minute_hour.1, minute_hour.0, 0)
        .unwrap_or_else(|| NaiveTime::from_hms_opt(3, 0, 0).expect("valid time"));
    let candidate = match Utc.from_local_datetime(&today.and_time(target_time)) {
        LocalResult::Single(value) => value,
        _ => base + Duration::days(1),
    };
    let next = if candidate.timestamp() > base_unix {
        candidate
    } else {
        candidate + Duration::days(1)
    };
    next.timestamp()
}

fn parse_daily_cron(cron_expr: &str) -> Option<(u32, u32)> {
    let parts = cron_expr.split_whitespace().collect::<Vec<_>>();
    if parts.len() != 5 {
        return None;
    }
    let minute = parts[0].parse::<u32>().ok()?;
    let hour = parts[1].parse::<u32>().ok()?;
    if minute > 59 || hour > 23 {
        return None;
    }
    Some((minute, hour))
}

fn unix_to_rfc3339(unix: i64) -> String {
    Utc.timestamp_opt(unix, 0)
        .single()
        .unwrap_or_else(Utc::now)
        .to_rfc3339()
}

fn repository_error(error: repository::RepositoryError) -> ApiError {
    match error {
        repository::RepositoryError::NotFound { entity, id } => {
            ApiError::not_found(format!("{entity} {id} not found"))
        }
        repository::RepositoryError::InvalidInput(message) => ApiError::bad_request(message),
        repository::RepositoryError::Conflict(message) => ApiError::conflict(message),
        repository::RepositoryError::Duplicate { entity, key } => {
            ApiError::conflict(format!("duplicate {entity}: {key}"))
        }
        repository::RepositoryError::Database(message) => {
            ApiError::internal_server_error(format!("repository error: {message}"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn computes_daily_next_run_from_simple_cron() {
        let schedule = BackupSchedule::from_value(&json!({
            "enabled": true,
            "cron": "30 2 * * *"
        }));
        assert_eq!(
            schedule.decision(1_780_713_600).unwrap(),
            BackupScheduleDecision::NotDue {
                next_run_at_unix: 1_780_799_400
            }
        );
        let due = BackupSchedule::from_value(&json!({
            "enabled": true,
            "cron": "30 2 * * *",
            "next_run_at_unix": 1_780_713_600
        }));
        assert_eq!(
            due.decision(1_780_713_600).unwrap(),
            BackupScheduleDecision::Due {
                scheduled_for_unix: 1_780_713_600
            }
        );
    }
}
