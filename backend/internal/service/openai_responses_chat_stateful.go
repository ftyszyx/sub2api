package service

import (
	"bytes"
	"context"
	"crypto/rand"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/http"
	"strings"
	"time"

	"github.com/Wei-Shaw/sub2api/internal/pkg/apicompat"
	"github.com/Wei-Shaw/sub2api/internal/pkg/logger"
	"github.com/gin-gonic/gin"
	"go.uber.org/zap"
)

const (
	extraKeyOpenAIResponsesChatStateful            = "openai_responses_chat_stateful"
	extraKeyOpenAIResponsesChatStateTTLSeconds     = "openai_responses_chat_state_ttl_seconds"
	extraKeyOpenAIResponsesChatContextWindowTokens = "openai_responses_chat_context_window_tokens"
	extraKeyOpenAIResponsesChatMaxOutputTokens     = "openai_responses_chat_max_output_tokens"
	extraKeyOpenAIResponsesChatCompaction          = "openai_responses_chat_compaction"
	extraKeyOpenAIResponsesChatCompactionModel     = "openai_responses_chat_compaction_model"
	extraKeyOpenAIResponsesChatKeepRecentTurns     = "openai_responses_chat_keep_recent_turns"
	extraKeyOpenAIResponsesChatMaxStateBytes       = "openai_responses_chat_max_state_bytes"
)

const (
	defaultResponsesChatStateTTLSeconds     = 3600
	defaultResponsesChatContextWindowTokens = 32000
	defaultResponsesChatMaxOutputTokens     = 4096
	defaultResponsesChatKeepRecentTurns     = 8
	defaultResponsesChatMaxStateBytes       = 1 << 20
	defaultResponsesChatCompactionMode      = "summarize"
)

var (
	errResponsesChatPreviousNotFound = errors.New("previous_response_not_found")
	errResponsesChatCompactionFailed = errors.New("responses_chat_compaction_failed")
	errResponsesChatContextExceeded  = errors.New("responses_chat_context_length_exceeded")
	errResponsesChatStateTooLarge    = errors.New("responses_chat_state_too_large")
	errResponsesChatStateUnavailable = errors.New("responses_chat_state_unavailable")
)

// ErrResponsesChatPreviousNotFound is returned by GatewayCache when a cached
// Responses-to-Chat state entry is missing or expired.
var ErrResponsesChatPreviousNotFound = errResponsesChatPreviousNotFound

type responsesChatStatefulConfig struct {
	Enabled             bool
	TTL                 time.Duration
	ContextWindowTokens int
	MaxOutputTokens     int
	CompactionMode      string
	CompactionModel     string
	KeepRecentTurns     int
	MaxStateBytes       int
}

type responsesChatState struct {
	Version            int                     `json:"version"`
	ResponseID         string                  `json:"response_id"`
	PreviousResponseID string                  `json:"previous_response_id,omitempty"`
	AccountID          int64                   `json:"account_id"`
	GroupID            int64                   `json:"group_id"`
	UserID             int64                   `json:"user_id,omitempty"`
	Model              string                  `json:"model"`
	UpstreamModel      string                  `json:"upstream_model"`
	Messages           []apicompat.ChatMessage `json:"messages"`
	TokenEstimate      int                     `json:"token_estimate,omitempty"`
	CreatedAt          time.Time               `json:"created_at"`
	UpdatedAt          time.Time               `json:"updated_at"`
}

type responsesChatBuildResult struct {
	ChatRequest    *apicompat.ChatCompletionsRequest
	Messages       []apicompat.ChatMessage
	History        *responsesChatState
	TokenEstimate  int
	Compacted      bool
	NewResponseID  string
	PreviousRespID string
}

type responsesChatFinalizeInput struct {
	BuildResult   *responsesChatBuildResult
	ResponseID    string
	Assistant     *apicompat.ChatMessage
	TokenEstimate int
	OriginalModel string
	UpstreamModel string
	BillingModel  string
}

