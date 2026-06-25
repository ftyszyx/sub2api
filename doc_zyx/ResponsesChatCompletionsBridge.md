# Responses 下游到 Chat Completions 上游兼容方案

## 目标

当下游客户端使用 OpenAI Responses 协议，而上游账号只支持 Chat Completions 协议时，中转站提供一层有状态兼容能力，让下游尽量无感使用。

典型场景：

- 下游：Codex / Cursor / 其他使用 `/v1/responses` 的客户端
- 中转站：sub2api
- 上游：DeepSeek / Kimi / GLM / Qwen 等只支持 `/v1/chat/completions` 的 OpenAI 兼容服务

核心目标：

- 下游 `/v1/chat/completions` 仍然直通上游 `/v1/chat/completions`，不增加缓存和转换成本
- 下游 `/v1/responses` 且上游只支持 Chat Completions 时，启用状态缓存和协议转换
- 支持 `previous_response_id` 续聊，将 Responses 状态展开成 Chat Completions `messages`
- 支持上下文压缩，避免缓存历史超过上游模型上下文窗口
- 模块化实现，避免把状态管理、压缩、协议转换和转发逻辑揉在一个服务函数里

非目标：

- 不完整模拟 OpenAI 原生 Responses 的所有 hosted tools
- 不还原 OpenAI 的加密 reasoning 状态
- 不保证所有 Responses WebSocket v2 事件语义都能映射到 Chat Completions
- 不对下游 Chat Completions 请求做历史缓存

## 当前基础

当前仓库已经有部分协议桥接能力：

- Chat Completions -> Responses 请求转换：`backend/internal/pkg/apicompat/chatcompletions_to_responses.go`
- Responses -> Chat Completions 请求转换：`backend/internal/pkg/apicompat/chatcompletions_responses_bridge.go`
- Chat Completions 返回 -> Responses 返回/流转换：`backend/internal/pkg/apicompat/chatcompletions_responses_bridge.go`
- `/responses` 走 Chat Completions fallback：`backend/internal/service/openai_gateway_responses_chat_fallback.go`
- Responses 能力判断：`backend/internal/pkg/openai_compat/upstream_capability.go`

当前缺口：

- 没有保存完整对话历史
- 没有将 `previous_response_id` 展开成上游需要的 `messages`
- 没有针对 Chat Completions 上游的上下文压缩

## 路由策略

建议按下游协议和上游能力分三路。

```text
下游 /v1/chat/completions + 上游 Chat Completions
=> 直通，不缓存，不压缩

下游 /v1/responses + 上游 Responses
=> 走原有 Responses 路径，让上游维护状态

下游 /v1/responses + 上游 Chat Completions
=> 进入有状态 fallback：缓存历史、展开 messages、必要时压缩
```

判断条件：

- 账号类型是 OpenAI APIKey
- `openai_responses_mode=force_chat_completions` 或探测结果表明上游不支持 Responses
- 请求入口是 `/v1/responses` 或兼容的 `/responses`
- 账号开启有状态 fallback

## 账号配置

建议先放在账号 `extra` 中，后续前端再做表单化。

```json
{
  "openai_responses_mode": "force_chat_completions",
  "openai_responses_chat_stateful": true,
  "openai_responses_chat_state_ttl_seconds": 3600,
  "openai_responses_chat_context_window_tokens": 64000,
  "openai_responses_chat_max_output_tokens": 4096,
  "openai_responses_chat_compaction": "summarize",
  "openai_responses_chat_compaction_model": "deepseek-v4-flash",
  "openai_responses_chat_keep_recent_turns": 8,
  "openai_responses_chat_max_state_bytes": 1048576
}
```

字段说明：

- `openai_responses_chat_stateful`：是否启用 Responses -> Chat Completions 有状态模拟
- `openai_responses_chat_state_ttl_seconds`：状态缓存 TTL
- `openai_responses_chat_context_window_tokens`：上游模型上下文窗口
- `openai_responses_chat_max_output_tokens`：为上游输出预留的 token
- `openai_responses_chat_compaction`：压缩策略，当前要求使用 `summarize`
- `openai_responses_chat_compaction_model`：用于压缩旧历史的上游 Chat Completions 模型
- `openai_responses_chat_keep_recent_turns`：压缩时保留最近轮数
- `openai_responses_chat_max_state_bytes`：单个会话状态最大存储体积

默认建议：

- 默认关闭 `openai_responses_chat_stateful`
- 开启后默认 `compaction=summarize`
- 未配置上下文窗口时使用保守默认值，例如 `32000`
- 如果触发压缩但 LLM 压缩失败，必须返回错误，不允许静默裁剪历史

## 状态缓存模型

需要新增一类缓存，区别于当前 `response_id -> account_id` 的路由粘连缓存。

建议缓存结构：

