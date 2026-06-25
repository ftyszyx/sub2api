use crate::gateway_scheduler::AccountRuntimeSnapshot;
use crate::response::ApiError;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{
    atomic::{AtomicI64, Ordering},
    RwLock,
};

const NOW: &str = "2026-06-06T00:00:00Z";

#[derive(Default)]
pub struct AdminOpsService {
    next_id: AtomicI64,
    alert_rules: RwLock<Vec<Value>>,
    alert_events: RwLock<Vec<Value>>,
    alert_silences: RwLock<Vec<Value>>,
    request_errors: RwLock<Vec<Value>>,
    upstream_errors: RwLock<Vec<Value>>,
    request_details: RwLock<Vec<Value>>,
    system_logs: RwLock<Vec<Value>>,
    email_notification_config: RwLock<Value>,
    alert_runtime_settings: RwLock<Value>,
    runtime_log_config: RwLock<Value>,
    advanced_settings: RwLock<Value>,
    metric_thresholds: RwLock<Value>,
}

impl AdminOpsService {
    pub fn new() -> Self {
        Self {
            next_id: AtomicI64::new(100),
            alert_rules: RwLock::new(Vec::new()),
            alert_events: RwLock::new(Vec::new()),
            alert_silences: RwLock::new(Vec::new()),
            request_errors: RwLock::new(Vec::new()),
            upstream_errors: RwLock::new(Vec::new()),
            request_details: RwLock::new(Vec::new()),
            system_logs: RwLock::new(Vec::new()),
            email_notification_config: RwLock::new(default_email_notification_config()),
            alert_runtime_settings: RwLock::new(default_alert_runtime_settings()),
            runtime_log_config: RwLock::new(default_runtime_log_config()),
            advanced_settings: RwLock::new(default_advanced_settings()),
            metric_thresholds: RwLock::new(default_metric_thresholds()),
        }
    }

    pub fn demo() -> Self {
        let service = Self::new();
        service
            .alert_events
            .write()
            .expect("ops alert events lock")
            .extend(seed_alert_events());
        service
            .request_errors
            .write()
            .expect("ops request errors lock")
            .extend(seed_request_errors());
        service
            .upstream_errors
            .write()
            .expect("ops upstream errors lock")
            .extend(seed_upstream_errors());
        service
            .request_details
            .write()
            .expect("ops request details lock")
            .extend(seed_request_details());
        service
            .system_logs
            .write()
            .expect("ops system logs lock")
            .extend(seed_system_logs());
        service
    }

    pub fn get(&self, full_path: &str, query: &HashMap<String, String>) -> Result<Value, ApiError> {
        self.get_with_runtime(full_path, query, &HashMap::new())
    }

    pub(crate) fn get_with_runtime(
        &self,
        full_path: &str,
        query: &HashMap<String, String>,
        runtime_snapshots: &HashMap<i64, AccountRuntimeSnapshot>,
    ) -> Result<Value, ApiError> {
        let path = ops_path(full_path);
        if path == "concurrency" {
            return Ok(self.concurrency_stats(query, runtime_snapshots));
        }
        if path == "user-concurrency" {
            return Ok(self.user_concurrency_stats(query));
        }
        if path == "account-availability" {
            return Ok(self.account_availability(query, runtime_snapshots));
        }
        if path == "realtime-traffic" {
            return Ok(self.realtime_traffic(query));
        }
        if path == "dashboard/overview" {
            return Ok(self.dashboard_overview(query));
        }
        if path == "dashboard/snapshot-v2" {
            return Ok(json!({
                "generated_at": NOW,
                "overview": self.dashboard_overview(query),
                "throughput_trend": self.throughput_trend(),
                "error_trend": self.error_trend()
            }));
        }
        if path == "dashboard/throughput-trend" {
            return Ok(self.throughput_trend());
        }
        if path == "dashboard/latency-histogram" {
            let request_details = self.request_details.read().expect("ops requests lock");
            return Ok(json!({
                "start_time": NOW,
                "end_time": NOW,
                "platform": query.get("platform").cloned().unwrap_or_else(|| "all".to_owned()),
                "group_id": query.get("group_id").and_then(|value| value.parse::<i64>().ok()),
                "total_requests": request_details.len(),
                "buckets": latency_histogram(&request_details),
                "duration": duration_percentiles(&request_details)
            }));
        }
        if path == "dashboard/error-trend" {
            return Ok(self.error_trend());
        }
        if path == "dashboard/error-distribution" {
            let request_errors = self.request_errors.read().expect("ops request errors lock");
            let upstream_errors = self
                .upstream_errors
                .read()
                .expect("ops upstream errors lock");
            return Ok(error_distribution(
                &request_errors
                    .iter()
                    .chain(upstream_errors.iter())
                    .filter(|item| matches_ops_scope(item, query))
                    .cloned()
                    .collect::<Vec<_>>(),
            ));
        }
        if path == "dashboard/openai-token-stats" {
            return Ok(self.openai_token_stats(query));
        }

        match path.as_str() {
            "alert-rules" => Ok(json!(self
                .alert_rules
                .read()
                .expect("ops alert rules lock")
                .clone())),
            "alert-events" => Ok(json!(self.filtered_alert_events(query))),
            "email-notification/config" => Ok(self
                .email_notification_config
                .read()
                .expect("ops email lock")
                .clone()),
            "runtime/alert" => Ok(self
                .alert_runtime_settings
                .read()
                .expect("ops alert runtime lock")
                .clone()),
            "runtime/logging" | "runtime/logging/reset" => Ok(self
                .runtime_log_config
                .read()
                .expect("ops runtime log lock")
                .clone()),
            "advanced-settings" => Ok(self
                .advanced_settings
                .read()
                .expect("ops advanced settings lock")
                .clone()),
            "settings/metric-thresholds" => Ok(self
                .metric_thresholds
                .read()
                .expect("ops metric thresholds lock")
                .clone()),
            "system-logs" => Ok(paginated(
                self.system_logs
                    .read()
                    .expect("ops system logs lock")
                    .clone(),
                query,
            )),
            "system-logs/health" => Ok(json!({
                "queue_depth": 0,
                "queue_capacity": 1000,
                "dropped_count": 0,
                "write_failed_count": 0,
                "written_count": self.system_logs.read().expect("ops system logs lock").len(),
                "avg_write_delay_ms": 0,
                "last_error": null,
                "healthy": true,
                "status": "ok"
            })),
            "errors" | "request-errors" => Ok(paginated(
                filter_errors(
                    self.request_errors
                        .read()
                        .expect("ops request errors lock")
                        .clone(),
                    query,
                ),
                query,
            )),
            "upstream-errors" => Ok(paginated(
                filter_errors(
                    self.upstream_errors
                        .read()
                        .expect("ops upstream errors lock")
                        .clone(),
                    query,
                ),
                query,
            )),
            "requests" => Ok(paginated(
                self.request_details
                    .read()
                    .expect("ops request details lock")
                    .clone(),
                query,
            )),
            _ => self.get_detail(&path, query),
        }
    }

    pub fn post(&self, full_path: &str, payload: Value) -> Result<Value, ApiError> {
        let path = ops_path(full_path);
        match path.as_str() {
            "alert-rules" => Ok(self.create_alert_rule(payload)),
            "alert-silences" => Ok(self.create_alert_silence(payload)),
            "runtime/logging/reset" => Ok(self.reset_runtime_logging()),
            "system-logs/cleanup" => Ok(self.cleanup_system_logs()),
            _ => Ok(json!({ "message": "ok", "path": full_path })),
        }
    }

    pub fn put(&self, full_path: &str, payload: Value) -> Result<Value, ApiError> {
        let path = ops_path(full_path);
        match path.as_str() {
            "email-notification/config" => {
                self.update_setting(&self.email_notification_config, payload)
            }
            "runtime/alert" => self.update_setting(&self.alert_runtime_settings, payload),
            "runtime/logging" => self.update_runtime_logging(payload),
            "advanced-settings" => self.update_setting(&self.advanced_settings, payload),
            "settings/metric-thresholds" => self.update_setting(&self.metric_thresholds, payload),
            _ => self.put_detail(&path, payload),
        }
    }

    pub fn delete(&self, full_path: &str) -> Result<Value, ApiError> {
        let path = ops_path(full_path);
        if let Some(id) = id_after(&path, "alert-rules") {
            let mut rules = self.alert_rules.write().expect("ops alert rules lock");
            let before = rules.len();
            rules.retain(|rule| item_id(rule) != Some(id));
            return Ok(json!({ "deleted": before != rules.len() }));
        }
        Ok(json!({ "message": "ok", "path": full_path }))
    }

