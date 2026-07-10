package service

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"net/http"
	"strings"
	"time"

	"github.com/Wei-Shaw/sub2api/internal/pkg/apicompat"
	"github.com/Wei-Shaw/sub2api/internal/pkg/logger"
	"github.com/Wei-Shaw/sub2api/internal/util/responseheaders"
	"github.com/gin-gonic/gin"
	"go.uber.org/zap"
)

// forwardResponsesViaRawChatCompletions serves /v1/responses clients through an
// upstream that only supports /v1/chat/completions.
func (s *OpenAIGatewayService) forwardResponsesViaRawChatCompletions(
	ctx context.Context,
	c *gin.Context,
	account *Account,
	body []byte,
) (*OpenAIForwardResult, error) {
	startTime := time.Now()

	var responsesReq apicompat.ResponsesRequest
	if err := json.Unmarshal(body, &responsesReq); err != nil {
		writeOpenAIResponsesFallbackError(c, http.StatusBadRequest, "invalid_request_error", "Failed to parse request body")
		return nil, fmt.Errorf("parse responses request: %w", err)
	}
	originalModel := strings.TrimSpace(responsesReq.Model)
	if originalModel == "" {
		writeOpenAIResponsesFallbackError(c, http.StatusBadRequest, "invalid_request_error", "model is required")
		return nil, fmt.Errorf("missing model in request")
	}

	clientStream := responsesReq.Stream
	serviceTier := extractOpenAIServiceTierFromBody(body)

	chatReq, err := apicompat.ResponsesToChatCompletionsRequest(&responsesReq)
	if err != nil {
		writeOpenAIResponsesFallbackError(c, http.StatusBadRequest, "invalid_request_error", err.Error())
		return nil, fmt.Errorf("convert responses to chat completions: %w", err)
	}

	billingModel := resolveOpenAIForwardModel(account, originalModel, "")
	upstreamModel := normalizeOpenAIModelForUpstream(account, billingModel)
	reasoningEffort := extractOpenAIReasoningEffortFromBody(body, upstreamModel, billingModel, originalModel)
	// 国产模型默认 effort 补充：需要 mappedModel 判定，推迟到 billingModel 算出之后。
	reasoningEffort = ApplyThinkingEnabledFallback(reasoningEffort, body, billingModel)
	statefulBuild, err := s.buildStatefulResponsesChatRequest(ctx, c, account, &responsesReq, body, originalModel, upstreamModel, billingModel)
	if err != nil {
		writeResponsesChatFallbackError(c, err)
		return nil, err
	}
	if statefulBuild != nil && statefulBuild.ChatRequest != nil {
		chatReq = statefulBuild.ChatRequest
	} else {
		if strings.TrimSpace(responsesReq.PreviousResponseID) != "" {
			err := fmt.Errorf("%w: previous_response_id requires openai_responses_chat_stateful", errResponsesChatPreviousNotFound)
			writeResponsesChatFallbackError(c, err)
			return nil, err
		}
		chatReq.Model = upstreamModel
	}
	if clientStream {
		chatReq.StreamOptions = &apicompat.ChatStreamOptions{IncludeUsage: true}
	}

	chatBody, err := json.Marshal(chatReq)
	if err != nil {
		return nil, fmt.Errorf("marshal chat completions fallback request: %w", err)
	}
	chatBody, err = s.applyOpenAIFastPolicyToBody(ctx, account, upstreamModel, chatBody)
	if err != nil {
		var blocked *OpenAIFastBlockedError
		if errors.As(err, &blocked) {
			writeOpenAIFastPolicyBlockedResponse(c, blocked)
		}
		return nil, err
	}
	if serviceTier == nil {
		serviceTier = extractOpenAIServiceTierFromBody(chatBody)
	}

	logger.L().Debug("openai responses: forwarding via raw chat completions",
		zap.Int64("account_id", account.ID),
		zap.String("original_model", originalModel),
		zap.String("billing_model", billingModel),
		zap.String("upstream_model", upstreamModel),
		zap.Bool("stream", clientStream),
	)

	// Build and send upstream request via the shared CC pipeline
	apiKey, targetURL, err := s.resolveCCFallbackTarget(account)
	if err != nil {
		return nil, err
	}
	resp, err := s.sendCCUpstreamRequest(ctx, c, account, targetURL, chatBody, clientStream, apiKey, account.GetOpenAIUserAgent())
	if err != nil {
		return nil, err
	}
	defer func() { _ = resp.Body.Close() }()

	if resp.StatusCode >= 400 {
		respBody, upstreamMsg := s.readOpenAIUpstreamError(resp)
		if foErr := s.failoverOpenAIUpstreamHTTPError(ctx, c, account, resp, respBody, upstreamMsg, upstreamModel); foErr != nil {
			return nil, foErr
		}
		return s.handleErrorResponse(ctx, resp, c, account, chatBody, billingModel)
	}

	if clientStream {
		return s.streamChatCompletionsAsResponses(ctx, c, account, resp, originalModel, billingModel, upstreamModel, reasoningEffort, serviceTier, startTime, statefulBuild)
	}
	return s.bufferChatCompletionsAsResponses(ctx, c, account, resp, originalModel, billingModel, upstreamModel, reasoningEffort, serviceTier, startTime, statefulBuild)
}

