use by_config::{
    load_dotenv_values, read_config, resolve_default_config_path, resolve_user_id, AgentConfig,
    BrainyardDirs, LlmConfig, UserIdInputs,
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

#[test]
fn dotenv_values_walk_from_working_dir_to_home_and_preserve_first_key() {
    let project = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let nested = project.path().join("nested/work");
    fs::create_dir_all(&nested).unwrap();
    fs::create_dir_all(home.path().join(".brainyard")).unwrap();
    fs::write(
        project.path().join(".env"),
        r#"
        BY_USER_ID="project-user"
        AWS_REGION=ap-northeast-2
        "#,
    )
    .unwrap();
    fs::write(
        home.path().join(".brainyard/.env"),
        r#"
        AWS_PROFILE='home-profile'
        AWS_REGION=us-east-1
        "#,
    )
    .unwrap();

    let dotenv = load_dotenv_values(&nested, Some(home.path()), None, false, &|_| false).unwrap();

    assert_eq!(dotenv.get("BY_USER_ID"), Some("project-user"));
    assert_eq!(dotenv.get("AWS_REGION"), Some("ap-northeast-2"));
    assert_eq!(dotenv.get("AWS_PROFILE"), Some("home-profile"));
    assert_eq!(dotenv.loaded_paths.len(), 2);
    assert_eq!(dotenv.loaded_paths[0].path, project.path().join(".env"));
    assert_eq!(
        dotenv.loaded_paths[0].keys,
        vec!["BY_USER_ID".to_string(), "AWS_REGION".to_string()]
    );
    assert_eq!(
        dotenv.loaded_paths[1].path,
        home.path().join(".brainyard/.env")
    );
    assert_eq!(dotenv.loaded_paths[1].keys, vec!["AWS_PROFILE".to_string()]);
}

#[test]
fn dotenv_values_skip_existing_env_keys_and_parse_export_quotes() {
    let project = tempfile::tempdir().unwrap();
    fs::write(
        project.path().join(".env"),
        r#"
        export AWS_PROFILE="dotenv-profile"
        BY_USER_ID='dotenv-user'
        "#,
    )
    .unwrap();

    let dotenv = load_dotenv_values(project.path(), None, None, false, &|key| {
        key == "AWS_PROFILE"
    })
    .unwrap();

    assert_eq!(dotenv.get("AWS_PROFILE"), None);
    assert_eq!(dotenv.get("BY_USER_ID"), Some("dotenv-user"));
    assert_eq!(dotenv.loaded_paths[0].keys, vec!["BY_USER_ID".to_string()]);
}

#[test]
fn dotenv_values_honor_explicit_file_and_skip_flag() {
    let project = tempfile::tempdir().unwrap();
    let explicit_dir = tempfile::tempdir().unwrap();
    let explicit = explicit_dir.path().join("brainyard.env");
    fs::write(project.path().join(".env"), "BY_USER_ID=project-user").unwrap();
    fs::write(&explicit, "BY_USER_ID=explicit-user").unwrap();

    let dotenv =
        load_dotenv_values(project.path(), None, Some(&explicit), false, &|_| false).unwrap();
    assert_eq!(dotenv.get("BY_USER_ID"), Some("explicit-user"));
    assert_eq!(dotenv.loaded_paths.len(), 1);
    assert_eq!(dotenv.loaded_paths[0].path, explicit);

    let skipped =
        load_dotenv_values(project.path(), None, Some(&explicit), true, &|_| false).unwrap();
    assert!(skipped.values.is_empty());
    assert!(skipped.loaded_paths.is_empty());
}