func resolveResponsesChatStatefulConfig(account *Account) responsesChatStatefulConfig {
	cfg := responsesChatStatefulConfig{
		Enabled:             false,
		TTL:                 time.Duration(defaultResponsesChatStateTTLSeconds) * time.Second,
		ContextWindowTokens: defaultResponsesChatContextWindowTokens,
		MaxOutputTokens:     defaultResponsesChatMaxOutputTokens,
		CompactionMode:      defaultResponsesChatCompactionMode,
		KeepRecentTurns:     defaultResponsesChatKeepRecentTurns,
		MaxStateBytes:       defaultResponsesChatMaxStateBytes,
	}
	if account == nil {
		return cfg
	}
	cfg.Enabled = resolveAccountExtraBool(account.Extra, extraKeyOpenAIResponsesChatStateful)
	if seconds, ok := resolveAccountExtraNumber(account.Extra, extraKeyOpenAIResponsesChatStateTTLSeconds); ok && seconds > 0 {
		cfg.TTL = time.Duration(seconds) * time.Second
	}
	if tokens, ok := resolveAccountExtraNumber(account.Extra, extraKeyOpenAIResponsesChatContextWindowTokens); ok && tokens > 0 {
		cfg.ContextWindowTokens = int(tokens)
	}
	if tokens, ok := resolveAccountExtraNumber(account.Extra, extraKeyOpenAIResponsesChatMaxOutputTokens); ok && tokens > 0 {
		cfg.MaxOutputTokens = int(tokens)
	}
	if turns, ok := resolveAccountExtraNumber(account.Extra, extraKeyOpenAIResponsesChatKeepRecentTurns); ok && turns > 0 {
		cfg.KeepRecentTurns = int(turns)
	}
	if bytes, ok := resolveAccountExtraNumber(account.Extra, extraKeyOpenAIResponsesChatMaxStateBytes); ok && bytes > 0 {
		cfg.MaxStateBytes = int(bytes)
	}
	if mode := strings.TrimSpace(resolveAccountExtraString(account.Extra, extraKeyOpenAIResponsesChatCompaction)); mode != "" {
		cfg.CompactionMode = strings.ToLower(mode)
	}
	if model := strings.TrimSpace(resolveAccountExtraString(account.Extra, extraKeyOpenAIResponsesChatCompactionModel)); model != "" {
		cfg.CompactionModel = model
	}
	return cfg
}

func resolveAccountExtraString(extra map[string]any, key string) string {
	if len(extra) == 0 || strings.TrimSpace(key) == "" {
		return ""
	}
	raw, ok := extra[key]
	if !ok || raw == nil {
		return ""
	}
	switch v := raw.(type) {
	case string:
		return v
	case fmt.Stringer:
		return v.String()
	case json.Number:
		return v.String()
	default:
		return fmt.Sprint(v)
	}
}

func (s *OpenAIGatewayService) shouldUseResponsesChatStatefulFallback(account *Account) bool {
	cfg := resolveResponsesChatStatefulConfig(account)
	return cfg.Enabled
}

