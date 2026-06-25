use serde_json::{json, Map, Value};
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct BridgeError {
    pub message: String,
}

impl BridgeError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

pub fn responses_to_chat_completions_request(
    responses: &Value,
    upstream_model: &str,
) -> Result<Value, BridgeError> {
    if !string_field(responses, "previous_response_id")
        .unwrap_or_default()
        .trim()
        .is_empty()
    {
        return Err(BridgeError::new(
            "previous_response_id requires responses chat state cache",
        ));
    }
    let messages = responses_to_chat_messages(responses)?;
    build_chat_completions_request(responses, upstream_model, messages)
}

pub fn responses_to_chat_messages(responses: &Value) -> Result<Vec<Value>, BridgeError> {
    let model = string_field(responses, "model").unwrap_or_default();
    if model.trim().is_empty() {
        return Err(BridgeError::new("model is required"));
    }

    let mut messages = Vec::new();
    if let Some(instructions) = string_field(responses, "instructions")
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        messages.push(json!({ "role": "system", "content": instructions }));
    }
    messages.extend(responses_input_to_chat_messages(responses.get("input"))?);
    Ok(messages)
}

pub fn build_chat_completions_request(
    responses: &Value,
    upstream_model: &str,
    messages: Vec<Value>,
) -> Result<Value, BridgeError> {
    let mut out = Map::new();
    out.insert("model".to_owned(), json!(upstream_model));
    out.insert("messages".to_owned(), Value::Array(messages));
    copy_optional(responses, &mut out, "temperature");
    copy_optional(responses, &mut out, "top_p");
    copy_optional_renamed(
        responses,
        &mut out,
        "max_output_tokens",
        "max_completion_tokens",
    );
    copy_optional(responses, &mut out, "stream");
    copy_reasoning_effort(responses, &mut out);
    copy_service_tier(responses, &mut out);
    copy_tools(responses, &mut out);
    copy_tool_choice(responses, &mut out);
    if responses
        .get("stream")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        out.insert(
            "stream_options".to_owned(),
            json!({ "include_usage": true }),
        );
    }
    Ok(Value::Object(out))
}

pub fn assistant_message_from_chat_response(chat: &Value) -> Option<Value> {
    chat.get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
        .and_then(|choice| choice.get("message"))
        .cloned()
}

pub fn chat_completions_response_to_responses(chat: &Value, requested_model: &str) -> Value {
    let id = string_field(chat, "id")
        .filter(|value| value.starts_with("resp_"))
        .map(ToOwned::to_owned)
        .unwrap_or_else(generate_responses_id);
    let choice = chat
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first());
    let message = choice.and_then(|choice| choice.get("message"));
    let finish_reason = choice
        .and_then(|choice| string_field(choice, "finish_reason"))
        .unwrap_or_default();
    let text = message
        .and_then(|message| message.get("content"))
        .map(chat_message_content_text)
        .unwrap_or_default();
    let mut response = json!({
        "id": id,
        "object": "response",
        "created_at": unix_timestamp(),
        "model": if requested_model.trim().is_empty() {
            string_field(chat, "model").unwrap_or_default()
        } else {
            requested_model
        },
        "status": if finish_reason == "length" { "incomplete" } else { "completed" },
        "output": [{
            "type": "message",
            "id": generate_item_id(),
            "role": "assistant",
            "content": [{
                "type": "output_text",
                "text": text
            }],
            "status": "completed"
        }]
    });
    if finish_reason == "length" {
        response["incomplete_details"] = json!({ "reason": "max_output_tokens" });
    }
    if let Some(usage) = chat.get("usage") {
        response["usage"] = chat_usage_to_responses_usage(usage);
    }
    response
}

#[derive(Debug, Clone)]
pub struct ChatToResponsesStreamState {
    response_id: String,
    model: String,
    created_at: i64,
    sequence_number: i64,
    created_sent: bool,
    message_item_id: Option<String>,
    content_part_id: Option<String>,
    text: String,
    finish_reason: Option<String>,
    usage: Option<Value>,
}

