//go:build unit

package service

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
	"time"

	"github.com/Wei-Shaw/sub2api/internal/pkg/apicompat"
	"github.com/Wei-Shaw/sub2api/internal/pkg/openai_compat"
	"github.com/gin-gonic/gin"
	"github.com/stretchr/testify/require"
	"github.com/tidwall/gjson"
)

type responsesChatMemoryCache struct {
	stubGatewayCache
	responsesChatStateCacheStub
	states map[string][]byte
	getErr error
}

func (c *responsesChatMemoryCache) GetResponsesChatState(_ context.Context, groupID int64, responseID string) ([]byte, error) {
	if c.getErr != nil {
		return nil, c.getErr
	}
	if c.states == nil {
		return nil, errResponsesChatPreviousNotFound
	}
	raw, ok := c.states[responsesChatMemoryKey(groupID, responseID)]
	if !ok {
		return nil, errResponsesChatPreviousNotFound
	}
	return append([]byte(nil), raw...), nil
}

func (c *responsesChatMemoryCache) SetResponsesChatState(_ context.Context, groupID int64, responseID string, data []byte, _ time.Duration) error {
	if c.states == nil {
		c.states = make(map[string][]byte)
	}
	c.states[responsesChatMemoryKey(groupID, responseID)] = append([]byte(nil), data...)
	return nil
}

func (c *responsesChatMemoryCache) DeleteResponsesChatState(_ context.Context, groupID int64, responseID string) error {
	delete(c.states, responsesChatMemoryKey(groupID, responseID))
	return nil
}

func responsesChatMemoryKey(groupID int64, responseID string) string {
	return fmt.Sprintf("%d:%s", groupID, responseID)
}

func TestForwardResponses_ForceChatCompletionsRoutesNonStreamingToChatCompletions(t *testing.T) {
	gin.SetMode(gin.TestMode)

	body := []byte(`{"model":"gpt-5.4","input":"hello","stream":false}`)
	rec := httptest.NewRecorder()
	c, _ := gin.CreateTestContext(rec)
	c.Request = httptest.NewRequest(http.MethodPost, "/v1/responses", bytes.NewReader(body))
	c.Request.Header.Set("Content-Type", "application/json")

	upstream := &httpUpstreamRecorder{resp: &http.Response{
		StatusCode: http.StatusOK,
		Header:     http.Header{"Content-Type": []string{"application/json"}, "x-request-id": []string{"rid_resp_chat_json"}},
		Body: io.NopCloser(strings.NewReader(
			`{"id":"chatcmpl_json","object":"chat.completion","model":"gpt-5.4","choices":[{"index":0,"message":{"role":"assistant","content":"ok"},"finish_reason":"stop"}],"usage":{"prompt_tokens":3,"completion_tokens":2,"total_tokens":5,"prompt_tokens_details":{"cached_tokens":1}}}`,
		)),
	}}
	svc := &OpenAIGatewayService{
		cfg:          rawChatCompletionsTestConfig(),
		httpUpstream: upstream,
	}

	result, err := svc.Forward(context.Background(), c, forceChatResponsesFallbackAccount(), body)
	require.NoError(t, err)
	require.NotNil(t, result)
	require.Equal(t, "http://upstream.example/v1/chat/completions", upstream.lastReq.URL.String())
	require.Equal(t, HTTPUpstreamProfileOpenAI, HTTPUpstreamProfileFromContext(upstream.lastReq.Context()))
	require.Equal(t, "hello", gjson.GetBytes(upstream.lastBody, "messages.0.content").String())
	require.False(t, gjson.GetBytes(upstream.lastBody, "input").Exists())
	require.Equal(t, "response", gjson.Get(rec.Body.String(), "object").String())
	require.Equal(t, "ok", gjson.Get(rec.Body.String(), "output.0.content.0.text").String())
	require.Equal(t, 3, result.Usage.InputTokens)
	require.Equal(t, 2, result.Usage.OutputTokens)
	require.Equal(t, 1, result.Usage.CacheReadInputTokens)
	require.False(t, result.Stream)
}

