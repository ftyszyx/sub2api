package openai

import "testing"

func TestDefaultModelsIncludeGPTImage2(t *testing.T) {
	foundModel := false
	for _, model := range DefaultModels {
		if model.ID == "gpt-image-2" {
			foundModel = true
			if model.DisplayName != "GPT Image 2" {
				t.Fatalf("DisplayName = %q, want %q", model.DisplayName, "GPT Image 2")
			}
			break
		}
	}
	if !foundModel {
		t.Fatal("DefaultModels should include gpt-image-2")
	}
}

func TestDefaultModelIDsIncludeGPTImage2(t *testing.T) {
	foundModel := false
	for _, modelID := range DefaultModelIDs() {
		if modelID == "gpt-image-2" {
			foundModel = true
			break
		}
	}
	if !foundModel {
		t.Fatal("DefaultModelIDs should include gpt-image-2")
	}
}
