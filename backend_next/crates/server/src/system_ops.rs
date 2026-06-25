use crate::response::ApiError;
use chrono::{DateTime, Duration, Utc};
use http::HeaderMap;
use repository::{AdminCollectionItemRecord, AppRepository, RepositoryError};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::time::Duration as StdDuration;

const SYSTEM_OPERATIONS_COLLECTION: &str = "system-operations";
const SYSTEM_UPDATE_NAMESPACE: &str = "system_update";
const SYSTEM_UPDATE_INFO_KEY: &str = "latest_release";
const SYSTEM_OPERATION_LEASE_SECONDS: i64 = 30;
const UPDATE_CACHE_TTL_SECONDS: i64 = 20 * 60;
const DEFAULT_GITHUB_REPO: &str = "Wei-Shaw/sub2api";

#[derive(Debug, Clone)]
struct SystemOperation {
    operation: String,
    operation_id: String,
    idempotency_key: Option<String>,
    triggered_by: String,
    path: String,
    started_at: String,
    locked_until: String,
}

enum OperationStart {
    Existing(Value),
    Started(SystemOperation),
}

pub fn version() -> Value {
    json!({
        "version": env!("CARGO_PKG_VERSION"),
        "backend": "backend_next",
        "runtime": "rust"
    })
}

async fn stored_update_info(repository: &dyn AppRepository) -> Result<Option<Value>, ApiError> {
    match repository
        .get_system_setting(SYSTEM_UPDATE_NAMESPACE, SYSTEM_UPDATE_INFO_KEY)
        .await
    {
        Ok(record) => Ok(Some(record.value)),
        Err(RepositoryError::NotFound { .. }) => Ok(None),
        Err(error) => Err(repository_error(error)),
    }
}