func (s *OpenAIGatewayService) buildStatefulResponsesChatRequest(
	ctx context.Context,
	c *gin.Context,
	account *Account,
	responsesReq *apicompat.ResponsesRequest,
	body []byte,
	originalModel string,
	upstreamModel string,
	billingModel string,
) (*responsesChatBuildResult, error) {
	cfg := resolveResponsesChatStatefulConfig(account)
	if !cfg.Enabled {
		return nil, nil
	}
	if cfg.CompactionMode != defaultResponsesChatCompactionMode {
		return nil, fmt.Errorf("%w: unsupported compaction mode %q", errResponsesChatCompactionFailed, cfg.CompactionMode)
	}
	if s.cache == nil {
		return nil, fmt.Errorf("%w: gateway cache is required", errResponsesChatStateUnavailable)
	}

	currentReq, err := apicompat.ResponsesToChatCompletionsRequest(responsesReq)
	if err != nil {
		return nil, err
	}
	currentMessages := cloneChatMessages(currentReq.Messages)
	previousResponseID := strings.TrimSpace(responsesReq.PreviousResponseID)

	var history *responsesChatState
	if previousResponseID != "" {
		history, err = s.getResponsesChatState(ctx, c, previousResponseID)
		if err != nil {
			return nil, err
		}
		if history == nil {
			return nil, fmt.Errorf("%w: %s", errResponsesChatPreviousNotFound, previousResponseID)
		}
	}

	messages := mergeResponsesChatMessages(history, currentMessages)
	estimate := estimateResponsesChatMessagesTokens(messages)
	limit := cfg.ContextWindowTokens - cfg.MaxOutputTokens
	if limit < 1024 {
		limit = cfg.ContextWindowTokens
	}

	compacted := false
	if estimate > limit {
		messages, estimate, err = s.compactResponsesChatMessages(ctx, c, account, cfg, messages, len(currentMessages), limit, upstreamModel)
		if err != nil {
			return nil, err
		}
		compacted = true
		if estimate > limit {
			return nil, fmt.Errorf("%w: compacted estimate %d exceeds limit %d", errResponsesChatContextExceeded, estimate, limit)
		}
	}

	currentReq.Model = upstreamModel
	currentReq.Messages = messages
	currentReq.Stream = responsesReq.Stream
	if responsesReq.Stream {
		currentReq.StreamOptions = &apicompat.ChatStreamOptions{IncludeUsage: true}
	}

	newID := generateResponsesChatStateResponseID()
	return &responsesChatBuildResult{
		ChatRequest:    currentReq,
		Messages:       messages,
		History:        history,
		TokenEstimate:  estimate,
		Compacted:      compacted,
		NewResponseID:  newID,
		PreviousRespID: previousResponseID,
	}, nil
}

func mergeResponsesChatMessages(history *responsesChatState, current []apicompat.ChatMessage) []apicompat.ChatMessage {
	if history == nil || len(history.Messages) == 0 {
		return cloneChatMessages(current)
	}
	merged := make([]apicompat.ChatMessage, 0, len(history.Messages)+len(current))
	merged = append(merged, cloneChatMessages(history.Messages)...)
	merged = append(merged, cloneChatMessages(current)...)
	return merged
}

func (s *OpenAIGatewayService) getResponsesChatState(ctx context.Context, c *gin.Context, responseID string) (*responsesChatState, error) {
	if s.cache == nil {
		return nil, fmt.Errorf("%w: gateway cache is required", errResponsesChatStateUnavailable)
	}
	groupID := currentOpenAIGroupID(c)
	data, err := s.cache.GetResponsesChatState(ctx, groupID, strings.TrimSpace(responseID))
	if err != nil {
		if errors.Is(err, errResponsesChatPreviousNotFound) {
			return nil, nil
		}
		return nil, fmt.Errorf("%w: %v", errResponsesChatStateUnavailable, err)
	}
	if len(data) == 0 {
		return nil, nil
	}
	var state responsesChatState
	if err := json.Unmarshal(data, &state); err != nil {
		return nil, fmt.Errorf("%w: parse responses chat state: %v", errResponsesChatStateUnavailable, err)
	}
	return &state, nil
}

func (s *OpenAIGatewayService) saveResponsesChatState(ctx context.Context, c *gin.Context, account *Account, cfg responsesChatStatefulConfig, state *responsesChatState) error {
	if s.cache == nil || state == nil || strings.TrimSpace(state.ResponseID) == "" {
		return nil
	}
	raw, err := json.Marshal(state)
	if err != nil {
		return err
	}
	if cfg.MaxStateBytes > 0 && len(raw) > cfg.MaxStateBytes {
		return fmt.Errorf("%w: %d > %d", errResponsesChatStateTooLarge, len(raw), cfg.MaxStateBytes)
	}
	if err := s.cache.SetResponsesChatState(ctx, currentOpenAIGroupID(c), state.ResponseID, raw, cfg.TTL); err != nil {
		return fmt.Errorf("%w: %v", errResponsesChatStateUnavailable, err)
	}
	return nil
}

