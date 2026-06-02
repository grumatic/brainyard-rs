use by_mcp::{
    build_http_headers, call_tool_request, extract_jsonrpc_result_from_json,
    extract_jsonrpc_result_from_sse, get_prompt_request, http_initialize_request,
    initialized_notification, list_resources_request, list_tools_request, make_error_response,
    make_notification, make_request, make_response, parse_sse_events,
    project_tools_list_command_result, read_resource_request, registered_tool_id,
    safe_clojure_symbol_name, stdio_initialize_request, tools_from_list_result,
    validate_server_config, CLIENT_NAME, CLIENT_VERSION, JSON_RPC_VERSION, MCP_VERSION,
};
use by_registry::load_mcp_servers_path;
use serde_json::json;

const ORACLE_MCP_SERVERS: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../fixtures/oracle/mcp-servers.json"
);

#[test]
fn initialize_requests_match_clojure_transport_shapes() {
    assert_eq!(
        stdio_initialize_request(1),
        json!({
            "jsonrpc": JSON_RPC_VERSION,
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": MCP_VERSION,
                "capabilities": {"roots": {"listChanged": false}},
                "sampling": {},
                "clientInfo": {"name": CLIENT_NAME, "version": CLIENT_VERSION}
            }
        })
    );

    assert_eq!(
        http_initialize_request(2),
        json!({
            "jsonrpc": JSON_RPC_VERSION,
            "id": 2,
            "method": "initialize",
            "params": {
                "protocolVersion": MCP_VERSION,
                "capabilities": {"roots": {"listChanged": false}, "sampling": {}},
                "clientInfo": {"name": CLIENT_NAME, "version": CLIENT_VERSION}
            }
        })
    );
}

#[test]
fn jsonrpc_builders_match_clojure_message_contract() {
    assert_eq!(
        make_request(9, "tools/list", json!({})).unwrap(),
        json!({"jsonrpc": "2.0", "id": 9, "method": "tools/list", "params": {}})
    );
    assert_eq!(
        make_notification("notifications/initialized", json!({})).unwrap(),
        json!({"jsonrpc": "2.0", "method": "notifications/initialized", "params": {}})
    );
    assert_eq!(
        make_response(9, json!({"tools": []})),
        json!({"jsonrpc": "2.0", "id": 9, "result": {"tools": []}})
    );
    assert_eq!(
        make_error_response(9, -32000, "boom", Some(json!({"detail": "x"}))),
        json!({
            "jsonrpc": "2.0",
            "id": 9,
            "error": {"code": -32000, "message": "boom", "data": {"detail": "x"}}
        })
    );
    assert!(make_request(1, "tools/list", json!([])).is_err());
    assert!(make_notification("", json!({})).is_err());
}

#[test]
fn standard_request_helpers_match_mcp_params() {
    assert_eq!(
        initialized_notification(),
        json!({"jsonrpc": "2.0", "method": "notifications/initialized", "params": {}})
    );
    assert_eq!(
        list_resources_request(11),
        json!({"jsonrpc": "2.0", "id": 11, "method": "resources/list", "params": {}})
    );
    assert_eq!(
        read_resource_request(12, "file:///tmp/a.txt").unwrap(),
        json!({"jsonrpc": "2.0", "id": 12, "method": "resources/read", "params": {"uri": "file:///tmp/a.txt"}})
    );
    assert_eq!(
        list_tools_request(13),
        json!({"jsonrpc": "2.0", "id": 13, "method": "tools/list", "params": {}})
    );
    assert_eq!(
        call_tool_request(14, "search", json!({"query": "brainyard"})).unwrap(),
        json!({"jsonrpc": "2.0", "id": 14, "method": "tools/call", "params": {"name": "search", "arguments": {"query": "brainyard"}}})
    );
    assert_eq!(
        get_prompt_request(15, "summarize", json!({"topic": "mcp"})).unwrap(),
        json!({"jsonrpc": "2.0", "id": 15, "method": "prompts/get", "params": {"name": "summarize", "arguments": {"topic": "mcp"}}})
    );
    assert!(call_tool_request(1, "search", json!([])).is_err());
    assert!(read_resource_request(1, "").is_err());
}

#[test]
fn http_headers_match_streamable_transport_contract() {
    let auth = json!({
        "Authorization": "Bearer token",
        "X-API-Version": "2026-01-01"
    });
    let headers = build_http_headers(auth.as_object(), Some("session-1")).unwrap();

    assert_eq!(
        headers.get("Content-Type").map(String::as_str),
        Some("application/json")
    );
    assert_eq!(
        headers.get("Accept").map(String::as_str),
        Some("application/json, text/event-stream")
    );
    assert_eq!(
        headers.get("Authorization").map(String::as_str),
        Some("Bearer token")
    );
    assert_eq!(
        headers.get("X-API-Version").map(String::as_str),
        Some("2026-01-01")
    );
    assert_eq!(
        headers.get("Mcp-Session-Id").map(String::as_str),
        Some("session-1")
    );

    let invalid = json!({"Authorization": 123});
    assert!(build_http_headers(invalid.as_object(), None).is_err());
}

