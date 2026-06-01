use by_config::{read_config, LlmConfig};
use std::fs;

#[test]
fn reads_llm_defaults_and_available_providers() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("config.edn");
    fs::write(
        &path,
        r#"
        {:llm {:default-provider :bedrock
               :default-model "amazon.nova-lite-v1:0"
               :available-providers [:bedrock :claude-code]}}
        "#,
    )
    .unwrap();

    let doc = read_config(&path).unwrap();

    assert_eq!(
        doc.llm(),
        LlmConfig {
            default_provider: Some("bedrock".to_string()),
            default_model: Some("amazon.nova-lite-v1:0".to_string()),
            available_providers: vec!["bedrock".to_string(), "claude-code".to_string()],
        }
    );
}

#[test]
fn missing_llm_section_returns_empty_defaults() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("config.edn");
    fs::write(&path, "{:agent {:default-agent :coact-agent}}").unwrap();

    let doc = read_config(&path).unwrap();

    assert_eq!(
        doc.llm(),
        LlmConfig {
            default_provider: None,
            default_model: None,
            available_providers: Vec::new(),
        }
    );
}
