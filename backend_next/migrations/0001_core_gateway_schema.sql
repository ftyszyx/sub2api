-- backend_next 初始核心迁移。
-- 目标是把认证、网关路由、计费、审计和多实例一致性需要的权威数据都落到 PostgreSQL。
-- 本文件保持幂等：重复执行时只补齐缺失对象，不依赖 Redis 或本地文件系统保存关键状态。

-- 用户基础资料。这里不放登录凭证，方便以后同时支持密码、OAuth、企业 SSO 等多种身份来源。
CREATE TABLE IF NOT EXISTS users (
    id BIGSERIAL PRIMARY KEY,
    email TEXT NOT NULL UNIQUE,
    username TEXT NOT NULL,
    role TEXT NOT NULL DEFAULT 'user',
    status TEXT NOT NULL DEFAULT 'active',
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_backend_next_users_status ON users(status);

-- 第三方登录身份绑定。同一个用户可以绑定多个 OAuth Provider，同一个外部身份只能绑定一次。
CREATE TABLE IF NOT EXISTS oauth_identities (
    id BIGSERIAL PRIMARY KEY,
    user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    provider TEXT NOT NULL,
    provider_key TEXT NOT NULL,
    provider_subject TEXT NOT NULL,
    email TEXT,
    bound_at_unix BIGINT NOT NULL,
    metadata JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_backend_next_oauth_identities_external
    ON oauth_identities(provider, provider_key, provider_subject);
CREATE INDEX IF NOT EXISTS idx_backend_next_oauth_identities_user_id
    ON oauth_identities(user_id);

-- 登录会话。access/refresh token 独立存储，支持后台吊销、过期清理和多端登录管理。
CREATE TABLE IF NOT EXISTS auth_sessions (
    id BIGSERIAL PRIMARY KEY,
    user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    access_token TEXT NOT NULL UNIQUE,
    refresh_token TEXT NOT NULL UNIQUE,
    access_expires_at_unix BIGINT NOT NULL,
    refresh_expires_at_unix BIGINT NOT NULL,
    revoked_at_unix BIGINT,
    created_at_unix BIGINT NOT NULL,
    metadata JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_backend_next_auth_sessions_user_id
    ON auth_sessions(user_id);
CREATE INDEX IF NOT EXISTS idx_backend_next_auth_sessions_access_token
    ON auth_sessions(access_token);
CREATE INDEX IF NOT EXISTS idx_backend_next_auth_sessions_refresh_token
    ON auth_sessions(refresh_token);

-- 密码登录凭证。和 users 分表，避免把认证细节耦合到用户资料表。
CREATE TABLE IF NOT EXISTS auth_credentials (
    user_id BIGINT PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    email TEXT NOT NULL UNIQUE,
    password_hash TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'active',
    updated_at_unix BIGINT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_backend_next_auth_credentials_email
    ON auth_credentials(email);

-- 用户侧的计价/权限分组。API Key 绑定 group，具体可用账号由 group 下面的账号能力决定。
CREATE TABLE IF NOT EXISTS groups (
    id BIGSERIAL PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    status TEXT NOT NULL DEFAULT 'active',
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_backend_next_groups_status ON groups(status);

-- 下游调用凭证。API Key 是用户访问网关的入口，同时承载配额和短期限流窗口的权威状态。
CREATE TABLE IF NOT EXISTS api_keys (
    id BIGSERIAL PRIMARY KEY,
    user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    key TEXT NOT NULL UNIQUE,
    name TEXT NOT NULL,
    group_id BIGINT REFERENCES groups(id) ON DELETE SET NULL,
    status TEXT NOT NULL DEFAULT 'active',
    quota DOUBLE PRECISION NOT NULL DEFAULT 0,
    quota_used DOUBLE PRECISION NOT NULL DEFAULT 0,
    rate_limit_5h DOUBLE PRECISION NOT NULL DEFAULT 0,
    rate_limit_1d DOUBLE PRECISION NOT NULL DEFAULT 0,
    rate_limit_7d DOUBLE PRECISION NOT NULL DEFAULT 0,
    usage_5h DOUBLE PRECISION NOT NULL DEFAULT 0,
    usage_1d DOUBLE PRECISION NOT NULL DEFAULT 0,
    usage_7d DOUBLE PRECISION NOT NULL DEFAULT 0,
    window_5h_start BIGINT,
    window_1d_start BIGINT,
    window_7d_start BIGINT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- 兼容早期 backend_next 数据库：如果库里已经有 api_keys，就只补齐新增的配额/限流字段。
ALTER TABLE api_keys ADD COLUMN IF NOT EXISTS key TEXT;
ALTER TABLE api_keys ADD COLUMN IF NOT EXISTS quota DOUBLE PRECISION NOT NULL DEFAULT 0;
ALTER TABLE api_keys ADD COLUMN IF NOT EXISTS quota_used DOUBLE PRECISION NOT NULL DEFAULT 0;
ALTER TABLE api_keys ADD COLUMN IF NOT EXISTS rate_limit_5h DOUBLE PRECISION NOT NULL DEFAULT 0;
ALTER TABLE api_keys ADD COLUMN IF NOT EXISTS rate_limit_1d DOUBLE PRECISION NOT NULL DEFAULT 0;
ALTER TABLE api_keys ADD COLUMN IF NOT EXISTS rate_limit_7d DOUBLE PRECISION NOT NULL DEFAULT 0;
ALTER TABLE api_keys ADD COLUMN IF NOT EXISTS usage_5h DOUBLE PRECISION NOT NULL DEFAULT 0;
ALTER TABLE api_keys ADD COLUMN IF NOT EXISTS usage_1d DOUBLE PRECISION NOT NULL DEFAULT 0;
ALTER TABLE api_keys ADD COLUMN IF NOT EXISTS usage_7d DOUBLE PRECISION NOT NULL DEFAULT 0;
ALTER TABLE api_keys ADD COLUMN IF NOT EXISTS window_5h_start BIGINT;
ALTER TABLE api_keys ADD COLUMN IF NOT EXISTS window_1d_start BIGINT;
ALTER TABLE api_keys ADD COLUMN IF NOT EXISTS window_7d_start BIGINT;
CREATE INDEX IF NOT EXISTS idx_backend_next_api_keys_user_id ON api_keys(user_id);
CREATE INDEX IF NOT EXISTS idx_backend_next_api_keys_group_id ON api_keys(group_id);
CREATE INDEX IF NOT EXISTS idx_backend_next_api_keys_status ON api_keys(status);
CREATE UNIQUE INDEX IF NOT EXISTS idx_backend_next_api_keys_key ON api_keys(key) WHERE key IS NOT NULL;

-- 上游账号。provider 表示供应商，default_upstream_protocol 表示账号默认使用的上游协议。
-- model_mapping/extra 用 JSONB 保存供应商差异，避免每加一个供应商就修改核心表结构。
CREATE TABLE IF NOT EXISTS accounts (
    id BIGSERIAL PRIMARY KEY,
    name TEXT NOT NULL,
    provider TEXT NOT NULL,
    default_upstream_protocol TEXT NOT NULL,
    base_url TEXT,
    api_key TEXT,
    model_mapping JSONB NOT NULL DEFAULT '[]'::jsonb,
    extra JSONB NOT NULL DEFAULT '{}'::jsonb,
    enabled BOOLEAN NOT NULL DEFAULT TRUE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_backend_next_accounts_provider ON accounts(provider);
CREATE INDEX IF NOT EXISTS idx_backend_next_accounts_enabled ON accounts(enabled);

-- 账号和 group 的绑定关系。
-- supported_downstream_protocols 表示这个账号在该 group 下能承接哪些下游协议。
-- upstream_protocol_override 用于少数账号在特定 group 下强制走不同的上游协议。
CREATE TABLE IF NOT EXISTS account_groups (
    account_id BIGINT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    group_id BIGINT NOT NULL REFERENCES groups(id) ON DELETE CASCADE,
    supported_downstream_protocols TEXT[] NOT NULL DEFAULT ARRAY[]::TEXT[],
    upstream_protocol_override TEXT,
    priority INT NOT NULL DEFAULT 50,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (account_id, group_id)
);

CREATE INDEX IF NOT EXISTS idx_backend_next_account_groups_group_id ON account_groups(group_id);
CREATE INDEX IF NOT EXISTS idx_backend_next_account_groups_priority ON account_groups(priority);

-- 调用流水。用于账单、审计、排障和后续统计；metadata 存放非固定结构的供应商返回细节。
CREATE TABLE IF NOT EXISTS usage_logs (
    id BIGSERIAL PRIMARY KEY,
    user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    api_key_id BIGINT NOT NULL REFERENCES api_keys(id) ON DELETE CASCADE,
    group_id BIGINT REFERENCES groups(id) ON DELETE SET NULL,
    account_id BIGINT REFERENCES accounts(id) ON DELETE SET NULL,
    downstream_protocol TEXT NOT NULL,
    upstream_protocol TEXT NOT NULL,
    provider TEXT NOT NULL,
    endpoint TEXT NOT NULL,
    requested_model TEXT NOT NULL,
    upstream_model TEXT NOT NULL,
    input_tokens BIGINT NOT NULL DEFAULT 0,
    output_tokens BIGINT NOT NULL DEFAULT 0,
    cache_creation_tokens BIGINT NOT NULL DEFAULT 0,
    cache_read_tokens BIGINT NOT NULL DEFAULT 0,
    actual_cost DOUBLE PRECISION NOT NULL DEFAULT 0,
    status TEXT NOT NULL DEFAULT 'success',
    created_at_unix BIGINT NOT NULL,
    metadata JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_backend_next_usage_user_id ON usage_logs(user_id);
CREATE INDEX IF NOT EXISTS idx_backend_next_usage_api_key_id ON usage_logs(api_key_id);
CREATE INDEX IF NOT EXISTS idx_backend_next_usage_group_id ON usage_logs(group_id);
CREATE INDEX IF NOT EXISTS idx_backend_next_usage_account_id ON usage_logs(account_id);
CREATE INDEX IF NOT EXISTS idx_backend_next_usage_downstream_protocol ON usage_logs(downstream_protocol);
CREATE INDEX IF NOT EXISTS idx_backend_next_usage_created_at_unix ON usage_logs(created_at_unix);

-- 用量清理任务。管理员发起清理时先记录任务，再由后台执行，便于取消、追踪和审计。
CREATE TABLE IF NOT EXISTS usage_cleanup_tasks (
    id BIGSERIAL PRIMARY KEY,
    status TEXT NOT NULL,
    filters JSONB NOT NULL,
    created_by BIGINT NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    deleted_rows BIGINT NOT NULL DEFAULT 0,
    error_message TEXT,
    canceled_by BIGINT REFERENCES users(id) ON DELETE SET NULL,
    canceled_at_text TEXT,
    started_at_text TEXT,
    finished_at_text TEXT,
    created_at_text TEXT NOT NULL DEFAULT '',
    updated_at_text TEXT NOT NULL DEFAULT '',
    canceled_at TIMESTAMPTZ,
    started_at TIMESTAMPTZ,
    finished_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_backend_next_usage_cleanup_tasks_status_created_at
    ON usage_cleanup_tasks(status, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_backend_next_usage_cleanup_tasks_created_at
    ON usage_cleanup_tasks(created_at DESC);
CREATE INDEX IF NOT EXISTS idx_backend_next_usage_cleanup_tasks_canceled_at
    ON usage_cleanup_tasks(canceled_at DESC);

-- 支付订单。覆盖余额充值、套餐购买、退款等支付生命周期。
CREATE TABLE IF NOT EXISTS payment_orders (
    id BIGSERIAL PRIMARY KEY,
    user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    amount DOUBLE PRECISION NOT NULL,
    pay_amount DOUBLE PRECISION NOT NULL,
    currency TEXT NOT NULL,
    fee_rate DOUBLE PRECISION NOT NULL DEFAULT 0,
    payment_type TEXT NOT NULL,
    out_trade_no TEXT NOT NULL UNIQUE,
    status TEXT NOT NULL,
    order_type TEXT NOT NULL,
    refund_amount DOUBLE PRECISION NOT NULL DEFAULT 0,
    refund_reason TEXT,
    refund_request_reason TEXT,
    plan_id BIGINT,
    provider_instance_id TEXT,
    created_at_text TEXT NOT NULL,
    expires_at_text TEXT NOT NULL,
    paid_at_text TEXT,
    completed_at_text TEXT,
    cancelled_at_text TEXT,
    refund_requested_at_text TEXT,
    refunded_at_text TEXT,
    webhook_count BIGINT NOT NULL DEFAULT 0,
    metadata JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_backend_next_payment_orders_user_id ON payment_orders(user_id);
CREATE INDEX IF NOT EXISTS idx_backend_next_payment_orders_status ON payment_orders(status);
CREATE INDEX IF NOT EXISTS idx_backend_next_payment_orders_payment_type ON payment_orders(payment_type);
CREATE INDEX IF NOT EXISTS idx_backend_next_payment_orders_provider_instance_id ON payment_orders(provider_instance_id);

-- 支付审计日志。记录订单状态变化、回调处理、人工操作和异常说明。
CREATE TABLE IF NOT EXISTS payment_audit_logs (
    id BIGSERIAL PRIMARY KEY,
    order_id TEXT NOT NULL,
    action TEXT NOT NULL,
    detail TEXT NOT NULL DEFAULT '',
    operator TEXT NOT NULL DEFAULT 'system',
    created_at_text TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_backend_next_payment_audit_logs_order_id ON payment_audit_logs(order_id);
CREATE INDEX IF NOT EXISTS idx_backend_next_payment_audit_logs_action ON payment_audit_logs(action);

-- 用户余额快照。真实余额变化仍以 balance_transactions 作为可追溯流水。
CREATE TABLE IF NOT EXISTS user_balances (
    user_id BIGINT PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    balance DOUBLE PRECISION NOT NULL DEFAULT 0,
    updated_at_text TEXT NOT NULL DEFAULT '',
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- 余额变动流水。UNIQUE(order_id, transaction_type) 用于防止同一订单同一动作重复入账。
CREATE TABLE IF NOT EXISTS balance_transactions (
    id BIGSERIAL PRIMARY KEY,
    user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    order_id TEXT NOT NULL,
    transaction_type TEXT NOT NULL,
    amount DOUBLE PRECISION NOT NULL,
    balance_after DOUBLE PRECISION NOT NULL,
    created_at_text TEXT NOT NULL,
    metadata JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE(order_id, transaction_type)
);

CREATE INDEX IF NOT EXISTS idx_backend_next_balance_transactions_user_id ON balance_transactions(user_id);
CREATE INDEX IF NOT EXISTS idx_backend_next_balance_transactions_order_id ON balance_transactions(order_id);

-- 用户订阅。记录套餐有效期、来源订单和按日/周/月统计的订阅内用量。
CREATE TABLE IF NOT EXISTS user_subscriptions (
    id BIGSERIAL PRIMARY KEY,
    user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    group_id BIGINT NOT NULL,
    plan_id BIGINT,
    status TEXT NOT NULL DEFAULT 'active',
    starts_at_text TEXT NOT NULL,
    expires_at_text TEXT NOT NULL,
    daily_usage_usd DOUBLE PRECISION NOT NULL DEFAULT 0,
    weekly_usage_usd DOUBLE PRECISION NOT NULL DEFAULT 0,
    monthly_usage_usd DOUBLE PRECISION NOT NULL DEFAULT 0,
    daily_window_start_text TEXT,
    weekly_window_start_text TEXT,
    monthly_window_start_text TEXT,
    source_order_id TEXT NOT NULL UNIQUE,
    created_at_text TEXT NOT NULL,
    metadata JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_backend_next_user_subscriptions_user_id ON user_subscriptions(user_id);
CREATE INDEX IF NOT EXISTS idx_backend_next_user_subscriptions_group_id ON user_subscriptions(group_id);
CREATE INDEX IF NOT EXISTS idx_backend_next_user_subscriptions_status ON user_subscriptions(status);

-- 兼容早期订阅表：补齐订阅限额统计窗口字段。
ALTER TABLE user_subscriptions ADD COLUMN IF NOT EXISTS daily_usage_usd DOUBLE PRECISION NOT NULL DEFAULT 0;
ALTER TABLE user_subscriptions ADD COLUMN IF NOT EXISTS weekly_usage_usd DOUBLE PRECISION NOT NULL DEFAULT 0;
ALTER TABLE user_subscriptions ADD COLUMN IF NOT EXISTS monthly_usage_usd DOUBLE PRECISION NOT NULL DEFAULT 0;
ALTER TABLE user_subscriptions ADD COLUMN IF NOT EXISTS daily_window_start_text TEXT;
ALTER TABLE user_subscriptions ADD COLUMN IF NOT EXISTS weekly_window_start_text TEXT;
ALTER TABLE user_subscriptions ADD COLUMN IF NOT EXISTS monthly_window_start_text TEXT;

-- 用户按平台维度的限额。deleted_at 用于软删除，唯一索引只约束未删除的有效配置。
CREATE TABLE IF NOT EXISTS user_platform_quotas (
    id BIGSERIAL PRIMARY KEY,
    user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    platform TEXT NOT NULL CHECK (platform IN ('anthropic', 'openai', 'gemini', 'antigravity')),
    daily_limit_usd DOUBLE PRECISION,
    weekly_limit_usd DOUBLE PRECISION,
    monthly_limit_usd DOUBLE PRECISION,
    daily_usage_usd DOUBLE PRECISION NOT NULL DEFAULT 0,
    weekly_usage_usd DOUBLE PRECISION NOT NULL DEFAULT 0,
    monthly_usage_usd DOUBLE PRECISION NOT NULL DEFAULT 0,
    daily_window_start_text TEXT,
    weekly_window_start_text TEXT,
    monthly_window_start_text TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    deleted_at TIMESTAMPTZ
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_backend_next_user_platform_quotas_active
    ON user_platform_quotas (user_id, platform)
    WHERE deleted_at IS NULL;
CREATE INDEX IF NOT EXISTS idx_backend_next_user_platform_quotas_user_id
    ON user_platform_quotas(user_id);

-- 用户在某个 group 下的个性化费率/请求频率覆盖。
CREATE TABLE IF NOT EXISTS user_group_rates (
    user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    group_id BIGINT NOT NULL,
    rate_multiplier DOUBLE PRECISION,
    rpm_override INTEGER,
    updated_at_text TEXT NOT NULL DEFAULT '',
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (user_id, group_id)
);

CREATE INDEX IF NOT EXISTS idx_backend_next_user_group_rates_user_id
    ON user_group_rates(user_id);
CREATE INDEX IF NOT EXISTS idx_backend_next_user_group_rates_group_id
    ON user_group_rates(group_id);

-- 用户属性值。用于承接后台自定义属性、标签和后续扩展条件。
CREATE TABLE IF NOT EXISTS user_attribute_values (
    user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    attribute_id BIGINT NOT NULL,
    value TEXT NOT NULL,
    created_at_text TEXT NOT NULL DEFAULT '',
    updated_at_text TEXT NOT NULL DEFAULT '',
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (user_id, attribute_id)
);

CREATE INDEX IF NOT EXISTS idx_backend_next_user_attribute_values_user_id
    ON user_attribute_values(user_id);
CREATE INDEX IF NOT EXISTS idx_backend_next_user_attribute_values_attribute_id
    ON user_attribute_values(attribute_id);

-- 上游账号或模型探活历史。用于后台展示通道健康情况和延迟趋势。
CREATE TABLE IF NOT EXISTS channel_monitor_history (
    id BIGSERIAL PRIMARY KEY,
    monitor_id BIGINT NOT NULL,
    model TEXT NOT NULL,
    status TEXT NOT NULL,
    latency_ms BIGINT,
    ping_latency_ms BIGINT,
    message TEXT NOT NULL DEFAULT '',
    checked_at_text TEXT NOT NULL DEFAULT '',
    metadata JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_backend_next_channel_monitor_history_monitor_id
    ON channel_monitor_history(monitor_id);
CREATE INDEX IF NOT EXISTS idx_backend_next_channel_monitor_history_checked_at
    ON channel_monitor_history(monitor_id, checked_at_text DESC, id DESC);

-- 内容安全审计日志。记录网关侧风控检查结果、命中分类、阈值快照和后续动作。
CREATE TABLE IF NOT EXISTS content_moderation_logs (
    id BIGSERIAL PRIMARY KEY,
    request_id TEXT NOT NULL DEFAULT '',
    user_id BIGINT REFERENCES users(id) ON DELETE SET NULL,
    user_email TEXT NOT NULL DEFAULT '',
    api_key_id BIGINT REFERENCES api_keys(id) ON DELETE SET NULL,
    api_key_name TEXT NOT NULL DEFAULT '',
    group_id BIGINT REFERENCES groups(id) ON DELETE SET NULL,
    group_name TEXT NOT NULL DEFAULT '',
    endpoint TEXT NOT NULL DEFAULT '',
    provider TEXT NOT NULL DEFAULT '',
    model TEXT NOT NULL DEFAULT '',
    mode TEXT NOT NULL DEFAULT '',
    action TEXT NOT NULL DEFAULT '',
    flagged BOOLEAN NOT NULL DEFAULT FALSE,
    highest_category TEXT NOT NULL DEFAULT '',
    highest_score DOUBLE PRECISION NOT NULL DEFAULT 0,
    category_scores JSONB NOT NULL DEFAULT '{}'::jsonb,
    threshold_snapshot JSONB NOT NULL DEFAULT '{}'::jsonb,
    input_excerpt TEXT NOT NULL DEFAULT '',
    upstream_latency_ms BIGINT,
    error TEXT NOT NULL DEFAULT '',
    violation_count BIGINT NOT NULL DEFAULT 0,
    auto_banned BOOLEAN NOT NULL DEFAULT FALSE,
    email_sent BOOLEAN NOT NULL DEFAULT FALSE,
    queue_delay_ms BIGINT,
    created_at_text TEXT NOT NULL DEFAULT '',
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_backend_next_content_moderation_logs_created_at_text
    ON content_moderation_logs(created_at_text DESC, id DESC);
CREATE INDEX IF NOT EXISTS idx_backend_next_content_moderation_logs_group_id
    ON content_moderation_logs(group_id);
CREATE INDEX IF NOT EXISTS idx_backend_next_content_moderation_logs_flagged
    ON content_moderation_logs(flagged);
CREATE INDEX IF NOT EXISTS idx_backend_next_content_moderation_logs_action
    ON content_moderation_logs(action);
CREATE INDEX IF NOT EXISTS idx_backend_next_content_moderation_logs_endpoint
    ON content_moderation_logs(endpoint);

-- 支付渠道实例。一个 provider 可以配置多个实例，支持排序、启停、限额和退款能力配置。
CREATE TABLE IF NOT EXISTS payment_provider_instances (
    id BIGSERIAL PRIMARY KEY,
    provider_key TEXT NOT NULL,
    name TEXT NOT NULL,
    config JSONB NOT NULL DEFAULT '{}'::jsonb,
    supported_types TEXT[] NOT NULL DEFAULT ARRAY[]::TEXT[],
    enabled BOOLEAN NOT NULL DEFAULT TRUE,
    payment_mode TEXT NOT NULL DEFAULT '',
    sort_order INT NOT NULL DEFAULT 0,
    limits JSONB NOT NULL DEFAULT '{}'::jsonb,
    refund_enabled BOOLEAN NOT NULL DEFAULT FALSE,
    allow_user_refund BOOLEAN NOT NULL DEFAULT FALSE,
    created_at_text TEXT NOT NULL DEFAULT '',
    updated_at_text TEXT NOT NULL DEFAULT '',
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_backend_next_payment_provider_instances_provider_key ON payment_provider_instances(provider_key);
CREATE INDEX IF NOT EXISTS idx_backend_next_payment_provider_instances_enabled ON payment_provider_instances(enabled);
CREATE INDEX IF NOT EXISTS idx_backend_next_payment_provider_instances_sort_order ON payment_provider_instances(sort_order);

-- 通用后台集合项。用于承接暂未拆成强类型表的管理配置，迁移过程中减少结构阻塞。
CREATE TABLE IF NOT EXISTS admin_collection_items (
    collection TEXT NOT NULL,
    id BIGINT NOT NULL,
    item JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (collection, id)
);

CREATE INDEX IF NOT EXISTS idx_backend_next_admin_collection_items_collection
    ON admin_collection_items(collection);

-- 系统设置。按 namespace/key 存 JSON 配置，适合全局开关、邮件配置、功能参数等低频变更数据。
CREATE TABLE IF NOT EXISTS system_settings (
    namespace TEXT NOT NULL,
    key TEXT NOT NULL,
    value JSONB NOT NULL DEFAULT '{}'::jsonb,
    updated_at_text TEXT NOT NULL DEFAULT '',
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (namespace, key)
);

CREATE INDEX IF NOT EXISTS idx_backend_next_system_settings_namespace
    ON system_settings(namespace);

-- 邮件任务队列。邮件发送状态进入 PostgreSQL，保证服务重启后任务仍可继续重试。
CREATE TABLE IF NOT EXISTS email_queue_tasks (
    id BIGSERIAL PRIMARY KEY,
    task_type TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'pending',
    payload JSONB NOT NULL DEFAULT '{}'::jsonb,
    attempts INT NOT NULL DEFAULT 0,
    max_attempts INT NOT NULL DEFAULT 1,
    last_error TEXT,
    created_at_text TEXT NOT NULL DEFAULT '',
    updated_at_text TEXT NOT NULL DEFAULT '',
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_backend_next_email_queue_tasks_status_id
    ON email_queue_tasks(status, id);

-- 分布式锁。多实例部署时用于保证定时任务、清理任务等同一时刻只有一个实例执行。
-- fencing_token 是单调递增的栅栏令牌，用来识别过期持锁者的迟到写入。
CREATE TABLE IF NOT EXISTS distributed_locks (
    name TEXT PRIMARY KEY,
    owner TEXT NOT NULL,
    fencing_token BIGINT NOT NULL DEFAULT 1,
    expires_at TIMESTAMPTZ NOT NULL,
    metadata JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_backend_next_distributed_locks_expires_at
    ON distributed_locks(expires_at);

-- 幂等任务。用 job_type + idempotency_key 去重，用 lease 字段支持多实例安全领取任务。
CREATE TABLE IF NOT EXISTS idempotent_jobs (
    id BIGSERIAL PRIMARY KEY,
    job_type TEXT NOT NULL,
    idempotency_key TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'pending',
    payload JSONB NOT NULL DEFAULT '{}'::jsonb,
    result JSONB,
    attempts INT NOT NULL DEFAULT 0,
    lease_owner TEXT,
    lease_expires_at TIMESTAMPTZ,
    last_error TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE(job_type, idempotency_key)
);

CREATE INDEX IF NOT EXISTS idx_backend_next_idempotent_jobs_claim
    ON idempotent_jobs(job_type, status, created_at, id);
CREATE INDEX IF NOT EXISTS idx_backend_next_idempotent_jobs_lease
    ON idempotent_jobs(lease_expires_at);

-- 账号并发槽位。每个正在使用上游账号的请求占一个槽，过期时间用于清理异常退出的请求。
CREATE TABLE IF NOT EXISTS account_concurrency_slots (
    account_id BIGINT NOT NULL,
    request_id TEXT NOT NULL,
    expires_at TIMESTAMPTZ NOT NULL,
    metadata JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY(account_id, request_id)
);

CREATE INDEX IF NOT EXISTS idx_backend_next_account_concurrency_slots_expires_at
    ON account_concurrency_slots(expires_at);

-- 共享限流计数器。count 用于请求次数，usage 用于金额/Token 等浮点用量，窗口由 scope + window_start 唯一确定。
CREATE TABLE IF NOT EXISTS rate_limit_counters (
    scope TEXT NOT NULL,
    window_start_unix BIGINT NOT NULL,
    window_seconds BIGINT NOT NULL,
    count BIGINT NOT NULL DEFAULT 0,
    usage DOUBLE PRECISION NOT NULL DEFAULT 0,
    expires_at TIMESTAMPTZ NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY(scope, window_start_unix)
);

-- 兼容早期限流表：补齐浮点用量字段。
ALTER TABLE IF EXISTS rate_limit_counters
    ADD COLUMN IF NOT EXISTS usage DOUBLE PRECISION NOT NULL DEFAULT 0;

CREATE INDEX IF NOT EXISTS idx_backend_next_rate_limit_counters_expires_at
    ON rate_limit_counters(expires_at);

-- 可售套餐。套餐绑定 group，购买后生成订阅并决定用户可用额度、有效期和展示信息。
CREATE TABLE IF NOT EXISTS payment_plans (
    id BIGSERIAL PRIMARY KEY,
    group_id BIGINT NOT NULL,
    name TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    price DOUBLE PRECISION NOT NULL,
    original_price DOUBLE PRECISION,
    validity_days INT NOT NULL,
    validity_unit TEXT NOT NULL DEFAULT 'day',
    features JSONB NOT NULL DEFAULT '[]'::jsonb,
    product_name TEXT NOT NULL DEFAULT '',
    for_sale BOOLEAN NOT NULL DEFAULT TRUE,
    sort_order INT NOT NULL DEFAULT 0,
    created_at_text TEXT NOT NULL DEFAULT '',
    updated_at_text TEXT NOT NULL DEFAULT '',
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_backend_next_payment_plans_group_id
    ON payment_plans(group_id);
CREATE INDEX IF NOT EXISTS idx_backend_next_payment_plans_for_sale
    ON payment_plans(for_sale);
CREATE INDEX IF NOT EXISTS idx_backend_next_payment_plans_sort_order
    ON payment_plans(sort_order);