func (s *OpenAIGatewayService) finalizeResponsesChatState(ctx context.Context, c *gin.Context, account *Account, input responsesChatFinalizeInput) error {
	if input.BuildResult == nil || input.Assistant == nil {
		return nil
	}
	cfg := resolveResponsesChatStatefulConfig(account)
	if !cfg.Enabled {
		return nil
	}
	messages := cloneChatMessages(input.BuildResult.Messages)
	messages = append(messages, cloneChatMessage(*input.Assistant))
	now := time.Now()
	createdAt := now
	if input.BuildResult.History != nil && !input.BuildResult.History.CreatedAt.IsZero() {
		createdAt = input.BuildResult.History.CreatedAt
	}
	state := &responsesChatState{
		Version:            1,
		ResponseID:         input.ResponseID,
		PreviousResponseID: input.BuildResult.PreviousRespID,
		AccountID:          account.ID,
		GroupID:            currentOpenAIGroupID(c),
		UserID:             currentOpenAIUserID(c),
		Model:              input.OriginalModel,
		UpstreamModel:      input.UpstreamModel,
		Messages:           messages,
		TokenEstimate:      estimateResponsesChatMessagesTokens(messages),
		CreatedAt:          createdAt,
		UpdatedAt:          now,
	}
	if input.TokenEstimate > 0 {
		state.TokenEstimate = input.TokenEstimate
	}
	if err := s.saveResponsesChatState(ctx, c, account, cfg, state); err != nil {
		logger.L().Warn("openai responses chat stateful: failed to save state",
			zap.Int64("account_id", account.ID),
			zap.String("response_id_hash", hashSensitiveValueForLog(input.ResponseID)),
			zap.Error(err),
		)
		return err
	}
	if store := s.getOpenAIWSStateStore(); store != nil {
		logOpenAIWSBindResponseAccountWarn(currentOpenAIGroupID(c), account.ID, input.ResponseID, store.BindResponseAccount(ctx, currentOpenAIGroupID(c), input.ResponseID, account.ID, cfg.TTL))
	}
	return nil
}

func (s *OpenAIGatewayService) compactResponsesChatMessages(
	ctx context.Context,
	c *gin.Context,
	account *Account,
	cfg responsesChatStatefulConfig,
	messages []apicompat.ChatMessage,
	currentMessageCount int,
	limit int,
	upstreamModel string,
) ([]apicompat.ChatMessage, int, error) {
	if len(messages) == 0 {
		return messages, 0, nil
	}
	if cfg.CompactionMode != defaultResponsesChatCompactionMode {
		return nil, 0, fmt.Errorf("%w: unsupported mode %q", errResponsesChatCompactionFailed, cfg.CompactionMode)
	}
	prefix, compactTarget, recent := splitMessagesForSummarization(messages, currentMessageCount, cfg.KeepRecentTurns)
	if len(compactTarget) == 0 {
		return nil, 0, fmt.Errorf("%w: nothing safe to summarize", errResponsesChatContextExceeded)
	}

	summary, err := s.summarizeResponsesChatMessages(ctx, c, account, cfg, compactTarget, upstreamModel)
	if err != nil {
		return nil, 0, fmt.Errorf("%w: %v", errResponsesChatCompactionFailed, err)
	}
	summary = strings.TrimSpace(summary)
	if summary == "" {
		return nil, 0, fmt.Errorf("%w: empty summary", errResponsesChatCompactionFailed)
	}

	summaryContent, _ := json.Marshal("Prior conversation summary:\n" + summary)
	compacted := make([]apicompat.ChatMessage, 0, len(prefix)+1+len(recent))
	compacted = append(compacted, prefix...)
	compacted = append(compacted, apicompat.ChatMessage{Role: "system", Content: summaryContent})
	compacted = append(compacted, recent...)
	estimate := estimateResponsesChatMessagesTokens(compacted)
	if estimate > limit {
		return nil, estimate, fmt.Errorf("%w: summary estimate %d exceeds limit %d", errResponsesChatContextExceeded, estimate, limit)
	}
	logger.L().Info("openai responses chat stateful: compacted history",
		zap.Int64("account_id", account.ID),
		zap.Int("messages_before", len(messages)),
		zap.Int("messages_after", len(compacted)),
		zap.Int("token_estimate_after", estimate),
	)
	return compacted, estimate, nil
}