func (s *OpenAIGatewayService) bufferChatCompletionsAsResponses(
	ctx context.Context,
	c *gin.Context,
	account *Account,
	resp *http.Response,
	originalModel string,
	billingModel string,
	upstreamModel string,
	reasoningEffort *string,
	serviceTier *string,
	startTime time.Time,
	statefulBuild *responsesChatBuildResult,
) (*OpenAIForwardResult, error) {
	requestID := resp.Header.Get("x-request-id")
	ccResp, usage, err := s.readCCUpstreamJSONResponse(c, resp, writeOpenAIResponsesFallbackError)
	if err != nil {
		return nil, err
	}
	responsesResp := apicompat.ChatCompletionsResponseToResponses(ccResp, originalModel)
	if statefulBuild != nil && statefulBuild.NewResponseID != "" {
		responsesResp.ID = statefulBuild.NewResponseID
	}

	if statefulBuild != nil {
		assistant := assistantMessageFromChatCompletion(ccResp)
		if assistant != nil {
			if err := s.finalizeResponsesChatState(ctx, c, account, responsesChatFinalizeInput{
				BuildResult:   statefulBuild,
				ResponseID:    responsesResp.ID,
				Assistant:     assistant,
				OriginalModel: originalModel,
				UpstreamModel: upstreamModel,
				BillingModel:  billingModel,
			}); err != nil {
				writeResponsesChatFallbackError(c, err)
				return nil, err
			}
		}
	}

	if s.responseHeaderFilter != nil {
		responseheaders.WriteFilteredHeaders(c.Writer.Header(), resp.Header, s.responseHeaderFilter)
	}
	c.JSON(http.StatusOK, responsesResp)

	return &OpenAIForwardResult{
		RequestID:       requestID,
		Usage:           usage,
		Model:           originalModel,
		BillingModel:    billingModel,
		UpstreamModel:   upstreamModel,
		ReasoningEffort: reasoningEffort,
		ServiceTier:     serviceTier,
		Stream:          false,
		Duration:        time.Since(startTime),
	}, nil
}

