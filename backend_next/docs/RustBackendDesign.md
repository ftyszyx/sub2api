# Rust 后端完整重写最终设计

## 1. 目标

本方案目标是用 Rust 完整重写现有 backend，而不是把现有 Go 代码逐文件翻译。

重写后的后端应明确表达：

- 下游协议是什么
- API key 绑定哪个 group
- group 的价格、套餐、权限和计费规则是什么
- group 内哪些账号支持当前下游协议
- 选中的账号使用什么上游 provider 和上游协议
- 是否需要协议转换、状态缓存、LLM 压缩和 provider 特性处理

核心设计结论：

```text
API key 仍绑定单个 group
group 是价格、套餐、权限和账号池边界
系统维护完整下游协议枚举
group 内账号声明 supported_downstream_protocols
请求只在 API key 绑定的 group 内筛选账号
账号决定 provider、上游协议和 bridge adapter
```

主转发链路：

```text
HTTP ingress
  -> auth
  -> downstream protocol resolve
  -> api key group resolve
  -> account protocol capability filter
  -> account select
  -> bridge select
  -> upstream call
  -> response adapt
  -> usage record
```

## 2. 当前问题

当前后端已经支持 OpenAI Responses、OpenAI Chat Completions、Anthropic Messages、Gemini、Antigravity 等多种协议和上游平台。

但现有结构更偏向：

```text
请求 URL -> 根据 group platform 选择 handler -> handler/service 内部判断转换方式
```

主要问题：

- 下游协议没有成为一等概念
- service 层同时处理账号选择、协议转换、SSE、计费、缓存、provider 差异，职责过重
- DeepSeek `reasoning_content`、Gemini thoughtSignature、Anthropic thinking 等 provider 差异容易散落
- Responses -> Chat Completions 这种有状态 bridge 与普通转发逻辑耦合

新架构需要从“按 group platform 进入不同 handler”演进为：

```text
请求 URL -> 下游协议 -> API key group -> group 内账号协议能力 -> adapter -> provider client
```

## 3. Rust 技术栈

推荐：

- Web: `axum`
- async runtime: `tokio`
- HTTP client: `reqwest`
- database: `sqlx` + PostgreSQL
- Redis: `redis` async API 或 `deadpool-redis`
- config: `serde` + `config`
- logging/tracing: `tracing` + `tracing-subscriber` + `tower-http`
- migration: `sqlx migrate` 或独立 migration runner

选择理由：

- `axum` 基于 `tower`，middleware 和 streaming 模型清晰
- `sqlx` 适合保留复杂 SQL，迁移现有查询成本低
- `reqwest` 对 SSE、代理、超时、自定义 header 支持成熟
- `tracing` 适合把 request_id、group_id、account_id、adapter 等字段贯穿整条链路

## 4. 代码整体要求

Rust 后端实现必须优先满足这些工程要求：

- 方便扩展：新增下游协议、上游协议、provider、bridge adapter 时，应主要通过新增模块和注册项完成，避免修改主 pipeline。
- 结构清晰：协议 DTO、领域模型、路由选择、provider client、bridge adapter、状态缓存、计费记录分层清楚，不互相穿透实现细节。
- 易于维护：核心类型使用强类型枚举和明确 trait，避免把重要能力长期塞进无结构的 JSON/map。
- 自动化测试覆盖：协议解析、group 内账号筛选、adapter registry、stateful bridge、provider 错误解析、计费边界都要有单元测试；关键 SSE 流程要有 golden tests。
- 主流程薄：gateway pipeline 只负责编排，不写 DeepSeek、Anthropic、Gemini 等 provider 特殊逻辑。
- 失败显式：不支持的协议、找不到可用账号、压缩失败、状态缺失、上游错误都返回明确错误类型，并写入结构化日志。
- 兼容迁移：保留现有 API key 单 group 绑定语义，新增字段应支持从旧数据平滑回填。

## 5. 目录结构