fn update_info_response(
    current_version: &str,
    stored: &Value,
    cached: bool,
    force: bool,
    warning_override: Option<String>,
) -> Value {
    let latest_version = stored
        .get("latest_version")
        .or_else(|| stored.get("version"))
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(current_version);
    let release_info = stored.get("release_info").cloned().unwrap_or(Value::Null);
    let release_notes = stored
        .get("release_notes")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .or_else(|| {
            release_info
                .get("body")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .unwrap_or_default();
    let has_update = stored
        .get("has_update")
        .and_then(Value::as_bool)
        .unwrap_or_else(|| compare_versions(current_version, latest_version) < 0);
    let warning = warning_override
        .map(Value::String)
        .or_else(|| stored.get("warning").cloned())
        .unwrap_or(Value::Null);
    let build_type = stored
        .get("build_type")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| default_build_type().to_owned());

    json!({
        "current_version": current_version,
        "latest_version": latest_version,
        "has_update": has_update,
        "release_info": release_info,
        "release_notes": release_notes,
        "cached": cached,
        "warning": warning,
        "build_type": build_type,
        "force": force
    })
}

fn update_cache_is_fresh(value: &Value) -> bool {
    let Some(checked_at) = value.get("checked_at").and_then(Value::as_str) else {
        return false;
    };
    parse_rfc3339_utc(checked_at)
        .map(|checked_at| Utc::now() - checked_at < Duration::seconds(UPDATE_CACHE_TTL_SECONDS))
        .unwrap_or(false)
}

fn configured_update_check_endpoint() -> Option<String> {
    if let Ok(url) = std::env::var("BACKEND_NEXT_UPDATE_CHECK_URL") {
        let url = url.trim();
        if !url.is_empty() {
            return Some(url.to_owned());
        }
    }
    let repo = match std::env::var("BACKEND_NEXT_UPDATE_GITHUB_REPO")
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
    {
        Some(repo) => repo,
        None if default_build_type() == "source" => return None,
        None => DEFAULT_GITHUB_REPO.to_owned(),
    };
    Some(format!(
        "https://api.github.com/repos/{repo}/releases/latest"
    ))
}

async fn fetch_latest_release(endpoint: Option<&str>) -> Result<GitHubRelease, String> {
    let endpoint = endpoint.ok_or_else(|| "update checker is not configured".to_owned())?;
    let client = reqwest::Client::builder()
        .timeout(StdDuration::from_secs(10))
        .user_agent("sub2api-backend-next")
        .build()
        .map_err(|error| format!("failed to build update HTTP client: {error}"))?;
    let response = client
        .get(endpoint)
        .send()
        .await
        .map_err(|error| format!("failed to fetch latest release: {error}"))?;
    let status = response.status();
    if !status.is_success() {
        return Err(format!("failed to fetch latest release: HTTP {status}"));
    }
    response
        .json::<GitHubRelease>()
        .await
        .map_err(|error| format!("invalid latest release response: {error}"))
}

fn update_info_from_release(current_version: &str, release: GitHubRelease) -> Value {
    let latest_version = release.tag_name.trim_start_matches('v').to_owned();
    let assets = release
        .assets
        .into_iter()
        .map(|asset| {
            json!({
                "name": asset.name,
                "download_url": asset.browser_download_url,
                "size": asset.size
            })
        })
        .collect::<Vec<_>>();
    let release_info = json!({
        "name": release.name,
        "body": release.body,
        "published_at": release.published_at,
        "html_url": release.html_url,
        "assets": assets
    });
    let release_notes = release_info
        .get("body")
        .cloned()
        .unwrap_or_else(|| json!(""));
    json!({
        "latest_version": latest_version,
        "has_update": compare_versions(current_version, &latest_version) < 0,
        "release_info": release_info,
        "release_notes": release_notes,
        "checked_at": Utc::now().to_rfc3339(),
        "build_type": default_build_type()
    })
}

fn compare_versions(current: &str, latest: &str) -> i32 {
    let current = parse_version(current);
    let latest = parse_version(latest);
    for index in 0..3 {
        if current[index] < latest[index] {
            return -1;
        }
        if current[index] > latest[index] {
            return 1;
        }
    }
    0
}

fn parse_version(value: &str) -> [i32; 3] {
    let mut result = [0, 0, 0];
    for (index, part) in value.trim_start_matches('v').split('.').take(3).enumerate() {
        result[index] = part.parse::<i32>().unwrap_or(0);
    }
    result
}

#[derive(Debug, Deserialize)]
struct GitHubRelease {
    #[serde(default)]
    tag_name: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    body: String,
    #[serde(default)]
    published_at: String,
    #[serde(default)]
    html_url: String,
    #[serde(default)]
    assets: Vec<GitHubAsset>,
}

#[derive(Debug, Deserialize)]
struct GitHubAsset {
    #[serde(default)]
    name: String,
    #[serde(default)]
    browser_download_url: String,
    #[serde(default)]
    size: i64,
}

pub async fn check_updates_from_url(
    repository: &dyn AppRepository,
    force: bool,
    endpoint: &str,
) -> Result<Value, ApiError> {
    check_updates_with_endpoint(repository, force, Some(endpoint)).await
}

pub async fn check_updates(repository: &dyn AppRepository, force: bool) -> Result<Value, ApiError> {
    let endpoint = configured_update_check_endpoint();
    check_updates_with_endpoint(repository, force, endpoint.as_deref()).await
}

async fn check_updates_with_endpoint(
    repository: &dyn AppRepository,
    force: bool,
    endpoint: Option<&str>,
) -> Result<Value, ApiError> {
    let current_version = env!("CARGO_PKG_VERSION");
    let stored = stored_update_info(repository).await?;
    if !force {
        if let Some(cached) = stored.as_ref().filter(|value| update_cache_is_fresh(value)) {
            return Ok(update_info_response(
                current_version,
                cached,
                true,
                force,
                None,
            ));
        }
    }

    match fetch_latest_release(endpoint).await {
        Ok(release) => {
            let latest = update_info_from_release(current_version, release);
            repository
                .upsert_system_setting(repository::SystemSettingRecord {
                    namespace: SYSTEM_UPDATE_NAMESPACE.to_owned(),
                    key: SYSTEM_UPDATE_INFO_KEY.to_owned(),
                    value: latest.clone(),
                    updated_at: Utc::now().to_rfc3339(),
                })
                .await
                .map_err(repository_error)?;
            Ok(update_info_response(
                current_version,
                &latest,
                false,
                force,
                None,
            ))
        }
        Err(error) => {
            if let Some(cached) = stored {
                return Ok(update_info_response(
                    current_version,
                    &cached,
                    true,
                    force,
                    Some(format!("Using cached data: {error}")),
                ));
            }
            Ok(json!({
                "current_version": current_version,
                "latest_version": current_version,
                "has_update": false,
                "release_info": Value::Null,
                "release_notes": "",
                "cached": false,
                "warning": error,
                "build_type": default_build_type(),
                "force": force
            }))
        }
    }
}

pub async fn perform_update(
    repository: &dyn AppRepository,
    headers: &HeaderMap,
    actor_scope: &str,
    path: &str,
) -> Result<Value, ApiError> {
    let operation = match begin_operation(repository, headers, actor_scope, path, "update").await? {
        OperationStart::Existing(result) => return Ok(result),
        OperationStart::Started(operation) => operation,
    };

    let update_info = check_updates(repository, true).await?;
    if !update_info
        .get("has_update")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        let result = json!({
            "success": true,
            "message": "Already up to date",
            "already_up_to_date": true,
            "need_restart": false,
            "operation_id": operation.operation_id,
            "status": "completed",
            "current_version": update_info["current_version"],
            "latest_version": update_info["latest_version"]
        });
        finish_operation(repository, &operation, "completed", result.clone(), None).await?;
        return Ok(result);
    }

    let executor = crate::system_update_executor::SystemUpdateExecutor::current_process()?;
    let mut result = match executor.perform_update(&update_info).await {
        Ok(result) => result,
        Err(error) => {
            let message = error.message().to_owned();
            let result = json!({
                "success": false,
                "message": message,
                "need_restart": false,
                "operation_id": operation.operation_id,
                "status": "failed",
                "current_version": update_info["current_version"],
                "latest_version": update_info["latest_version"]
            });
            finish_operation(repository, &operation, "failed", result, Some(&message)).await?;
            return Err(ApiError::internal_server_error(format!(
                "{message}; operation_id={}",
                operation.operation_id
            )));
        }
    };
    merge_operation_update_fields(&mut result, &operation, &update_info);
    finish_operation(repository, &operation, "completed", result.clone(), None).await?;
    Ok(result)
}