func TestForwardResponses_ForceChatCompletionsRoutesStreamingToChatCompletions(t *testing.T) {
	gin.SetMode(gin.TestMode)

	body := []byte(`{"model":"gpt-5.4","input":"hello","stream":true}`)
	rec := httptest.NewRecorder()
	c, _ := gin.CreateTestContext(rec)
	c.Request = httptest.NewRequest(http.MethodPost, "/v1/responses", bytes.NewReader(body))
	c.Request.Header.Set("Content-Type", "application/json")

	upstreamBody := strings.Join([]string{
		`data: {"id":"chatcmpl_stream","object":"chat.completion.chunk","model":"gpt-5.4","choices":[{"index":0,"delta":{"role":"assistant"},"finish_reason":null}]}`,
		"",
		`data: {"id":"chatcmpl_stream","object":"chat.completion.chunk","model":"gpt-5.4","choices":[{"index":0,"delta":{"content":"he"},"finish_reason":null}]}`,
		"",
		`data: {"id":"chatcmpl_stream","object":"chat.completion.chunk","model":"gpt-5.4","choices":[{"index":0,"delta":{"content":"llo"},"finish_reason":null}]}`,
		"",
		`data: {"id":"chatcmpl_stream","object":"chat.completion.chunk","model":"gpt-5.4","choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}`,
		"",
		`data: {"id":"chatcmpl_stream","object":"chat.completion.chunk","model":"gpt-5.4","choices":[],"usage":{"prompt_tokens":4,"completion_tokens":3,"total_tokens":7}}`,
		"",
		"data: [DONE]",
		"",
	}, "\n")
	upstream := &httpUpstreamRecorder{resp: &http.Response{
		StatusCode: http.StatusOK,
		Header:     http.Header{"Content-Type": []string{"text/event-stream"}, "x-request-id": []string{"rid_resp_chat_stream"}},
		Body:       io.NopCloser(strings.NewReader(upstreamBody)),
	}}
	svc := &OpenAIGatewayService{
		cfg:          rawChatCompletionsTestConfig(),
		httpUpstream: upstream,
	}

	result, err := svc.Forward(context.Background(), c, forceChatResponsesFallbackAccount(), body)
	require.NoError(t, err)
	require.NotNil(t, result)
	require.Equal(t, "http://upstream.example/v1/chat/completions", upstream.lastReq.URL.String())
	require.True(t, gjson.GetBytes(upstream.lastBody, "stream_options.include_usage").Bool())
	require.Contains(t, rec.Body.String(), "event: response.output_text.delta")
	require.Contains(t, rec.Body.String(), `"delta":"he"`)
	require.Contains(t, rec.Body.String(), "event: response.completed")
	require.Contains(t, rec.Body.String(), `"input_tokens":4`)
	require.Contains(t, rec.Body.String(), "data: [DONE]")
	require.Equal(t, 4, result.Usage.InputTokens)
	require.Equal(t, 3, result.Usage.OutputTokens)
	require.True(t, result.Stream)
	require.NotNil(t, result.FirstTokenMs)
}

