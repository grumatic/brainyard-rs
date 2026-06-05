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
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, ExitStatus, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

const DEFAULT_BEDROCK_REGION: &str = "us-east-1";
const MAX_SYSTEM_CACHE_POINTS: usize = 3;
const CLAUDE_CODE_SYSTEM_PROMPT_SPOOL_THRESHOLD_BYTES: usize = 262_144;
const CLAUDE_CODE_SYSTEM_PROMPT_PLACEHOLDER: &str = "<spooled-system-prompt>";
const ACP_DEFAULT_TIMEOUT_MS: u64 = 600_000;

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
pub struct ClaudeCodeResponse {
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

#[derive(Clone, Debug, PartialEq)]
struct BedrockSdkConverseRequestParts {
    model_id: String,
    messages: Vec<Message>,
    system: Option<Vec<SystemContentBlock>>,
    inference_config: Option<InferenceConfiguration>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct BedrockRuntimeInputs {
    pub explicit_region: Option<String>,
    pub catalog_region: Option<String>,
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

pub fn build_claude_code_request(config: &ProviderChatConfig, messages: &[ChatMessage]) -> Value {
    let (system_prompt, prompt) = flatten_claude_code_messages(messages);
    let mut argv = vec![
        "claude".to_string(),
        "-p".to_string(),
        "--no-session-persistence".to_string(),
        "--output-format".to_string(),
        "json".to_string(),
        "--tools".to_string(),
        String::new(),
        "--setting-sources".to_string(),
        String::new(),
        "--strict-mcp-config".to_string(),
        "--disable-slash-commands".to_string(),
        "--max-turns".to_string(),
        "1".to_string(),
    ];

    if !config.model.trim().is_empty() {
        argv.extend(["--model".to_string(), config.model.clone()]);
    }
    if let Some(max_tokens) = config.max_tokens {
        argv.extend(["--max-tokens".to_string(), max_tokens.to_string()]);
    }

    let system_prompt_spooled = system_prompt
        .as_ref()
        .map(|prompt| prompt.len() > CLAUDE_CODE_SYSTEM_PROMPT_SPOOL_THRESHOLD_BYTES)
        .unwrap_or(false);
    if let Some(system_prompt) = system_prompt {
        if system_prompt_spooled {
            argv.extend([
                "--system-prompt-file".to_string(),
                "<spooled-system-prompt>".to_string(),
            ]);
        } else {
            argv.extend(["--system-prompt".to_string(), system_prompt]);
        }
    }

    json!({
        "argv": argv,
        "stdin": prompt,
        "system_prompt_spooled": system_prompt_spooled
    })
}

pub fn build_acp_request(config: &ProviderChatConfig, messages: &[ChatMessage]) -> Value {
    json!({
        "backend": "stub",
        "model": config.model.clone(),
        "prompt": [{"type": "text", "text": flatten_acp_messages(messages)}],
        "timeout_ms": ACP_DEFAULT_TIMEOUT_MS
    })
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
        "claude-code" => Ok(ProviderRequestProjection {
            operation: "claude-code/subprocess",
            request: build_claude_code_request(config, messages),
        }),
        "acp" => Ok(ProviderRequestProjection {
            operation: "acp/session-prompt",
            request: build_acp_request(config, messages),
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
            inputs.catalog_region,
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

pub fn invoke_claude_code(
    config: &ProviderChatConfig,
    messages: &[ChatMessage],
) -> Result<ClaudeCodeResponse> {
    let request = build_claude_code_request(config, messages);
    let mut argv = request
        .get("argv")
        .and_then(Value::as_array)
        .context("claude-code request missing argv")?
        .iter()
        .map(|arg| {
            arg.as_str()
                .map(str::to_string)
                .context("claude-code argv items must be strings")
        })
        .collect::<Result<Vec<_>>>()?;
    let _spooled_system_prompt =
        spool_claude_code_system_prompt_if_needed(&request, &mut argv, messages)?;
    let (program, args) = argv
        .split_first()
        .context("claude-code request argv must not be empty")?;
    let stdin_text = request.get("stdin").and_then(Value::as_str).unwrap_or("");

    let mut child = Command::new(program)
        .args(args)
        .env_remove("CLAUDECODE")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("failed to launch Claude Code CLI `{program}`"))?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(stdin_text.as_bytes())
            .context("failed to write prompt to Claude Code CLI")?;
    } else {
        bail!("failed to open Claude Code CLI stdin");
    }

    let output = child
        .wait_with_output()
        .context("failed to wait for Claude Code CLI")?;
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let raw = Value::String(stdout.clone());
    let projected = reshape_claude_code_response(raw.clone());
    let text = projected_response_text(&projected);

    if !output.status.success() && text.trim().is_empty() {
        let detail = if stderr.trim().is_empty() {
            stdout.trim()
        } else {
            stderr.trim()
        };
        bail!(
            "Claude Code CLI exited with {}: {}",
            exit_status_display(output.status),
            detail
        );
    }

    let stop_reason = projected
        .get("stop_reason")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    Ok(ClaudeCodeResponse {
        raw,
        projected,
        text,
        stop_reason,
    })
}

struct TempFileGuard {
    path: PathBuf,
}

impl Drop for TempFileGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

fn spool_claude_code_system_prompt_if_needed(
    request: &Value,
    argv: &mut [String],
    messages: &[ChatMessage],
) -> Result<Option<TempFileGuard>> {
    let spooled = request
        .get("system_prompt_spooled")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if !spooled {
        return Ok(None);
    }

    let (system_prompt, _) = flatten_claude_code_messages(messages);
    let system_prompt = system_prompt
        .context("claude-code request marked system prompt as spooled without system prompt")?;
    let guard = write_spooled_claude_code_system_prompt(&system_prompt)?;
    let path = guard.path.to_string_lossy().to_string();

    let mut replaced = false;
    for arg in argv.iter_mut() {
        if arg == CLAUDE_CODE_SYSTEM_PROMPT_PLACEHOLDER {
            *arg = path.clone();
            replaced = true;
        }
    }
    if !replaced {
        bail!("claude-code request marked system prompt as spooled without placeholder path");
    }

    Ok(Some(guard))
}

fn write_spooled_claude_code_system_prompt(system_prompt: &str) -> Result<TempFileGuard> {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();

    for attempt in 0..32 {
        let mut path = std::env::temp_dir();
        path.push(format!(
            "by-rs-claude-code-system-{}-{nonce}-{attempt}.txt",
            std::process::id()
        ));

        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut file) => {
                file.write_all(system_prompt.as_bytes()).with_context(|| {
                    format!(
                        "failed to write Claude Code system prompt file {}",
                        path.display()
                    )
                })?;
                return Ok(TempFileGuard { path });
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(error).with_context(|| {
                    format!(
                        "failed to create Claude Code system prompt file {}",
                        path.display()
                    )
                });
            }
        }
    }

    bail!("failed to allocate Claude Code system prompt temp file");
}

fn exit_status_display(status: ExitStatus) -> String {
    match status.code() {
        Some(code) => format!("code {code}"),
        None => "signal termination".to_string(),
    }
}

pub async fn converse_bedrock(request: BedrockConverseRequest) -> Result<BedrockConverseResponse> {
    let sdk_request = build_bedrock_sdk_converse_request(&request)?;
    let sdk_config = load_bedrock_sdk_config(&request.runtime).await;
    let client = Client::new(&sdk_config);

    let mut builder = client
        .converse()
        .model_id(sdk_request.model_id)
        .set_messages(Some(sdk_request.messages));

    if let Some(system) = sdk_request.system {
        builder = builder.set_system(Some(system));
    }

    if let Some(inference) = sdk_request.inference_config {
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

pub fn project_bedrock_sdk_converse_request(request: &BedrockConverseRequest) -> Result<Value> {
    let sdk_request = build_bedrock_sdk_converse_request(request)?;
    Ok(bedrock_sdk_converse_request_value(&sdk_request))
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

pub fn reshape_claude_code_response(response: Value) -> Value {
    let events = claude_code_events(&response);
    let result_event = claude_code_result_event(&events);
    let result_text = result_event
        .and_then(|event| event.get("result"))
        .and_then(Value::as_str)
        .filter(|text| !text.trim().is_empty())
        .map(str::to_string);
    let text = claude_code_structured_output(&events)
        .or(result_text)
        .or_else(|| claude_code_assistant_text(&events))
        .or_else(|| response.as_str().map(str::to_string))
        .unwrap_or_default();

    let mut projected = Map::new();
    projected.insert(
        "content".to_string(),
        Value::Array(vec![json!({"type": "text", "text": text})]),
    );
    projected.insert("role".to_string(), Value::String("assistant".to_string()));
    projected.insert(
        "stop_reason".to_string(),
        claude_code_stop_reason(result_event).unwrap_or(Value::Null),
    );

    if let Some(usage) = result_event
        .and_then(|event| event.get("usage"))
        .and_then(Value::as_object)
    {
        let usage = project_anthropic_usage(usage);
        if !usage.is_empty() {
            projected.insert("usage".to_string(), Value::Object(usage));
        }
    }

    Value::Object(projected)
}

pub fn reshape_acp_response(response: Value) -> Value {
    let mut text_blocks = anthropic_text_blocks(response.get("content"));
    if text_blocks.is_empty() {
        if let Some(text) = response
            .get("result")
            .or_else(|| response.get("text"))
            .and_then(Value::as_str)
        {
            text_blocks.push(json!({"type": "text", "text": text}));
        }
    }

    let mut projected = Map::new();
    projected.insert("content".to_string(), Value::Array(text_blocks));
    projected.insert("role".to_string(), Value::String("assistant".to_string()));
    projected.insert(
        "stop_reason".to_string(),
        response
            .get("stop_reason")
            .or_else(|| response.get("stop-reason"))
            .cloned()
            .unwrap_or(Value::Null),
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
        "claude-code" => Ok(reshape_claude_code_response(response)),
        "acp" => Ok(reshape_acp_response(response)),
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

fn flatten_claude_code_messages(messages: &[ChatMessage]) -> (Option<String>, String) {
    let system_prompt = collect_system_text(messages);
    let other_messages = messages
        .iter()
        .filter(|message| message.role != "system")
        .collect::<Vec<_>>();
    let prompt = if other_messages.len() == 1 {
        other_messages[0].content.clone()
    } else {
        other_messages
            .iter()
            .map(|message| format!("[{}]: {}", message.role, message.content))
            .collect::<Vec<_>>()
            .join("\n\n")
    };
    (system_prompt, prompt)
}

fn flatten_acp_messages(messages: &[ChatMessage]) -> String {
    let system_text = collect_system_text(messages);
    let other_messages = messages
        .iter()
        .filter(|message| message.role != "system")
        .collect::<Vec<_>>();
    let body = if other_messages.len() == 1 {
        other_messages[0].content.clone()
    } else {
        other_messages
            .iter()
            .map(|message| format!("[{}]: {}", message.role, message.content))
            .collect::<Vec<_>>()
            .join("\n\n")
    };

    match system_text {
        Some(system_text) if !body.is_empty() => format!("{system_text}\n\n{body}"),
        Some(system_text) => system_text,
        None => body,
    }
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

fn build_bedrock_sdk_converse_request(
    request: &BedrockConverseRequest,
) -> Result<BedrockSdkConverseRequestParts> {
    Ok(BedrockSdkConverseRequestParts {
        model_id: request.config.model.clone(),
        messages: build_sdk_messages(&request.config, &request.messages)?,
        system: build_sdk_system_blocks(&request.config, &request.messages, &request.cache_zones)?,
        inference_config: build_sdk_inference_config(&request.config)?,
    })
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

fn bedrock_sdk_converse_request_value(request: &BedrockSdkConverseRequestParts) -> Value {
    let mut root = Map::new();
    root.insert(
        "modelId".to_string(),
        Value::String(request.model_id.clone()),
    );
    root.insert(
        "messages".to_string(),
        Value::Array(request.messages.iter().map(message_input_value).collect()),
    );

    if let Some(inference) = &request.inference_config {
        root.insert(
            "inferenceConfig".to_string(),
            Value::Object(inference_config_value(inference)),
        );
    }

    if let Some(system) = &request.system {
        root.insert(
            "system".to_string(),
            Value::Array(system.iter().filter_map(system_input_block_value).collect()),
        );
    }

    Value::Object(root)
}

fn message_input_value(message: &Message) -> Value {
    json!({
        "role": message.role().as_str(),
        "content": message.content().iter().filter_map(content_input_block_value).collect::<Vec<_>>(),
    })
}

fn content_input_block_value(block: &ContentBlock) -> Option<Value> {
    if let Ok(text) = block.as_text() {
        return Some(json!({"text": text}));
    }
    if let Ok(cache_point) = block.as_cache_point() {
        return Some(cache_point_value(cache_point));
    }
    None
}

fn system_input_block_value(block: &SystemContentBlock) -> Option<Value> {
    if let Ok(text) = block.as_text() {
        return Some(json!({"text": text}));
    }
    if let Ok(cache_point) = block.as_cache_point() {
        return Some(cache_point_value(cache_point));
    }
    None
}

fn cache_point_value(cache_point: &CachePointBlock) -> Value {
    json!({"cachePoint": {"type": cache_point.r#type().as_str()}})
}

fn inference_config_value(inference: &InferenceConfiguration) -> Map<String, Value> {
    let mut value = Map::new();
    if let Some(temperature) = inference.temperature() {
        value.insert("temperature".to_string(), json!(temperature));
    }
    if let Some(max_tokens) = inference.max_tokens() {
        value.insert("maxTokens".to_string(), json!(max_tokens));
    }
    if let Some(top_p) = inference.top_p() {
        value.insert("topP".to_string(), json!(top_p));
    }
    let stop_sequences = inference.stop_sequences();
    if !stop_sequences.is_empty() {
        value.insert("stopSequences".to_string(), json!(stop_sequences));
    }
    value
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

fn claude_code_events(response: &Value) -> Vec<Value> {
    if let Some(stdout) = response.get("stdout").and_then(Value::as_str) {
        return parse_claude_code_stdout(stdout);
    }
    if let Some(stdout) = response.as_str() {
        return parse_claude_code_stdout(stdout);
    }
    if let Some(events) = response.as_array() {
        return events.clone();
    }
    if response.is_object() {
        return vec![response.clone()];
    }
    Vec::new()
}

fn parse_claude_code_stdout(stdout: &str) -> Vec<Value> {
    let trimmed = stdout.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }
    if let Ok(parsed) = serde_json::from_str::<Value>(trimmed) {
        return match parsed {
            Value::Array(events) => events,
            Value::Object(_) => vec![parsed],
            _ => Vec::new(),
        };
    }

    trimmed
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line.trim()).ok())
        .collect()
}

fn claude_code_result_event(events: &[Value]) -> Option<&Value> {
    events.iter().rev().find(|event| {
        event
            .get("type")
            .and_then(Value::as_str)
            .is_some_and(|event_type| event_type == "result")
    })
}

fn claude_code_structured_output(events: &[Value]) -> Option<String> {
    events
        .iter()
        .filter(|event| {
            event
                .get("type")
                .and_then(Value::as_str)
                .is_some_and(|event_type| event_type == "assistant")
        })
        .filter_map(|event| event.pointer("/message/content").and_then(Value::as_array))
        .flat_map(|blocks| blocks.iter())
        .find_map(|block| {
            let object = block.as_object()?;
            let block_type = object.get("type").and_then(Value::as_str);
            let name = object.get("name").and_then(Value::as_str);
            if block_type != Some("tool_use") || name != Some("StructuredOutput") {
                return None;
            }
            let input = object.get("input")?;
            input
                .as_str()
                .map(str::to_string)
                .or_else(|| serde_json::to_string(input).ok())
        })
}

fn claude_code_assistant_text(events: &[Value]) -> Option<String> {
    let text = events
        .iter()
        .filter(|event| {
            event
                .get("type")
                .and_then(Value::as_str)
                .is_some_and(|event_type| event_type == "assistant")
        })
        .filter_map(|event| event.pointer("/message/content").and_then(Value::as_array))
        .flat_map(|blocks| blocks.iter())
        .filter_map(|block| {
            let object = block.as_object()?;
            if object.get("type").and_then(Value::as_str) == Some("text") {
                object.get("text").and_then(Value::as_str)
            } else {
                None
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    (!text.is_empty()).then_some(text)
}

fn claude_code_stop_reason(result_event: Option<&Value>) -> Option<Value> {
    let event = result_event?;
    event
        .get("stop_reason")
        .or_else(|| event.get("stop-reason"))
        .or_else(|| event.get("terminal_reason"))
        .cloned()
        .or_else(|| {
            let subtype = event.get("subtype").and_then(Value::as_str);
            matches!(subtype, Some("success")).then(|| Value::String("end_turn".to_string()))
        })
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