pub async fn rollback(
    repository: &dyn AppRepository,
    headers: &HeaderMap,
    actor_scope: &str,
    path: &str,
) -> Result<Value, ApiError> {
    let operation =
        match begin_operation(repository, headers, actor_scope, path, "rollback").await? {
            OperationStart::Existing(result) => return Ok(result),
            OperationStart::Started(operation) => operation,
        };

    let executor = crate::system_update_executor::SystemUpdateExecutor::current_process()?;
    let mut result = match executor.rollback() {
        Ok(result) => result,
        Err(error) => {
            let message = error.message().to_owned();
            let result = json!({
                "success": false,
                "message": message,
                "need_restart": false,
                "operation_id": operation.operation_id,
                "status": "failed"
            });
            finish_operation(repository, &operation, "failed", result, Some(&message)).await?;
            return Err(ApiError::internal_server_error(format!(
                "{message}; operation_id={}",
                operation.operation_id
            )));
        }
    };
    if let Some(object) = result.as_object_mut() {
        object.insert("operation_id".to_owned(), json!(operation.operation_id));
    }
    finish_operation(repository, &operation, "completed", result.clone(), None).await?;
    Ok(result)
}

fn merge_operation_update_fields(
    result: &mut Value,
    operation: &SystemOperation,
    update_info: &Value,
) {
    if let Some(object) = result.as_object_mut() {
        object.insert("operation_id".to_owned(), json!(operation.operation_id));
        object.insert(
            "current_version".to_owned(),
            update_info
                .get("current_version")
                .cloned()
                .unwrap_or(Value::Null),
        );
        object.insert(
            "latest_version".to_owned(),
            update_info
                .get("latest_version")
                .cloned()
                .unwrap_or(Value::Null),
        );
    }
}

