#![forbid(unsafe_code)]

use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, BTreeSet};

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

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct McpTool {
    pub server_name: String,
    pub name: String,
    pub description: Option<String>,
    pub parameters: Value,
}

#[derive(Clone, Debug, PartialEq)]
pub struct McpToolCall {
    pub server_name: String,
    pub tool_name: String,
    pub tool_args: Value,
    pub arguments: Value,
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

pub fn tools_from_list_result(server_name: &str, result: &Value) -> Result<Vec<McpTool>> {
    ensure_nonblank(server_name, "server_name")?;
    let object = result
        .as_object()
        .ok_or_else(|| anyhow!("MCP tools/list result must be an object"))?;
    let tools = object
        .get("tools")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("MCP tools/list result requires tools array"))?;

    tools
        .iter()
        .map(|tool| tool_from_list_value(server_name, tool))
        .collect()
}

pub fn project_tools_list_command_result(tools: &[McpTool]) -> Value {
    let projected_tools = tools
        .iter()
        .map(|tool| {
            json!({
                "server-name": tool.server_name,
                "name": tool.name,
                "description": tool.description,
                "parameters": tool.parameters,
            })
        })
        .collect::<Vec<_>>();

    json!({
        "result": {
            "tools": projected_tools,
            "total": tools.len(),
        }
    })
}

pub fn project_server_resources_command_result(
    server_name: &str,
    resources: Value,
) -> Result<Value> {
    ensure_nonblank(server_name, "server_name")?;
    Ok(json!({
        "result": {
            "name": server_name,
            "resources": resources,
        }
    }))
}

pub fn project_server_prompts_command_result(server_name: &str, prompts: Value) -> Result<Value> {
    ensure_nonblank(server_name, "server_name")?;
    Ok(json!({
        "result": {
            "name": server_name,
            "prompts": prompts,
        }
    }))
}

pub fn project_server_info_command_result(server_name: &str, server_info: Value) -> Result<Value> {
    ensure_nonblank(server_name, "server_name")?;
    Ok(json!({
        "result": {
            "name": server_name,
            "server-info": server_info,
        }
    }))
}

pub fn project_server_capabilities_command_result(
    server_name: &str,
    capabilities: Value,
) -> Result<Value> {
    ensure_nonblank(server_name, "server_name")?;
    Ok(json!({
        "result": {
            "name": server_name,
            "capabilities": capabilities,
        }
    }))
}

pub fn project_server_health_command_result(
    server_name: &str,
    status: &str,
    timestamp_ms: u64,
) -> Result<Value> {
    ensure_nonblank(server_name, "server_name")?;
    ensure_nonblank(status, "status")?;
    Ok(json!({
        "result": {
            "status": status,
            "timestamp": timestamp_ms,
            "name": server_name,
        }
    }))
}

pub fn project_lifecycle_command_result(server_name: &str, op: &str) -> Result<Value> {
    ensure_nonblank(server_name, "server_name")?;
    let verb = match op {
        "start" => "started",
        "stop" => "stopped",
        "restart" => "restarted",
        other => bail!("unsupported MCP lifecycle op '{other}'"),
    };
    Ok(json!({
        "result": format!("MCP server '{server_name}' {verb} successfully")
    }))
}

pub fn mcp_input_schema_to_malli(schema: &Value) -> Value {
    let properties = object_field(schema, "properties")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let required = required_field_names(schema);
    let mut map_schema = vec![json!("map")];

    for (name, spec) in properties {
        let optional = !required.contains(&name);
        let inner_schema = json_schema_to_malli(&spec, false);
        map_schema.push(malli_map_field(&name, optional, inner_schema));
    }

    Value::Array(map_schema)
}

pub fn project_registered_tool_descriptors(tools: &[McpTool]) -> Vec<Value> {
    tools
        .iter()
        .filter_map(|tool| {
            let id = registered_tool_id(&tool.server_name, &tool.name).ok()?;
            Some(json!({
                "id": id,
                "type": "tool",
                "description": tool.description.as_deref().unwrap_or("MCP tool"),
                "input-schema": mcp_input_schema_to_malli(&tool.parameters),
                "output-schema": ["map"],
                "mcp-server": tool.server_name,
                "mcp-tool": tool.name,
            }))
        })
        .collect()
}

pub fn project_registered_tools_command_result(tools: &[McpTool]) -> Value {
    let projected_tools = project_registered_tool_descriptors(tools);
    json!({
        "result": {
            "tools": projected_tools,
            "total": projected_tools.len(),
        }
    })
}

