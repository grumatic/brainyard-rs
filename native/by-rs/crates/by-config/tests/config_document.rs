use by_config::{
    read_config, resolve_default_config_path, resolve_user_id, AgentConfig, BrainyardDirs,
    LlmConfig, UserIdInputs,
};
use std::fs;

#[test]
fn reads_llm_defaults_and_available_providers() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("config.edn");
    fs::write(
        &path,
        r#"
        {:agent {:default-agent :coact-agent}
         :llm {:default-provider :bedrock
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
    assert_eq!(
        doc.agent(),
        AgentConfig {
            default_agent: Some("coact-agent".to_string()),
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
    assert_eq!(
        doc.agent(),
        AgentConfig {
            default_agent: Some("coact-agent".to_string()),
        }
    );
}

#[test]
fn default_config_path_prefers_project_config_when_present() {
    let project = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let nested = project.path().join("nested/work");
    fs::create_dir_all(project.path().join(".git")).unwrap();
    fs::create_dir_all(project.path().join(".brainyard")).unwrap();
    fs::create_dir_all(home.path().join(".brainyard")).unwrap();
    fs::create_dir_all(&nested).unwrap();

    let project_config = project.path().join(".brainyard/config.edn");
    let user_config = home.path().join(".brainyard/config.edn");
    fs::write(&project_config, "{:llm {:default-provider :bedrock}}").unwrap();
    fs::write(&user_config, "{:llm {:default-provider :claude-code}}").unwrap();

    let dirs = BrainyardDirs::resolve(&nested, Some(home.path()), None::<&std::path::Path>);

    assert_eq!(resolve_default_config_path(&dirs), Some(project_config));
}

#[test]
fn default_config_path_falls_back_to_user_config_when_project_is_absent() {
    let project = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    fs::create_dir_all(project.path().join(".git")).unwrap();
    fs::create_dir_all(home.path().join(".brainyard")).unwrap();

    let user_config = home.path().join(".brainyard/config.edn");
    fs::write(&user_config, "{:llm {:default-provider :bedrock}}").unwrap();

    let dirs = BrainyardDirs::resolve(project.path(), Some(home.path()), None::<&std::path::Path>);

    assert_eq!(resolve_default_config_path(&dirs), Some(user_config));
}

#[test]
fn default_config_path_honors_project_dir_override() {
    let cwd = tempfile::tempdir().unwrap();
    let override_project = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    fs::create_dir_all(override_project.path().join(".brainyard")).unwrap();
    fs::create_dir_all(home.path().join(".brainyard")).unwrap();

    let project_config = override_project.path().join(".brainyard/config.edn");
    fs::write(&project_config, "{:llm {:default-provider :bedrock}}").unwrap();
    fs::write(
        home.path().join(".brainyard/config.edn"),
        "{:llm {:default-provider :claude-code}}",
    )
    .unwrap();

    let dirs = BrainyardDirs::resolve(cwd.path(), Some(home.path()), Some(override_project.path()));

    assert_eq!(resolve_default_config_path(&dirs), Some(project_config));
}

#[test]
fn user_id_resolution_matches_main_startup_precedence() {
    let resolved = resolve_user_id(UserIdInputs {
        explicit: Some(" cli-user "),
        by_user_id_env: Some("env-user"),
        by_user_id_property: Some("property-user"),
        os_user_name: Some("os-user"),
    });
    assert_eq!(resolved, "cli-user");

    let resolved = resolve_user_id(UserIdInputs {
        explicit: Some("   "),
        by_user_id_env: Some(" env-user "),
        by_user_id_property: Some("property-user"),
        os_user_name: Some("os-user"),
    });
    assert_eq!(resolved, "env-user");

    let resolved = resolve_user_id(UserIdInputs {
        explicit: None,
        by_user_id_env: Some("   "),
        by_user_id_property: Some(" property-user "),
        os_user_name: Some("os-user"),
    });
    assert_eq!(resolved, "property-user");

    let resolved = resolve_user_id(UserIdInputs {
        explicit: None,
        by_user_id_env: None,
        by_user_id_property: Some("   "),
        os_user_name: Some(" os-user "),
    });
    assert_eq!(resolved, "os-user");
}

#[test]
fn user_id_resolution_falls_back_to_by_user() {
    let resolved = resolve_user_id(UserIdInputs {
        explicit: Some(""),
        by_user_id_env: Some("   "),
        by_user_id_property: None,
        os_user_name: None,
    });

    assert_eq!(resolved, "by-user");
}