func splitMessagesForSummarization(messages []apicompat.ChatMessage, currentMessageCount int, keepRecentTurns int) ([]apicompat.ChatMessage, []apicompat.ChatMessage, []apicompat.ChatMessage) {
	msgs := cloneChatMessages(messages)
	prefixEnd := 0
	for prefixEnd < len(msgs) && msgs[prefixEnd].Role == "system" {
		prefixEnd++
	}
	historyEnd := len(msgs)
	if currentMessageCount > 0 && currentMessageCount < len(msgs) {
		historyEnd = len(msgs) - currentMessageCount
	}
	if historyEnd < prefixEnd {
		historyEnd = prefixEnd
	}
	recentStart := findRecentMessagesStart(msgs[:historyEnd], prefixEnd, keepRecentTurns)
	if recentStart < prefixEnd {
		recentStart = prefixEnd
	}
	if recentStart == prefixEnd && historyEnd > prefixEnd {
		recentStart = historyEnd
	}
	recent := make([]apicompat.ChatMessage, 0, len(msgs)-recentStart)
	recent = append(recent, msgs[recentStart:historyEnd]...)
	recent = append(recent, msgs[historyEnd:]...)
	return msgs[:prefixEnd], msgs[prefixEnd:recentStart], recent
}

func findRecentMessagesStart(messages []apicompat.ChatMessage, minIndex int, keepRecentTurns int) int {
	if keepRecentTurns <= 0 {
		keepRecentTurns = defaultResponsesChatKeepRecentTurns
	}
	userCount := 0
	start := len(messages)
	for i := len(messages) - 1; i >= minIndex; i-- {
		start = i
		if messages[i].Role == "user" {
			userCount++
			if userCount >= keepRecentTurns {
				break
			}
		}
	}
	for start > minIndex && messages[start].Role == "tool" {
		start--
	}
	return start
}

