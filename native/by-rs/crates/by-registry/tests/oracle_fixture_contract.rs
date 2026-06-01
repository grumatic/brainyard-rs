use by_registry::load_registry_path;

const ORACLE_REGISTRY: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../fixtures/oracle/registry.json"
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
}