    pub fn record_gateway_request(&self, event: GatewayOpsEvent) {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let request = json!({
            "id": id,
            "request_id": event.request_id,
            "client_request_id": event.client_request_id,
            "kind": if event.status_code >= 400 { "error" } else { "success" },
            "platform": event.platform,
            "provider": event.platform,
            "group_id": event.group_id,
            "group_name": event.group_name,
            "account_id": event.account_id,
            "account_name": event.account_name,
            "api_key_id": event.api_key_id,
            "user_id": event.user_id,
            "model": event.model,
            "upstream_model": event.upstream_model,
            "method": event.method,
            "path": event.path,
            "endpoint": event.endpoint,
            "downstream_protocol": event.downstream_protocol,
            "upstream_protocol": event.upstream_protocol,
            "stream": event.stream,
            "status_code": event.status_code,
            "duration_ms": event.duration_ms,
            "input_tokens": event.input_tokens,
            "output_tokens": event.output_tokens,
            "cache_creation_tokens": event.cache_creation_tokens,
            "cache_read_tokens": event.cache_read_tokens,
            "total_tokens": event.total_tokens,
            "created_at": NOW
        });
        self.request_details
            .write()
            .expect("ops request details lock")
            .push(request);

        if event.status_code >= 400 {
            let error = json!({
                "id": self.next_id.fetch_add(1, Ordering::SeqCst),
                "request_id": event.request_id,
                "client_request_id": event.client_request_id,
                "phase": if event.account_id.is_some() { "upstream" } else { "request" },
                "error_owner": if event.account_id.is_some() { "provider" } else { "gateway" },
                "error_source": if event.account_id.is_some() { "upstream" } else { "gateway" },
                "platform": event.platform,
                "provider": event.platform,
                "group_id": event.group_id,
                "group_name": event.group_name,
                "account_id": event.account_id,
                "account_name": event.account_name,
                "api_key_id": event.api_key_id,
                "user_id": event.user_id,
                "status_code": event.status_code,
                "method": event.method,
                "path": event.path,
                "endpoint": event.endpoint,
                "model": event.model,
                "upstream_model": event.upstream_model,
                "message": event.message,
                "upstream_response": event.upstream_response,
                "resolved": false,
                "resolved_at": null,
                "resolution_note": null,
                "created_at": NOW
            });
            if event.account_id.is_some() {
                self.upstream_errors
                    .write()
                    .expect("ops upstream errors lock")
                    .push(error);
            } else {
                self.request_errors
                    .write()
                    .expect("ops request errors lock")
                    .push(error);
            }
        }

        self.system_logs
            .write()
            .expect("ops system logs lock")
            .push(json!({
                "id": self.next_id.fetch_add(1, Ordering::SeqCst),
                "level": if event.status_code >= 500 { "error" } else if event.status_code >= 400 { "warn" } else { "info" },
                "component": "gateway",
                "message": event.message,
                "request_id": event.request_id,
                "client_request_id": event.client_request_id,
                "platform": event.platform,
                "model": event.model,
                "status_code": event.status_code,
                "created_at": NOW
            }));

        self.evaluate_alert_rules();
    }

    fn get_detail(&self, path: &str, query: &HashMap<String, String>) -> Result<Value, ApiError> {
        if let Some(id) = id_after(path, "alert-rules") {
            return find_by_id(
                &self.alert_rules.read().expect("ops alert rules lock"),
                id,
                "alert rule",
            );
        }
        if let Some(id) = id_after(path, "alert-events") {
            return find_by_id(
                &self.alert_events.read().expect("ops alert events lock"),
                id,
                "alert event",
            );
        }
        if let Some(id) = id_after(path, "errors").or_else(|| id_after(path, "request-errors")) {
            if path.ends_with("/upstream-errors") {
                let request_error = find_by_id(
                    &self.request_errors.read().expect("ops request errors lock"),
                    id,
                    "request error",
                )?;
                let request_id = request_error
                    .get("request_id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                let linked = self
                    .upstream_errors
                    .read()
                    .expect("ops upstream errors lock")
                    .iter()
                    .filter(|item| {
                        item.get("request_id")
                            .and_then(Value::as_str)
                            .map(|value| value == request_id)
                            .unwrap_or(false)
                    })
                    .cloned()
                    .collect::<Vec<_>>();
                return Ok(paginated(linked, query));
            }
            return find_by_id(
                &self.request_errors.read().expect("ops request errors lock"),
                id,
                "request error",
            );
        }
        if let Some(id) = id_after(path, "upstream-errors") {
            return find_by_id(
                &self
                    .upstream_errors
                    .read()
                    .expect("ops upstream errors lock"),
                id,
                "upstream error",
            );
        }
        Ok(paginated(Vec::new(), query))
    }

    fn put_detail(&self, path: &str, payload: Value) -> Result<Value, ApiError> {
        if let Some(id) = id_after(path, "alert-rules") {
            return self.update_alert_rule(id, payload);
        }
        if let Some(id) = id_after(path, "alert-events") {
            if path.ends_with("/status") {
                return self.update_alert_event_status(id, payload);
            }
        }
        if let Some(id) = id_after(path, "errors").or_else(|| id_after(path, "request-errors")) {
            if path.ends_with("/resolve") {
                return self.resolve_error(id, payload, ErrorCollection::Request);
            }
        }
        if let Some(id) = id_after(path, "upstream-errors") {
            if path.ends_with("/resolve") {
                return self.resolve_error(id, payload, ErrorCollection::Upstream);
            }
        }
        Ok(json!({ "message": "ok", "path": path }))
    }

    fn create_alert_rule(&self, payload: Value) -> Value {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let mut rule = default_alert_rule(id);
        merge_json(&mut rule, payload);
        set_id(&mut rule, id);
        rule["created_at"] = json!(NOW);
        rule["updated_at"] = json!(NOW);
        self.alert_rules
            .write()
            .expect("ops alert rules lock")
            .push(rule.clone());
        rule
    }

    fn update_alert_rule(&self, id: i64, payload: Value) -> Result<Value, ApiError> {
        let mut rules = self.alert_rules.write().expect("ops alert rules lock");
        let rule = rules
            .iter_mut()
            .find(|rule| item_id(rule) == Some(id))
            .ok_or_else(|| ApiError::not_found("alert rule not found"))?;
        merge_json(rule, payload);
        set_id(rule, id);
        rule["updated_at"] = json!(NOW);
        Ok(rule.clone())
    }

    fn update_alert_event_status(&self, id: i64, payload: Value) -> Result<Value, ApiError> {
        let status = payload
            .get("status")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| ApiError::bad_request("status is required"))?
            .to_owned();
        let mut events = self.alert_events.write().expect("ops alert events lock");
        let event = events
            .iter_mut()
            .find(|event| item_id(event) == Some(id))
            .ok_or_else(|| ApiError::not_found("alert event not found"))?;
        event["status"] = json!(status);
        if event["status"] == "resolved" || event["status"] == "acknowledged" {
            event["resolved_at"] = json!(NOW);
        }
        event["updated_at"] = json!(NOW);
        Ok(event.clone())
    }

    fn create_alert_silence(&self, payload: Value) -> Value {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let mut silence = json!({
            "id": id,
            "reason": "",
            "scope": {},
            "starts_at": NOW,
            "ends_at": null,
            "created_at": NOW
        });
        merge_json(&mut silence, payload);
        set_id(&mut silence, id);
        self.alert_silences
            .write()
            .expect("ops alert silences lock")
            .push(silence.clone());
        self.sync_silences_into_runtime_settings();
        silence
    }

    fn update_setting(&self, lock: &RwLock<Value>, payload: Value) -> Result<Value, ApiError> {
        let mut value = lock.write().expect("ops setting lock");
        merge_json(&mut value, payload);
        Ok(value.clone())
    }

    fn update_runtime_logging(&self, payload: Value) -> Result<Value, ApiError> {
        let mut config = self
            .runtime_log_config
            .write()
            .expect("ops runtime log lock");
        merge_json(&mut config, payload);
        config["source"] = json!("runtime");
        config["updated_at"] = json!(NOW);
        Ok(config.clone())
    }

    fn reset_runtime_logging(&self) -> Value {
        let mut config = self
            .runtime_log_config
            .write()
            .expect("ops runtime log lock");
        *config = default_runtime_log_config();
        config.clone()
    }

