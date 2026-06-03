package openai_compat

type ResponsesPathMode string

const (
	ResponsesPathModeStandardV1    ResponsesPathMode = "standard_v1"
	ResponsesPathModeBareResponses ResponsesPathMode = "bare_responses"
)

const ExtraKeyResponsesPathMode = "openai_responses_path_mode"

func NormalizeResponsesPathMode(mode string) ResponsesPathMode {
	if ResponsesPathMode(mode) == ResponsesPathModeBareResponses {
		return ResponsesPathModeBareResponses
	}
	return ResponsesPathModeStandardV1
}

func ShouldUseBareResponsesPath(extra map[string]any) bool {
	if extra == nil {
		return false
	}
	mode, _ := extra[ExtraKeyResponsesPathMode].(string)
	return NormalizeResponsesPathMode(mode) == ResponsesPathModeBareResponses
}