建议使用 workspace，将协议、网关、领域模型、仓储拆开：

```text
backend_rs/
  Cargo.toml

  crates/
    server/
      src/
        main.rs
        app.rs
        routes.rs

    gateway/
      src/
        lib.rs
        context.rs
        pipeline.rs
        error.rs
        ingress/
          mod.rs
          endpoint.rs
          protocol_resolver.rs
        routing/
          mod.rs
          group_resolver.rs
          account_protocol_filter.rs
          account_selector.rs
        bridge/
          mod.rs
          registry.rs
          adapter.rs
          responses_to_chat/
            mod.rs
            request.rs
            response.rs
            stream.rs
            stateful.rs
            compaction.rs
          chat_to_responses/
            mod.rs
          responses_to_anthropic/
            mod.rs
          anthropic_to_responses/
            mod.rs
          messages_to_gemini/
            mod.rs
        provider/
          mod.rs
          registry.rs
          client.rs
          openai/
            mod.rs
          deepseek/
            mod.rs
          anthropic/
            mod.rs
          gemini/
            mod.rs
          vertex/
            mod.rs
        state/
          mod.rs
          response_state.rs
          store.rs
          redis_store.rs
          compactor.rs
        billing/
          mod.rs
          usage.rs
          recorder.rs

    protocol/
      src/
        lib.rs
        downstream.rs
        openai/
          responses.rs
          chat.rs
          sse.rs
        anthropic/
          messages.rs
        gemini/
          generate_content.rs

    domain/
      src/
        api_key.rs
        group.rs
        account.rs
        account_protocol_capability.rs
        subscription.rs

    repository/
      src/
        lib.rs
        api_key_repo.rs
        group_repo.rs
        account_repo.rs
        usage_repo.rs
        subscription_repo.rs

    config/
      src/lib.rs

    observability/
      src/lib.rs

  migrations/
  docs/
```

职责边界：

- `protocol`: DTO、SSE event 类型、基础序列化
- `bridge`: 协议之间的 request/response/stream 转换
- `provider`: 上游 provider 差异、endpoint、header、错误解析
- `routing`: 解析 API key 绑定 group，并在 group 内筛选支持当前下游协议的账号
- `state`: Responses 模拟有状态时的缓存和压缩
- `billing`: 计费、用量记录、quota 扣减
- `pipeline`: 只编排，不写 DeepSeek/Anthropic/Gemini 特殊逻辑

## 6. 核心模型

### 5.1 下游协议枚举