func TestForwardResponses_DeepSeekReasoningOnlyStreamProducesVisibleText(t *testing.T) {
	gin.SetMode(gin.TestMode)

	body := []byte(`{"model":"deepseek-reasoner","input":"hello","stream":true}`)
	rec := httptest.NewRecorder()
	c, _ := gin.CreateTestContext(rec)
	c.Request = httptest.NewRequest(http.MethodPost, "/v1/responses", bytes.NewReader(body))
	c.Request.Header.Set("Content-Type", "application/json")

	upstreamBody := strings.Join([]string{
		`data: {"id":"chatcmpl_reasoning","object":"chat.completion.chunk","model":"deepseek-reasoner","choices":[{"index":0,"delta":{"role":"assistant","content":null,"reasoning_content":""},"finish_reason":null}]}`,
		"",
		`data: {"id":"chatcmpl_reasoning","object":"chat.completion.chunk","model":"deepseek-reasoner","choices":[{"index":0,"delta":{"reasoning_content":"visible fallback"},"finish_reason":null}]}`,
		"",
		`data: {"id":"chatcmpl_reasoning","object":"chat.completion.chunk","model":"deepseek-reasoner","choices":[{"index":0,"delta":{"content":""},"finish_reason":"length"}],"usage":{"prompt_tokens":4,"completion_tokens":3,"total_tokens":7}}`,
		"",
		"data: [DONE]",
		"",
	}, "\n")
	upstream := &httpUpstreamRecorder{resp: &http.Response{
		StatusCode: http.StatusOK,
		Header:     http.Header{"Content-Type": []string{"text/event-stream"}, "x-request-id": []string{"rid_deepseek_reasoning_responses_stream"}},
		Body:       io.NopCloser(strings.NewReader(upstreamBody)),
	}}
	svc := &OpenAIGatewayService{
		cfg:          rawChatCompletionsTestConfig(),
		httpUpstream: upstream,
	}

	result, err := svc.Forward(context.Background(), c, forceChatResponsesFallbackAccount(), body)
	require.NoError(t, err)
	require.NotNil(t, result)
	require.True(t, result.Stream)
	require.Contains(t, rec.Body.String(), "event: response.output_text.delta")
	require.Contains(t, rec.Body.String(), `"delta":"visible fallback"`)
	require.Contains(t, rec.Body.String(), `"status":"incomplete"`)
	require.Contains(t, rec.Body.String(), "data: [DONE]")
}

func TestForwardResponses_AutoSupportedAccountStillUsesResponsesEndpoint(t *testing.T) {
	gin.SetMode(gin.TestMode)

	body := []byte(`{"model":"gpt-5.4","input":"hello","stream":false}`)
	rec := httptest.NewRecorder()
	c, _ := gin.CreateTestContext(rec)
	c.Request = httptest.NewRequest(http.MethodPost, "/v1/responses", bytes.NewReader(body))
	c.Request.Header.Set("Content-Type", "application/json")

	upstream := &httpUpstreamRecorder{resp: &http.Response{
		StatusCode: http.StatusOK,
		Header:     http.Header{"Content-Type": []string{"application/json"}, "x-request-id": []string{"rid_resp_native"}},
		Body: io.NopCloser(strings.NewReader(
			`{"id":"resp_native","object":"response","model":"gpt-5.4","status":"completed","output":[{"type":"message","role":"assistant","content":[{"type":"output_text","text":"ok"}],"status":"completed"}],"usage":{"input_tokens":5,"output_tokens":2,"total_tokens":7}}`,
		)),
	}}
	svc := &OpenAIGatewayService{
		cfg:          rawChatCompletionsTestConfig(),
		httpUpstream: upstream,
	}
	account := rawChatCompletionsTestAccount()
	account.Extra = map[string]any{
		openai_compat.ExtraKeyResponsesMode:      string(openai_compat.ResponsesSupportModeAuto),
		openai_compat.ExtraKeyResponsesSupported: true,
	}

	result, err := svc.Forward(context.Background(), c, account, body)
	require.NoError(t, err)
	require.NotNil(t, result)
	require.Equal(t, "http://upstream.example/v1/responses", upstream.lastReq.URL.String())
	require.True(t, gjson.GetBytes(upstream.lastBody, "input").Exists())
	require.False(t, gjson.GetBytes(upstream.lastBody, "messages").Exists())
	require.Equal(t, "ok", gjson.Get(rec.Body.String(), "output.0.content.0.text").String())
}