```json
{
  "version": 1,
  "response_id": "resp_xxx",
  "previous_response_id": "resp_prev",
  "account_id": 2,
  "group_id": 2,
  "user_id": 1,
  "model": "gpt-5.4",
  "upstream_model": "deepseek-v4-flash",
  "instructions": "...",
  "messages": [
    {"role": "system", "content": "..."},
    {"role": "user", "content": "..."},
    {"role": "assistant", "content": "..."}
  ],
  "pending_tool_calls": [],
  "summary_message": null,
  "token_estimate": 12345,
  "created_at": "2026-06-04T00:00:00+08:00",
  "updated_at": "2026-06-04T00:01:00+08:00"
}
```

存储建议：

- Redis 为主，支持多实例部署
- 本地内存只作为热缓存，可选
- key 示例：`openai:responses_chat_state:{group_id}:{response_id}`
- TTL 使用账号配置
- 存储前检查 `max_state_bytes`

隐私与安全：

- 缓存中会保存用户消息、模型输出、工具输出，属于敏感数据
- 必须明确 opt-in
- 日志中不要打印完整 messages
- 错误日志只打印 response_id hash、account_id、token 估算等摘要信息

## 模块拆分

建议新增独立模块，避免污染现有 gateway 主流程。

### 状态服务

建议文件：

- `backend/internal/service/openai_responses_chat_state.go`
- `backend/internal/repository/openai_responses_chat_state_cache.go`

职责：

- 根据 `previous_response_id` 读取历史状态
- 保存新 `response_id` 的完整状态
- 控制 TTL、最大体积、缓存 key
- 提供本地热缓存扩展点

接口草案：

```go
type ResponsesChatStateStore interface {
    Get(ctx context.Context, groupID int64, responseID string) (*ResponsesChatState, error)
    Set(ctx context.Context, state *ResponsesChatState, ttl time.Duration) error
    Delete(ctx context.Context, groupID int64, responseID string) error
}
```

### 状态构建器

建议文件：

- `backend/internal/service/openai_responses_chat_state_builder.go`

职责：

- 将当前 Responses 请求转换成 Chat Completions messages
- 合并历史 messages
- 处理 `instructions`
- 处理 `input_text`
- 处理 `function_call_output`
- 处理多模态输入的可兼容部分

接口草案：

```go
type ResponsesChatStateBuilder interface {
    Build(ctx context.Context, input ResponsesChatBuildInput) (*ResponsesChatBuildResult, error)
}
```

### 上下文预算与压缩

建议文件：

- `backend/internal/service/openai_responses_chat_context_budget.go`
- `backend/internal/service/openai_responses_chat_compactor.go`

职责：

- 估算 messages token 数
- 判断是否超过上游上下文窗口
- 按策略压缩历史
- 保留工具调用链完整性

接口草案：

```go
type ResponsesChatTokenEstimator interface {
    EstimateMessages(model string, messages []apicompat.ChatMessage) int
}

type ResponsesChatCompactor interface {
    Compact(ctx context.Context, input CompactInput) (*CompactResult, error)
}
```

压缩策略：

- `error`：超限时返回 `context_length_exceeded`，让下游主动压缩后重试
- `summarize`：用指定压缩模型总结早期历史，保留最近 N 轮原文

第一版必须实现：

- `summarize`

不允许使用 `truncate` 作为兜底。压缩失败、超时、压缩后仍超限时，返回明确错误。

### 协议桥接编排器

建议文件：

- `backend/internal/service/openai_responses_chat_stateful.go`

职责：

- 作为 `/responses` -> `/chat/completions` 有状态 fallback 的入口
- 调用状态服务读取历史
- 调用构建器生成 messages
- 调用压缩器控制上下文
- 调用现有上游 Chat Completions 转发逻辑
- 将 Chat Completions 返回转换为 Responses 返回
- 将新状态写入缓存

尽量复用当前：

- `forwardResponsesViaRawChatCompletions`
- `ResponsesToChatCompletionsRequest`
- `ChatCompletionsResponseToResponses`
- `ChatCompletionsChunkToResponsesEvents`
- `FinalizeChatCompletionsResponsesStream`

## 请求处理流程

非流式流程：

```text
1. 解析 Responses 请求
2. 判断是否启用 stateful fallback
3. 如果有 previous_response_id，读取历史 state
4. 将当前 input 转为 Chat messages
5. 合并历史 messages
6. 估算 token
7. 超限则压缩或返回 context_length_exceeded
8. 请求上游 /v1/chat/completions
9. 将上游响应转换成 Responses 响应
10. 生成新的 response_id
11. 保存新 state
12. 返回下游
```

流式流程：

```text
1. 前 1-7 步同非流式
2. 请求上游 /v1/chat/completions stream
3. 边读 Chat Completions SSE，边转换成 Responses SSE
4. 同时累积 assistant 文本和 tool_calls
5. stream 完成后保存新 state
6. 如果 stream 中断，不保存或只保存明确完成的状态
```