impl ChatToResponsesStreamState {
    pub fn new(model: &str) -> Self {
        Self {
            response_id: generate_responses_id(),
            model: model.to_owned(),
            created_at: unix_timestamp(),
            sequence_number: 0,
            created_sent: false,
            message_item_id: None,
            content_part_id: None,
            text: String::new(),
            finish_reason: None,
            usage: None,
        }
    }

    pub fn response_id(&self) -> &str {
        &self.response_id
    }

    pub fn assistant_message(&self) -> Value {
        json!({ "role": "assistant", "content": self.text })
    }

    pub fn apply_chat_chunk(&mut self, chunk: &Value) -> Vec<Value> {
        if let Some(id) = string_field(chunk, "id").filter(|value| value.starts_with("resp_")) {
            self.response_id = id.to_owned();
        }
        if self.model.is_empty() {
            if let Some(model) = string_field(chunk, "model") {
                self.model = model.to_owned();
            }
        }
        if let Some(usage) = chunk.get("usage").filter(|value| !value.is_null()) {
            self.usage = Some(chat_usage_to_responses_usage(usage));
        }

        let mut events = self.ensure_created_events();
        let Some(choices) = chunk.get("choices").and_then(Value::as_array) else {
            return events;
        };
        for choice in choices {
            if let Some(content) = choice
                .get("delta")
                .and_then(|delta| delta.get("content"))
                .and_then(Value::as_str)
            {
                events.extend(self.ensure_message_item_events());
                events.extend(self.ensure_content_part_events());
                self.text.push_str(content);
                events.push(self.event(json!({
                    "type": "response.output_text.delta",
                    "output_index": 0,
                    "content_index": 0,
                    "delta": content,
                    "item_id": self.message_item_id.clone().unwrap_or_default()
                })));
            }
            if let Some(finish_reason) = choice.get("finish_reason").and_then(Value::as_str) {
                if !finish_reason.is_empty() {
                    self.finish_reason = Some(finish_reason.to_owned());
                }
            }
        }
        events
    }

    pub fn finalize(&mut self) -> Vec<Value> {
        let mut events = self.ensure_created_events();
        if self.message_item_id.is_some() {
            events.extend(self.ensure_content_part_events());
            events.push(self.event(json!({
                "type": "response.output_text.done",
                "output_index": 0,
                "content_index": 0,
                "text": self.text,
                "item_id": self.message_item_id.clone().unwrap_or_default()
            })));
            events.push(self.event(json!({
                "type": "response.content_part.done",
                "output_index": 0,
                "content_index": 0,
                "item_id": self.message_item_id.clone().unwrap_or_default(),
                "part": output_text_part(&self.text)
            })));
            events.push(self.event(json!({
                "type": "response.output_item.done",
                "output_index": 0,
                "item": {
                    "type": "message",
                    "id": self.message_item_id.clone().unwrap_or_default(),
                    "role": "assistant",
                    "status": "completed",
                    "content": [output_text_part(&self.text)]
                }
            })));
        }

        let status = if self.finish_reason.as_deref() == Some("length") {
            "incomplete"
        } else {
            "completed"
        };
        let mut response = json!({
            "id": self.response_id,
            "object": "response",
            "created_at": self.created_at,
            "model": self.model,
            "status": status,
            "output": self.output_items()
        });
        if status == "incomplete" {
            response["incomplete_details"] = json!({ "reason": "max_output_tokens" });
        }
        if let Some(usage) = &self.usage {
            response["usage"] = usage.clone();
        }
        events.push(self.event(json!({
            "type": "response.completed",
            "response": response
        })));
        events
    }