func TestForwardResponses_StatefulFallbackCachesAndContinuesConversation(t *testing.T) {
	gin.SetMode(gin.TestMode)

	cache := &responsesChatMemoryCache{}
	upstream := &httpUpstreamRecorder{responses: []*http.Response{
		{
			StatusCode: http.StatusOK,
			Header:     http.Header{"Content-Type": []string{"application/json"}},
			Body: io.NopCloser(strings.NewReader(
				`{"id":"chatcmpl_1","object":"chat.completion","model":"deepseek-v4-flash","choices":[{"index":0,"message":{"role":"assistant","content":"first answer"},"finish_reason":"stop"}]}`,
			)),
		},
		{
			StatusCode: http.StatusOK,
			Header:     http.Header{"Content-Type": []string{"application/json"}},
			Body: io.NopCloser(strings.NewReader(
				`{"id":"chatcmpl_2","object":"chat.completion","model":"deepseek-v4-flash","choices":[{"index":0,"message":{"role":"assistant","content":"second answer"},"finish_reason":"stop"}]}`,
			)),
		},
	}}
	svc := &OpenAIGatewayService{
		cfg:          rawChatCompletionsTestConfig(),
		httpUpstream: upstream,
		cache:        cache,
	}
	account := forceChatResponsesStatefulAccount()

	rec1 := httptest.NewRecorder()
	c1, _ := gin.CreateTestContext(rec1)
	body1 := []byte(`{"model":"gpt-5.4","input":"hello","stream":false}`)
	c1.Request = httptest.NewRequest(http.MethodPost, "/v1/responses", bytes.NewReader(body1))
	c1.Request.Header.Set("Content-Type", "application/json")
	result1, err := svc.Forward(context.Background(), c1, account, body1)
	require.NoError(t, err)
	require.NotNil(t, result1)
	respID := gjson.Get(rec1.Body.String(), "id").String()
	require.True(t, strings.HasPrefix(respID, "resp_"))
	require.Equal(t, "hello", gjson.GetBytes(upstream.bodies[0], "messages.0.content").String())
	require.Equal(t, "first answer", gjson.Get(rec1.Body.String(), "output.0.content.0.text").String())

	rec2 := httptest.NewRecorder()
	c2, _ := gin.CreateTestContext(rec2)
	body2 := []byte(`{"model":"gpt-5.4","previous_response_id":"` + respID + `","input":"continue","stream":false}`)
	c2.Request = httptest.NewRequest(http.MethodPost, "/v1/responses", bytes.NewReader(body2))
	c2.Request.Header.Set("Content-Type", "application/json")
	result2, err := svc.Forward(context.Background(), c2, account, body2)
	require.NoError(t, err)
	require.NotNil(t, result2)
	require.Len(t, upstream.bodies, 2)
	require.Equal(t, "hello", gjson.GetBytes(upstream.bodies[1], "messages.0.content").String())
	require.Equal(t, "first answer", gjson.GetBytes(upstream.bodies[1], "messages.1.content").String())
	require.Equal(t, "continue", gjson.GetBytes(upstream.bodies[1], "messages.2.content").String())
	require.Equal(t, "second answer", gjson.Get(rec2.Body.String(), "output.0.content.0.text").String())
}