    fn resolve_error(
        &self,
        id: i64,
        payload: Value,
        collection: ErrorCollection,
    ) -> Result<Value, ApiError> {
        let resolved = payload
            .get("resolved")
            .and_then(Value::as_bool)
            .unwrap_or(true);
        let note = payload
            .get("resolution_note")
            .or_else(|| payload.get("note"))
            .cloned()
            .unwrap_or(Value::Null);

        let lock = match collection {
            ErrorCollection::Request => &self.request_errors,
            ErrorCollection::Upstream => &self.upstream_errors,
        };
        let mut errors = lock.write().expect("ops error lock");
        let item = errors
            .iter_mut()
            .find(|item| item_id(item) == Some(id))
            .ok_or_else(|| ApiError::not_found("error log not found"))?;
        item["resolved"] = json!(resolved);
        item["resolved_at"] = if resolved { json!(NOW) } else { Value::Null };
        item["resolution_note"] = note;
        Ok(item.clone())
    }

    fn cleanup_system_logs(&self) -> Value {
        let mut logs = self.system_logs.write().expect("ops system logs lock");
        let deleted = logs.len();
        logs.clear();
        json!({ "deleted": deleted })
    }

    fn sync_silences_into_runtime_settings(&self) {
        let entries = self
            .alert_silences
            .read()
            .expect("ops alert silences lock")
            .clone();
        let mut runtime = self
            .alert_runtime_settings
            .write()
            .expect("ops alert runtime lock");
        runtime["silencing"]["entries"] = json!(entries);
    }

    fn evaluate_alert_rules(&self) {
        if runtime_alerts_disabled(
            &self
                .alert_runtime_settings
                .read()
                .expect("ops alert runtime lock"),
        ) {
            return;
        }

        let request_details = self.request_details.read().expect("ops requests lock");
        let upstream_errors = self
            .upstream_errors
            .read()
            .expect("ops upstream errors lock");
        let mut rules = self.alert_rules.write().expect("ops alert rules lock");
        let mut new_events = Vec::new();
        for rule in rules.iter_mut() {
            if !rule
                .get("enabled")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                continue;
            }
            if !alert_rule_cooldown_elapsed(rule) {
                continue;
            }
            let Some(evaluation) = evaluate_alert_rule(rule, &request_details, &upstream_errors)
            else {
                continue;
            };
            if !evaluation.triggered {
                continue;
            }
            let id = self.next_id.fetch_add(1, Ordering::SeqCst);
            rule["last_triggered_at"] = json!(NOW);
            rule["updated_at"] = json!(NOW);
            new_events.push(alert_event_from_rule(id, rule, &evaluation));
        }
        drop(rules);