    fn ensure_created_events(&mut self) -> Vec<Value> {
        if self.created_sent {
            return Vec::new();
        }
        self.created_sent = true;
        let response = json!({
            "id": self.response_id,
            "object": "response",
            "created_at": self.created_at,
            "model": self.model,
            "status": "in_progress",
            "output": []
        });
        vec![
            self.event(json!({
                "type": "response.created",
                "response": response.clone()
            })),
            self.event(json!({
                "type": "response.in_progress",
                "response": response
            })),
        ]
    }

    fn ensure_message_item_events(&mut self) -> Vec<Value> {
        if self.message_item_id.is_some() {
            return Vec::new();
        }
        let item_id = generate_item_id();
        self.message_item_id = Some(item_id.clone());
        vec![self.event(json!({
            "type": "response.output_item.added",
            "output_index": 0,
            "item": {
                "type": "message",
                "id": item_id,
                "role": "assistant",
                "status": "in_progress"
            }
        }))]
    }

    fn ensure_content_part_events(&mut self) -> Vec<Value> {
        if self.content_part_id.is_some() {
            return Vec::new();
        }
        self.content_part_id = Some(format!("content_{}", Uuid::new_v4().simple()));
        vec![self.event(json!({
            "type": "response.content_part.added",
            "output_index": 0,
            "content_index": 0,
            "item_id": self.message_item_id.clone().unwrap_or_default(),
            "part": output_text_part("")
        }))]
    }

    fn output_items(&self) -> Vec<Value> {
        vec![json!({
            "type": "message",
            "id": self.message_item_id.clone().unwrap_or_else(generate_item_id),
            "role": "assistant",
            "content": [output_text_part(&self.text)],
            "status": "completed"
        })]
    }

    fn event(&mut self, mut event: Value) -> Value {
        self.sequence_number += 1;
        event["sequence_number"] = json!(self.sequence_number);
        event
    }
}

pub fn responses_stream_events_to_sse(events: &[Value]) -> Result<String, BridgeError> {
    let mut out = String::new();
    for event in events {
        let event_type = string_field(event, "type").unwrap_or("message");
        let data = serde_json::to_string(event).map_err(|error| {
            BridgeError::new(format!("marshal responses stream event: {error}"))
        })?;
        out.push_str("event: ");
        out.push_str(event_type);
        out.push('\n');
        out.push_str("data: ");
        out.push_str(&data);
        out.push_str("\n\n");
    }
    Ok(out)
}

fn output_text_part(text: &str) -> Value {
    json!({ "type": "output_text", "text": text })
}

fn responses_input_to_chat_messages(input: Option<&Value>) -> Result<Vec<Value>, BridgeError> {
    let Some(input) = input else {
        return Ok(Vec::new());
    };
    match input {
        Value::Null => Ok(Vec::new()),
        Value::String(text) => Ok(vec![json!({ "role": "user", "content": text })]),
        Value::Array(items) => {
            let mut messages = Vec::new();
            for item in items {
                messages.extend(responses_input_item_to_chat_messages(item)?);
            }
            Ok(messages)
        }
        _ => Err(BridgeError::new(
            "parse responses input: unsupported input shape",
        )),
    }
}

