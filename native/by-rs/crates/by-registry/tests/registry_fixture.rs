use by_registry::load_registry_str;

#[test]
fn loads_agents_and_models_from_fixture_object() {
    let registry = load_registry_str(
        r#"{
          "agents": [{"id": "coder", "name": "Coder", "description": "Writes code"}],
          "models": [{"provider": "bedrock", "id": "amazon.nova-lite-v1:0", "description": "Nova Lite"}]
        }"#,
    )
    .expect("registry fixture should load");

    assert_eq!(registry.agents.len(), 1);
    assert_eq!(registry.agents[0].id, "coder");
    assert_eq!(registry.agents[0].name, "Coder");
    assert_eq!(registry.models[0].provider, "bedrock");
    assert_eq!(registry.models[0].id, "amazon.nova-lite-v1:0");
    assert_eq!(registry.models[0].label(), "bedrock:amazon.nova-lite-v1:0");
}

#[test]
fn uses_agent_name_as_id_when_id_is_missing() {
    let registry = load_registry_str(r#"{"agents": [{"name": "reviewer"}], "models": []}"#)
        .expect("fixture should load with name-only agent");

    assert_eq!(registry.agents[0].id, "reviewer");
    assert_eq!(registry.agents[0].name, "reviewer");
}

#[test]
fn preserves_optional_model_region() {
    let registry = load_registry_str(
        r#"{"agents": [], "models": [{"provider": "bedrock", "id": "openai.gpt-oss-120b-1:0", "region": "us-east-1"}]}"#,
    )
    .expect("fixture should load region-bearing model");

    assert_eq!(registry.models[0].region.as_deref(), Some("us-east-1"));
}