        if !new_events.is_empty() {
            self.alert_events
                .write()
                .expect("ops alert events lock")
                .extend(new_events);
        }
    }

    fn filtered_alert_events(&self, query: &HashMap<String, String>) -> Vec<Value> {
        let mut events = self
            .alert_events
            .read()
            .expect("ops alert events lock")
            .iter()
            .filter(|event| matches_alert_event_query(event, query))
            .cloned()
            .collect::<Vec<_>>();
        events.sort_by(|left, right| {
            value_string(right, "fired_at")
                .cmp(&value_string(left, "fired_at"))
                .then_with(|| value_i64(right, "id").cmp(&value_i64(left, "id")))
        });
        let limit = query_i64(query, "limit", 20).clamp(1, 500) as usize;
        events.truncate(limit);
        events
    }

    fn concurrency_stats(
        &self,
        query: &HashMap<String, String>,
        runtime_snapshots: &HashMap<i64, AccountRuntimeSnapshot>,
    ) -> Value {
        let request_details = self.filtered_request_details(query);
        let mut platform: HashMap<String, Value> = HashMap::new();
        let mut group: HashMap<String, Value> = HashMap::new();
        let mut account: HashMap<String, Value> = HashMap::new();

        let mut latest_items = latest_account_items_map(&request_details);
        for snapshot in runtime_snapshots.values() {
            if !latest_items.contains_key(&snapshot.account_id) {
                latest_items.insert(snapshot.account_id, runtime_snapshot_item(snapshot));
            }
        }

        for (account_id, item) in latest_items {
            if !matches_ops_scope(&item, query) {
                continue;
            }
            let platform_name =
                value_string(&item, "platform").unwrap_or_else(|| "unknown".to_owned());
            let group_id = value_i64(&item, "group_id").unwrap_or(0);
            let group_name = display_group_name(&item, group_id);
            let runtime = runtime_snapshots.get(&account_id);
            let current_in_use = runtime
                .map(|snapshot| snapshot.in_flight as i64)
                .unwrap_or(0);
            let max_capacity = runtime
                .and_then(|snapshot| snapshot.max_concurrent)
                .map(|value| value.max(1) as i64)
                .or_else(|| value_i64(&item, "max_capacity"))
                .unwrap_or(1);
            let load_percentage = load_percentage(current_in_use, max_capacity);

            account.insert(
                account_id.to_string(),
                json!({
                    "account_id": account_id,
                    "account_name": display_account_name(&item, account_id),
                    "platform": platform_name,
                    "group_id": group_id,
                    "group_name": group_name,
                    "current_in_use": current_in_use,
                    "max_capacity": max_capacity,
                    "load_percentage": load_percentage,
                    "waiting_in_queue": 0
                }),
            );

            let platform_entry = platform.entry(platform_name.clone()).or_insert_with(|| {
                json!({
                    "platform": platform_name,
                    "current_in_use": 0,
                    "max_capacity": 0,
                    "load_percentage": 0,
                    "waiting_in_queue": 0
                })
            });
            increment_json_i64(platform_entry, "current_in_use", current_in_use);
            increment_json_i64(platform_entry, "max_capacity", max_capacity);
            refresh_load_percentage(platform_entry);

            if group_id > 0 {
                let group_entry = group.entry(group_id.to_string()).or_insert_with(|| {
                    json!({
                        "group_id": group_id,
                        "group_name": group_name,
                        "platform": value_string(&item, "platform").unwrap_or_else(|| "unknown".to_owned()),
                        "current_in_use": 0,
                        "max_capacity": 0,
                        "load_percentage": 0,
                        "waiting_in_queue": 0
                    })
                });
                increment_json_i64(group_entry, "current_in_use", current_in_use);
                increment_json_i64(group_entry, "max_capacity", max_capacity);
                refresh_load_percentage(group_entry);
            }
        }

        json!({
            "enabled": true,
            "platform": platform,
            "group": group,
            "account": account,
            "timestamp": NOW
        })
    }

    fn user_concurrency_stats(&self, query: &HashMap<String, String>) -> Value {
        let request_details = self.filtered_request_details(query);
        let mut user: HashMap<String, Value> = HashMap::new();
        for item in request_details {
            let Some(user_id) = value_i64(&item, "user_id") else {
                continue;
            };
            user.entry(user_id.to_string()).or_insert_with(|| {
                json!({
                    "user_id": user_id,
                    "user_email": value_string(&item, "user_email").unwrap_or_default(),
                    "username": value_string(&item, "username").unwrap_or_else(|| format!("user-{user_id}")),
                    "current_in_use": 0,
                    "max_capacity": 1,
                    "load_percentage": 0,
                    "waiting_in_queue": 0
                })
            });
        }
        json!({ "enabled": true, "user": user, "timestamp": NOW })
    }

    fn account_availability(
        &self,
        query: &HashMap<String, String>,
        runtime_snapshots: &HashMap<i64, AccountRuntimeSnapshot>,
    ) -> Value {
        let request_details = self.filtered_request_details(query);
        let mut platform: HashMap<String, Value> = HashMap::new();
        let mut group: HashMap<String, Value> = HashMap::new();
        let mut account: HashMap<String, Value> = HashMap::new();

        let mut latest_items = latest_account_items_map(&request_details);
        for snapshot in runtime_snapshots.values() {
            if !latest_items.contains_key(&snapshot.account_id) {
                latest_items.insert(snapshot.account_id, runtime_snapshot_item(snapshot));
            }
        }

        for (account_id, item) in latest_items {
            if !matches_ops_scope(&item, query) {
                continue;
            }
            let status_code = value_i64(&item, "status_code").unwrap_or(0);
            let platform_name =
                value_string(&item, "platform").unwrap_or_else(|| "unknown".to_owned());
            let group_id = value_i64(&item, "group_id").unwrap_or(0);
            let group_name = display_group_name(&item, group_id);
            let runtime = runtime_snapshots.get(&account_id);
            let is_cooling_down = runtime
                .and_then(|snapshot| snapshot.retry_after_seconds)
                .is_some_and(|seconds| seconds > 0);
            let is_rate_limited = matches!(status_code, 429 | 529);
            let has_error = is_cooling_down || (status_code >= 500 && !is_rate_limited);
            let is_available = !is_cooling_down && (200..400).contains(&status_code);
            let overload_remaining_sec = runtime.and_then(|snapshot| {
                snapshot
                    .retry_after_seconds
                    .filter(|seconds| *seconds > 0)
                    .map(|seconds| seconds as i64)
            });
            let overload_until = runtime.and_then(|snapshot| snapshot.cooling_until_unix);
            let error_message = if has_error {
                runtime
                    .and_then(|snapshot| snapshot.last_error.clone())
                    .or_else(|| value_string(&item, "message"))
                    .unwrap_or_else(|| format!("HTTP {status_code}"))
            } else {
                String::new()
            };
            let status = if is_available {
                "active"
            } else if is_rate_limited {
                "rate_limited"
            } else if is_cooling_down {
                "overloaded"
            } else if has_error {
                "error"
            } else {
                "unavailable"
            };

            account.insert(
                account_id.to_string(),
                json!({
                    "account_id": account_id,
                    "account_name": display_account_name(&item, account_id),
                    "platform": platform_name,
                    "group_id": group_id,
                    "group_name": group_name,
                    "status": status,
                    "is_available": is_available,
                    "is_rate_limited": is_rate_limited,
                    "rate_limit_reset_at": null,
                    "rate_limit_remaining_sec": null,
                    "is_overloaded": is_cooling_down,
                    "overload_until": overload_until,
                    "overload_remaining_sec": overload_remaining_sec,
                    "has_error": has_error,
                    "error_message": error_message
                }),
            );

            let platform_entry = platform.entry(platform_name.clone()).or_insert_with(|| {
                json!({
                    "platform": platform_name,
                    "total_accounts": 0,
                    "available_count": 0,
                    "rate_limit_count": 0,
                    "error_count": 0
                })
            });
            increment_json_i64(platform_entry, "total_accounts", 1);
            if is_available {
                increment_json_i64(platform_entry, "available_count", 1);
            }
            if is_rate_limited {
                increment_json_i64(platform_entry, "rate_limit_count", 1);
            }
            if has_error {
                increment_json_i64(platform_entry, "error_count", 1);
            }

            if group_id > 0 {
                let group_entry = group.entry(group_id.to_string()).or_insert_with(|| {
                    json!({
                        "group_id": group_id,
                        "group_name": group_name,
                        "platform": value_string(&item, "platform").unwrap_or_else(|| "unknown".to_owned()),
                        "total_accounts": 0,
                        "available_count": 0,
                        "rate_limit_count": 0,
                        "error_count": 0
                    })
                });
                increment_json_i64(group_entry, "total_accounts", 1);
                if is_available {
                    increment_json_i64(group_entry, "available_count", 1);
                }
                if is_rate_limited {
                    increment_json_i64(group_entry, "rate_limit_count", 1);
                }
                if has_error {
                    increment_json_i64(group_entry, "error_count", 1);
                }
            }
        }

        json!({
            "enabled": true,
            "platform": platform,
            "group": group,
            "account": account,
            "timestamp": NOW
        })
    }

    fn realtime_traffic(&self, query: &HashMap<String, String>) -> Value {
        let (window, window_seconds) = realtime_window(query);
        let request_details = self.filtered_request_details(query);
        let request_count = request_details.len() as i64;
        let total_tokens = sum_i64(&request_details, "total_tokens");
        json!({
            "enabled": true,
            "summary": {
                "window": window,
                "start_time": NOW,
                "end_time": NOW,
                "platform": query.get("platform").cloned().unwrap_or_else(|| "all".to_owned()),
                "group_id": query.get("group_id").and_then(|value| value.parse::<i64>().ok()),
                "qps": observed_window_rate_summary(request_count, window_seconds),
                "tps": observed_window_rate_summary(total_tokens, window_seconds)
            },
            "timestamp": NOW
        })
    }

    fn openai_token_stats(&self, query: &HashMap<String, String>) -> Value {
        let mut aggregates: HashMap<String, TokenStatsAggregate> = HashMap::new();
        for item in self.filtered_request_details(query) {
            let status_code = value_i64(&item, "status_code").unwrap_or(0);
            if !(200..400).contains(&status_code) {
                continue;
            }
            let Some(model) = value_string(&item, "model")
                .or_else(|| value_string(&item, "upstream_model"))
                .filter(|value| !value.trim().is_empty())
            else {
                continue;
            };
            let duration_ms = value_i64(&item, "duration_ms").unwrap_or(0);
            let output_tokens = value_i64(&item, "output_tokens").unwrap_or(0);
            let entry = aggregates.entry(model).or_default();
            entry.request_count += 1;
            entry.total_output_tokens += output_tokens;
            entry.total_duration_ms += duration_ms.max(0);
            if output_tokens > 0 && duration_ms > 0 {
                entry.tokens_per_second_sum += output_tokens as f64 / (duration_ms as f64 / 1000.0);
                entry.tokens_per_second_count += 1;
            }
            if value_i64(&item, "time_to_first_token_ms").unwrap_or(0) > 0 {
                entry.requests_with_first_token += 1;
                entry.first_token_ms_sum += value_i64(&item, "time_to_first_token_ms").unwrap_or(0);
            }
        }

        let mut rows = aggregates
            .into_iter()
            .map(|(model, aggregate)| aggregate.into_json(model))
            .collect::<Vec<_>>();
        rows.sort_by(|left, right| {
            right
                .get("total_output_tokens")
                .and_then(Value::as_i64)
                .cmp(&left.get("total_output_tokens").and_then(Value::as_i64))
                .then_with(|| {
                    right
                        .get("request_count")
                        .and_then(Value::as_i64)
                        .cmp(&left.get("request_count").and_then(Value::as_i64))
                })
                .then_with(|| {
                    left.get("model")
                        .and_then(Value::as_str)
                        .cmp(&right.get("model").and_then(Value::as_str))
                })
        });

        let total = rows.len() as i64;
        let top_n = query
            .get("top_n")
            .and_then(|value| value.parse::<i64>().ok());
        let page = query_i64(query, "page", 1).max(1);
        let page_size = query_i64(query, "page_size", 20).clamp(1, 100);
        let items: Vec<Value> = if let Some(top_n) = top_n.filter(|value| *value > 0) {
            rows.into_iter().take(top_n.min(100) as usize).collect()
        } else {
            let start = ((page - 1) * page_size) as usize;
            rows.into_iter()
                .skip(start)
                .take(page_size as usize)
                .collect()
        };

        json!({
            "time_range": query.get("time_range").cloned().unwrap_or_else(|| "1h".to_owned()),
            "start_time": NOW,
            "end_time": NOW,
            "platform": query.get("platform").cloned(),
            "group_id": query.get("group_id").and_then(|value| value.parse::<i64>().ok()),
            "items": items,
            "total": total,
            "page": if top_n.is_some() { Value::Null } else { json!(page) },
            "page_size": if top_n.is_some() { Value::Null } else { json!(page_size) },
            "top_n": top_n
        })
    }

    fn filtered_request_details(&self, query: &HashMap<String, String>) -> Vec<Value> {
        let request_details = self.request_details.read().expect("ops requests lock");
        request_details
            .iter()
            .filter(|item| matches_ops_scope(item, query))
            .cloned()
            .collect()
    }

    fn dashboard_overview(&self, query: &HashMap<String, String>) -> Value {
        let request_details = self.filtered_request_details(query);
        let request_errors = self
            .request_errors
            .read()
            .expect("ops request errors lock")
            .iter()
            .filter(|item| matches_ops_scope(item, query))
            .cloned()
            .collect::<Vec<_>>();
        let upstream_errors = self
            .upstream_errors
            .read()
            .expect("ops upstream errors lock")
            .iter()
            .filter(|item| matches_ops_scope(item, query))
            .cloned()
            .collect::<Vec<_>>();
        let total_requests = request_details.len() as i64;
        let total_errors = request_errors.len() as i64 + upstream_errors.len() as i64;
        let upstream_429_count = count_status(&upstream_errors, 429);
        let upstream_529_count = count_status(&upstream_errors, 529);
        let upstream_sla_errors = upstream_errors
            .iter()
            .filter(|item| {
                !matches!(
                    item.get("status_code").and_then(Value::as_i64),
                    Some(429 | 529)
                )
            })
            .count() as i64;
        let request_count_sla = total_requests.max(1);
        let error_count_sla = (request_errors.len() as i64) + upstream_sla_errors;
        let success_count = (total_requests - error_count_sla).max(0);
        let error_rate = ratio(error_count_sla, request_count_sla);
        let upstream_error_rate = ratio(upstream_sla_errors, request_count_sla);
        json!({
            "start_time": NOW,
            "end_time": NOW,
            "platform": query.get("platform").cloned().unwrap_or_else(|| "all".to_owned()),
            "group_id": query.get("group_id").and_then(|value| value.parse::<i64>().ok()),
            "health_score": health_score(error_rate, upstream_error_rate),
            "system_metrics": {
                "uptime_seconds": 0,
                "memory_rss_bytes": 0,
                "cpu_percent": 0
            },
            "job_heartbeats": [],
            "success_count": success_count,
            "error_count_total": total_errors,
            "business_limited_count": upstream_429_count + upstream_529_count,
            "error_count_sla": error_count_sla,
            "request_count_total": total_requests,
            "request_count_sla": request_count_sla,
            "token_consumed": sum_i64(&request_details, "total_tokens"),
            "sla": (1.0 - error_rate).clamp(0.0, 1.0),
            "error_rate": error_rate,
            "upstream_error_rate": upstream_error_rate,
            "upstream_error_count_excl_429_529": upstream_sla_errors,
            "upstream_429_count": upstream_429_count,
            "upstream_529_count": upstream_529_count,
            "qps": observed_rate_summary(total_requests),
            "tps": observed_rate_summary(sum_i64(&request_details, "total_tokens")),
            "duration": duration_percentiles(&request_details),
            "ttft": percentiles()
        })
    }

    fn throughput_trend(&self) -> Value {
        let request_details = self.request_details.read().expect("ops requests lock");
        json!({
            "bucket": "minute",
            "points": [{
                "time": NOW,
                "requests": request_details.len() as i64,
                "tokens": sum_i64(&request_details, "total_tokens")
            }],
            "by_platform": group_count(&request_details, "platform"),
            "top_groups": group_count(&request_details, "group_id")
        })
    }

    fn error_trend(&self) -> Value {
        let request_errors = self.request_errors.read().expect("ops request errors lock");
        let upstream_errors = self
            .upstream_errors
            .read()
            .expect("ops upstream errors lock");
        json!({
            "bucket": "minute",
            "points": [{
                "time": NOW,
                "request_errors": request_errors.len() as i64,
                "upstream_errors": upstream_errors.len() as i64,
                "total_errors": (request_errors.len() + upstream_errors.len()) as i64
            }]
        })
    }
}

