#![forbid(unsafe_code)]

use serde_json::{json, Map, Value};

#[derive(Clone, Debug, PartialEq)]
pub struct BedrockConfig {
    pub model: String,
    pub temperature: Option<f64>,
    pub max_tokens: Option<u32>,
    pub prompt_cache: bool,
    pub drop_temperature: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

impl ChatMessage {
    pub fn system(content: impl Into<String>) -> Self {
        Self::new("system", content)
    }

    pub fn user(content: impl Into<String>) -> Self {
        Self::new("user", content)
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self::new("assistant", content)
    }

    pub fn new(role: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: role.into(),
            content: content.into(),
        }
    }
}

pub fn build_bedrock_request(config: &BedrockConfig, messages: &[ChatMessage]) -> Value {
    let system_text = collect_system_text(messages);
    let mut converse_messages = convert_converse_messages(messages);
    if config.prompt_cache {
        append_cache_point_to_last_user(&mut converse_messages);
    }

    let mut request = Map::new();
    request.insert("modelId".to_string(), Value::String(config.model.clone()));
    request.insert("messages".to_string(), Value::Array(converse_messages));

    let inference = build_inference_config(config);
    if !inference.is_empty() {
        request.insert("inferenceConfig".to_string(), Value::Object(inference));
    }

    if let Some(system_text) = system_text {
        request.insert(
            "system".to_string(),
            Value::Array(build_system_blocks(&system_text, config.prompt_cache)),
        );
    }

    Value::Object(request)
}

pub fn reshape_bedrock_response(response: Value) -> Value {
    let text_blocks = response
        .pointer("/output/message/content")
        .and_then(Value::as_array)
        .map(|blocks| {
            blocks
                .iter()
                .filter_map(|block| block.get("text").and_then(Value::as_str))
                .map(|text| json!({"type": "text", "text": text}))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    let role = response
        .pointer("/output/message/role")
        .and_then(Value::as_str)
        .unwrap_or("assistant");

    let mut projected = Map::new();
    projected.insert("content".to_string(), Value::Array(text_blocks));
    projected.insert("role".to_string(), Value::String(role.to_string()));
    projected.insert(
        "stop_reason".to_string(),
        response.get("stopReason").cloned().unwrap_or(Value::Null),
    );

    if let Some(usage) = response.get("usage").and_then(Value::as_object) {
        let usage = project_usage(usage);
        if !usage.is_empty() {
            projected.insert("usage".to_string(), Value::Object(usage));
        }
    }

    Value::Object(projected)
}

pub fn bedrock_drops_temperature(model: &str) -> bool {
    matches!(
        model,
        "gpt-5" | "gpt-5-mini" | "gpt-5-nano" | "o1" | "o1-mini" | "o3" | "o3-mini" | "o4-mini"
    ) || model.contains("claude-opus-4-7")
}

pub fn bedrock_supports_prompt_cache(model: &str) -> bool {
    model.contains("anthropic.")
        || model.contains("amazon.nova-pro")
        || model.contains("amazon.nova-lite")
        || model.contains("amazon.nova-micro")
}

fn collect_system_text(messages: &[ChatMessage]) -> Option<String> {
    let system_messages = messages
        .iter()
        .filter(|message| message.role == "system")
        .map(|message| message.content.as_str())
        .collect::<Vec<_>>();
    if system_messages.is_empty() {
        None
    } else {
        Some(system_messages.join("\n\n"))
    }
}

fn convert_converse_messages(messages: &[ChatMessage]) -> Vec<Value> {
    messages
        .iter()
        .filter(|message| message.role != "system")
        .map(|message| {
            json!({
                "role": message.role,
                "content": [{"text": message.content}],
            })
        })
        .collect()
}

fn append_cache_point_to_last_user(messages: &mut [Value]) {
    if let Some(message) = messages
        .iter_mut()
        .rev()
        .find(|message| message.get("role").and_then(Value::as_str) == Some("user"))
    {
        if let Some(content) = message.get_mut("content").and_then(Value::as_array_mut) {
            content.push(cache_point_block());
        }
    }
}

fn build_inference_config(config: &BedrockConfig) -> Map<String, Value> {
    let mut inference = Map::new();
    if let Some(temperature) = config.temperature {
        if !config.drop_temperature {
            inference.insert("temperature".to_string(), json!(temperature));
        }
    }
    if let Some(max_tokens) = config.max_tokens {
        inference.insert("maxTokens".to_string(), json!(max_tokens));
    }
    inference
}

fn build_system_blocks(system_text: &str, prompt_cache: bool) -> Vec<Value> {
    let mut blocks = vec![json!({"text": system_text})];
    if prompt_cache {
        blocks.push(cache_point_block());
    }
    blocks
}

fn cache_point_block() -> Value {
    json!({"cachePoint": {"type": "default"}})
}

fn project_usage(usage: &Map<String, Value>) -> Map<String, Value> {
    let mut projected = Map::new();
    copy_usage_key(usage, &mut projected, "inputTokens", "input_tokens");
    copy_usage_key(usage, &mut projected, "outputTokens", "output_tokens");
    copy_usage_key(usage, &mut projected, "totalTokens", "total_tokens");
    copy_usage_key(
        usage,
        &mut projected,
        "cacheReadInputTokens",
        "cache_read_input_tokens",
    );
    copy_usage_key(
        usage,
        &mut projected,
        "cacheWriteInputTokens",
        "cache_creation_input_tokens",
    );
    projected
}

fn copy_usage_key(
    source: &Map<String, Value>,
    target: &mut Map<String, Value>,
    source_key: &str,
    target_key: &str,
) {
    if let Some(value) = source.get(source_key) {
        target.insert(target_key.to_string(), value.clone());
    }
}
