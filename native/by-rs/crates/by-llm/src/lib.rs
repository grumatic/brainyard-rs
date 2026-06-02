#![forbid(unsafe_code)]

use anyhow::{bail, Context, Result};
use aws_config::BehaviorVersion;
use aws_sdk_bedrockruntime::{
    operation::converse::ConverseOutput as AwsConverseOperationOutput,
    types::{
        CachePointBlock, CachePointType, ContentBlock, ConversationRole,
        ConverseOutput as AwsConverseOutput, InferenceConfiguration, Message, SystemContentBlock,
    },
    Client,
};
use aws_types::region::Region;
use serde_json::{json, Map, Value};

const DEFAULT_BEDROCK_REGION: &str = "us-east-1";
const MAX_SYSTEM_CACHE_POINTS: usize = 3;

#[derive(Clone, Debug, PartialEq)]
pub struct BedrockConfig {
    pub model: String,
    pub temperature: Option<f64>,
    pub max_tokens: Option<u32>,
    pub prompt_cache: bool,
    pub drop_temperature: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CacheZone {
    pub key: String,
    pub text: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BedrockRuntimeOptions {
    pub region: String,
    pub aws_profile: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct BedrockConverseRequest {
    pub config: BedrockConfig,
    pub runtime: BedrockRuntimeOptions,
    pub messages: Vec<ChatMessage>,
    pub cache_zones: Vec<CacheZone>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct BedrockConverseResponse {
    pub raw: Value,
    pub projected: Value,
    pub text: String,
    pub stop_reason: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ProviderChatConfig {
    pub model: String,
    pub temperature: Option<f64>,
    pub max_tokens: Option<u32>,
    pub drop_temperature: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ProviderRequestProjection {
    pub operation: &'static str,
    pub request: Value,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct BedrockRuntimeInputs {
    pub explicit_region: Option<String>,
    pub aws_region: Option<String>,
    pub aws_default_region: Option<String>,
    pub explicit_profile: Option<String>,
    pub aws_profile: Option<String>,
    pub aws_default_profile: Option<String>,
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
    build_bedrock_request_with_cache_zones(config, messages, &[])
}

pub fn build_openai_compatible_request(
    config: &ProviderChatConfig,
    messages: &[ChatMessage],
) -> Value {
    let mut request = Map::new();
    request.insert("model".to_string(), Value::String(config.model.clone()));
    request.insert("messages".to_string(), openai_messages(messages));

    if let Some(temperature) = config.temperature {
        if !config.drop_temperature {
            request.insert("temperature".to_string(), json!(temperature));
        }
    }
    if let Some(max_tokens) = config.max_tokens {
        request.insert("max_tokens".to_string(), json!(max_tokens));
    }

    Value::Object(request)
}

pub fn build_anthropic_request(config: &ProviderChatConfig, messages: &[ChatMessage]) -> Value {
    let mut request = Map::new();
    request.insert("model".to_string(), Value::String(config.model.clone()));
    if let Some(system_text) = collect_system_text(messages) {
        request.insert("system".to_string(), Value::String(system_text));
    }
    request.insert("messages".to_string(), anthropic_messages(messages));
    request.insert(
        "max_tokens".to_string(),
        json!(config.max_tokens.unwrap_or(4096)),
    );

    if let Some(temperature) = config.temperature {
        if !config.drop_temperature {
            request.insert("temperature".to_string(), json!(temperature));
        }
    }

    Value::Object(request)
}

pub fn build_provider_request(
    provider: &str,
    config: &ProviderChatConfig,
    messages: &[ChatMessage],
) -> Result<ProviderRequestProjection> {
    match provider.trim() {
        "anthropic" | "anthropic-max" => Ok(ProviderRequestProjection {
            operation: "messages",
            request: build_anthropic_request(config, messages),
        }),
        "openai" | "google" | "azure" | "groq" | "together" | "fireworks" | "openrouter"
        | "ollama" | "mistral" | "deepseek" | "apple-fm" => Ok(ProviderRequestProjection {
            operation: "chat/completions",
            request: build_openai_compatible_request(config, messages),
        }),
        other => bail!("dry-run request shaping does not support provider '{other}'"),
    }
}

pub fn build_bedrock_request_with_cache_zones(
    config: &BedrockConfig,
    messages: &[ChatMessage],
    cache_zones: &[CacheZone],
) -> Value {
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
            Value::Array(build_system_blocks(
                &system_text,
                config.prompt_cache,
                cache_zones,
            )),
        );
    }

    Value::Object(request)
}

pub fn resolve_bedrock_runtime_options(inputs: BedrockRuntimeInputs) -> BedrockRuntimeOptions {
    BedrockRuntimeOptions {
        region: first_non_blank([
            inputs.explicit_region,
            inputs.aws_region,
            inputs.aws_default_region,
        ])
        .unwrap_or_else(|| DEFAULT_BEDROCK_REGION.to_string()),
        aws_profile: first_non_blank([
            inputs.explicit_profile,
            inputs.aws_profile,
            inputs.aws_default_profile,
        ]),
    }
}

pub fn default_bedrock_region() -> &'static str {
    DEFAULT_BEDROCK_REGION
}

pub async fn converse_bedrock(request: BedrockConverseRequest) -> Result<BedrockConverseResponse> {
    let sdk_config = load_bedrock_sdk_config(&request.runtime).await;
    let client = Client::new(&sdk_config);

    let mut builder = client
        .converse()
        .model_id(request.config.model.clone())
        .set_messages(Some(build_sdk_messages(
            &request.config,
            &request.messages,
        )?));

    if let Some(system) =
        build_sdk_system_blocks(&request.config, &request.messages, &request.cache_zones)?
    {
        builder = builder.set_system(Some(system));
    }

    if let Some(inference) = build_sdk_inference_config(&request.config)? {
        builder = builder.inference_config(inference);
    }

    let output = builder.send().await.with_context(|| {
        format!(
            "failed to call Bedrock Converse in {}",
            request.runtime.region
        )
    })?;
    let raw = project_bedrock_converse_output(&output);
    let projected = reshape_bedrock_response(raw.clone());
    let text = projected_response_text(&projected);
    let stop_reason = output.stop_reason().as_str().to_string();

    Ok(BedrockConverseResponse {
        raw,
        projected,
        text,
        stop_reason,
    })
}

pub fn project_bedrock_converse_output(output: &AwsConverseOperationOutput) -> Value {
    let mut root = Map::new();

    if let Some(AwsConverseOutput::Message(message)) = output.output() {
        root.insert("output".to_string(), message_output_value(message));
    }

    root.insert(
        "stopReason".to_string(),
        Value::String(output.stop_reason().as_str().to_string()),
    );

    if let Some(usage) = output.usage() {
        let mut usage_value = Map::new();
        usage_value.insert("inputTokens".to_string(), json!(usage.input_tokens()));
        usage_value.insert("outputTokens".to_string(), json!(usage.output_tokens()));
        usage_value.insert("totalTokens".to_string(), json!(usage.total_tokens()));
        if let Some(tokens) = usage.cache_read_input_tokens() {
            usage_value.insert("cacheReadInputTokens".to_string(), json!(tokens));
        }
        if let Some(tokens) = usage.cache_write_input_tokens() {
            usage_value.insert("cacheWriteInputTokens".to_string(), json!(tokens));
        }
        root.insert("usage".to_string(), Value::Object(usage_value));
    }

    Value::Object(root)
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

pub fn reshape_openai_compatible_response(response: Value) -> Value {
    let choice = response
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first());
    let message = choice.and_then(|choice| choice.get("message"));
    let text_blocks = openai_text_blocks(message.and_then(|message| message.get("content")));
    let role = message
        .and_then(|message| message.get("role"))
        .and_then(Value::as_str)
        .unwrap_or("assistant");

    let mut projected = Map::new();
    projected.insert("content".to_string(), Value::Array(text_blocks));
    projected.insert("role".to_string(), Value::String(role.to_string()));
    projected.insert(
        "stop_reason".to_string(),
        choice
            .and_then(|choice| choice.get("finish_reason"))
            .cloned()
            .unwrap_or(Value::Null),
    );

    if let Some(usage) = response.get("usage").and_then(Value::as_object) {
        let usage = project_openai_usage(usage);
        if !usage.is_empty() {
            projected.insert("usage".to_string(), Value::Object(usage));
        }
    }

    Value::Object(projected)
}

pub fn reshape_anthropic_response(response: Value) -> Value {
    let text_blocks = anthropic_text_blocks(response.get("content"));
    let role = response
        .get("role")
        .and_then(Value::as_str)
        .unwrap_or("assistant");

    let mut projected = Map::new();
    projected.insert("content".to_string(), Value::Array(text_blocks));
    projected.insert("role".to_string(), Value::String(role.to_string()));
    projected.insert(
        "stop_reason".to_string(),
        response.get("stop_reason").cloned().unwrap_or(Value::Null),
    );

    if let Some(usage) = response.get("usage").and_then(Value::as_object) {
        let usage = project_anthropic_usage(usage);
        if !usage.is_empty() {
            projected.insert("usage".to_string(), Value::Object(usage));
        }
    }

    Value::Object(projected)
}

pub fn reshape_provider_response(provider: &str, response: Value) -> Result<Value> {
    match provider.trim() {
        "bedrock" => Ok(reshape_bedrock_response(response)),
        "anthropic" | "anthropic-max" => Ok(reshape_anthropic_response(response)),
        "openai" | "google" | "azure" | "groq" | "together" | "fireworks" | "openrouter"
        | "ollama" | "mistral" | "deepseek" | "apple-fm" => {
            Ok(reshape_openai_compatible_response(response))
        }
        other => bail!("fixture response replay does not support provider '{other}'"),
    }
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

fn openai_messages(messages: &[ChatMessage]) -> Value {
    Value::Array(
        messages
            .iter()
            .map(|message| {
                json!({
                    "role": message.role,
                    "content": message.content,
                })
            })
            .collect(),
    )
}

fn anthropic_messages(messages: &[ChatMessage]) -> Value {
    Value::Array(
        messages
            .iter()
            .filter(|message| message.role != "system")
            .map(|message| {
                json!({
                    "role": message.role,
                    "content": message.content,
                })
            })
            .collect(),
    )
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

fn build_system_blocks(
    system_text: &str,
    prompt_cache: bool,
    cache_zones: &[CacheZone],
) -> Vec<Value> {
    if prompt_cache {
        if let Some(blocks) = system_blocks_from_zones(system_text, cache_zones) {
            return blocks;
        }
    }

    let mut blocks = vec![json!({"text": system_text})];
    if prompt_cache {
        blocks.push(cache_point_block());
    }
    blocks
}

fn system_blocks_from_zones(system_text: &str, cache_zones: &[CacheZone]) -> Option<Vec<Value>> {
    if cache_zones.is_empty() {
        return None;
    }

    let mut remaining = system_text;
    let mut blocks = Vec::new();

    for zone in cache_zones.iter().take(MAX_SYSTEM_CACHE_POINTS) {
        let index = remaining.find(&zone.text)?;
        let preamble = remaining[..index]
            .strip_suffix("\n\n")
            .unwrap_or(&remaining[..index]);
        if !preamble.trim().is_empty() {
            blocks.push(json!({"text": preamble}));
        }
        blocks.push(json!({"text": zone.text}));
        blocks.push(cache_point_block());
        remaining = &remaining[index + zone.text.len()..];
        remaining = remaining.strip_prefix("\n\n").unwrap_or(remaining);
    }

    if !remaining.trim().is_empty() {
        blocks.push(json!({"text": remaining}));
    }

    Some(blocks)
}

fn cache_point_block() -> Value {
    json!({"cachePoint": {"type": "default"}})
}

async fn load_bedrock_sdk_config(runtime: &BedrockRuntimeOptions) -> aws_config::SdkConfig {
    let mut loader =
        aws_config::defaults(BehaviorVersion::latest()).region(Region::new(runtime.region.clone()));
    if let Some(profile) = &runtime.aws_profile {
        loader = loader.profile_name(profile.clone());
    }
    loader.load().await
}

fn build_sdk_messages(config: &BedrockConfig, messages: &[ChatMessage]) -> Result<Vec<Message>> {
    let mut sdk_messages = messages
        .iter()
        .filter(|message| message.role != "system")
        .map(build_sdk_message)
        .collect::<Result<Vec<_>>>()?;

    if config.prompt_cache {
        append_sdk_cache_point_to_last_user(&mut sdk_messages)?;
    }

    Ok(sdk_messages)
}

fn build_sdk_message(message: &ChatMessage) -> Result<Message> {
    Message::builder()
        .role(conversation_role(&message.role)?)
        .content(ContentBlock::Text(message.content.clone()))
        .build()
        .with_context(|| {
            format!(
                "failed to build Bedrock message for role '{}'",
                message.role
            )
        })
}

fn conversation_role(role: &str) -> Result<ConversationRole> {
    match role {
        "assistant" => Ok(ConversationRole::Assistant),
        "user" => Ok(ConversationRole::User),
        other => bail!("unsupported Bedrock conversation role '{other}'"),
    }
}

fn append_sdk_cache_point_to_last_user(messages: &mut [Message]) -> Result<()> {
    if let Some(message) = messages
        .iter_mut()
        .rev()
        .find(|message| message.role.as_str() == "user")
    {
        message
            .content
            .push(ContentBlock::CachePoint(default_cache_point_block()?));
    }
    Ok(())
}

fn build_sdk_system_blocks(
    config: &BedrockConfig,
    messages: &[ChatMessage],
    cache_zones: &[CacheZone],
) -> Result<Option<Vec<SystemContentBlock>>> {
    let Some(system_text) = collect_system_text(messages) else {
        return Ok(None);
    };

    build_system_blocks(&system_text, config.prompt_cache, cache_zones)
        .into_iter()
        .map(sdk_system_block_from_value)
        .collect::<Result<Vec<_>>>()
        .map(Some)
}

fn sdk_system_block_from_value(block: Value) -> Result<SystemContentBlock> {
    if let Some(text) = block.get("text").and_then(Value::as_str) {
        return Ok(SystemContentBlock::Text(text.to_string()));
    }
    if block.get("cachePoint").is_some() {
        return Ok(SystemContentBlock::CachePoint(default_cache_point_block()?));
    }
    bail!("unsupported Bedrock system block: {block}");
}

fn default_cache_point_block() -> Result<CachePointBlock> {
    CachePointBlock::builder()
        .r#type(CachePointType::Default)
        .build()
        .context("failed to build Bedrock cachePoint block")
}

fn build_sdk_inference_config(config: &BedrockConfig) -> Result<Option<InferenceConfiguration>> {
    let mut has_config = false;
    let mut builder = InferenceConfiguration::builder();

    if let Some(temperature) = config.temperature {
        if !config.drop_temperature {
            builder = builder.temperature(temperature as f32);
            has_config = true;
        }
    }

    if let Some(max_tokens) = config.max_tokens {
        let max_tokens =
            i32::try_from(max_tokens).context("Bedrock max_tokens exceeds i32 range")?;
        builder = builder.max_tokens(max_tokens);
        has_config = true;
    }

    if has_config {
        Ok(Some(builder.build()))
    } else {
        Ok(None)
    }
}

fn message_output_value(message: &Message) -> Value {
    json!({
        "message": {
            "role": message.role().as_str(),
            "content": message.content().iter().filter_map(content_block_value).collect::<Vec<_>>(),
        }
    })
}

fn content_block_value(block: &ContentBlock) -> Option<Value> {
    block.as_text().ok().map(|text| json!({"text": text}))
}

pub fn projected_response_text(projected: &Value) -> String {
    projected
        .get("content")
        .and_then(Value::as_array)
        .map(|blocks| {
            blocks
                .iter()
                .filter_map(|block| block.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("")
        })
        .unwrap_or_default()
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

fn project_openai_usage(usage: &Map<String, Value>) -> Map<String, Value> {
    let mut projected = Map::new();
    copy_usage_key(usage, &mut projected, "prompt_tokens", "input_tokens");
    copy_usage_key(usage, &mut projected, "completion_tokens", "output_tokens");
    copy_usage_key(usage, &mut projected, "total_tokens", "total_tokens");

    if let Some(cached_tokens) = usage
        .get("prompt_tokens_details")
        .and_then(Value::as_object)
        .and_then(|details| details.get("cached_tokens"))
    {
        projected.insert("cache_read_input_tokens".to_string(), cached_tokens.clone());
    }

    projected
}

fn project_anthropic_usage(usage: &Map<String, Value>) -> Map<String, Value> {
    let mut projected = Map::new();
    copy_usage_key(usage, &mut projected, "input_tokens", "input_tokens");
    copy_usage_key(usage, &mut projected, "output_tokens", "output_tokens");
    copy_usage_key(usage, &mut projected, "total_tokens", "total_tokens");
    copy_usage_key(
        usage,
        &mut projected,
        "cache_read_input_tokens",
        "cache_read_input_tokens",
    );
    copy_usage_key(
        usage,
        &mut projected,
        "cache_creation_input_tokens",
        "cache_creation_input_tokens",
    );

    projected
}

fn openai_text_blocks(content: Option<&Value>) -> Vec<Value> {
    match content {
        Some(Value::String(text)) => vec![json!({"type": "text", "text": text})],
        Some(Value::Array(parts)) => parts
            .iter()
            .filter_map(openai_content_part_text)
            .map(|text| json!({"type": "text", "text": text}))
            .collect(),
        _ => Vec::new(),
    }
}

fn openai_content_part_text(part: &Value) -> Option<&str> {
    if let Some(text) = part.as_str() {
        return Some(text);
    }

    let object = part.as_object()?;
    let part_type = object.get("type").and_then(Value::as_str);
    if matches!(part_type, Some("text") | Some("output_text") | None) {
        object.get("text").and_then(Value::as_str)
    } else {
        None
    }
}

fn anthropic_text_blocks(content: Option<&Value>) -> Vec<Value> {
    content
        .and_then(Value::as_array)
        .map(|blocks| {
            blocks
                .iter()
                .filter_map(|block| {
                    let object = block.as_object()?;
                    if object.get("type").and_then(Value::as_str) == Some("text") {
                        object.get("text").and_then(Value::as_str)
                    } else {
                        None
                    }
                })
                .map(|text| json!({"type": "text", "text": text}))
                .collect()
        })
        .unwrap_or_default()
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

fn first_non_blank(values: impl IntoIterator<Item = Option<String>>) -> Option<String> {
    values
        .into_iter()
        .flatten()
        .find(|value| !value.trim().is_empty())
}