#[derive(Debug, Clone)]
pub struct GatewayOpsEvent {
    pub request_id: String,
    pub client_request_id: Option<String>,
    pub user_id: Option<i64>,
    pub api_key_id: Option<i64>,
    pub group_id: Option<i64>,
    pub group_name: Option<String>,
    pub account_id: Option<i64>,
    pub account_name: Option<String>,
    pub platform: String,
    pub downstream_protocol: Option<String>,
    pub upstream_protocol: Option<String>,
    pub endpoint: String,
    pub method: String,
    pub path: String,
    pub model: Option<String>,
    pub upstream_model: Option<String>,
    pub stream: bool,
    pub status_code: u16,
    pub duration_ms: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_creation_tokens: i64,
    pub cache_read_tokens: i64,
    pub total_tokens: i64,
    pub message: String,
    pub upstream_response: Value,
}

#[derive(Clone, Copy)]
enum ErrorCollection {
    Request,
    Upstream,
}

fn ops_path(full_path: &str) -> String {
    full_path
        .trim_start_matches("/api/v1/admin/ops")
        .trim_start_matches('/')
        .trim_end_matches('/')
        .to_owned()
}

fn id_after(path: &str, prefix: &str) -> Option<i64> {
    let mut parts = path.split('/');
    if parts.next()? != prefix {
        return None;
    }
    parts.next()?.parse::<i64>().ok()
}

fn query_i64(query: &HashMap<String, String>, key: &str, default: i64) -> i64 {
    query
        .get(key)
        .and_then(|value| value.parse::<i64>().ok())
        .unwrap_or(default)
}

fn paginated(items: Vec<Value>, query: &HashMap<String, String>) -> Value {
    let page = query_i64(query, "page", 1).max(1);
    let page_size = query_i64(query, "page_size", 20).clamp(1, 500);
    let total = items.len() as i64;
    let start = ((page - 1) * page_size) as usize;
    let page_items = items
        .into_iter()
        .skip(start)
        .take(page_size as usize)
        .collect::<Vec<_>>();
    json!({
        "items": page_items,
        "total": total,
        "page": page,
        "page_size": page_size
    })
}

fn filter_errors(items: Vec<Value>, query: &HashMap<String, String>) -> Vec<Value> {
    let resolved = query
        .get("resolved")
        .and_then(|value| match value.as_str() {
            "1" | "true" | "yes" => Some(true),
            "0" | "false" | "no" => Some(false),
            _ => None,
        });
    let platform = query.get("platform").map(|value| value.to_lowercase());
    let q = query.get("q").map(|value| value.to_lowercase());

    items
        .into_iter()
        .filter(|item| {
            if let Some(resolved) = resolved {
                if item
                    .get("resolved")
                    .and_then(Value::as_bool)
                    .map(|value| value != resolved)
                    .unwrap_or(false)
                {
                    return false;
                }
            }
            if let Some(platform) = &platform {
                if item
                    .get("platform")
                    .and_then(Value::as_str)
                    .map(|value| value.to_lowercase() != *platform)
                    .unwrap_or(false)
                {
                    return false;
                }
            }
            if let Some(q) = &q {
                if !item.to_string().to_lowercase().contains(q) {
                    return false;
                }
            }
            true
        })
        .collect()
}

fn find_by_id(items: &[Value], id: i64, label: &str) -> Result<Value, ApiError> {
    items
        .iter()
        .find(|item| item_id(item) == Some(id))
        .cloned()
        .ok_or_else(|| ApiError::not_found(format!("{label} not found")))
}

fn item_id(item: &Value) -> Option<i64> {
    item.get("id").and_then(Value::as_i64)
}

fn set_id(item: &mut Value, id: i64) {
    if let Value::Object(map) = item {
        map.insert("id".to_owned(), json!(id));
    }
}

fn merge_json(target: &mut Value, update: Value) {
    match (target, update) {
        (Value::Object(target_map), Value::Object(update_map)) => {
            for (key, value) in update_map {
                if value.is_null() {
                    target_map.insert(key, Value::Null);
                } else {
                    merge_json(target_map.entry(key).or_insert(Value::Null), value);
                }
            }
        }
        (target_slot, value) => *target_slot = value,
    }
}

fn percentiles() -> Value {
    json!({
        "p50_ms": null,
        "p90_ms": null,
        "p95_ms": null,
        "p99_ms": null,
        "avg_ms": null,
        "max_ms": null
    })
}

fn observed_rate_summary(total: i64) -> Value {
    json!({ "current": total, "peak": total, "avg": total })
}

fn ratio(numerator: i64, denominator: i64) -> f64 {
    if denominator <= 0 {
        0.0
    } else {
        numerator as f64 / denominator as f64
    }
}

fn health_score(error_rate: f64, upstream_error_rate: f64) -> i64 {
    let penalty = (error_rate * 70.0 + upstream_error_rate * 30.0).round() as i64;
    (100 - penalty).clamp(0, 100)
}

fn count_status(items: &[Value], status_code: i64) -> i64 {
    items
        .iter()
        .filter(|item| item.get("status_code").and_then(Value::as_i64) == Some(status_code))
        .count() as i64
}

fn sum_i64(items: &[Value], key: &str) -> i64 {
    items
        .iter()
        .map(|item| item.get(key).and_then(Value::as_i64).unwrap_or(0))
        .sum()
}

fn duration_percentiles(items: &[Value]) -> Value {
    let mut values = items
        .iter()
        .filter_map(|item| item.get("duration_ms").and_then(Value::as_i64))
        .collect::<Vec<_>>();
    if values.is_empty() {
        return percentiles();
    }
    values.sort_unstable();
    let avg = values.iter().sum::<i64>() / values.len() as i64;
    json!({
        "p50_ms": percentile(&values, 50),
        "p90_ms": percentile(&values, 90),
        "p95_ms": percentile(&values, 95),
        "p99_ms": percentile(&values, 99),
        "avg_ms": avg,
        "max_ms": values.last().copied().unwrap_or(0)
    })
}

fn percentile(values: &[i64], percentile: usize) -> i64 {
    if values.is_empty() {
        return 0;
    }
    let index = ((values.len() - 1) * percentile).div_ceil(100);
    values[index.min(values.len() - 1)]
}

fn latency_histogram(items: &[Value]) -> Vec<Value> {
    let buckets = [
        ("0-50", 0, 50),
        ("51-100", 51, 100),
        ("101-250", 101, 250),
        ("251-500", 251, 500),
        ("501-1000", 501, 1000),
        ("1000+", 1001, i64::MAX),
    ];
    let durations = items
        .iter()
        .filter_map(|item| item.get("duration_ms").and_then(Value::as_i64))
        .collect::<Vec<_>>();
    buckets
        .into_iter()
        .map(|(label, min, max)| {
            json!({
                "bucket": label,
                "min_ms": min,
                "max_ms": if max == i64::MAX { Value::Null } else { json!(max) },
                "count": durations.iter().filter(|value| **value >= min && **value <= max).count() as i64
            })
        })
        .collect()
}