func (s *OpenAIGatewayService) summarizeResponsesChatMessages(
	ctx context.Context,
	c *gin.Context,
	account *Account,
	cfg responsesChatStatefulConfig,
	messages []apicompat.ChatMessage,
	upstreamModel string,
) (string, error) {
	model := strings.TrimSpace(cfg.CompactionModel)
	if model == "" {
		model = upstreamModel
	}
	if model == "" {
		model = account.GetMappedModel("")
	}
	if model == "" {
		return "", errors.New("missing compaction model")
	}
	apiKey := account.GetOpenAIApiKey()
	if apiKey == "" {
		return "", fmt.Errorf("account %d missing api_key", account.ID)
	}
	baseURL := account.GetOpenAIBaseURL()
	if baseURL == "" {
		baseURL = "https://api.openai.com"
	}
	validatedURL, err := s.validateUpstreamBaseURL(baseURL)
	if err != nil {
		return "", err
	}
	targetURL := buildOpenAIChatCompletionsURL(validatedURL)

	historyJSON, err := json.Marshal(messages)
	if err != nil {
		return "", err
	}
	systemContent, _ := json.Marshal("Summarize the prior conversation for a coding agent. Preserve user requirements, decisions, files, tool results, unresolved tasks, and constraints. Do not invent facts. Return only the concise summary.")
	userContent, _ := json.Marshal("Conversation messages JSON:\n" + string(historyJSON))
	maxTokens := 1024
	reqBody, err := json.Marshal(apicompat.ChatCompletionsRequest{
		Model: model,
		Messages: []apicompat.ChatMessage{
			{Role: "system", Content: systemContent},
			{Role: "user", Content: userContent},
		},
		MaxCompletionTokens: &maxTokens,
		Stream:              false,
	})
	if err != nil {
		return "", err
	}

	upstreamCtx, release := detachUpstreamContext(ctx)
	req, err := http.NewRequestWithContext(upstreamCtx, http.MethodPost, targetURL, bytes.NewReader(reqBody))
	release()
	if err != nil {
		return "", err
	}
	req = req.WithContext(WithHTTPUpstreamProfile(req.Context(), HTTPUpstreamProfileOpenAI))
	req.Header.Set("Content-Type", "application/json")
	req.Header.Set("Accept", "application/json")
	req.Header.Set("Authorization", "Bearer "+apiKey)
	if customUA := account.GetOpenAIUserAgent(); customUA != "" {
		req.Header.Set("user-agent", customUA)
	}

	proxyURL := ""
	if account.Proxy != nil {
		proxyURL = account.Proxy.URL()
	}
	resp, err := s.httpUpstream.Do(req, proxyURL, account.ID, account.Concurrency)
	if err != nil {
		return "", err
	}
	defer func() { _ = resp.Body.Close() }()
	respBody, err := io.ReadAll(io.LimitReader(resp.Body, 2<<20))
	if err != nil {
		return "", err
	}
	if resp.StatusCode >= 400 {
		return "", fmt.Errorf("compaction upstream returned %d: %s", resp.StatusCode, sanitizeUpstreamErrorMessage(extractUpstreamErrorMessage(respBody)))
	}
	var ccResp apicompat.ChatCompletionsResponse
	if err := json.Unmarshal(respBody, &ccResp); err != nil {
		return "", err
	}
	if len(ccResp.Choices) == 0 {
		return "", errors.New("compaction response has no choices")
	}
	return chatMessagePlainText(ccResp.Choices[0].Message), nil
}

func assistantMessageFromChatCompletion(resp *apicompat.ChatCompletionsResponse) *apicompat.ChatMessage {
	if resp == nil || len(resp.Choices) == 0 {
		return nil
	}
	msg := cloneChatMessage(resp.Choices[0].Message)
	if strings.TrimSpace(msg.Role) == "" {
		msg.Role = "assistant"
	}
	return &msg
}

func assistantMessageFromResponsesStreamState(state *apicompat.ChatCompletionsToResponsesStreamState) *apicompat.ChatMessage {
	if state == nil {
		return nil
	}
	outputs := state.ChatOutput()
	if len(outputs) == 0 {
		return nil
	}
	var content strings.Builder
	var toolCalls []apicompat.ChatToolCall
	for _, out := range outputs {
		switch out.Type {
		case "message":
			for _, part := range out.Content {
				if part.Text != "" {
					content.WriteString(part.Text)
				}
			}
		case "function_call":
			toolCalls = append(toolCalls, apicompat.ChatToolCall{
				ID:   out.CallID,
				Type: "function",
				Function: apicompat.ChatFunctionCall{
					Name:      out.Name,
					Arguments: out.Arguments,
				},
			})
		}
	}
	rawContent, _ := json.Marshal(content.String())
	return &apicompat.ChatMessage{
		Role:             "assistant",
		Content:          rawContent,
		ReasoningContent: state.ReasoningText(),
		ToolCalls:        toolCalls,
	}
}