func TestForwardResponses_StatefulFallbackPreservesStreamingReasoningContent(t *testing.T) {
	gin.SetMode(gin.TestMode)

	cache := &responsesChatMemoryCache{}
	upstreamBody1 := strings.Join([]string{
		`data: {"id":"chatcmpl_reasoning","object":"chat.completion.chunk","model":"deepseek-v4-pro","choices":[{"index":0,"delta":{"role":"assistant"},"finish_reason":null}]}`,
		"",
		`data: {"id":"chatcmpl_reasoning","object":"chat.completion.chunk","model":"deepseek-v4-pro","choices":[{"index":0,"delta":{"reasoning_content":"think first"},"finish_reason":null}]}`,
		"",
		`data: {"id":"chatcmpl_reasoning","object":"chat.completion.chunk","model":"deepseek-v4-pro","choices":[{"index":0,"delta":{"content":"first answer"},"finish_reason":null}]}`,
		"",
		`data: {"id":"chatcmpl_reasoning","object":"chat.completion.chunk","model":"deepseek-v4-pro","choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}`,
		"",
		"data: [DONE]",
		"",
	}, "\n")
	upstream := &httpUpstreamRecorder{responses: []*http.Response{
		{
			StatusCode: http.StatusOK,
			Header:     http.Header{"Content-Type": []string{"text/event-stream"}},
			Body:       io.NopCloser(strings.NewReader(upstreamBody1)),
		},
		{
			StatusCode: http.StatusOK,
			Header:     http.Header{"Content-Type": []string{"application/json"}},
			Body: io.NopCloser(strings.NewReader(
				`{"id":"chatcmpl_2","object":"chat.completion","model":"deepseek-v4-pro","choices":[{"index":0,"message":{"role":"assistant","content":"second answer"},"finish_reason":"stop"}]}`,
			)),
		},
	}}
	svc := &OpenAIGatewayService{
		cfg:          rawChatCompletionsTestConfig(),
		httpUpstream: upstream,
		cache:        cache,
	}
	account := forceChatResponsesStatefulAccount()

	rec1 := httptest.NewRecorder()
	c1, _ := gin.CreateTestContext(rec1)
	body1 := []byte(`{"model":"gpt-5.4","input":"hello","stream":true}`)
	c1.Request = httptest.NewRequest(http.MethodPost, "/v1/responses", bytes.NewReader(body1))
	c1.Request.Header.Set("Content-Type", "application/json")
	result1, err := svc.Forward(context.Background(), c1, account, body1)
	require.NoError(t, err)
	require.NotNil(t, result1)
	respID := extractResponseIDFromSSEBody(rec1.Body.String())
	require.True(t, strings.HasPrefix(respID, "resp_"))

	rec2 := httptest.NewRecorder()
	c2, _ := gin.CreateTestContext(rec2)
	body2 := []byte(`{"model":"gpt-5.4","previous_response_id":"` + respID + `","input":"continue","stream":false}`)
	c2.Request = httptest.NewRequest(http.MethodPost, "/v1/responses", bytes.NewReader(body2))
	c2.Request.Header.Set("Content-Type", "application/json")
	result2, err := svc.Forward(context.Background(), c2, account, body2)
	require.NoError(t, err)
	require.NotNil(t, result2)
	require.Len(t, upstream.bodies, 2)
	require.Equal(t, "hello", gjson.GetBytes(upstream.bodies[1], "messages.0.content").String())
	require.Equal(t, "think first", gjson.GetBytes(upstream.bodies[1], "messages.1.reasoning_content").String())
	require.Equal(t, "first answer", gjson.GetBytes(upstream.bodies[1], "messages.1.content").String())
	require.Equal(t, "continue", gjson.GetBytes(upstream.bodies[1], "messages.2.content").String())
}