fn responses_input_item_to_chat_messages(item: &Value) -> Result<Vec<Value>, BridgeError> {
    if let Some(text) = item.as_str() {
        return Ok(vec![json!({ "role": "user", "content": text })]);
    }
    let Some(object) = item.as_object() else {
        return Err(BridgeError::new(
            "parse responses input item: expected object or string",
        ));
    };
    let item_type = object.get("type").and_then(Value::as_str).unwrap_or("");
    match item_type {
        "function_call_output" => Ok(vec![json!({
            "role": "tool",
            "tool_call_id": string_field(item, "call_id").unwrap_or_default(),
            "content": string_field(item, "output").unwrap_or_default()
        })]),
        "input_text" | "text" => Ok(vec![json!({
            "role": "user",
            "content": string_field(item, "text").unwrap_or_default()
        })]),
        "function_call" => Ok(vec![json!({
            "role": "assistant",
            "tool_calls": [{
                "id": string_field(item, "call_id").unwrap_or_default(),
                "type": "function",
                "function": {
                    "name": string_field(item, "name").unwrap_or_default(),
                    "arguments": string_field(item, "arguments")
                        .filter(|value| !value.trim().is_empty())
                        .unwrap_or("{}")
                }
            }]
        })]),
        _ => {
            let role = normalize_chat_role(string_field(item, "role").unwrap_or("user"));
            let content = object
                .get("content")
                .map(|content| responses_content_to_chat_content(content, role))
                .transpose()?
                .unwrap_or_else(|| {
                    string_field(item, "text")
                        .map(|value| Value::String(value.to_owned()))
                        .unwrap_or_else(|| Value::String(String::new()))
                });
            Ok(vec![json!({ "role": role, "content": content })])
        }
    }
}

fn responses_content_to_chat_content(raw: &Value, role: &str) -> Result<Value, BridgeError> {
    match raw {
        Value::Null => Ok(Value::String(String::new())),
        Value::String(_) => Ok(raw.clone()),
        Value::Array(parts) => {
            let mut text_parts = Vec::new();
            let mut chat_parts = Vec::new();
            let mut has_non_text = false;
            for part in parts {
                let Some(object) = part.as_object() else {
                    continue;
                };
                let part_type = object.get("type").and_then(Value::as_str).unwrap_or("");
                match part_type {
                    "input_text" | "output_text" | "text" | "" => {
                        if let Some(text) = object.get("text").and_then(Value::as_str) {
                            if !text.is_empty() {
                                text_parts.push(text.to_owned());
                                chat_parts.push(json!({ "type": "text", "text": text }));
                            }
                        }
                    }
                    "input_image" | "image_url" => {
                        let image_url = object
                            .get("image_url")
                            .and_then(value_to_image_url)
                            .unwrap_or_default();
                        if !image_url.is_empty() {
                            has_non_text = true;
                            chat_parts.push(json!({
                                "type": "image_url",
                                "image_url": { "url": image_url }
                            }));
                        }
                    }
                    _ => {}
                }
            }
            if has_non_text && role == "user" && !chat_parts.is_empty() {
                Ok(Value::Array(chat_parts))
            } else {
                Ok(Value::String(text_parts.join("\n\n")))
            }
        }
        Value::Object(object) => {
            let part_type = object.get("type").and_then(Value::as_str).unwrap_or("");
            match part_type {
                "input_image" | "image_url" if role == "user" => {
                    let image_url = object
                        .get("image_url")
                        .and_then(value_to_image_url)
                        .unwrap_or_default();
                    Ok(json!([{ "type": "image_url", "image_url": { "url": image_url } }]))
                }
                _ => Ok(Value::String(
                    object
                        .get("text")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned(),
                )),
            }
        }
        _ => Ok(raw.clone()),
    }
}

fn normalize_chat_role(role: &str) -> &str {
    match role.trim() {
        "" => "user",
        "developer" => "system",
        value => value,
    }
}

fn copy_optional(source: &Value, target: &mut Map<String, Value>, key: &str) {
    if let Some(value) = source.get(key).filter(|value| !value.is_null()) {
        target.insert(key.to_owned(), value.clone());
    }
}

fn copy_optional_renamed(source: &Value, target: &mut Map<String, Value>, from: &str, to: &str) {
    if let Some(value) = source.get(from).filter(|value| !value.is_null()) {
        target.insert(to.to_owned(), value.clone());
    }
}

fn copy_reasoning_effort(source: &Value, target: &mut Map<String, Value>) {
    if let Some(effort) = source
        .get("reasoning")
        .and_then(|value| value.get("effort"))
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
    {
        target.insert("reasoning_effort".to_owned(), json!(effort));
    }
}