pub fn normalize_tool_args(tool_args: &Value) -> Value {
    if let Some(object) = tool_args.as_object() {
        return Value::Object(string_keyed_map(object));
    }

    let Some(entries) = tool_args.as_array() else {
        return json!({});
    };

    let named_args = entries
        .iter()
        .filter_map(|entry| {
            let object = entry.as_object()?;
            let name = object.get("name").and_then(Value::as_str)?;
            let value = object.get("value")?;
            Some((name.to_string(), value.clone()))
        })
        .collect::<Map<_, _>>();

    if !named_args.is_empty() {
        return Value::Object(named_args);
    }

    let mut flattened = Map::new();
    for entry in entries {
        if let Some(object) = entry.as_object() {
            flattened.extend(string_keyed_map(object));
        }
    }
    Value::Object(flattened)
}

pub fn tool_calls_from_value(value: &Value) -> Result<Vec<McpToolCall>> {
    let calls = value
        .as_array()
        .filter(|calls| !calls.is_empty())
        .ok_or_else(|| anyhow!("tool-calls must be a non-empty array"))?;

    calls.iter().map(tool_call_from_value).collect()
}

pub fn tool_call_request_from_call(request_id: u64, call: &McpToolCall) -> Result<Value> {
    call_tool_request(request_id, &call.tool_name, call.arguments.clone())
}

pub fn project_tool_calls_command_result(
    calls: &[McpToolCall],
    tool_results: &[Value],
) -> Result<Value> {
    if calls.len() != tool_results.len() {
        bail!(
            "tool call count ({}) must match result count ({})",
            calls.len(),
            tool_results.len()
        );
    }

    let results = calls
        .iter()
        .zip(tool_results)
        .map(|(call, result)| {
            json!({
                "server-name": call.server_name,
                "tool-name": call.tool_name,
                "tool-args": call.tool_args,
                "tool-result": {
                    "success": true,
                    "result": result,
                }
            })
        })
        .collect::<Vec<_>>();
    let total = results.len();

    Ok(json!({
        "result": {
            "tool-results": results,
            "total": total,
        }
    }))
}

pub fn project_read_resource_command_result(
    server_name: &str,
    resource_uri: &str,
    resource: Value,
) -> Result<Value> {
    ensure_nonblank(server_name, "server_name")?;
    ensure_nonblank(resource_uri, "resource_uri")?;
    Ok(json!({
        "result": {
            "name": server_name,
            "uri": resource_uri,
            "resource": resource,
        }
    }))
}

pub fn project_get_prompt_command_result(
    server_name: &str,
    prompt_name: &str,
    prompt: Value,
) -> Result<Value> {
    ensure_nonblank(server_name, "server_name")?;
    ensure_nonblank(prompt_name, "prompt_name")?;
    Ok(json!({
        "result": {
            "name": server_name,
            "prompt-name": prompt_name,
            "prompt": prompt,
        }
    }))
}

pub fn registered_tool_id(server_name: &str, tool_name: &str) -> Result<String> {
    ensure_nonblank(server_name, "server_name")?;
    ensure_nonblank(tool_name, "tool_name")?;
    if !safe_clojure_symbol_name(server_name) {
        bail!("MCP server name '{server_name}' cannot be registered as a Clojure symbol");
    }
    if !safe_clojure_symbol_name(tool_name) {
        bail!("MCP tool name '{tool_name}' cannot be registered as a Clojure symbol");
    }
    Ok(format!("mcp${server_name}${tool_name}"))
}

pub fn safe_clojure_symbol_name(value: &str) -> bool {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first.is_ascii_alphabetic() || first == '_') {
        return false;
    }
    chars.all(|ch| {
        ch.is_ascii_alphanumeric()
            || matches!(ch, '_' | '-' | '.' | '+' | '!' | '?' | '<' | '>' | '=')
    })
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

fn tool_from_list_value(server_name: &str, value: &Value) -> Result<McpTool> {
    let object = value
        .as_object()
        .ok_or_else(|| anyhow!("MCP tool descriptor must be an object"))?;
    let name = object
        .get("name")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| anyhow!("MCP tool descriptor requires name"))?;
    let description = object
        .get("description")
        .and_then(Value::as_str)
        .map(ToString::to_string);
    let parameters = object
        .get("inputSchema")
        .or_else(|| object.get("input_schema"))
        .or_else(|| object.get("parameters"))
        .cloned()
        .unwrap_or_else(|| json!({}));

    Ok(McpTool {
        server_name: server_name.to_string(),
        name: name.to_string(),
        description,
        parameters,
    })
}