pub async fn restart(
    repository: &dyn AppRepository,
    headers: &HeaderMap,
    actor_scope: &str,
    path: &str,
) -> Result<Value, ApiError> {
    let operation = match begin_operation(repository, headers, actor_scope, path, "restart").await?
    {
        OperationStart::Existing(result) => return Ok(result),
        OperationStart::Started(operation) => operation,
    };

    let result = json!({
        "accepted": true,
        "message": "Service restart initiated",
        "need_restart": false,
        "operation_id": operation.operation_id,
        "status": "accepted",
        "scheduled": true
    });
    finish_operation(repository, &operation, "accepted", result.clone(), None).await?;
    Ok(result)
}

async fn begin_operation(
    repository: &dyn AppRepository,
    headers: &HeaderMap,
    actor_scope: &str,
    path: &str,
    operation: &str,
) -> Result<OperationStart, ApiError> {
    let idempotency_key = idempotency_key(headers);
    let operation_id = operation_id(operation, actor_scope, path, idempotency_key.as_deref());

    if idempotency_key.is_some() {
        match repository
            .get_admin_collection_item(
                SYSTEM_OPERATIONS_COLLECTION,
                storage_id(&operation_id, "operation id")?,
            )
            .await
        {
            Ok(record) if operation_is_running(&record.item) => {
                return Err(system_operation_busy(&record.item));
            }
            Ok(record) => return existing_operation_result(record.item),
            Err(RepositoryError::NotFound { .. }) => {}
            Err(error) => return Err(repository_error(error)),
        }
    }

    if let Some(active) = active_running_operation(repository).await? {
        return Err(system_operation_busy(&active));
    }

    let now = Utc::now();
    let started_at = now.to_rfc3339();
    let locked_until = (now + Duration::seconds(SYSTEM_OPERATION_LEASE_SECONDS)).to_rfc3339();
    let record = json!({
        "operation_id": operation_id,
        "operation": operation,
        "status": "running",
        "triggered_by": actor_scope,
        "idempotency_key": idempotency_key.clone().unwrap_or_default(),
        "path": path,
        "started_at": started_at,
        "updated_at": started_at,
        "finished_at": Value::Null,
        "locked_until": locked_until,
        "message": "operation started",
        "result": Value::Null
    });
    repository
        .upsert_admin_collection_item(AdminCollectionItemRecord {
            collection: SYSTEM_OPERATIONS_COLLECTION.to_owned(),
            id: storage_id(&operation_id, "operation id")?,
            item: record,
        })
        .await
        .map_err(repository_error)?;

    Ok(OperationStart::Started(SystemOperation {
        operation: operation.to_owned(),
        operation_id,
        idempotency_key,
        triggered_by: actor_scope.to_owned(),
        path: path.to_owned(),
        started_at,
        locked_until,
    }))
}