func TestForwardResponses_StatefulFallbackUsesLLMCompactionWhenOverLimit(t *testing.T) {
	gin.SetMode(gin.TestMode)

	cache := &responsesChatMemoryCache{}
	upstream := &httpUpstreamRecorder{responses: []*http.Response{
		{
			StatusCode: http.StatusOK,
			Header:     http.Header{"Content-Type": []string{"application/json"}},
			Body: io.NopCloser(strings.NewReader(
				`{"id":"chatcmpl_compact","object":"chat.completion","model":"deepseek-v4-flash","choices":[{"index":0,"message":{"role":"assistant","content":"summary: user greeted and first answer was stored"},"finish_reason":"stop"}]}`,
			)),
		},
		{
			StatusCode: http.StatusOK,
			Header:     http.Header{"Content-Type": []string{"application/json"}},
			Body: io.NopCloser(strings.NewReader(
				`{"id":"chatcmpl_2","object":"chat.completion","model":"deepseek-v4-flash","choices":[{"index":0,"message":{"role":"assistant","content":"after compact"},"finish_reason":"stop"}]}`,
			)),
		},
	}}
	svc := &OpenAIGatewayService{
		cfg:          rawChatCompletionsTestConfig(),
		httpUpstream: upstream,
		cache:        cache,
	}
	account := forceChatResponsesStatefulAccount()
	account.Extra[extraKeyOpenAIResponsesChatContextWindowTokens] = 150
	account.Extra[extraKeyOpenAIResponsesChatMaxOutputTokens] = 32
	account.Extra[extraKeyOpenAIResponsesChatKeepRecentTurns] = 1
	account.Extra[extraKeyOpenAIResponsesChatCompactionModel] = "deepseek-v4-flash"

	longText := strings.Repeat("older context ", 80)
	state := &responsesChatState{
		Version:    1,
		ResponseID: "resp_prev",
		AccountID:  account.ID,
		GroupID:    0,
		Model:      "gpt-5.4",
		Messages: []apicompat.ChatMessage{
			{Role: "user", Content: mustJSONRawString(longText)},
			{Role: "assistant", Content: mustJSONRawString("first answer")},
		},
		CreatedAt: time.Now(),
		UpdatedAt: time.Now(),
	}
	rawState, err := json.Marshal(state)
	require.NoError(t, err)
	require.NoError(t, cache.SetResponsesChatState(context.Background(), 0, "resp_prev", rawState, time.Hour))

	rec := httptest.NewRecorder()
	c, _ := gin.CreateTestContext(rec)
	body := []byte(`{"model":"gpt-5.4","previous_response_id":"resp_prev","input":"continue","stream":false}`)
	c.Request = httptest.NewRequest(http.MethodPost, "/v1/responses", bytes.NewReader(body))
	c.Request.Header.Set("Content-Type", "application/json")
	result, err := svc.Forward(context.Background(), c, account, body)
	require.NoError(t, err)
	require.NotNil(t, result)
	require.Len(t, upstream.bodies, 2)
	require.Contains(t, gjson.GetBytes(upstream.bodies[0], "messages.1.content").String(), "older context")
	require.Equal(t, "summary: user greeted and first answer was stored", gjson.GetBytes(upstream.bodies[1], "messages.0.content").String()[len("Prior conversation summary:\n"):])
	require.Equal(t, "continue", gjson.GetBytes(upstream.bodies[1], "messages.1.content").String())
	require.Equal(t, "after compact", gjson.Get(rec.Body.String(), "output.0.content.0.text").String())
}

func TestForwardResponses_StatefulFallbackCompactionFailureReturnsError(t *testing.T) {
	gin.SetMode(gin.TestMode)

	cache := &responsesChatMemoryCache{}
	upstream := &httpUpstreamRecorder{responses: []*http.Response{
		{
			StatusCode: http.StatusBadGateway,
			Header:     http.Header{"Content-Type": []string{"application/json"}},
			Body:       io.NopCloser(strings.NewReader(`{"error":{"message":"compact failed"}}`)),
		},
	}}
	svc := &OpenAIGatewayService{
		cfg:          rawChatCompletionsTestConfig(),
		httpUpstream: upstream,
		cache:        cache,
	}
	account := forceChatResponsesStatefulAccount()
	account.Extra[extraKeyOpenAIResponsesChatContextWindowTokens] = 120
	account.Extra[extraKeyOpenAIResponsesChatMaxOutputTokens] = 32
	account.Extra[extraKeyOpenAIResponsesChatCompactionModel] = "deepseek-v4-flash"

	state := &responsesChatState{
		Version:    1,
		ResponseID: "resp_prev_fail",
		AccountID:  account.ID,
		Messages: []apicompat.ChatMessage{
			{Role: "user", Content: mustJSONRawString(strings.Repeat("older context ", 80))},
		},
		CreatedAt: time.Now(),
		UpdatedAt: time.Now(),
	}
	rawState, err := json.Marshal(state)
	require.NoError(t, err)
	require.NoError(t, cache.SetResponsesChatState(context.Background(), 0, "resp_prev_fail", rawState, time.Hour))

	rec := httptest.NewRecorder()
	c, _ := gin.CreateTestContext(rec)
	body := []byte(`{"model":"gpt-5.4","previous_response_id":"resp_prev_fail","input":"continue","stream":false}`)
	c.Request = httptest.NewRequest(http.MethodPost, "/v1/responses", bytes.NewReader(body))
	c.Request.Header.Set("Content-Type", "application/json")
	result, err := svc.Forward(context.Background(), c, account, body)
	require.Error(t, err)
	require.Nil(t, result)
	require.Equal(t, http.StatusBadGateway, rec.Code)
	require.Equal(t, "Failed to compact response history", gjson.Get(rec.Body.String(), "error.message").String())
	require.Len(t, upstream.bodies, 1)
}