#[test]
fn sse_parser_matches_clojure_event_rules() {
    let events = parse_sse_events(
        r#": keepalive
ignored: line
event: message
data: {"jsonrpc":"2.0","method":"notifications/message","params":{"level":"info"}}

event: message
data: {"jsonrpc":"2.0",
data: "id":7,
data: "result":{"ok":true}}
"#,
    );

    assert_eq!(events.len(), 2);
    assert_eq!(events[0].event.as_deref(), Some("message"));
    assert_eq!(
        events[0].data,
        r#"{"jsonrpc":"2.0","method":"notifications/message","params":{"level":"info"}}"#
    );
    assert_eq!(events[1].event.as_deref(), Some("message"));
    assert_eq!(
        events[1].data,
        "{\"jsonrpc\":\"2.0\",\n\"id\":7,\n\"result\":{\"ok\":true}}"
    );
}

#[test]
fn sse_result_extraction_skips_notifications_and_mismatched_responses() {
    let body = r#": keepalive
event: message
data: {"jsonrpc":"2.0","method":"notifications/tools/list_changed","params":{}}

event: message
data: {"jsonrpc":"2.0","id":41,"result":{"wrong":true}}

event: message
data: {"jsonrpc":"2.0","id":42,"result":{"tools":[{"name":"search"}]}}
"#;

    let result = extract_jsonrpc_result_from_sse(body, 42).unwrap();
    assert_eq!(result, json!({"tools": [{"name": "search"}]}));
}

#[test]
fn json_response_extraction_matches_clojure_read_response_rules() {
    assert_eq!(
        extract_jsonrpc_result_from_json(
            r#"{"jsonrpc":"2.0","method":"notifications/message","params":{}}"#,
            1
        )
        .unwrap(),
        None
    );
    assert_eq!(
        extract_jsonrpc_result_from_json(
            r#"{"jsonrpc":"2.0","id":"3","result":{"content":[]}}"#,
            3
        )
        .unwrap(),
        Some(json!({"content": []}))
    );
    assert!(extract_jsonrpc_result_from_json(
        r#"{"jsonrpc":"2.0","id":3,"error":{"code":-32000,"message":"boom"}}"#,
        3
    )
    .is_err());
}

#[test]
fn tools_list_projection_matches_clojure_cache_shape() {
    let result = extract_jsonrpc_result_from_json(
        r#"{"jsonrpc":"2.0","id":7,"result":{"tools":[
          {"name":"read_file","description":"Read a file","inputSchema":{"type":"object","properties":{"path":{"type":"string"}},"required":["path"]}},
          {"name":"list-dir","parameters":{"type":"object","properties":{"root":{"type":"string"}}}}
        ]}}"#,
        7,
    )
    .unwrap()
    .unwrap();
    let tools = tools_from_list_result("filesystem", &result).unwrap();

    assert_eq!(tools.len(), 2);
    assert_eq!(tools[0].server_name, "filesystem");
    assert_eq!(tools[0].name, "read_file");
    assert_eq!(tools[0].description.as_deref(), Some("Read a file"));
    assert_eq!(tools[0].parameters["required"][0], "path");
    assert_eq!(tools[1].parameters["properties"]["root"]["type"], "string");

    assert_eq!(
        project_tools_list_command_result(&tools),
        json!({
            "result": {
                "tools": [
                    {
                        "server-name": "filesystem",
                        "name": "read_file",
                        "description": "Read a file",
                        "parameters": {"type":"object","properties":{"path":{"type":"string"}},"required":["path"]}
                    },
                    {
                        "server-name": "filesystem",
                        "name": "list-dir",
                        "description": null,
                        "parameters": {"type":"object","properties":{"root":{"type":"string"}}}
                    }
                ],
                "total": 2
            }
        })
    );
}

#[test]
fn registered_tool_ids_match_clojure_dynamic_tool_rules() {
    assert!(safe_clojure_symbol_name("filesystem"));
    assert!(safe_clojure_symbol_name("read_file"));
    assert!(safe_clojure_symbol_name("list-dir"));
    assert!(safe_clojure_symbol_name("ask?"));
    assert_eq!(
        registered_tool_id("filesystem", "read_file").unwrap(),
        "mcp$filesystem$read_file"
    );

    assert!(!safe_clojure_symbol_name("2bad"));
    assert!(!safe_clojure_symbol_name("bad/name"));
    assert!(registered_tool_id("filesystem", "bad/name").is_err());
}

#[test]
fn server_config_validation_matches_clojure_required_fields() {
    validate_server_config(
        "stdio",
        &json!({"command": "npx", "args": ["-y", "server"]}),
    )
    .unwrap();
    validate_server_config(
        "http",
        &json!({"url": "https://api.example.com", "headers": {"Authorization": "Bearer token"}}),
    )
    .unwrap();
    validate_server_config("http-sse", &json!({"url": "https://api.example.com"})).unwrap();

    assert!(validate_server_config("stdio", &json!({"command": "npx"})).is_err());
    assert!(validate_server_config("http", &json!({"headers": {}})).is_err());
    assert!(validate_server_config(
        "http",
        &json!({"url": "https://api.example.com", "headers": []})
    )
    .is_err());
}

#[test]
fn oracle_mcp_seed_configs_are_valid_by_transport() {
    let servers = load_mcp_servers_path(ORACLE_MCP_SERVERS).expect("MCP fixture should load");

    assert_eq!(servers.len(), 12);
    for server in servers {
        validate_server_config(&server.transport, &server.config)
            .unwrap_or_else(|error| panic!("{} config should validate: {error}", server.name));
    }
}