fn group_count(items: &[Value], key: &str) -> Vec<Value> {
    let mut counts: HashMap<String, i64> = HashMap::new();
    for item in items {
        let value = item
            .get(key)
            .map(|value| match value {
                Value::String(value) => value.clone(),
                other => other.to_string(),
            })
            .unwrap_or_else(|| "unknown".to_owned());
        *counts.entry(value).or_insert(0) += 1;
    }
    let mut out = counts
        .into_iter()
        .map(|(name, count)| json!({ "name": name, "count": count }))
        .collect::<Vec<_>>();
    out.sort_by(|left, right| {
        right
            .get("count")
            .and_then(Value::as_i64)
            .cmp(&left.get("count").and_then(Value::as_i64))
    });
    out
}

fn error_distribution(items: &[Value]) -> Value {
    let mut counts: HashMap<i64, (i64, i64, i64)> = HashMap::new();
    for item in items {
        let status_code = value_i64(item, "upstream_status_code")
            .or_else(|| value_i64(item, "status_code"))
            .unwrap_or(0);
        if status_code < 400 {
            continue;
        }
        let entry = counts.entry(status_code).or_default();
        entry.0 += 1;
        if is_business_limited_status(status_code) {
            entry.2 += 1;
        } else {
            entry.1 += 1;
        }
    }

    let mut rows = counts
        .into_iter()
        .map(|(status_code, (total, sla, business_limited))| {
            json!({
                "status_code": status_code,
                "total": total,
                "sla": sla,
                "business_limited": business_limited
            })
        })
        .collect::<Vec<_>>();
    rows.sort_by(|left, right| {
        right
            .get("total")
            .and_then(Value::as_i64)
            .cmp(&left.get("total").and_then(Value::as_i64))
            .then_with(|| {
                left.get("status_code")
                    .and_then(Value::as_i64)
                    .cmp(&right.get("status_code").and_then(Value::as_i64))
            })
    });
    rows.truncate(20);
    json!({
        "total": rows.iter().map(|item| value_i64(item, "total").unwrap_or(0)).sum::<i64>(),
        "items": rows
    })
}

fn is_business_limited_status(status_code: i64) -> bool {
    matches!(status_code, 429 | 529)
}

fn matches_ops_scope(item: &Value, query: &HashMap<String, String>) -> bool {
    if let Some(platform) = query.get("platform").map(|value| value.trim()) {
        if !platform.is_empty()
            && !platform.eq_ignore_ascii_case("all")
            && value_string(item, "platform")
                .or_else(|| value_string(item, "provider"))
                .map(|value| !value.eq_ignore_ascii_case(platform))
                .unwrap_or(true)
        {
            return false;
        }
    }
    if let Some(group_id) = query
        .get("group_id")
        .and_then(|value| value.parse::<i64>().ok())
    {
        if group_id > 0 && value_i64(item, "group_id") != Some(group_id) {
            return false;
        }
    }
    if let Some(account_id) = query
        .get("account_id")
        .and_then(|value| value.parse::<i64>().ok())
    {
        if account_id > 0 && value_i64(item, "account_id") != Some(account_id) {
            return false;
        }
    }
    true
}

fn latest_account_items_map(items: &[Value]) -> HashMap<i64, Value> {
    let mut latest: HashMap<i64, Value> = HashMap::new();
    for item in items {
        if let Some(account_id) = value_i64(item, "account_id") {
            latest.insert(account_id, item.clone());
        }
    }
    latest
}

fn runtime_snapshot_item(snapshot: &AccountRuntimeSnapshot) -> Value {
    json!({
        "account_id": snapshot.account_id,
        "account_name": snapshot.account_name,
        "platform": snapshot.platform,
        "provider": snapshot.platform,
        "group_id": snapshot.group_id,
        "group_name": snapshot.group_name,
        "max_capacity": snapshot.max_concurrent.map(|value| value.max(1) as i64).unwrap_or(1),
        "status_code": if snapshot.retry_after_seconds.unwrap_or(0) > 0 { 503 } else { 200 },
        "message": snapshot.last_error
    })
}

fn load_percentage(current_in_use: i64, max_capacity: i64) -> i64 {
    if max_capacity <= 0 {
        0
    } else {
        ((current_in_use.max(0) as f64 / max_capacity as f64) * 100.0).round() as i64
    }
}

fn value_i64(item: &Value, key: &str) -> Option<i64> {
    item.get(key).and_then(|value| {
        value
            .as_i64()
            .or_else(|| value.as_u64().and_then(|value| i64::try_from(value).ok()))
            .or_else(|| value.as_str().and_then(|value| value.parse::<i64>().ok()))
    })
}

fn value_string(item: &Value, key: &str) -> Option<String> {
    item.get(key).and_then(|value| match value {
        Value::String(value) if !value.trim().is_empty() => Some(value.clone()),
        Value::Number(value) => Some(value.to_string()),
        _ => None,
    })
}

fn display_account_name(item: &Value, account_id: i64) -> String {
    value_string(item, "account_name").unwrap_or_else(|| format!("account-{account_id}"))
}

fn display_group_name(item: &Value, group_id: i64) -> String {
    value_string(item, "group_name").unwrap_or_else(|| {
        if group_id > 0 {
            format!("group-{group_id}")
        } else {
            String::new()
        }
    })
}

fn increment_json_i64(item: &mut Value, key: &str, delta: i64) {
    if let Value::Object(map) = item {
        let next = map.get(key).and_then(Value::as_i64).unwrap_or(0) + delta;
        map.insert(key.to_owned(), json!(next));
    }
}

fn refresh_load_percentage(item: &mut Value) {
    let current_in_use = value_i64(item, "current_in_use").unwrap_or(0);
    let max_capacity = value_i64(item, "max_capacity").unwrap_or(0);
    if let Value::Object(map) = item {
        map.insert(
            "load_percentage".to_owned(),
            json!(load_percentage(current_in_use, max_capacity)),
        );
    }
}

fn realtime_window(query: &HashMap<String, String>) -> (String, i64) {
    match query
        .get("window")
        .map(|value| value.trim().to_lowercase())
        .as_deref()
    {
        Some("5m" | "5min") => ("5min".to_owned(), 300),
        Some("30m" | "30min") => ("30min".to_owned(), 1_800),
        Some("1h" | "60m" | "60min") => ("1h".to_owned(), 3_600),
        _ => ("1min".to_owned(), 60),
    }
}

fn observed_window_rate_summary(total: i64, window_seconds: i64) -> Value {
    let avg = if window_seconds > 0 {
        total as f64 / window_seconds as f64
    } else {
        0.0
    };
    json!({ "current": avg, "peak": avg, "avg": avg })
}

#[derive(Default)]
struct TokenStatsAggregate {
    request_count: i64,
    total_output_tokens: i64,
    total_duration_ms: i64,
    tokens_per_second_sum: f64,
    tokens_per_second_count: i64,
    first_token_ms_sum: i64,
    requests_with_first_token: i64,
}

impl TokenStatsAggregate {
    fn into_json(self, model: String) -> Value {
        let avg_tokens_per_sec = if self.tokens_per_second_count > 0 {
            json!(self.tokens_per_second_sum / self.tokens_per_second_count as f64)
        } else {
            Value::Null
        };
        let avg_first_token_ms = if self.requests_with_first_token > 0 {
            json!(self.first_token_ms_sum as f64 / self.requests_with_first_token as f64)
        } else {
            Value::Null
        };
        json!({
            "model": model,
            "request_count": self.request_count,
            "avg_tokens_per_sec": avg_tokens_per_sec,
            "avg_first_token_ms": avg_first_token_ms,
            "total_output_tokens": self.total_output_tokens,
            "avg_duration_ms": if self.request_count > 0 { self.total_duration_ms / self.request_count } else { 0 },
            "requests_with_first_token": self.requests_with_first_token
        })
    }
}

#[derive(Debug, Clone, Copy)]
struct AlertEvaluation {
    triggered: bool,
    metric_value: f64,
    threshold_value: f64,
}

fn runtime_alerts_disabled(settings: &Value) -> bool {
    settings.get("enabled").and_then(Value::as_bool) == Some(false)
}

fn alert_rule_cooldown_elapsed(rule: &Value) -> bool {
    let cooldown_minutes = value_i64(rule, "cooldown_minutes")
        .or_else(|| value_i64(rule, "cooldown"))
        .unwrap_or(0);
    if cooldown_minutes <= 0 {
        return true;
    }
    rule.get("last_triggered_at")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .is_none()
}