func (s *OpenAIGatewayService) streamChatCompletionsAsResponses(
	ctx context.Context,
	c *gin.Context,
	account *Account,
	resp *http.Response,
	originalModel string,
	billingModel string,
	upstreamModel string,
	reasoningEffort *string,
	serviceTier *string,
	startTime time.Time,
	statefulBuild *responsesChatBuildResult,
) (*OpenAIForwardResult, error) {
	requestID := resp.Header.Get("x-request-id")
	writeStreamHeaders := s.newStreamHeaderWriter(c, resp.Header)

	state := apicompat.NewChatCompletionsToResponsesStreamState(originalModel)
	if statefulBuild != nil && statefulBuild.NewResponseID != "" {
		state.ResponseID = statefulBuild.NewResponseID
	}
	clientDisconnected := false

	writeEvents := func(events []apicompat.ResponsesStreamEvent) {
		if clientDisconnected || len(events) == 0 {
			return
		}
		writeStreamHeaders()
		for _, event := range events {
			sse, err := apicompat.ResponsesEventToSSE(event)
			if err != nil {
				logger.L().Warn("openai responses chat fallback: failed to marshal stream event",
					zap.Error(err),
					zap.String("request_id", requestID),
				)
				continue
			}
			if _, err := fmt.Fprint(c.Writer, sse); err != nil {
				clientDisconnected = true
				logger.L().Debug("openai responses chat fallback: client disconnected, continuing to drain upstream for billing",
					zap.Error(err),
					zap.String("request_id", requestID),
				)
				return
			}
		}
		c.Writer.Flush()
	}

	scan := s.scanCCStream(resp, "openai responses chat fallback", requestID, startTime, func(chunk *apicompat.ChatCompletionsChunk) {
		writeEvents(apicompat.ChatCompletionsChunkToResponsesEvents(chunk, state))
	})

	if scan.Err != nil {
		return &OpenAIForwardResult{
			RequestID:       requestID,
			Usage:           scan.Usage,
			Model:           originalModel,
			BillingModel:    billingModel,
			UpstreamModel:   upstreamModel,
			ReasoningEffort: reasoningEffort,
			ServiceTier:     serviceTier,
			Stream:          true,
			Duration:        time.Since(startTime),
			FirstTokenMs:    scan.FirstTokenMs,
		}, fmt.Errorf("stream usage incomplete: %w", scan.Err)
	}

	writeEvents(apicompat.FinalizeChatCompletionsResponsesStream(state))
	if statefulBuild != nil {
		assistant := assistantMessageFromResponsesStreamState(state)
		if assistant != nil {
			if err := s.finalizeResponsesChatState(ctx, c, account, responsesChatFinalizeInput{
				BuildResult:   statefulBuild,
				ResponseID:    state.ResponseID,
				Assistant:     assistant,
				OriginalModel: originalModel,
				UpstreamModel: upstreamModel,
				BillingModel:  billingModel,
			}); err != nil {
				if !clientDisconnected {
					writeResponsesChatStreamFailedEvent(c, state, "api_error", "Failed to save response state")
				}
				return &OpenAIForwardResult{
					RequestID:       requestID,
					Usage:           scan.Usage,
					Model:           originalModel,
					BillingModel:    billingModel,
					UpstreamModel:   upstreamModel,
					ReasoningEffort: reasoningEffort,
					ServiceTier:     serviceTier,
					Stream:          true,
					Duration:        time.Since(startTime),
					FirstTokenMs:    scan.FirstTokenMs,
				}, err
			}
		}
	}
	if !clientDisconnected {
		writeStreamHeaders()
		if _, err := fmt.Fprint(c.Writer, "data: [DONE]\n\n"); err != nil {
			clientDisconnected = true
		}
		if !clientDisconnected {
			c.Writer.Flush()
		}
	}
	if !scan.SawDone {
		logCCStreamMissingDoneSentinel("openai responses chat fallback", requestID)
	}

	return &OpenAIForwardResult{
		RequestID:       requestID,
		Usage:           scan.Usage,
		Model:           originalModel,
		BillingModel:    billingModel,
		UpstreamModel:   upstreamModel,
		ReasoningEffort: reasoningEffort,
		ServiceTier:     serviceTier,
		Stream:          true,
		Duration:        time.Since(startTime),
		FirstTokenMs:    scan.FirstTokenMs,
	}, nil
}

func writeResponsesChatStreamFailedEvent(c *gin.Context, state *apicompat.ChatCompletionsToResponsesStreamState, errType, message string) {
	if c == nil || c.Writer == nil {
		return
	}
	responseID := ""
	model := ""
	if state != nil {
		responseID = state.ResponseID
		model = state.Model
	}
	if responseID == "" {
		responseID = generateResponsesChatStateResponseID()
	}
	payload, err := json.Marshal(apicompat.ResponsesStreamEvent{
		Type: "response.failed",
		Response: &apicompat.ResponsesResponse{
			ID:     responseID,
			Object: "response",
			Model:  model,
			Status: "failed",
			Output: []apicompat.ResponsesOutput{},
			Error: &apicompat.ResponsesError{
				Code:    errType,
				Message: message,
			},
		},
	})
	if err != nil {
		return
	}
	if _, err := fmt.Fprintf(c.Writer, "event: response.failed\ndata: %s\n\n", payload); err == nil {
		c.Writer.Flush()
	}
}

func chatChunkStartsResponsesOutput(chunk *apicompat.ChatCompletionsChunk) bool {
	if chunk == nil {
		return false
	}
	for _, choice := range chunk.Choices {
		if choice.Delta.Content != nil || choice.Delta.ReasoningContent != nil || len(choice.Delta.ToolCalls) > 0 {
			return true
		}
	}
	return false
}