## 上下文压缩规则

压缩时必须优先保留：

- system / instructions
- 当前轮用户输入
- 最近 N 轮对话
- 未完成 tool_call
- tool_call 与对应 tool_result 成对记录

可以交给 LLM 总结压缩：

- 早期普通 user / assistant 对话
- 已完成且不影响当前任务的工具结果
- reasoning 内容
- 长日志、重复上下文、大段无关输出

`summarize` 策略建议：

```text
1. 将早期历史总结成一条 system/developer message
2. 保留最近 N 轮原文
3. 保存 summary_message 到 state
4. 后续请求继续复用 summary_message
5. 如果总结失败或总结后仍超限，返回 context_length_exceeded / compaction_failed
```

## 下游是否知道压缩

下游 Codex 可能会根据模型上下文长度主动压缩，但中转站不能完全依赖下游。

原因：

- 下游看到的模型可能是 `gpt-5.4`
- 实际上游可能映射到 `deepseek-v4-flash`
- 两者上下文窗口可能不同
- Responses 下游可能只传 `previous_response_id`，不知道中转站需要展开成完整 messages

建议：

- 中转站维护上游真实上下文窗口
- `error` 策略可让 Codex 有机会触发自己的压缩
- `summarize` 策略作为服务端压缩方式
- 不做静默 truncate，避免丢失关键上下文导致下游无感错误
- 日志记录压缩原因、压缩前后 token 估算、保留轮数，不记录正文

## 错误处理

建议错误类型：

- `previous_response_not_found`：缓存不存在或过期
- `context_length_exceeded`：压缩后仍超过上游上下文
- `state_too_large`：状态体积超过配置上限
- `unsupported_responses_tool`：请求包含无法映射的 hosted tool
- `tool_state_incomplete`：工具调用链无法安全恢复

对于 `previous_response_not_found`：

- 如果请求没有工具结果，可以降级为无历史首轮请求
- 如果请求包含 `function_call_output`，应返回错误，避免工具结果丢上下文

## 可观测性

日志字段建议：

- `stateful_fallback_enabled`
- `previous_response_id_hash`
- `new_response_id_hash`
- `account_id`
- `group_id`
- `model`
- `upstream_model`
- `messages_count_before`
- `messages_count_after`
- `token_estimate_before`
- `token_estimate_after`
- `compaction_mode`
- `compaction_applied`
- `state_cache_hit`

指标建议：

- state cache hit/miss
- compaction count
- compaction failure count
- context exceeded count
- state save failure count
- average state bytes
- fallback request latency

## 分阶段实现

### 第一阶段：最小可用

- 增加账号配置读取
- 增加 Redis state store
- 支持 `previous_response_id` 读取历史
- 支持普通文本 input 转 messages
- 支持流式和非流式 assistant 文本保存
- 支持 `summarize` 压缩
- `/chat/completions` 保持直通，不缓存

### 第二阶段：工具调用

- 保存 assistant tool_calls
- 支持 `function_call_output` 合并进 messages
- 保证 tool_call 和 tool_result 成对保留
- 完善流式 tool_call 状态保存

### 第三阶段：压缩增强

- 支持按模型配置 token estimator
- 支持按账号覆盖上下文窗口
- 支持压缩摘要复用

### 第四阶段：前端配置

- 账号设置页增加：
  - Responses 状态模拟
  - 状态 TTL
  - 上下文窗口
  - 压缩策略
  - 保留最近轮数
- 增加风险提示：
  - 会缓存对话内容
  - 不完整支持 hosted tools

## 测试计划

单元测试：

- Responses input -> Chat messages
- `previous_response_id` 命中/未命中
- state 保存和 TTL
- `summarize` 压缩保留 system、摘要和最近轮
- LLM 压缩失败时返回错误
- tool_call/tool_result 成对保留
- 超过上下文后返回 `context_length_exceeded`

集成测试：

- 下游 `/v1/chat/completions` + force_chat_completions 仍直通
- 下游 `/v1/responses` 首轮生成新 response_id 并保存 state
- 下游 `/v1/responses` 二轮使用 previous_response_id 续聊
- stream 响应完成后保存 assistant 内容
- stream 中断不保存不完整状态

回归测试：

- 现有 `apicompat` 转换测试
- 现有 raw Chat Completions fallback 测试
- 现有 OpenAI Responses 路径测试

## 维护原则

- 协议转换只放在 `apicompat`
- 状态缓存只放在 state store
- 压缩只放在 compactor
- gateway service 只负责编排
- 默认不开启有状态 fallback，避免影响现有账号
- 所有隐私敏感正文不得进入日志
- 所有新增配置都要有保守默认值
