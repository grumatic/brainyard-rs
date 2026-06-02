use by_registry::{load_embedded_oracle_registry, load_registry_path, load_tools_path};

const ORACLE_REGISTRY: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../fixtures/oracle/registry.json"
);
const ORACLE_TOOLS: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../fixtures/oracle/tools.json"
);

#[test]
fn clojure_oracle_registry_fixture_is_loadable() {
    let registry =
        load_registry_path(ORACLE_REGISTRY).expect("Clojure oracle registry fixture should load");

    assert!(
        registry
            .agents
            .iter()
            .any(|agent| agent.id == "coact-agent"),
        "oracle fixture should include the built-in coact-agent"
    );
    assert!(
        registry.agents.iter().any(|agent| agent.id == "main-agent"),
        "oracle fixture should include the built-in main-agent"
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
}

#[test]
fn embedded_clojure_oracle_registry_is_loadable() {
    let registry =
        load_embedded_oracle_registry().expect("embedded Clojure oracle registry should load");

    assert!(
        registry
            .agents
            .iter()
            .any(|agent| agent.id == "coact-agent"),
        "embedded registry should include the built-in coact-agent"
    );
    assert!(
        registry
            .models
            .iter()
            .any(|model| model.provider == "bedrock"),
        "embedded registry should include Bedrock model metadata"
    );
    assert!(
        registry.tools.iter().any(|tool| tool.id == "code$eval"),
        "embedded registry should preserve tool metadata"
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