系统维护完整下游协议类型列表。所有入口 URL 先归一成下游协议。

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DownstreamProtocol {
    OpenAiResponses,
    OpenAiChatCompletions,
    AnthropicMessages,
    GeminiGenerateContent,
    OpenAiEmbeddings,
    OpenAiImages,
}
```

示例映射：

```text
/responses                    -> OpenAiResponses
/v1/responses                 -> OpenAiResponses
/backend-api/codex/responses  -> OpenAiResponses
/v1/chat/completions          -> OpenAiChatCompletions
/v1/messages                  -> AnthropicMessages
/v1beta/models/*              -> GeminiGenerateContent
/v1/embeddings                -> OpenAiEmbeddings
/v1/images/*                  -> OpenAiImages
```

### 5.2 上游协议和 provider

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UpstreamProtocol {
    OpenAiResponses,
    OpenAiChatCompletions,
    AnthropicMessages,
    GeminiGenerateContent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Provider {
    OpenAi,
    DeepSeek,
    Anthropic,
    Gemini,
    Vertex,
    Antigravity,
}
```

### 5.3 GatewayContext

```rust
pub struct GatewayContext {
    pub request_id: String,
    pub user_id: i64,
    pub api_key_id: i64,
    pub group_id: i64,
    pub account_id: Option<i64>,

    pub downstream_protocol: DownstreamProtocol,
    pub upstream_protocol: Option<UpstreamProtocol>,
    pub provider: Option<Provider>,

    pub original_model: String,
    pub upstream_model: String,
    pub stream: bool,
    pub previous_response_id: Option<String>,
}
```

## 7. Group 内协议能力路由

API key 仍绑定单个 group。这个 group 是：

- API key 的授权边界
- 价格和计费规则边界
- 套餐和订阅边界
- 一组上游账号池

请求进来后，只在 API key 绑定的 group 内选择支持当前下游协议的账号。

```text
request path
  -> downstream protocol
  -> api_key.group_id
  -> group account pool
  -> filter accounts by supported_downstream_protocols
  -> selected account
  -> bridge adapter
```

Group 不强行代表某个固定 provider。账号决定：

- 上游 provider
- 上游协议
- 支持的下游协议
- 模型映射
- 是否启用 stateful bridge
- 是否保留 reasoning/thinking 相关字段

示例：

```text
group: codex-pro-plan
price: 按 Codex 套餐定价

accounts:
  - provider = anthropic
    upstream_protocol = anthropic_messages
    supported_downstream_protocols = [openai_responses]

  - provider = deepseek
    upstream_protocol = openai_chat_completions
    supported_downstream_protocols = [openai_responses, openai_chat_completions]

  - provider = openai
    upstream_protocol = openai_responses
    supported_downstream_protocols = [openai_responses, openai_chat_completions]
```

### 路由选择顺序

```text
1. 根据 URL path 解析 downstream_protocol
2. 认证 API key，得到 api_key.group_id
3. 校验 group 存在、启用、用户订阅和余额
4. 查询该 group 内可调度账号
5. 过滤 supported_downstream_protocols 包含当前 downstream_protocol 的账号
6. 根据模型映射、模型支持、限流、优先级、权重筛选账号
7. 根据 account.upstream_protocol 选择 bridge adapter
8. 转发到上游
9. 按 api_key.group_id 的价格规则计费
```

## 8. 数据模型

保留 API key 单 group 绑定：

```sql
ALTER TABLE api_keys
    ADD COLUMN group_id BIGINT REFERENCES groups(id) ON DELETE SET NULL;
```

账号协议能力建议放在 `account_groups` 上，作为账号在某个 group 内的最终生效配置：

```sql
ALTER TABLE account_groups
    ADD COLUMN supported_downstream_protocols TEXT[] NOT NULL DEFAULT '{}',
    ADD COLUMN upstream_protocol_override TEXT,
    ADD COLUMN model_mapping JSONB NOT NULL DEFAULT '{}',
    ADD COLUMN priority INT NOT NULL DEFAULT 50;
```

如果一个账号在所有 group 中能力一致，也可以在 `accounts` 上保存默认能力：

```sql
ALTER TABLE accounts
    ADD COLUMN upstream_protocol TEXT NOT NULL DEFAULT 'openai_responses',
    ADD COLUMN supported_downstream_protocols TEXT[] NOT NULL DEFAULT '{}';
```

推荐规则：

- `account_groups.supported_downstream_protocols` 优先生效
- 如果为空，回退到 `accounts.supported_downstream_protocols`
- 如果仍为空，按 provider/upstream_protocol 默认能力回填
- `usage_logs` 继续记录最终命中的 `group_id`、`account_id`、`downstream_protocol`、`upstream_protocol`
- billing 和 subscription 只按 API key 绑定的 group 结算

## 9. Bridge Adapter

协议转换通过 adapter registry 选择。

```rust
#[async_trait::async_trait]
pub trait BridgeAdapter: Send + Sync {
    fn name(&self) -> &'static str;
    fn downstream(&self) -> DownstreamProtocol;
    fn upstream(&self) -> UpstreamProtocol;

    async fn convert_request(
        &self,
        ctx: &GatewayContext,
        body: bytes::Bytes,
    ) -> anyhow::Result<UpstreamRequest>;

    async fn convert_response(
        &self,
        ctx: &GatewayContext,
        response: UpstreamResponse,
    ) -> anyhow::Result<DownstreamResponse>;

    async fn convert_stream(
        &self,
        ctx: &GatewayContext,
        stream: UpstreamStream,
    ) -> anyhow::Result<DownstreamStream>;
}
```

有状态 bridge 单独扩展：

```rust
#[async_trait::async_trait]
pub trait StatefulBridgeAdapter: BridgeAdapter {
    async fn build_stateful_request(
        &self,
        ctx: &GatewayContext,
        body: bytes::Bytes,
    ) -> anyhow::Result<StatefulUpstreamRequest>;

    async fn finalize_state(
        &self,
        ctx: &GatewayContext,
        result: StateBuildResult,
        output: BridgeOutput,
    ) -> anyhow::Result<()>;
}
```

协议矩阵：

| 下游协议 | 上游协议 | 是否需要 bridge | 是否需要状态 | 典型场景 |
| --- | --- | --- | --- | --- |
| Responses | Responses | 否 | 上游负责 | OpenAI 原生 Codex |
| Responses | Chat Completions | 是 | 是 | Codex -> DeepSeek |
| Chat Completions | Chat Completions | 否 | 否 | 普通 OpenAI-compatible 客户端 |
| Chat Completions | Responses | 是 | 否 | 老客户端 -> OpenAI Responses |
| Anthropic Messages | Anthropic Messages | 否 | 否 | Claude 兼容 |
| Anthropic Messages | Gemini | 是 | 视 provider 而定 | Claude 客户端 -> Gemini |
| Responses | Anthropic Messages | 是 | 可能需要 | Codex -> Claude |
| Chat Completions | Anthropic Messages | 是 | 否 | Chat 客户端 -> Claude |

## 10. Responses -> Chat Completions

这是 DeepSeek / 传统 OpenAI-compatible 上游的关键链路。

非流式：

```text
OpenAI Responses request
  -> 读取 previous_response_id
  -> 从 state store 恢复历史
  -> Responses input 转 Chat messages
  -> 拼接历史 messages
  -> 检查上下文长度
  -> 必要时调用 LLM summarization 压缩
  -> 发送 Chat Completions 请求
  -> Chat response 转 Responses response
  -> 保存新的 response_id 状态
```

流式：

```text
Chat Completions SSE chunk
  -> Responses response.created
  -> Responses response.in_progress
  -> Responses response.output_item.added
  -> Responses response.content_part.added
  -> Responses response.output_text.delta
  -> Responses response.output_text.done
  -> Responses response.content_part.done
  -> Responses response.output_item.done
  -> Responses response.completed
```

流式结束后，根据累积状态保存 assistant message。

对 DeepSeek 这类 provider，需要保存 assistant 的 `reasoning_content`，否则下一轮可能因为 thinking mode 历史不完整被上游拒绝。

## 11. 状态缓存和压缩

Responses -> Chat 的状态层独立为 store：

```rust
#[async_trait::async_trait]
pub trait ResponseStateStore: Send + Sync {
    async fn get(&self, group_id: i64, response_id: &str) -> anyhow::Result<Option<ResponseState>>;
    async fn set(&self, group_id: i64, response_id: &str, state: ResponseState, ttl: Duration) -> anyhow::Result<()>;
    async fn delete(&self, group_id: i64, response_id: &str) -> anyhow::Result<()>;
}
```

压缩器：

```rust
#[async_trait::async_trait]
pub trait ConversationCompactor: Send + Sync {
    async fn summarize(
        &self,
        ctx: &GatewayContext,
        messages: Vec<ChatMessage>,
    ) -> anyhow::Result<ChatMessage>;
}
```

压缩规则：

- 只能 LLM summarize
- summarize 失败直接返回错误
- 不做 truncate fallback
- 压缩后的 token 估算仍超限，也返回错误
- 失败时不继续请求上游，不写入不完整 state

## 12. Provider Client

provider client 负责上游差异，不负责协议转换。

```rust
#[async_trait::async_trait]
pub trait ProviderClient: Send + Sync {
    fn provider(&self) -> Provider;
    fn protocols(&self) -> &'static [UpstreamProtocol];

    async fn call(
        &self,
        ctx: &GatewayContext,
        request: UpstreamRequest,
    ) -> anyhow::Result<UpstreamResponse>;

    async fn stream(
        &self,
        ctx: &GatewayContext,
        request: UpstreamRequest,
    ) -> anyhow::Result<UpstreamStream>;

    fn parse_error(&self, status: u16, body: &[u8]) -> GatewayError;
}
```

provider 模块负责：

- endpoint 构造
- header 构造
- auth
- proxy/TLS
- provider 错误格式解析
- provider 特殊字段保留或剔除

## 13. Pipeline 伪代码

```rust
pub async fn handle_gateway_request(
    state: AppState,
    request: axum::http::Request<axum::body::Body>,
) -> Result<Response, GatewayError> {
    let mut ctx = state.ingress.resolve_context(&request).await?;

    let api_key = state.auth.authenticate(&request).await?;
    ctx.user_id = api_key.user_id;
    ctx.api_key_id = api_key.id;

    let group = state.routing.resolve_api_key_group(&ctx, &api_key).await?;
    ctx.group_id = group.id;

    let account = state.accounts.select_by_downstream_protocol(&ctx).await?;
    ctx.account_id = Some(account.id);
    ctx.provider = Some(account.provider);
    ctx.upstream_protocol = Some(account.upstream_protocol);
    ctx.upstream_model = account.resolve_model(&ctx.original_model);

    let adapter = state.bridges.resolve(ctx.downstream_protocol, account.upstream_protocol)?;
    let provider = state.providers.resolve(account.provider)?;

    let body = read_body(request).await?;
    let upstream_req = adapter.convert_request(&ctx, body).await?;
    let upstream_resp = provider.call(&ctx, upstream_req).await?;
    let downstream_resp = adapter.convert_response(&ctx, upstream_resp).await?;

    state.billing.record(&ctx, &downstream_resp.usage).await?;
    Ok(downstream_resp.into_http_response())
}
```

## 14. 日志字段

每条请求统一输出：

```text
request_id
user_id
api_key_id
group_id
account_id
downstream_protocol
upstream_protocol
provider
adapter
model
upstream_model
stream
previous_response_id_present
stateful_enabled
compacted
status_code
latency_ms
```

这样排查问题时，不需要猜当前请求走了哪条转换链路。

## 15. 完整重写阶段

目标是完整替换 Go backend。分阶段只是工程执行方式，不是长期维护两套后端。

### 阶段 A：基础设施和领域模型

- axum server
- config
- database pool
- Redis pool
- tracing
- error model
- migration runner
- repository traits
- domain models

### 阶段 B：认证、用户、API key、group、account

- 用户登录 / 注册
- API key 创建、更新、删除
- group 管理
- account 管理
- account-group 绑定
- account supported downstream protocols
- subscription 基础校验

### 阶段 C：Gateway 主链路

第一条验收链路：

```text
OpenAI Responses downstream
  -> API key group
  -> DeepSeek account selected by supported_downstream_protocols
  -> Chat Completions upstream
  -> stateful cache
  -> LLM compaction
  -> reasoning_content preserve
```

然后补齐：

1. Chat Completions passthrough
2. Chat Completions -> Responses
3. Responses -> Responses passthrough
4. Anthropic Messages
5. Gemini
6. embeddings / images
7. WebSocket / Codex ingress

### 阶段 D：计费、用量、风控

- usage log
- quota 扣费
- rate limit
- user subscription
- group/user rate override
- content moderation
- ops error logging
- dashboard 聚合

### 阶段 E：支付和后台任务

- payment provider
- redeem code
- scheduled test
- account refresh / OAuth
- scheduler snapshot
- backup / restore
- system settings

### 阶段 E2：PostgreSQL 一致性层

第一版运行时一致性以 PostgreSQL 为事实来源，先保证正确性和可维护性，再按压力把高频能力替换为 Redis。

需要沉淀成独立 repository trait，而不是散落在业务 handler 中：

- `distributed_locks`：带 TTL 和 fencing token 的租约锁，用于备份调度、恢复、系统更新、全局单例任务。
- `idempotent_jobs`：用 `job_type + idempotency_key` 保证调度窗口和外部重试只创建一条任务；worker 领取时使用 lease 字段和 `FOR UPDATE SKIP LOCKED`。
- `account_concurrency_slots`：用 `account_id + request_id` 表示正在占用的上游账号槽位，过期自动释放，保证多个后端实例合计不超过账号并发上限。
- `rate_limit_counters`：固定窗口计数，适合第一版 API key、用户、账号、group 的短期限流；高 QPS 后可替换为 Redis 原子计数。

设计原则：

- 数据库是最终事实来源，唯一索引负责“只创建一次”，条件更新/行锁负责“只领取一次”。
- 普通 HTTP 转发尽量保持无状态；只有账号并发、后台任务、调度、扣费、限流这类全局一致性能力进 repository。
- 内存实现只用于 demo 和单进程测试；生产 PostgreSQL 实现通过同一个 trait 提供共享一致性。
- 如果 PostgreSQL 一致性写入失败，不能默默放行破坏限制，应返回明确错误或保守拒绝。

### 阶段 F：全量替换

- Rust 后端可以独立启动
- Rust 后端提供原 Go 后端所有必要 API
- 前端切到 Rust 后端
- Go 后端只保留为临时回滚方案
- 稳定后删除 Go 后端运行依赖

## 16. 测试策略

### Adapter 单元测试

每个 bridge 覆盖：

- request 转换
- non-stream response 转换
- stream event 转换
- tool call
- reasoning / thinking
- usage
- error mapping

### Golden SSE 测试

Codex 对 Responses SSE 事件顺序敏感，需要固定 golden 文件：

- event type 顺序
- response id 格式
- output item id 格式
- content part lifecycle
- completed event
- error event

### Stateful 多轮测试

Responses -> Chat Completions 必须覆盖：

- 第一轮无 previous_response_id
- 第二轮带 previous_response_id
- previous_response_id 过期
- 历史包含 reasoning_content
- 历史包含 tool calls
- 历史超过上下文后触发 LLM 压缩
- LLM 压缩失败直接报错
- 压缩后仍超过上下文直接报错

### Provider contract 测试

每个 provider 覆盖：

- endpoint 构造
- header 构造
- 错误解析
- 特殊字段保留
- 不支持字段剔除

### 完整验收

- 同一个 API key 绑定一个 group，但可在 group 内按下游协议选择不同上游账号
- `/responses` 下游可以走 DeepSeek `/chat/completions`
- 支持 `previous_response_id`
- 支持 Redis 状态缓存
- 支持 LLM summarize 压缩
- DeepSeek `reasoning_content` 能保存并回传
- SSE event 顺序兼容 Codex
- 现有前端主要页面可以接入 Rust API
- 用户、API key、group、account、subscription、settings 均可管理
- 用量统计、dashboard、日志查询可用
- 支付相关功能可用

## 17. 结论

最终架构核心：

```text
Path -> DownstreamProtocol
API Key -> Group
Group + DownstreamProtocol -> Account
Account -> Provider + UpstreamProtocol
DownstreamProtocol + UpstreamProtocol -> BridgeAdapter
```

这保留了 group 的业务语义：价格、套餐、权限和计费边界。

同时通过 `supported_downstream_protocols` 让同一个 group 可以服务多种客户端协议，例如 Codex、Chat Completions、Claude Messages。账号决定实际上游类型和适配方式，gateway 主流程只负责清晰编排。