fn evaluate_alert_rule(
    rule: &Value,
    request_details: &[Value],
    upstream_errors: &[Value],
) -> Option<AlertEvaluation> {
    let metric_type = value_string(rule, "metric_type")?;
    let threshold = value_f64(rule, "threshold")?;
    let operator = value_string(rule, "operator").unwrap_or_else(|| ">".to_owned());
    let filters = rule.get("filters").cloned().unwrap_or_else(|| json!({}));
    let filtered_requests = request_details
        .iter()
        .filter(|item| matches_alert_filters(item, &filters))
        .collect::<Vec<_>>();
    let filtered_upstream_errors = upstream_errors
        .iter()
        .filter(|item| matches_alert_filters(item, &filters))
        .collect::<Vec<_>>();
    let metric_value = alert_metric_value(
        &metric_type,
        &filtered_requests,
        &filtered_upstream_errors,
        request_details,
    )?;
    Some(AlertEvaluation {
        triggered: compare_metric(metric_value, &operator, threshold),
        metric_value,
        threshold_value: threshold,
    })
}

fn alert_metric_value(
    metric_type: &str,
    request_details: &[&Value],
    upstream_errors: &[&Value],
    all_request_details: &[Value],
) -> Option<f64> {
    let request_count = request_details.len() as f64;
    let error_count = request_details
        .iter()
        .filter(|item| value_i64(item, "status_code").unwrap_or(0) >= 400)
        .count() as f64;
    let upstream_error_count = upstream_errors.len() as f64;
    match metric_type {
        "success_rate" => Some(if request_count <= 0.0 {
            100.0
        } else {
            ((request_count - error_count).max(0.0) / request_count) * 100.0
        }),
        "error_rate" => Some(if request_count <= 0.0 {
            0.0
        } else {
            (error_count / request_count) * 100.0
        }),
        "upstream_error_rate" => Some(if request_count <= 0.0 {
            0.0
        } else {
            (upstream_error_count / request_count) * 100.0
        }),
        "request_count" => Some(request_count),
        "upstream_error_count" => Some(upstream_error_count),
        "business_limited_count" => Some(
            request_details
                .iter()
                .filter(|item| {
                    value_i64(item, "status_code")
                        .map(is_business_limited_status)
                        .unwrap_or(false)
                })
                .count() as f64,
        ),
        "token_consumed" => Some(
            request_details
                .iter()
                .map(|item| value_i64(item, "total_tokens").unwrap_or(0))
                .sum::<i64>() as f64,
        ),
        "duration_p95_ms" => Some(duration_percentile_for_refs(request_details, 95) as f64),
        "group_available_accounts" => Some(group_available_accounts(request_details) as f64),
        "group_available_ratio" => Some(group_available_ratio(request_details)),
        "group_rate_limit_ratio" => Some(group_rate_limit_ratio(request_details)),
        "account_rate_limited_count" => Some(account_rate_limited_count(request_details) as f64),
        "account_error_count" => Some(account_error_count(request_details) as f64),
        "account_error_ratio" => Some(account_error_ratio(request_details)),
        "overload_account_count" => Some(overload_account_count(request_details) as f64),
        "cpu_usage_percent" | "memory_usage_percent" | "concurrency_queue_depth" => None,
        _ if metric_type.ends_with("_rate") && !all_request_details.is_empty() => Some(0.0),
        _ => None,
    }
}

fn matches_alert_filters(item: &Value, filters: &Value) -> bool {
    if let Some(platform) = filters
        .get("platform")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty() && !value.eq_ignore_ascii_case("all"))
    {
        if value_string(item, "platform")
            .or_else(|| value_string(item, "provider"))
            .map(|value| !value.eq_ignore_ascii_case(platform))
            .unwrap_or(true)
        {
            return false;
        }
    }
    for key in ["group_id", "account_id", "api_key_id", "user_id"] {
        if let Some(expected) = filters.get(key).and_then(|value| {
            value
                .as_i64()
                .or_else(|| value.as_str().and_then(|value| value.parse::<i64>().ok()))
        }) {
            if expected > 0 && value_i64(item, key) != Some(expected) {
                return false;
            }
        }
    }
    true
}

fn matches_alert_event_query(event: &Value, query: &HashMap<String, String>) -> bool {
    if let Some(status) = query
        .get("status")
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
    {
        if value_string(event, "status")
            .map(|value| value != status)
            .unwrap_or(true)
        {
            return false;
        }
    }
    if let Some(severity) = query
        .get("severity")
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
    {
        if value_string(event, "severity")
            .map(|value| value != severity)
            .unwrap_or(true)
        {
            return false;
        }
    }
    if let Some(email_sent) = query.get("email_sent").and_then(|value| parse_bool(value)) {
        if event
            .get("email_sent")
            .and_then(Value::as_bool)
            .map(|value| value != email_sent)
            .unwrap_or(true)
        {
            return false;
        }
    }
    if let Some(platform) = query
        .get("platform")
        .map(|value| value.trim())
        .filter(|value| !value.is_empty() && !value.eq_ignore_ascii_case("all"))
    {
        if event
            .get("dimensions")
            .and_then(|dimensions| value_string(dimensions, "platform"))
            .map(|value| !value.eq_ignore_ascii_case(platform))
            .unwrap_or(true)
        {
            return false;
        }
    }
    if let Some(group_id) = query
        .get("group_id")
        .and_then(|value| value.parse::<i64>().ok())
    {
        if group_id > 0
            && event
                .get("dimensions")
                .and_then(|dimensions| value_i64(dimensions, "group_id"))
                != Some(group_id)
        {
            return false;
        }
    }
    if let (Some(before_id), Some(before_fired_at)) = (
        query
            .get("before_id")
            .and_then(|value| value.parse::<i64>().ok()),
        query
            .get("before_fired_at")
            .map(|value| value.trim())
            .filter(|value| !value.is_empty()),
    ) {
        let event_fired_at = value_string(event, "fired_at").unwrap_or_default();
        let event_id = value_i64(event, "id").unwrap_or(0);
        if event_fired_at.as_str() > before_fired_at {
            return false;
        }
        if event_fired_at == before_fired_at && event_id >= before_id {
            return false;
        }
    }
    true
}

fn parse_bool(value: &str) -> Option<bool> {
    match value.trim().to_lowercase().as_str() {
        "true" | "1" | "yes" => Some(true),
        "false" | "0" | "no" => Some(false),
        _ => None,
    }
}

fn compare_metric(actual: f64, operator: &str, threshold: f64) -> bool {
    match operator.trim() {
        ">" => actual > threshold,
        ">=" => actual >= threshold,
        "<" => actual < threshold,
        "<=" => actual <= threshold,
        "==" | "=" => (actual - threshold).abs() < f64::EPSILON,
        "!=" => (actual - threshold).abs() >= f64::EPSILON,
        _ => actual > threshold,
    }
}

fn alert_event_from_rule(id: i64, rule: &Value, evaluation: &AlertEvaluation) -> Value {
    let rule_id = item_id(rule).unwrap_or_default();
    let metric_type = value_string(rule, "metric_type").unwrap_or_else(|| "unknown".to_owned());
    let severity = value_string(rule, "severity").unwrap_or_else(|| "P2".to_owned());
    json!({
        "id": id,
        "rule_id": rule_id,
        "rule_name": value_string(rule, "name").unwrap_or_else(|| format!("rule-{rule_id}")),
        "severity": severity,
        "status": "firing",
        "title": format!("{} threshold breached", metric_type),
        "description": format!("{} value {} breached threshold {}", metric_type, evaluation.metric_value, evaluation.threshold_value),
        "metric_type": metric_type,
        "metric_value": evaluation.metric_value,
        "threshold_value": evaluation.threshold_value,
        "dimensions": rule.get("filters").cloned().unwrap_or_else(|| json!({})),
        "fired_at": NOW,
        "resolved_at": null,
        "email_sent": false,
        "created_at": NOW,
        "updated_at": NOW
    })
}

fn value_f64(item: &Value, key: &str) -> Option<f64> {
    item.get(key).and_then(|value| {
        value
            .as_f64()
            .or_else(|| value.as_i64().map(|value| value as f64))
            .or_else(|| value.as_str().and_then(|value| value.parse::<f64>().ok()))
    })
}

fn duration_percentile_for_refs(items: &[&Value], percentile_rank: usize) -> i64 {
    let mut values = items
        .iter()
        .filter_map(|item| value_i64(item, "duration_ms"))
        .collect::<Vec<_>>();
    if values.is_empty() {
        return 0;
    }
    values.sort_unstable();
    percentile(&values, percentile_rank)
}