fn copy_service_tier(source: &Value, target: &mut Map<String, Value>) {
    if let Some(service_tier) = string_field(source, "service_tier")
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        target.insert("service_tier".to_owned(), json!(service_tier));
    }
}

fn copy_tools(source: &Value, target: &mut Map<String, Value>) {
    let Some(tools) = source.get("tools").and_then(Value::as_array) else {
        return;
    };
    let tools = tools
        .iter()
        .filter_map(|tool| {
            let tool_type = string_field(tool, "type")?;
            if tool_type != "function" {
                return None;
            }
            Some(json!({
                "type": "function",
                "function": {
                    "name": string_field(tool, "name").unwrap_or_default(),
                    "description": string_field(tool, "description").unwrap_or_default(),
                    "parameters": tool.get("parameters").cloned().unwrap_or_else(|| json!({})),
                    "strict": tool.get("strict").cloned().unwrap_or(Value::Null)
                }
            }))
        })
        .collect::<Vec<_>>();
    if !tools.is_empty() {
        target.insert("tools".to_owned(), Value::Array(tools));
    }
}

fn copy_tool_choice(source: &Value, target: &mut Map<String, Value>) {
    if let Some(choice) = source.get("tool_choice").filter(|value| !value.is_null()) {
        if choice
            .get("type")
            .and_then(Value::as_str)
            .is_some_and(|value| value == "function")
        {
            if let Some(name) = string_field(choice, "name").or_else(|| {
                choice
                    .get("function")
                    .and_then(|value| string_field(value, "name"))
            }) {
                target.insert(
                    "tool_choice".to_owned(),
                    json!({ "type": "function", "function": { "name": name } }),
                );
                return;
            }
        }
        target.insert("tool_choice".to_owned(), choice.clone());
    }
}

fn chat_message_content_text(raw: &Value) -> String {
    match raw {
        Value::String(value) => value.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter_map(|part| {
                part.as_object()
                    .filter(|object| {
                        object
                            .get("type")
                            .and_then(Value::as_str)
                            .is_some_and(|value| value == "text")
                    })
                    .and_then(|object| object.get("text"))
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned)
            })
            .collect::<Vec<_>>()
            .join("\n\n"),
        _ => String::new(),
    }
}

fn chat_usage_to_responses_usage(usage: &Value) -> Value {
    let input_tokens = int_field(usage, "prompt_tokens");
    let output_tokens = int_field(usage, "completion_tokens");
    let total_tokens = int_field(usage, "total_tokens")
        .unwrap_or_else(|| input_tokens.unwrap_or_default() + output_tokens.unwrap_or_default());
    let mut out = json!({
        "input_tokens": input_tokens.unwrap_or_default(),
        "output_tokens": output_tokens.unwrap_or_default(),
        "total_tokens": total_tokens
    });
    if let Some(cached_tokens) = usage
        .get("prompt_tokens_details")
        .and_then(|value| int_field(value, "cached_tokens"))
        .filter(|value| *value > 0)
    {
        out["input_tokens_details"] = json!({ "cached_tokens": cached_tokens });
    }
    if let Some(reasoning_tokens) = usage
        .get("completion_tokens_details")
        .and_then(|value| int_field(value, "reasoning_tokens"))
        .filter(|value| *value > 0)
    {
        out["output_tokens_details"] = json!({ "reasoning_tokens": reasoning_tokens });
    }
    out
}

fn value_to_image_url(value: &Value) -> Option<String> {
    value.as_str().map(ToOwned::to_owned).or_else(|| {
        value
            .get("url")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned)
    })
}

fn string_field<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(Value::as_str)
}

fn int_field(value: &Value, key: &str) -> Option<i64> {
    value.get(key).and_then(Value::as_i64)
}

fn unix_timestamp() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or_default()
}

fn generate_responses_id() -> String {
    format!("resp_{}", Uuid::new_v4().simple())
}