func TestForwardResponses_StatefulFallbackCacheFailureReturnsError(t *testing.T) {
	gin.SetMode(gin.TestMode)

	cache := &responsesChatMemoryCache{getErr: errors.New("redis unavailable")}
	upstream := &httpUpstreamRecorder{}
	svc := &OpenAIGatewayService{
		cfg:          rawChatCompletionsTestConfig(),
		httpUpstream: upstream,
		cache:        cache,
	}
	account := forceChatResponsesStatefulAccount()

	rec := httptest.NewRecorder()
	c, _ := gin.CreateTestContext(rec)
	body := []byte(`{"model":"gpt-5.4","previous_response_id":"resp_prev_cache_down","input":"continue","stream":false}`)
	c.Request = httptest.NewRequest(http.MethodPost, "/v1/responses", bytes.NewReader(body))
	c.Request.Header.Set("Content-Type", "application/json")
	result, err := svc.Forward(context.Background(), c, account, body)
	require.Error(t, err)
	require.Nil(t, result)
	require.Equal(t, http.StatusBadGateway, rec.Code)
	require.Equal(t, "Response state cache is unavailable", gjson.Get(rec.Body.String(), "error.message").String())
	require.Empty(t, upstream.bodies)
}

func TestForwardResponses_PreviousResponseRequiresStatefulFallback(t *testing.T) {
	gin.SetMode(gin.TestMode)

	upstream := &httpUpstreamRecorder{}
	svc := &OpenAIGatewayService{
		cfg:          rawChatCompletionsTestConfig(),
		httpUpstream: upstream,
	}
	rec := httptest.NewRecorder()
	c, _ := gin.CreateTestContext(rec)
	body := []byte(`{"model":"gpt-5.4","previous_response_id":"resp_missing","input":"continue","stream":false}`)
	c.Request = httptest.NewRequest(http.MethodPost, "/v1/responses", bytes.NewReader(body))
	c.Request.Header.Set("Content-Type", "application/json")
	result, err := svc.Forward(context.Background(), c, forceChatResponsesFallbackAccount(), body)
	require.Error(t, err)
	require.Nil(t, result)
	require.Equal(t, http.StatusBadRequest, rec.Code)
	require.Equal(t, "previous_response_id was not found or has expired", gjson.Get(rec.Body.String(), "error.message").String())
	require.Empty(t, upstream.bodies)
}

func forceChatResponsesFallbackAccount() *Account {
	account := rawChatCompletionsTestAccount()
	account.Extra = map[string]any{
		openai_compat.ExtraKeyResponsesMode: string(openai_compat.ResponsesSupportModeForceChatCompletions),
	}
	return account
}

func forceChatResponsesStatefulAccount() *Account {
	account := forceChatResponsesFallbackAccount()
	account.Extra[extraKeyOpenAIResponsesChatStateful] = true
	account.Extra[extraKeyOpenAIResponsesChatCompaction] = "summarize"
	account.Extra[extraKeyOpenAIResponsesChatContextWindowTokens] = 32000
	account.Extra[extraKeyOpenAIResponsesChatMaxOutputTokens] = 4096
	return account
}

func mustJSONRawString(s string) json.RawMessage {
	raw, err := json.Marshal(s)
	if err != nil {
		panic(err)
	}
	return raw
}

func extractResponseIDFromSSEBody(body string) string {
	for _, line := range strings.Split(body, "\n") {
		line = strings.TrimSpace(line)
		if !strings.HasPrefix(line, "data: ") {
			continue
		}
		payload := strings.TrimSpace(strings.TrimPrefix(line, "data: "))
		if payload == "" || payload == "[DONE]" || !gjson.Valid(payload) {
			continue
		}
		id := gjson.Get(payload, "response.id").String()
		if id != "" {
			return id
		}
	}
	return ""
}