fn latest_account_refs<'a>(items: &'a [&'a Value]) -> HashMap<i64, &'a Value> {
    let mut latest = HashMap::new();
    for item in items {
        if let Some(account_id) = value_i64(item, "account_id") {
            latest.insert(account_id, *item);
        }
    }
    latest
}

fn group_available_accounts(items: &[&Value]) -> i64 {
    latest_account_refs(items)
        .values()
        .filter(|item| {
            value_i64(item, "status_code")
                .map(|status| (200..400).contains(&status))
                .unwrap_or(false)
        })
        .count() as i64
}

fn group_available_ratio(items: &[&Value]) -> f64 {
    let accounts = latest_account_refs(items);
    if accounts.is_empty() {
        100.0
    } else {
        accounts
            .values()
            .filter(|item| {
                value_i64(item, "status_code")
                    .map(|status| (200..400).contains(&status))
                    .unwrap_or(false)
            })
            .count() as f64
            / accounts.len() as f64
            * 100.0
    }
}

fn group_rate_limit_ratio(items: &[&Value]) -> f64 {
    let accounts = latest_account_refs(items);
    if accounts.is_empty() {
        0.0
    } else {
        accounts
            .values()
            .filter(|item| {
                value_i64(item, "status_code")
                    .map(is_business_limited_status)
                    .unwrap_or(false)
            })
            .count() as f64
            / accounts.len() as f64
            * 100.0
    }
}

fn account_rate_limited_count(items: &[&Value]) -> i64 {
    items
        .iter()
        .filter(|item| {
            value_i64(item, "status_code")
                .map(is_business_limited_status)
                .unwrap_or(false)
        })
        .count() as i64
}

fn account_error_count(items: &[&Value]) -> i64 {
    items
        .iter()
        .filter(|item| value_i64(item, "status_code").unwrap_or(0) >= 500)
        .count() as i64
}

fn account_error_ratio(items: &[&Value]) -> f64 {
    if items.is_empty() {
        0.0
    } else {
        account_error_count(items) as f64 / items.len() as f64 * 100.0
    }
}

fn overload_account_count(items: &[&Value]) -> i64 {
    latest_account_refs(items)
        .values()
        .filter(|item| value_i64(item, "status_code") == Some(529))
        .count() as i64
}

fn default_alert_rule(id: i64) -> Value {
    json!({
        "id": id,
        "name": "demo",
        "enabled": false,
        "metric_type": "error_rate",
        "operator": ">",
        "threshold": 0,
        "window_minutes": 5,
        "sustained_minutes": 1,
        "severity": "info",
        "cooldown_minutes": 10,
        "notify_email": false,
        "filters": {},
        "created_at": NOW,
        "updated_at": NOW,
        "last_triggered_at": null
    })
}

fn seed_alert_events() -> Vec<Value> {
    vec![json!({
        "id": 1,
        "rule_id": 0,
        "severity": "info",
        "status": "firing",
        "title": "Seed alert",
        "description": "Seed alert event for backend_next compatibility",
        "metric_value": 1,
        "threshold_value": 0,
        "dimensions": {},
        "fired_at": NOW,
        "resolved_at": null,
        "email_sent": false,
        "created_at": NOW
    })]
}

fn seed_request_errors() -> Vec<Value> {
    vec![json!({
        "id": 1,
        "request_id": "req-seed-1",
        "client_request_id": "client-seed-1",
        "phase": "request",
        "error_owner": "gateway",
        "error_source": "gateway",
        "platform": "openai",
        "group_id": 1,
        "group_name": "seed group",
        "account_id": 1,
        "account_name": "seed openai account",
        "api_key_id": 1,
        "user_id": 1,
        "status_code": 500,
        "method": "POST",
        "path": "/v1/chat/completions",
        "model": "gpt-5",
        "message": "seed request error",
        "resolved": false,
        "resolved_at": null,
        "resolution_note": null,
        "created_at": NOW
    })]
}

fn seed_upstream_errors() -> Vec<Value> {
    vec![json!({
        "id": 2,
        "request_id": "req-seed-1",
        "client_request_id": "client-seed-1",
        "phase": "upstream",
        "error_owner": "provider",
        "error_source": "upstream",
        "platform": "openai",
        "group_id": 1,
        "group_name": "seed group",
        "account_id": 1,
        "account_name": "seed openai account",
        "api_key_id": 1,
        "user_id": 1,
        "status_code": 502,
        "method": "POST",
        "path": "/v1/chat/completions",
        "model": "gpt-5",
        "message": "seed upstream error",
        "upstream_response": { "error": "bad gateway" },
        "resolved": false,
        "resolved_at": null,
        "resolution_note": null,
        "created_at": NOW
    })]
}

fn seed_request_details() -> Vec<Value> {
    vec![json!({
        "id": 1,
        "request_id": "req-seed-1",
        "client_request_id": "client-seed-1",
        "kind": "error",
        "platform": "openai",
        "provider": "openai",
        "group_id": 1,
        "group_name": "seed group",
        "account_id": 1,
        "account_name": "seed openai account",
        "api_key_id": 1,
        "user_id": 1,
        "model": "gpt-5",
        "upstream_model": "gpt-5",
        "method": "POST",
        "path": "/v1/chat/completions",
        "endpoint": "/v1/chat/completions",
        "downstream_protocol": "openai_chat_completions",
        "upstream_protocol": "openai_chat_completions",
        "stream": false,
        "status_code": 500,
        "duration_ms": 42,
        "input_tokens": 3,
        "output_tokens": 4,
        "cache_creation_tokens": 0,
        "cache_read_tokens": 0,
        "total_tokens": 7,
        "created_at": NOW
    })]
}

fn seed_system_logs() -> Vec<Value> {
    vec![json!({
        "id": 1,
        "level": "info",
        "component": "backend_next",
        "message": "ops service initialized",
        "request_id": null,
        "client_request_id": null,
        "platform": null,
        "model": null,
        "created_at": NOW
    })]
}

fn default_email_notification_config() -> Value {
    json!({
        "alert": {
            "enabled": false,
            "recipients": [],
            "min_severity": "",
            "rate_limit_per_hour": 10,
            "batching_window_seconds": 300,
            "include_resolved_alerts": false
        },
        "report": {
            "enabled": false,
            "recipients": [],
            "daily_summary_enabled": false,
            "daily_summary_schedule": "09:00",
            "weekly_summary_enabled": false,
            "weekly_summary_schedule": "MON 09:00",
            "error_digest_enabled": false,
            "error_digest_schedule": "09:00",
            "error_digest_min_count": 1,
            "account_health_enabled": false,
            "account_health_schedule": "09:00",
            "account_health_error_rate_threshold": 0
        }
    })
}

fn default_alert_runtime_settings() -> Value {
    json!({
        "evaluation_interval_seconds": 60,
        "distributed_lock": {
            "enabled": false,
            "key": "sub2api:ops:alert",
            "ttl_seconds": 120
        },
        "silencing": {
            "enabled": false,
            "global_until_rfc3339": "",
            "global_reason": "",
            "entries": []
        },
        "thresholds": default_metric_thresholds()
    })
}

fn default_runtime_log_config() -> Value {
    json!({
        "level": "info",
        "enable_sampling": false,
        "sampling_initial": 100,
        "sampling_thereafter": 100,
        "caller": false,
        "stacktrace_level": "error",
        "retention_days": 7,
        "source": "backend_next",
        "updated_at": NOW,
        "updated_by_user_id": 1
    })
}

fn default_advanced_settings() -> Value {
    json!({
        "data_retention": {
            "cleanup_enabled": false,
            "cleanup_schedule": "0 3 * * *",
            "error_log_retention_days": 7,
            "minute_metrics_retention_days": 1,
            "hourly_metrics_retention_days": 30
        },
        "aggregation": {
            "aggregation_enabled": false
        },
        "openai_account_quota_auto_pause": {
            "default_threshold_5h": 0,
            "default_threshold_7d": 0
        },
        "ignore_count_tokens_errors": true,
        "ignore_context_canceled": true,
        "ignore_no_available_accounts": false,
        "ignore_invalid_api_key_errors": false,
        "ignore_insufficient_balance_errors": false,
        "display_openai_token_stats": true,
        "display_alert_events": true,
        "auto_refresh_enabled": true,
        "auto_refresh_interval_seconds": 30
    })
}

fn default_metric_thresholds() -> Value {
    json!({
        "sla_percent_min": null,
        "ttft_p99_ms_max": null,
        "request_error_rate_percent_max": null,
        "upstream_error_rate_percent_max": null
    })
}
