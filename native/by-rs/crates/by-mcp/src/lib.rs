#![forbid(unsafe_code)]

use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;

pub const MCP_VERSION: &str = "2024-11-05";
pub const JSON_RPC_VERSION: &str = "2.0";
pub const CLIENT_NAME: &str = "Brainyard Agent";
pub const CLIENT_VERSION: &str = "1.0.0";

pub mod methods {
    pub const INITIALIZE: &str = "initialize";
    pub const INITIALIZED: &str = "notifications/initialized";
    pub const PING: &str = "ping";
    pub const LIST_RESOURCES: &str = "resources/list";
    pub const READ_RESOURCE: &str = "resources/read";
    pub const LIST_TOOLS: &str = "tools/list";
    pub const CALL_TOOL: &str = "tools/call";
    pub const LIST_PROMPTS: &str = "prompts/list";
    pub const GET_PROMPT: &str = "prompts/get";
    pub const COMPLETE: &str = "completion/complete";
    pub const SET_LEVEL: &str = "logging/setLevel";
    pub const RESOURCES_LIST_CHANGED: &str = "notifications/resources/list_changed";
    pub const TOOLS_LIST_CHANGED: &str = "notifications/tools/list_changed";
    pub const PROMPTS_LIST_CHANGED: &str = "notifications/prompts/list_changed";
    pub const NOTIFICATION_MESSAGE: &str = "notifications/message";
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SseEvent {
    pub event: Option<String>,
    pub data: String,
}

pub fn make_request(id: u64, method: &str, params: Value) -> Result<Value> {
    ensure_method(method)?;
    ensure_object(&params, "params")?;
    Ok(json!({
        "jsonrpc": JSON_RPC_VERSION,
        "id": id,
        "method": method,
        "params": params
    }))
}

pub fn make_notification(method: &str, params: Value) -> Result<Value> {
    ensure_method(method)?;
    ensure_object(&params, "params")?;
    Ok(json!({
        "jsonrpc": JSON_RPC_VERSION,
        "method": method,
        "params": params
    }))
}

pub fn make_response(id: u64, result: Value) -> Value {
    json!({
        "jsonrpc": JSON_RPC_VERSION,
        "id": id,
        "result": result
    })
}

pub fn make_error_response(
    id: u64,
    error_code: i64,
    error_message: &str,
    error_data: Option<Value>,
) -> Value {
    let mut error = Map::new();
    error.insert("code".to_string(), json!(error_code));
    error.insert("message".to_string(), json!(error_message));
    if let Some(data) = error_data {
        error.insert("data".to_string(), data);
    }

    json!({
        "jsonrpc": JSON_RPC_VERSION,
        "id": id,
        "error": Value::Object(error)
    })
}

pub fn stdio_initialize_request(id: u64) -> Value {
    make_request(
        id,
        methods::INITIALIZE,
        json!({
            "protocolVersion": MCP_VERSION,
            "capabilities": {
                "roots": {"listChanged": false}
            },
            "sampling": {},
            "clientInfo": {
                "name": CLIENT_NAME,
                "version": CLIENT_VERSION
            }
        }),
    )
    .expect("static stdio initialize request is valid")
}

pub fn http_initialize_request(id: u64) -> Value {
    make_request(
        id,
        methods::INITIALIZE,
        json!({
            "protocolVersion": MCP_VERSION,
            "capabilities": {
                "roots": {"listChanged": false},
                "sampling": {}
            },
            "clientInfo": {
                "name": CLIENT_NAME,
                "version": CLIENT_VERSION
            }
        }),
    )
    .expect("static HTTP initialize request is valid")
}

pub fn initialized_notification() -> Value {
    make_notification(methods::INITIALIZED, json!({}))
        .expect("static initialized notification is valid")
}

pub fn ping_request(id: u64) -> Value {
    make_request(id, methods::PING, json!({})).expect("static ping request is valid")
}

pub fn list_resources_request(id: u64) -> Value {
    make_request(id, methods::LIST_RESOURCES, json!({}))
        .expect("static list resources request is valid")
}

pub fn read_resource_request(id: u64, uri: &str) -> Result<Value> {
    ensure_nonblank(uri, "uri")?;
    make_request(id, methods::READ_RESOURCE, json!({"uri": uri}))
}

pub fn list_tools_request(id: u64) -> Value {
    make_request(id, methods::LIST_TOOLS, json!({})).expect("static list tools request is valid")
}

pub fn call_tool_request(id: u64, name: &str, arguments: Value) -> Result<Value> {
    ensure_nonblank(name, "name")?;
    ensure_object(&arguments, "arguments")?;
    make_request(
        id,
        methods::CALL_TOOL,
        json!({"name": name, "arguments": arguments}),
    )
}

pub fn list_prompts_request(id: u64) -> Value {
    make_request(id, methods::LIST_PROMPTS, json!({}))
        .expect("static list prompts request is valid")
}

pub fn get_prompt_request(id: u64, name: &str, arguments: Value) -> Result<Value> {
    ensure_nonblank(name, "name")?;
    ensure_object(&arguments, "arguments")?;
    make_request(
        id,
        methods::GET_PROMPT,
        json!({"name": name, "arguments": arguments}),
    )
}

pub fn build_http_headers(
    auth_headers: Option<&Map<String, Value>>,
    session_id: Option<&str>,
) -> Result<BTreeMap<String, String>> {
    let mut headers = BTreeMap::from([
        (
            "Accept".to_string(),
            "application/json, text/event-stream".to_string(),
        ),
        ("Content-Type".to_string(), "application/json".to_string()),
    ]);

    if let Some(auth_headers) = auth_headers {
        for (key, value) in auth_headers {
            if key.trim().is_empty() {
                bail!("HTTP header names must not be blank");
            }
            let value = value
                .as_str()
                .ok_or_else(|| anyhow!("HTTP header '{key}' must be a string"))?;
            headers.insert(key.clone(), value.to_string());
        }
    }

    if let Some(session_id) = session_id.filter(|value| !value.trim().is_empty()) {
        headers.insert("Mcp-Session-Id".to_string(), session_id.to_string());
    }

    Ok(headers)
}

pub fn parse_sse_events(body_text: &str) -> Vec<SseEvent> {
    let mut events = Vec::new();
    let mut current_event: Option<String> = None;
    let mut current_data: Option<String> = None;

    for raw_line in body_text.lines() {
        let line = raw_line.trim_end_matches('\r');
        if line.trim().is_empty() {
            push_sse_event(&mut events, &mut current_event, &mut current_data);
            continue;
        }
        if line.starts_with(':') {
            continue;
        }
        if let Some(value) = line.strip_prefix("event:") {
            current_event = Some(value.trim().to_string());
        } else if let Some(value) = line.strip_prefix("data:") {
            let value = value.trim();
            if let Some(data) = &mut current_data {
                data.push('\n');
                data.push_str(value);
            } else {
                current_data = Some(value.to_string());
            }
        }
    }

    push_sse_event(&mut events, &mut current_event, &mut current_data);
    events
}

pub fn extract_jsonrpc_result_from_sse(body_text: &str, request_id: u64) -> Result<Value> {
    extract_jsonrpc_result_from_events(&parse_sse_events(body_text), request_id)
}

pub fn extract_jsonrpc_result_from_events(events: &[SseEvent], request_id: u64) -> Result<Value> {
    for event in events {
        let parsed: Value = serde_json::from_str(&event.data).context("invalid SSE JSON data")?;
        let Some(object) = parsed.as_object() else {
            continue;
        };
        let Some(id) = object.get("id") else {
            continue;
        };
        if !jsonrpc_id_matches(id, request_id) {
            continue;
        }
        if let Some(error) = object.get("error") {
            bail!("MCP request failed: {error}");
        }
        return object
            .get("result")
            .cloned()
            .ok_or_else(|| anyhow!("MCP response missing result for request id {request_id}"));
    }

    bail!("MCP response for request id {request_id} was not found")
}

pub fn extract_jsonrpc_result_from_json(body_text: &str, request_id: u64) -> Result<Option<Value>> {
    let parsed: Value = serde_json::from_str(body_text).context("invalid JSON-RPC body")?;
    let Some(object) = parsed.as_object() else {
        bail!("JSON-RPC body must be an object");
    };
    let Some(id) = object.get("id") else {
        return Ok(None);
    };
    if !jsonrpc_id_matches(id, request_id) {
        bail!("unexpected JSON-RPC response id {id}; expected {request_id}");
    }
    if let Some(error) = object.get("error") {
        bail!("MCP request failed: {error}");
    }
    Ok(object.get("result").cloned())
}

pub fn validate_server_config(transport: &str, config: &Value) -> Result<()> {
    let transport = normalize_transport(transport);
    let config = config
        .as_object()
        .ok_or_else(|| anyhow!("MCP server config must be a JSON object"))?;

    match transport.as_str() {
        "stdio" => {
            ensure_string_field(config, "command", "STDIO config requires command")?;
            ensure_array_field(config, "args", "STDIO config requires args array")?;
        }
        "http" | "http-sse" | "http+sse" => {
            ensure_string_field(config, "url", "HTTP config requires url")?;
            if let Some(headers) = config.get("headers") {
                if !headers.is_object() {
                    bail!("HTTP config headers must be an object when provided");
                }
            }
        }
        _ => bail!("unknown MCP transport '{transport}'"),
    }

    Ok(())
}

fn push_sse_event(
    events: &mut Vec<SseEvent>,
    current_event: &mut Option<String>,
    current_data: &mut Option<String>,
) {
    if let Some(data) = current_data.take() {
        events.push(SseEvent {
            event: current_event.take(),
            data,
        });
    } else {
        *current_event = None;
    }
}

fn jsonrpc_id_matches(id: &Value, request_id: u64) -> bool {
    match id {
        Value::Number(number) => number.as_u64() == Some(request_id),
        Value::String(value) => value == &request_id.to_string(),
        _ => false,
    }
}

fn normalize_transport(transport: &str) -> String {
    transport.trim().to_ascii_lowercase()
}

fn ensure_method(method: &str) -> Result<()> {
    ensure_nonblank(method, "method")
}

fn ensure_nonblank(value: &str, label: &str) -> Result<()> {
    if value.trim().is_empty() {
        bail!("{label} must not be blank");
    }
    Ok(())
}

fn ensure_object(value: &Value, label: &str) -> Result<()> {
    if !value.is_object() {
        bail!("{label} must be an object");
    }
    Ok(())
}

fn ensure_string_field<'a>(
    object: &'a Map<String, Value>,
    key: &str,
    message: &str,
) -> Result<&'a str> {
    object
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| anyhow!(message.to_string()))
}

fn ensure_array_field<'a>(
    object: &'a Map<String, Value>,
    key: &str,
    message: &str,
) -> Result<&'a Vec<Value>> {
    object
        .get(key)
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!(message.to_string()))
}