func estimateResponsesChatMessagesTokens(messages []apicompat.ChatMessage) int {
	total := 0
	for _, msg := range messages {
		total += 4
		total += len(msg.Role) / 4
		total += len(msg.Name) / 4
		total += len(msg.ToolCallID) / 4
		total += len(msg.Content) / 4
		total += len(msg.ReasoningContent) / 4
		if msg.FunctionCall != nil {
			total += len(msg.FunctionCall.Name)/4 + len(msg.FunctionCall.Arguments)/4
		}
		for _, call := range msg.ToolCalls {
			total += 8
			total += len(call.ID)/4 + len(call.Type)/4 + len(call.Function.Name)/4 + len(call.Function.Arguments)/4
		}
	}
	if total < len(messages)*4 {
		return len(messages) * 4
	}
	return total
}

func cloneChatMessages(messages []apicompat.ChatMessage) []apicompat.ChatMessage {
	if len(messages) == 0 {
		return nil
	}
	out := make([]apicompat.ChatMessage, len(messages))
	for i, msg := range messages {
		out[i] = cloneChatMessage(msg)
	}
	return out
}

func cloneChatMessage(msg apicompat.ChatMessage) apicompat.ChatMessage {
	out := msg
	if msg.Content != nil {
		out.Content = append(json.RawMessage(nil), msg.Content...)
	}
	if msg.ToolCalls != nil {
		out.ToolCalls = append([]apicompat.ChatToolCall(nil), msg.ToolCalls...)
	}
	if msg.FunctionCall != nil {
		cp := *msg.FunctionCall
		out.FunctionCall = &cp
	}
	return out
}

func chatMessagePlainText(msg apicompat.ChatMessage) string {
	raw := bytes.TrimSpace(msg.Content)
	if len(raw) == 0 || bytes.Equal(raw, []byte("null")) {
		return ""
	}
	var text string
	if err := json.Unmarshal(raw, &text); err == nil {
		return text
	}
	var parts []apicompat.ChatContentPart
	if err := json.Unmarshal(raw, &parts); err == nil {
		var texts []string
		for _, part := range parts {
			if part.Text != "" {
				texts = append(texts, part.Text)
			}
		}
		return strings.Join(texts, "\n\n")
	}
	return string(raw)
}

func currentOpenAIGroupID(c *gin.Context) int64 {
	if apiKey := getAPIKeyFromContext(c); apiKey != nil && apiKey.GroupID != nil {
		return *apiKey.GroupID
	}
	return 0
}

func currentOpenAIUserID(c *gin.Context) int64 {
	if apiKey := getAPIKeyFromContext(c); apiKey != nil {
		return apiKey.UserID
	}
	return 0
}

func generateResponsesChatStateResponseID() string {
	var b [12]byte
	if _, err := rand.Read(b[:]); err != nil {
		return fmt.Sprintf("resp_%d", time.Now().UnixNano())
	}
	return "resp_" + hex.EncodeToString(b[:])
}

func writeResponsesChatFallbackError(c *gin.Context, err error) {
	if c == nil || err == nil {
		return
	}
	status := http.StatusBadRequest
	errType := "invalid_request_error"
	message := err.Error()
	switch {
	case errors.Is(err, errResponsesChatPreviousNotFound):
		status = http.StatusBadRequest
		message = "previous_response_id was not found or has expired"
	case errors.Is(err, errResponsesChatCompactionFailed):
		status = http.StatusBadGateway
		errType = "api_error"
		message = "Failed to compact response history"
	case errors.Is(err, errResponsesChatContextExceeded):
		status = http.StatusBadRequest
		message = "Context length exceeded after compaction"
	case errors.Is(err, errResponsesChatStateTooLarge):
		status = http.StatusBadRequest
		message = "Response state is too large to cache"
	case errors.Is(err, errResponsesChatStateUnavailable):
		status = http.StatusBadGateway
		errType = "api_error"
		message = "Response state cache is unavailable"
	}
	c.JSON(status, gin.H{
		"error": gin.H{
			"type":    errType,
			"message": message,
		},
	})
}