fn tool_call_from_value(value: &Value) -> Result<McpToolCall> {
    let object = value
        .as_object()
        .ok_or_else(|| anyhow!("MCP tool call must be an object"))?;
    let server_name = object_string_any(object, &["server-name", "server_name", "serverName"])
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| anyhow!("MCP tool call requires server-name"))?;
    let tool_name = object_string_any(object, &["tool-name", "tool_name", "toolName"])
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| anyhow!("MCP tool call requires tool-name"))?;
    let tool_args = object
        .get("tool-args")
        .or_else(|| object.get("tool_args"))
        .or_else(|| object.get("toolArgs"))
        .or_else(|| object.get("parameters"))
        .or_else(|| object.get("args"))
        .or_else(|| object.get("arguments"))
        .cloned()
        .unwrap_or_else(|| json!([]));
    let arguments = normalize_tool_args(&tool_args);

    Ok(McpToolCall {
        server_name: server_name.to_string(),
        tool_name: tool_name.to_string(),
        tool_args,
        arguments,
    })
}

fn string_keyed_map(object: &Map<String, Value>) -> Map<String, Value> {
    object
        .iter()
        .map(|(key, value)| (key.to_string(), value.clone()))
        .collect()
}

fn json_schema_to_malli(spec: &Value, optional: bool) -> Value {
    let type_name = object_field(spec, "type").and_then(Value::as_str);
    let enum_values = object_field(spec, "enum").and_then(Value::as_array);
    let items = object_field(spec, "items");
    let properties = object_field(spec, "properties").and_then(Value::as_object);
    let required = required_field_names(spec);

    let bare_schema = if let Some(values) = enum_values {
        let mut schema = vec![json!("enum")];
        schema.extend(values.iter().cloned());
        Value::Array(schema)
    } else {
        match type_name {
            Some("string") => json!("string"),
            Some("integer") => json!("int"),
            Some("number") => json!(["or", "int", "double"]),
            Some("boolean") => json!("boolean"),
            Some("array") => {
                let item_schema = items
                    .map(|item| json_schema_to_malli(item, false))
                    .unwrap_or_else(|| json!("any"));
                json!(["vector", item_schema])
            }
            Some("object") => {
                if let Some(properties) = properties.filter(|properties| !properties.is_empty()) {
                    let mut schema = vec![json!("map")];
                    for (name, nested_spec) in properties {
                        let nested_optional = !required.contains(name);
                        let inner_schema = json_schema_to_malli(nested_spec, nested_optional);
                        schema.push(malli_map_field(name, nested_optional, inner_schema));
                    }
                    Value::Array(schema)
                } else {
                    json!("map")
                }
            }
            _ => json!("any"),
        }
    };

    attach_malli_meta(bare_schema, malli_meta(spec, optional))
}

fn malli_map_field(name: &str, optional: bool, schema: Value) -> Value {
    if optional {
        json!([name, {"optional": true}, schema])
    } else {
        json!([name, schema])
    }
}

fn malli_meta(spec: &Value, optional: bool) -> Map<String, Value> {
    let mut meta = Map::new();
    if let Some(description) = object_field(spec, "description").and_then(Value::as_str) {
        meta.insert("desc".to_string(), json!(description));
    }
    if let Some(default) = object_field(spec, "default") {
        meta.insert("default".to_string(), default.clone());
    }
    if optional {
        meta.insert("optional".to_string(), json!(true));
    }
    meta
}

fn attach_malli_meta(schema: Value, meta: Map<String, Value>) -> Value {
    if meta.is_empty() {
        return schema;
    }

    match schema {
        Value::String(name) => Value::Array(vec![Value::String(name), Value::Object(meta)]),
        Value::Array(mut values) if matches!(values.first(), Some(Value::String(_))) => {
            if matches!(values.get(1), Some(Value::Object(_))) {
                let mut merged = values
                    .get(1)
                    .and_then(Value::as_object)
                    .cloned()
                    .unwrap_or_default();
                for (key, value) in meta {
                    merged.insert(key, value);
                }
                values[1] = Value::Object(merged);
            } else {
                values.insert(1, Value::Object(meta));
            }
            Value::Array(values)
        }
        other => other,
    }
}

fn object_field<'a>(value: &'a Value, key: &str) -> Option<&'a Value> {
    value.as_object()?.get(key)
}

fn object_string_any<'a>(object: &'a Map<String, Value>, keys: &[&str]) -> Option<&'a str> {
    keys.iter()
        .find_map(|key| object.get(*key).and_then(Value::as_str))
}

fn required_field_names(value: &Value) -> BTreeSet<String> {
    object_field(value, "required")
        .and_then(Value::as_array)
        .map(|required| {
            required
                .iter()
                .filter_map(Value::as_str)
                .map(ToString::to_string)
                .collect()
        })
        .unwrap_or_default()
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