fn generate_item_id() -> String {
    format!("msg_{}", Uuid::new_v4().simple())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_text_responses_request_to_chat_messages() {
        let request = json!({
            "model": "gpt-5.4",
            "instructions": "be brief",
            "input": "hello",
            "max_output_tokens": 256,
            "temperature": 0.2,
            "reasoning": { "effort": "low" },
            "stream": true
        });

        let chat = responses_to_chat_completions_request(&request, "deepseek-chat").unwrap();

        assert_eq!(chat["model"], "deepseek-chat");
        assert_eq!(chat["messages"][0]["role"], "system");
        assert_eq!(chat["messages"][0]["content"], "be brief");
        assert_eq!(chat["messages"][1]["role"], "user");
        assert_eq!(chat["messages"][1]["content"], "hello");
        assert_eq!(chat["max_completion_tokens"], 256);
        assert_eq!(chat["reasoning_effort"], "low");
        assert_eq!(chat["stream_options"]["include_usage"], true);
    }

    #[test]
    fn converts_chat_response_to_responses_shape() {
        let chat = json!({
            "id": "chatcmpl_1",
            "model": "deepseek-chat",
            "choices": [{
                "index": 0,
                "message": { "role": "assistant", "content": "answer" },
                "finish_reason": "stop"
            }],
            "usage": {
                "prompt_tokens": 3,
                "completion_tokens": 4,
                "total_tokens": 7
            }
        });

        let response = chat_completions_response_to_responses(&chat, "gpt-5.4");

        assert_eq!(response["object"], "response");
        assert_eq!(response["model"], "gpt-5.4");
        assert_eq!(response["status"], "completed");
        assert_eq!(response["output"][0]["content"][0]["text"], "answer");
        assert_eq!(response["usage"]["input_tokens"], 3);
        assert_eq!(response["usage"]["output_tokens"], 4);
        assert_eq!(response["usage"]["total_tokens"], 7);
    }

    #[test]
    fn rejects_previous_response_without_state_cache() {
        let request = json!({
            "model": "gpt-5.4",
            "previous_response_id": "resp_old",
            "input": "hello"
        });

        let error = responses_to_chat_completions_request(&request, "deepseek-chat").unwrap_err();

        assert!(error.message.contains("previous_response_id"));
    }

    #[test]
    fn converts_chat_stream_chunks_to_responses_events() {
        let mut state = ChatToResponsesStreamState::new("gpt-5.4");

        let mut events = state.apply_chat_chunk(&json!({
            "id": "chatcmpl_stream",
            "object": "chat.completion.chunk",
            "model": "deepseek-chat",
            "choices": [{
                "index": 0,
                "delta": { "content": "hel" },
                "finish_reason": null
            }]
        }));
        events.extend(state.apply_chat_chunk(&json!({
            "id": "chatcmpl_stream",
            "object": "chat.completion.chunk",
            "model": "deepseek-chat",
            "choices": [{
                "index": 0,
                "delta": { "content": "lo" },
                "finish_reason": "stop"
            }],
            "usage": {
                "prompt_tokens": 2,
                "completion_tokens": 1,
                "total_tokens": 3
            }
        })));
        events.extend(state.finalize());
        let event_types = events
            .iter()
            .filter_map(|event| event.get("type").and_then(Value::as_str))
            .collect::<Vec<_>>();

        assert!(event_types.contains(&"response.created"));
        assert!(event_types.contains(&"response.output_text.delta"));
        assert!(event_types.contains(&"response.output_text.done"));
        assert!(event_types.contains(&"response.completed"));
        let completed = events
            .iter()
            .find(|event| event.get("type").and_then(Value::as_str) == Some("response.completed"))
            .unwrap();
        assert_eq!(
            completed["response"]["output"][0]["content"][0]["text"],
            "hello"
        );
        assert_eq!(completed["response"]["usage"]["input_tokens"], 2);
        assert_eq!(state.assistant_message()["content"], "hello");
    }
}