async fn finish_operation(
    repository: &dyn AppRepository,
    operation: &SystemOperation,
    status: &str,
    result: Value,
    error_message: Option<&str>,
) -> Result<(), ApiError> {
    let now = Utc::now().to_rfc3339();
    let message = error_message
        .map(str::to_owned)
        .or_else(|| {
            result
                .get("message")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .unwrap_or_else(|| "operation finished".to_owned());
    repository
        .upsert_admin_collection_item(AdminCollectionItemRecord {
            collection: SYSTEM_OPERATIONS_COLLECTION.to_owned(),
            id: storage_id(&operation.operation_id, "operation id")?,
            item: json!({
                "operation_id": operation.operation_id,
                "operation": operation.operation,
                "status": status,
                "triggered_by": operation.triggered_by,
                "idempotency_key": operation.idempotency_key.clone().unwrap_or_default(),
                "path": operation.path,
                "started_at": operation.started_at,
                "updated_at": now,
                "finished_at": now,
                "locked_until": operation.locked_until,
                "message": message,
                "error_message": error_message.unwrap_or_default(),
                "result": result
            }),
        })
        .await
        .map_err(repository_error)?;
    Ok(())
}

async fn active_running_operation(
    repository: &dyn AppRepository,
) -> Result<Option<Value>, ApiError> {
    let items = repository
        .list_admin_collection_items(SYSTEM_OPERATIONS_COLLECTION)
        .await
        .map_err(repository_error)?;
    Ok(items
        .into_iter()
        .map(|record| record.item)
        .find(operation_is_running))
}

fn operation_is_running(item: &Value) -> bool {
    if item.get("status").and_then(Value::as_str) != Some("running") {
        return false;
    }
    let Some(locked_until) = item.get("locked_until").and_then(Value::as_str) else {
        return true;
    };
    parse_rfc3339_utc(locked_until)
        .map(|locked_until| locked_until > Utc::now())
        .unwrap_or(true)
}

fn existing_operation_result(item: Value) -> Result<OperationStart, ApiError> {
    if item.get("status").and_then(Value::as_str) == Some("failed") {
        let message = item
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("system operation failed");
        return Err(ApiError::internal_server_error(message));
    }

    let mut result = item.get("result").cloned().unwrap_or_else(|| {
        json!({
            "operation_id": item.get("operation_id").cloned().unwrap_or(Value::Null),
            "status": item.get("status").cloned().unwrap_or(Value::Null),
            "message": item.get("message").cloned().unwrap_or(Value::Null)
        })
    });
    if let Some(object) = result.as_object_mut() {
        object.insert("idempotent".to_owned(), json!(true));
    }
    Ok(OperationStart::Existing(result))
}

fn system_operation_busy(item: &Value) -> ApiError {
    let operation_id = item
        .get("operation_id")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    ApiError::conflict(format!(
        "another system operation is in progress: {operation_id}"
    ))
}

fn idempotency_key(headers: &HeaderMap) -> Option<String> {
    headers
        .get("idempotency-key")
        .or_else(|| headers.get("x-idempotency-key"))
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn default_build_type() -> &'static str {
    option_env!("SUB2API_BUILD_TYPE").unwrap_or("source")
}

fn operation_id(
    operation: &str,
    actor_scope: &str,
    path: &str,
    idempotency_key: Option<&str>,
) -> String {
    if let Some(key) = idempotency_key {
        let mut hasher = Sha256::new();
        hasher.update(operation.as_bytes());
        hasher.update(b"|");
        hasher.update(actor_scope.as_bytes());
        hasher.update(b"|");
        hasher.update(path.as_bytes());
        hasher.update(b"|");
        hasher.update(key.as_bytes());
        let digest = hasher.finalize();
        let hash = hex::encode(digest);
        return format!("sysop-{}", &hash[..24]);
    }
    format!("sysop-{operation}-{}", uuid::Uuid::new_v4().simple())
}

fn storage_id(value: &str, label: &str) -> Result<i64, ApiError> {
    let value = value.trim();
    if value.is_empty() {
        return Err(ApiError::bad_request(format!("{label} is required")));
    }
    let mut hasher = Sha256::new();
    hasher.update(value.as_bytes());
    let digest = hasher.finalize();
    let mut bytes = [0_u8; 8];
    bytes.copy_from_slice(&digest[..8]);
    Ok((i64::from_be_bytes(bytes) & 0x7fff_ffff_ffff_ffff) as i64)
}

fn parse_rfc3339_utc(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|value| value.with_timezone(&Utc))
}

fn repository_error(error: RepositoryError) -> ApiError {
    match error {
        RepositoryError::NotFound { entity, id } => {
            ApiError::not_found(format!("{entity} {id} not found"))
        }
        RepositoryError::InvalidInput(message) => ApiError::bad_request(message),
        RepositoryError::Conflict(message) => ApiError::conflict(message),
        RepositoryError::Duplicate { entity, key } => {
            ApiError::conflict(format!("duplicate {entity}: {key}"))
        }
        RepositoryError::Database(message) => {
            ApiError::internal_server_error(format!("repository error: {message}"))
        }
    }
}
