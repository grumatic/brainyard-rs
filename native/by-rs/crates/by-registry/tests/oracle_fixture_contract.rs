use by_registry::{
    load_embedded_oracle_registry, load_mcp_servers_path, load_registry_path, load_tools_path,
};

const ORACLE_REGISTRY: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../fixtures/oracle/registry.json"
);
const ORACLE_TOOLS: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../fixtures/oracle/tools.json"
);
const ORACLE_MCP_SERVERS: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../fixtures/oracle/mcp-servers.json"
);

#[test]
fn clojure_oracle_registry_fixture_is_loadable() {
    let registry =
        load_registry_path(ORACLE_REGISTRY).expect("Clojure oracle registry fixture should load");

    assert!(
        registry
            .agents
            .iter()
            .any(|agent| agent.id == "coact-agent" && agent.agent_type == "agent"),
        "oracle fixture should include the built-in coact-agent as an agent entry"
    );
    assert!(
        registry.agents.iter().any(|agent| agent.id == "main-agent"),
        "oracle fixture should include the built-in main-agent"
    );
    assert!(
        registry
            .agents
            .iter()
            .any(|agent| agent.id == "debug-agent" && agent.max_iterations == Some(30)),
        "oracle fixture should preserve agent max-iterations metadata"
    );
    assert!(
        registry
            .models
            .iter()
            .any(|model| model.provider == "bedrock"),
        "oracle fixture should include at least one Bedrock model"
    );
    assert!(
        registry.tools.iter().any(|tool| tool.id == "code$eval"),
        "oracle fixture should include the code$eval command contract"
    );
    assert!(
        registry
            .tools
            .iter()
            .any(|tool| tool.id == "grep" && tool.tool_type == "tool"),
        "oracle fixture should include tool entries separately from agents"
    );
    assert!(
        registry
            .mcp_servers
            .iter()
            .any(|server| server.name == "gmail"
                && server.transport == "stdio"
                && !server.enabled
                && server.auto_register_tools
                && server.config["command"] == "bash"
                && server.config["args"][0] == "-c"
                && server.config["args"][1]
                    .as_str()
                    .is_some_and(|arg| arg.contains("gmailmcp.googleapis.com/mcp/v1"))),
        "oracle fixture should include the Gmail hosted MCP seed"
    );
    assert!(
        registry
            .mcp_servers
            .iter()
            .any(|server| server.name == "google-calendar"
                && server.transport == "stdio"
                && !server.enabled
                && server.auto_register_tools
                && server.config["command"] == "bash"
                && server.config["args"][0] == "-c"
                && server.config["args"][1]
                    .as_str()
                    .is_some_and(|arg| arg.contains("calendarmcp.googleapis.com/mcp/v1"))),
        "oracle fixture should include the Google Calendar hosted MCP seed"
    );
}

#[test]
fn embedded_clojure_oracle_registry_is_loadable() {
    let registry =
        load_embedded_oracle_registry().expect("embedded Clojure oracle registry should load");

    assert!(
        registry
            .agents
            .iter()
            .any(|agent| agent.id == "coact-agent" && agent.agent_type == "agent"),
        "embedded registry should include the built-in coact-agent as an agent entry"
    );
    assert!(
        registry
            .models
            .iter()
            .any(|model| model.provider == "bedrock"),
        "embedded registry should include Bedrock model metadata"
    );
    assert!(
        registry
            .agents
            .iter()
            .any(|agent| agent.id == "debug-agent" && agent.max_iterations == Some(30)),
        "embedded registry should preserve agent max-iterations metadata"
    );
    assert!(
        registry.tools.iter().any(|tool| tool.id == "code$eval"),
        "embedded registry should preserve tool metadata"
    );
    assert!(
        registry
            .mcp_servers
            .iter()
            .any(|server| server.name == "gmail"),
        "embedded registry should preserve hosted MCP server seeds"
    );
}

#[test]
fn standalone_clojure_oracle_tools_fixture_is_loadable() {
    let tools = load_tools_path(ORACLE_TOOLS).expect("Clojure oracle tools fixture should load");

    assert!(
        tools.iter().any(|tool| tool.id == "code$eval"
            && tool.tool_type == "command"
            && tool.input_schema.is_array()),
        "tools fixture should preserve command ids, types, and schemas"
    );
}

#[test]
fn standalone_clojure_oracle_mcp_servers_fixture_is_loadable() {
    let servers =
        load_mcp_servers_path(ORACLE_MCP_SERVERS).expect("Clojure oracle MCP fixture should load");

    assert!(
        servers.iter().any(|server| server.name == "gmail"
            && server.config["args"][1]
                .as_str()
                .is_some_and(|arg| arg.contains("GCP_OAUTH_CLIENT_ID"))),
        "MCP fixture should preserve the Gmail static OAuth env bridge"
    );
    assert!(
        servers.iter().any(|server| server.name == "google-calendar"
            && server.config["args"][1]
                .as_str()
                .is_some_and(|arg| arg.contains("GCP_OAUTH_CLIENT_SECRET"))),
        "MCP fixture should preserve the Google Calendar static OAuth env bridge"
    );
}
