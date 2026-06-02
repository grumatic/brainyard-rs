use assert_cmd::Command;
use predicates::prelude::*;
use rusqlite::Connection;

#[test]
fn top_help_matches_clojure_command_surface() {
    Command::cargo_bin("by-rs")
        .unwrap()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("NAME:\n by - Brainyard Agent CLI"))
        .stdout(predicate::str::contains(
            "USAGE:\n by [global-options] command [command options] [arguments...]",
        ))
        .stdout(predicate::str::contains("VERSION:"))
        .stdout(predicate::str::contains(
            "run                  Start interactive TUI agent session (default)",
        ))
        .stdout(predicate::str::contains("agents"))
        .stdout(predicate::str::contains("models"))
        .stdout(predicate::str::contains("sessions"))
        .stdout(predicate::str::contains("tools").not())
        .stdout(predicate::str::contains("memory").not())
        .stdout(predicate::str::contains("tui").not());
}

#[test]
fn run_help_matches_clojure_command_surface() {
    Command::cargo_bin("by-rs")
        .unwrap()
        .args(["run", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "NAME:\n by run - Start interactive TUI agent session (default)",
        ))
        .stdout(predicate::str::contains("--[no-]inline"))
        .stdout(predicate::str::contains("--[no-]with-tmux"))
        .stdout(predicate::str::contains("--[no-]select-resume"));
}

#[test]
fn no_args_defaults_to_run_command() {
    Command::cargo_bin("by-rs")
        .unwrap()
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("by-rs run is not implemented yet"))
        .stderr(predicate::str::contains("Usage:").not());
}

#[test]
fn root_level_run_flags_are_routed_to_run_command() {
    Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "--inline",
            "--no-inline",
            "--verbose",
            "--no-verbose",
            "--with-tmux",
            "--no-with-tmux",
            "--select-resume",
            "--no-select-resume",
            "--new",
            "--no-new",
            "--max-iterations",
            "3",
            "--agent",
            "coact-agent",
            "--provider",
            "bedrock",
            "--model",
            "amazon.nova-lite-v1:0",
            "--user-id",
            "alice",
        ])
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("by-rs run is not implemented yet"))
        .stderr(predicate::str::contains("unexpected argument").not());
}

#[test]
fn bare_agent_id_is_routed_to_run_command() {
    Command::cargo_bin("by-rs")
        .unwrap()
        .arg("coact-agent")
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("by-rs run is not implemented yet"))
        .stderr(predicate::str::contains("unrecognized subcommand").not());
}

#[test]
fn run_accepts_bare_resume_flag_like_clojure() {
    Command::cargo_bin("by-rs")
        .unwrap()
        .args(["run", "--resume"])
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("by-rs run is not implemented yet"))
        .stderr(predicate::str::contains("a value is required").not());
}

#[test]
fn agents_help_matches_clojure_command_surface() {
    Command::cargo_bin("by-rs")
        .unwrap()
        .args(["agents", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "NAME:\n by agents - List available agents",
        ))
        .stdout(predicate::str::contains(
            "USAGE:\n by agents [command options] [arguments...]",
        ))
        .stdout(predicate::str::contains("--fixture").not());
}

#[test]
fn agents_command_reads_registry_fixture() {
    let dir = tempfile::tempdir().unwrap();
    let fixture = dir.path().join("registry.json");
    std::fs::write(
        &fixture,
        r#"{"agents":[{"id":"coder","name":"Coder","description":"Writes code"}],"models":[]}"#,
    )
    .unwrap();

    Command::cargo_bin("by-rs")
        .unwrap()
        .args(["agents", "--fixture"])
        .arg(&fixture)
        .assert()
        .success()
        .stdout(predicate::str::contains("1 agent(s) available:"))
        .stdout(predicate::str::contains("AGENT"))
        .stdout(predicate::str::contains("coder"))
        .stdout(predicate::str::contains("Writes code"));
}

#[test]
fn agents_command_uses_embedded_registry_by_default() {
    Command::cargo_bin("by-rs")
        .unwrap()
        .arg("agents")
        .assert()
        .success()
        .stdout(predicate::str::contains("agent(s) available:"))
        .stdout(predicate::str::contains("coact-agent"))
        .stdout(predicate::str::contains("main-agent"));
}

#[test]
fn models_help_matches_clojure_command_surface() {
    Command::cargo_bin("by-rs")
        .unwrap()
        .args(["models", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "NAME:\n by models - List available LLM models (provider/model)",
        ))
        .stdout(predicate::str::contains(
            "-p, --provider S  Filter to a single provider",
        ))
        .stdout(predicate::str::contains("--fixture").not());
}

#[test]
fn models_command_reads_registry_fixture() {
    let dir = tempfile::tempdir().unwrap();
    let fixture = dir.path().join("registry.json");
    std::fs::write(
        &fixture,
        r#"{"agents":[],"models":[{"provider":"bedrock","id":"amazon.nova-lite-v1:0"}]}"#,
    )
    .unwrap();

    Command::cargo_bin("by-rs")
        .unwrap()
        .args(["models", "--fixture"])
        .arg(&fixture)
        .assert()
        .success()
        .stdout(predicate::str::contains("PROVIDER"))
        .stdout(predicate::str::contains("MODEL"))
        .stdout(predicate::str::contains("bedrock"))
        .stdout(predicate::str::contains("amazon.nova-lite-v1:0"))
        .stdout(predicate::str::contains("1 model(s) listed."));
}

#[test]
fn models_command_uses_embedded_registry_by_default() {
    Command::cargo_bin("by-rs")
        .unwrap()
        .arg("models")
        .assert()
        .success()
        .stdout(predicate::str::contains("PROVIDER"))
        .stdout(predicate::str::contains("MODEL"))
        .stdout(predicate::str::contains("bedrock"))
        .stdout(predicate::str::contains("amazon.nova-lite-v1:0"))
        .stdout(predicate::str::contains("model(s) listed."));
}

#[test]
fn sessions_help_matches_clojure_command_surface() {
    Command::cargo_bin("by-rs")
        .unwrap()
        .args(["sessions", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "NAME:\n by sessions - List or prune persisted agent sessions",
        ))
        .stdout(predicate::str::contains(
            "USAGE:\n by sessions [global-options] command [command options] [arguments...]",
        ))
        .stdout(predicate::str::contains(
            "list                 List all persisted sessions",
        ))
        .stdout(predicate::str::contains(
            "prune                Delete a persisted session",
        ));
}

#[test]
fn sessions_list_help_matches_clojure_command_surface() {
    Command::cargo_bin("by-rs")
        .unwrap()
        .args(["sessions", "list", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "NAME:\n by sessions list - List all persisted sessions",
        ))
        .stdout(predicate::str::contains(
            "USAGE:\n by sessions list [command options] [arguments...]",
        ))
        .stdout(predicate::str::contains("--root").not());
}

#[test]
fn sessions_prune_help_matches_clojure_command_surface() {
    Command::cargo_bin("by-rs")
        .unwrap()
        .args(["sessions", "prune", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "NAME:\n by sessions prune - Delete a persisted session",
        ))
        .stdout(predicate::str::contains("-s, --session-id S  Session ID"))
        .stdout(predicate::str::contains("--root").not());
}

#[test]
fn sessions_list_reads_fixture_root_without_writing() {
    let root = tempfile::tempdir().unwrap();
    let session = root.path().join("alpha");
    std::fs::create_dir_all(&session).unwrap();
    std::fs::write(
        session.join("meta.edn"),
        r#"{:id "alpha"
            :label "Alpha session"
            :defagent-id :coact-agent
            :started-at 1780290000000
            :last-attached-at 1780290100000}"#,
    )
    .unwrap();
    std::fs::write(session.join("messages.log"), "hello").unwrap();

    Command::cargo_bin("by-rs")
        .unwrap()
        .args(["sessions", "list", "--root"])
        .arg(root.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("session-id"))
        .stdout(predicate::str::contains("alpha"))
        .stdout(predicate::str::contains("Alpha session"))
        .stdout(predicate::str::contains("coact-agent"))
        .stdout(predicate::str::contains("B"));
}

#[test]
fn sessions_prune_deletes_fixture_session_by_positional_id() {
    let root = tempfile::tempdir().unwrap();
    let session = root.path().join("doomed");
    std::fs::create_dir_all(session.join("nested")).unwrap();
    std::fs::write(session.join("nested/messages.log"), "goodbye").unwrap();

    Command::cargo_bin("by-rs")
        .unwrap()
        .args(["sessions", "prune", "--root"])
        .arg(root.path())
        .arg("doomed")
        .assert()
        .success()
        .stdout(predicate::str::contains("Deleted session: doomed"));

    assert!(!session.exists());
}

#[test]
fn sessions_prune_reports_missing_session_without_creating_it() {
    let root = tempfile::tempdir().unwrap();

    Command::cargo_bin("by-rs")
        .unwrap()
        .args(["sessions", "prune", "--root"])
        .arg(root.path())
        .args(["--session-id", "missing"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Session not found: missing"));

    assert!(!root.path().join("missing").exists());
}

#[test]
fn models_command_filters_by_provider() {
    let dir = tempfile::tempdir().unwrap();
    let fixture = dir.path().join("registry.json");
    std::fs::write(
        &fixture,
        r#"{"agents":[],"models":[{"provider":"bedrock","id":"amazon.nova-lite-v1:0"},{"provider":"openai","id":"gpt-5"}]}"#,
    )
    .unwrap();

    Command::cargo_bin("by-rs")
        .unwrap()
        .args(["models", "--fixture"])
        .arg(&fixture)
        .args(["--provider", "bedrock"])
        .assert()
        .success()
        .stdout(predicate::str::contains("amazon.nova-lite-v1:0"))
        .stdout(predicate::str::contains(
            "1 model(s) listed. (filtered to bedrock)",
        ))
        .stdout(predicate::str::contains("gpt-5").not())
        .stdout(predicate::str::contains("openai").not());
}

#[test]
fn models_command_displays_region_when_present() {
    let dir = tempfile::tempdir().unwrap();
    let fixture = dir.path().join("registry.json");
    std::fs::write(
        &fixture,
        r#"{"agents":[],"models":[{"provider":"bedrock","id":"openai.gpt-oss-120b-1:0","region":"us-east-1"}]}"#,
    )
    .unwrap();

    Command::cargo_bin("by-rs")
        .unwrap()
        .args(["models", "--fixture"])
        .arg(&fixture)
        .assert()
        .success()
        .stdout(predicate::str::contains("bedrock"))
        .stdout(predicate::str::contains(
            "openai.gpt-oss-120b-1:0 (us-east-1)",
        ));
}

#[test]
fn tools_command_reads_registry_fixture_and_filters_by_type() {
    let dir = tempfile::tempdir().unwrap();
    let fixture = dir.path().join("registry.json");
    std::fs::write(
        &fixture,
        r#"{"tools":[
             {"id":"grep","type":"tool","description":"Search files"},
             {"id":"code$eval","type":"command","description":"Evaluate code"}
           ],"agents":[],"models":[]}"#,
    )
    .unwrap();

    Command::cargo_bin("by-rs")
        .unwrap()
        .args(["tools", "--fixture"])
        .arg(&fixture)
        .args(["--type", "command"])
        .assert()
        .success()
        .stdout(predicate::str::contains("TOOL"))
        .stdout(predicate::str::contains("TYPE"))
        .stdout(predicate::str::contains("code$eval"))
        .stdout(predicate::str::contains("command"))
        .stdout(predicate::str::contains("Evaluate code"))
        .stdout(predicate::str::contains(
            "1 tool(s) listed. (filtered to type command)",
        ))
        .stdout(predicate::str::contains("grep").not());
}

#[test]
fn tools_command_reads_standalone_tools_fixture_and_filters_by_id() {
    let dir = tempfile::tempdir().unwrap();
    let fixture = dir.path().join("tools.json");
    std::fs::write(
        &fixture,
        r#"[
             {"id":"grep","type":"tool","description":"Search files"},
             {"id":"memory$recall","type":"command","description":"Recall memory"}
           ]"#,
    )
    .unwrap();

    Command::cargo_bin("by-rs")
        .unwrap()
        .args(["tools", "--fixture"])
        .arg(&fixture)
        .args(["--id", "grep"])
        .assert()
        .success()
        .stdout(predicate::str::contains("grep"))
        .stdout(predicate::str::contains("Search files"))
        .stdout(predicate::str::contains(
            "1 tool(s) listed. (filtered to id grep)",
        ))
        .stdout(predicate::str::contains("memory$recall").not());
}

#[test]
fn config_help_matches_clojure_bootstrap_surface() {
    Command::cargo_bin("by-rs")
        .unwrap()
        .args(["config", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "NAME:\n by config - Bootstrap pipeline (detect → ladder → handoff)",
        ))
        .stdout(predicate::str::contains(
            "USAGE:\n by config [command options] [arguments...]",
        ))
        .stdout(predicate::str::contains("--[no-]auto"))
        .stdout(predicate::str::contains("--profile S"))
        .stdout(predicate::str::contains("--[no-]dry-run"))
        .stdout(predicate::str::contains("show").not());
}

#[test]
fn config_requires_auto_when_stdin_is_non_interactive() {
    Command::cargo_bin("by-rs")
        .unwrap()
        .args(["config", "--dry-run"])
        .assert()
        .code(2)
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains(
            "Non-interactive stdin detected. Use --auto for non-interactive runs.",
        ));
}

#[test]
fn config_bootstrap_projection_stays_read_only() {
    Command::cargo_bin("by-rs")
        .unwrap()
        .args(["config", "--auto", "--profile", "dev", "--dry-run"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"operation\": \"config\""))
        .stdout(predicate::str::contains("\"status\": \"not-implemented\""))
        .stdout(predicate::str::contains("\"network\": false"))
        .stdout(predicate::str::contains("\"writes\": false"))
        .stdout(predicate::str::contains("\"auto\": true"))
        .stdout(predicate::str::contains("\"profile\": \"dev\""))
        .stdout(predicate::str::contains("\"dry_run\": true"));
}

#[test]
fn config_show_reads_llm_defaults_without_writing() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.edn");
    std::fs::write(
        &path,
        r#"{:agent {:default-agent :coact-agent}
            :llm {:default-provider :bedrock
                 :default-model "amazon.nova-lite-v1:0"
                 :available-providers [:bedrock :claude-code]}}"#,
    )
    .unwrap();

    Command::cargo_bin("by-rs")
        .unwrap()
        .args(["config", "show", "--path"])
        .arg(&path)
        .assert()
        .success()
        .stdout(predicate::str::contains("agent.default-agent\tcoact-agent"))
        .stdout(predicate::str::contains("llm.default-provider\tbedrock"))
        .stdout(predicate::str::contains(
            "llm.default-model\tamazon.nova-lite-v1:0",
        ))
        .stdout(predicate::str::contains(
            "llm.available-providers\tbedrock,claude-code",
        ));
}

#[test]
fn config_show_prefers_project_config_over_user_config_by_default() {
    let project = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let nested = project.path().join("subdir");
    std::fs::create_dir_all(project.path().join(".git")).unwrap();
    std::fs::create_dir_all(project.path().join(".brainyard")).unwrap();
    std::fs::create_dir_all(home.path().join(".brainyard")).unwrap();
    std::fs::create_dir_all(&nested).unwrap();
    std::fs::write(
        project.path().join(".brainyard/config.edn"),
        r#"{:llm {:default-provider :bedrock
                 :default-model "project-model"}}"#,
    )
    .unwrap();
    std::fs::write(
        home.path().join(".brainyard/config.edn"),
        r#"{:llm {:default-provider :claude-code
                 :default-model "user-model"}}"#,
    )
    .unwrap();

    Command::cargo_bin("by-rs")
        .unwrap()
        .current_dir(&nested)
        .env("HOME", home.path())
        .env_remove("BRAINYARD_PROJECT_DIR")
        .args(["config", "show"])
        .assert()
        .success()
        .stdout(predicate::str::contains("llm.default-provider\tbedrock"))
        .stdout(predicate::str::contains("llm.default-model\tproject-model"))
        .stdout(predicate::str::contains("user-model").not());
}

#[test]
fn config_show_missing_default_config_prints_empty_defaults_without_writing() {
    let cwd = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();

    Command::cargo_bin("by-rs")
        .unwrap()
        .current_dir(cwd.path())
        .env("HOME", home.path())
        .env_remove("BRAINYARD_PROJECT_DIR")
        .args(["config", "show"])
        .assert()
        .success()
        .stdout(predicate::str::contains("agent.default-agent\t"))
        .stdout(predicate::str::contains("llm.default-provider\t"))
        .stdout(predicate::str::contains("llm.default-model\t"))
        .stdout(predicate::str::contains("llm.available-providers\t"));

    assert!(!cwd.path().join(".brainyard").exists());
    assert!(!home.path().join(".brainyard").exists());
}

#[test]
fn ask_dry_run_uses_project_config_defaults_before_user_config() {
    let project = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(project.path().join(".git")).unwrap();
    std::fs::create_dir_all(project.path().join(".brainyard")).unwrap();
    std::fs::create_dir_all(home.path().join(".brainyard")).unwrap();
    std::fs::write(
        project.path().join(".brainyard/config.edn"),
        r#"{:llm {:default-provider :bedrock
                 :default-model "amazon.nova-lite-v1:0"}}"#,
    )
    .unwrap();
    std::fs::write(
        home.path().join(".brainyard/config.edn"),
        r#"{:llm {:default-provider :bedrock
                 :default-model "user-model"}}"#,
    )
    .unwrap();

    Command::cargo_bin("by-rs")
        .unwrap()
        .current_dir(project.path())
        .env("HOME", home.path())
        .env_remove("BRAINYARD_PROJECT_DIR")
        .args(["ask", "--dry-run", "What is 2+2?"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "\"modelId\": \"amazon.nova-lite-v1:0\"",
        ))
        .stdout(predicate::str::contains("user-model").not());
}

#[test]
fn help_exposes_memory_search_command() {
    Command::cargo_bin("by-rs")
        .unwrap()
        .args(["memory", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("inspect"))
        .stdout(predicate::str::contains("search"));
}

#[test]
fn memory_search_reads_sqlite_fts_without_writing() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("memory.db");
    let conn = Connection::open(&db_path).unwrap();
    create_memory_schema(&conn);
    conn.execute(
        "INSERT INTO episodes (session_id, user_id, episode_type, role, content)
         VALUES ('s1', 'u1', 'conversation', 'assistant', 'blue deploy note')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO semantic_facts (user_id, fact_type, content, confidence)
         VALUES ('u1', 'preference', 'green release preference', 0.9)",
        [],
    )
    .unwrap();
    drop(conn);

    Command::cargo_bin("by-rs")
        .unwrap()
        .args(["memory", "search", "--db"])
        .arg(&db_path)
        .args(["--query", "blue green"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "l2\tconversation\tblue deploy note",
        ))
        .stdout(predicate::str::contains(
            "l3\tpreference\tgreen release preference",
        ));
}

#[test]
fn memory_inspect_reports_schema_and_counts_without_writing() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("memory.db");
    let conn = Connection::open(&db_path).unwrap();
    create_memory_schema(&conn);
    conn.execute(
        "INSERT OR REPLACE INTO memory_metadata (key, value)
         VALUES ('schema_version', '2.0.0')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO episodes (session_id, user_id, episode_type, role, content)
         VALUES ('s1', 'u1', 'conversation', 'assistant', 'blue deploy note')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO semantic_facts (user_id, fact_type, content, confidence)
         VALUES ('u1', 'preference', 'green release preference', 0.9)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO memory_audit
         (user_id, session_id, agent_id, turn_id, total_turns, entry_id, layer, byte_cost)
         VALUES ('u1', 's1', 'coact-agent', 1, 1, 'entry-1', 'l2', 42)",
        [],
    )
    .unwrap();
    drop(conn);

    Command::cargo_bin("by-rs")
        .unwrap()
        .args(["memory", "inspect", "--db"])
        .arg(&db_path)
        .assert()
        .success()
        .stdout(predicate::str::contains("schema-version: 2.0.0"))
        .stdout(predicate::str::contains("sqlite-user-version: 0"))
        .stdout(predicate::str::contains("journal-mode:"))
        .stdout(predicate::str::contains("memory_metadata"))
        .stdout(predicate::str::contains("episodes"))
        .stdout(predicate::str::contains("episodes_fts"))
        .stdout(predicate::str::contains("semantic_facts"))
        .stdout(predicate::str::contains("semantic_fts"))
        .stdout(predicate::str::contains("memory_audit"));
}

#[test]
fn ask_dry_run_accepts_user_id_flag_from_main_contract() {
    Command::cargo_bin("by-rs")
        .unwrap()
        .env("BY_USER_ID", "env-user")
        .args([
            "ask",
            "--provider",
            "bedrock",
            "--model",
            "amazon.nova-lite-v1:0",
            "--user-id",
            "alice",
            "--dry-run",
            "What is 2+2?",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "\"modelId\": \"amazon.nova-lite-v1:0\"",
        ))
        .stdout(predicate::str::contains("\"user_id\": \"alice\""))
        .stdout(predicate::str::contains("env-user").not());
}

#[test]
fn ask_dry_run_resolves_user_id_from_environment_when_flag_blank() {
    Command::cargo_bin("by-rs")
        .unwrap()
        .env("BY_USER_ID", "env-user")
        .args([
            "ask",
            "--provider",
            "bedrock",
            "--model",
            "amazon.nova-lite-v1:0",
            "--user-id",
            "   ",
            "--dry-run",
            "What is 2+2?",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"user_id\": \"env-user\""));
}

#[test]
fn ask_dry_run_reads_user_id_and_bedrock_runtime_from_project_dotenv() {
    let project = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let nested = project.path().join("nested/work");
    std::fs::create_dir_all(&nested).unwrap();
    std::fs::write(
        project.path().join(".env"),
        r#"
        BY_USER_ID=dotenv-user
        AWS_REGION=ap-northeast-2
        AWS_PROFILE=dotenv-profile
        "#,
    )
    .unwrap();

    Command::cargo_bin("by-rs")
        .unwrap()
        .current_dir(&nested)
        .env("HOME", home.path())
        .env_remove("BY_ENV_FILE")
        .env_remove("BY_NO_DOTENV")
        .env_remove("BY_USER_ID")
        .env_remove("AWS_REGION")
        .env_remove("AWS_DEFAULT_REGION")
        .env_remove("AWS_PROFILE")
        .env_remove("AWS_DEFAULT_PROFILE")
        .args([
            "ask",
            "--provider",
            "bedrock",
            "--model",
            "amazon.nova-lite-v1:0",
            "--dry-run",
            "What is 2+2?",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"user_id\": \"dotenv-user\""))
        .stdout(predicate::str::contains("\"region\": \"ap-northeast-2\""))
        .stdout(predicate::str::contains(
            "\"aws_profile\": \"dotenv-profile\"",
        ));
}

#[test]
fn ask_dry_run_prefers_environment_over_project_dotenv() {
    let project = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    std::fs::write(
        project.path().join(".env"),
        r#"
        BY_USER_ID=dotenv-user
        AWS_REGION=ap-northeast-2
        AWS_PROFILE=dotenv-profile
        "#,
    )
    .unwrap();

    Command::cargo_bin("by-rs")
        .unwrap()
        .current_dir(project.path())
        .env("HOME", home.path())
        .env_remove("BY_ENV_FILE")
        .env_remove("BY_NO_DOTENV")
        .env("BY_USER_ID", "env-user")
        .env("AWS_REGION", "eu-west-1")
        .env("AWS_PROFILE", "env-profile")
        .args([
            "ask",
            "--provider",
            "bedrock",
            "--model",
            "amazon.nova-lite-v1:0",
            "--dry-run",
            "What is 2+2?",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"user_id\": \"env-user\""))
        .stdout(predicate::str::contains("\"region\": \"eu-west-1\""))
        .stdout(predicate::str::contains("\"aws_profile\": \"env-profile\""))
        .stdout(predicate::str::contains("dotenv-user").not())
        .stdout(predicate::str::contains("ap-northeast-2").not())
        .stdout(predicate::str::contains("dotenv-profile").not());
}

#[test]
fn ask_dry_run_renders_bedrock_converse_request_without_network() {
    Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "ask",
            "--provider",
            "bedrock",
            "--model",
            "amazon.nova-lite-v1:0",
            "--dry-run",
            "What is 2+2?",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"provider\": \"bedrock\""))
        .stdout(predicate::str::contains("\"operation\": \"Converse\""))
        .stdout(predicate::str::contains(
            "\"modelId\": \"amazon.nova-lite-v1:0\"",
        ))
        .stdout(predicate::str::contains("\"role\": \"user\""))
        .stdout(predicate::str::contains("\"text\": \"What is 2+2?\""));
}

#[test]
fn ask_dry_run_renders_openai_chat_completions_request_without_network() {
    Command::cargo_bin("by-rs")
        .unwrap()
        .env("BY_NO_DOTENV", "1")
        .args([
            "ask",
            "--provider",
            "openai",
            "--model",
            "gpt-4o",
            "--temperature",
            "0.2",
            "--max-tokens",
            "64",
            "--dry-run",
            "What is 2+2?",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"provider\": \"openai\""))
        .stdout(predicate::str::contains(
            "\"operation\": \"chat/completions\"",
        ))
        .stdout(predicate::str::contains("\"network\": false"))
        .stdout(predicate::str::contains("\"model\": \"gpt-4o\""))
        .stdout(predicate::str::contains("\"temperature\": 0.2"))
        .stdout(predicate::str::contains("\"max_tokens\": 64"))
        .stdout(predicate::str::contains("\"role\": \"user\""))
        .stdout(predicate::str::contains("\"content\": \"What is 2+2?\""));
}

#[test]
fn ask_dry_run_drops_openai_temperature_for_gpt5_family() {
    Command::cargo_bin("by-rs")
        .unwrap()
        .env("BY_NO_DOTENV", "1")
        .args([
            "ask",
            "--provider",
            "openai",
            "--model",
            "gpt-5",
            "--temperature",
            "0.2",
            "--dry-run",
            "What is 2+2?",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"provider\": \"openai\""))
        .stdout(predicate::str::contains("\"model\": \"gpt-5\""))
        .stdout(predicate::str::contains("\"temperature\"").not());
}

#[test]
fn ask_dry_run_renders_anthropic_messages_request_without_network() {
    Command::cargo_bin("by-rs")
        .unwrap()
        .env("BY_NO_DOTENV", "1")
        .args([
            "ask",
            "--provider",
            "anthropic",
            "--model",
            "claude-sonnet-4-5",
            "--dry-run",
            "What is 2+2?",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"provider\": \"anthropic\""))
        .stdout(predicate::str::contains("\"operation\": \"messages\""))
        .stdout(predicate::str::contains("\"network\": false"))
        .stdout(predicate::str::contains("\"model\": \"claude-sonnet-4-5\""))
        .stdout(predicate::str::contains("\"max_tokens\": 4096"))
        .stdout(predicate::str::contains("\"role\": \"user\""))
        .stdout(predicate::str::contains("\"content\": \"What is 2+2?\""));
}

#[test]
fn ask_help_matches_clojure_command_surface() {
    Command::cargo_bin("by-rs")
        .unwrap()
        .args(["ask", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "NAME:\n by ask - Ask a one-shot question (non-interactive)",
        ))
        .stdout(predicate::str::contains("coact-agent  Agent ID"))
        .stdout(predicate::str::contains("claude-code  LM provider"))
        .stdout(predicate::str::contains("--dry-run").not())
        .stdout(predicate::str::contains("--live").not())
        .stdout(predicate::str::contains("--fixture-response").not());
}

#[test]
fn ask_fixture_response_replays_bedrock_output_without_network() {
    let fixture = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/bedrock/converse-response.json");

    Command::cargo_bin("by-rs")
        .unwrap()
        .env_remove("AWS_REGION")
        .env_remove("AWS_DEFAULT_REGION")
        .env_remove("AWS_PROFILE")
        .env_remove("AWS_DEFAULT_PROFILE")
        .args([
            "ask",
            "--provider",
            "bedrock",
            "--model",
            "amazon.nova-lite-v1:0",
            "--fixture-response",
        ])
        .arg(fixture)
        .arg("What is 2+2?")
        .assert()
        .success()
        .stdout(predicate::str::contains("Hello world"));
}

#[test]
fn ask_fixture_response_replays_openai_output_without_network() {
    let fixture = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/openai/chat-completion-response.json");

    Command::cargo_bin("by-rs")
        .unwrap()
        .env("BY_NO_DOTENV", "1")
        .env_remove("AWS_REGION")
        .env_remove("AWS_DEFAULT_REGION")
        .env_remove("AWS_PROFILE")
        .env_remove("AWS_DEFAULT_PROFILE")
        .args([
            "ask",
            "--provider",
            "openai",
            "--model",
            "gpt-5",
            "--fixture-response",
        ])
        .arg(fixture)
        .arg("What is 2+2?")
        .assert()
        .success()
        .stdout(predicate::str::contains("Hello from OpenAI fixture"));
}

#[test]
fn ask_fixture_response_replays_anthropic_output_without_network() {
    let fixture = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/anthropic/messages-response.json");

    Command::cargo_bin("by-rs")
        .unwrap()
        .env("BY_NO_DOTENV", "1")
        .env_remove("AWS_REGION")
        .env_remove("AWS_DEFAULT_REGION")
        .env_remove("AWS_PROFILE")
        .env_remove("AWS_DEFAULT_PROFILE")
        .args([
            "ask",
            "--provider",
            "anthropic",
            "--model",
            "claude-sonnet-4-5",
            "--fixture-response",
        ])
        .arg(fixture)
        .arg("What is 2+2?")
        .assert()
        .success()
        .stdout(predicate::str::contains("Hello from Anthropic fixture"));
}

#[test]
fn ask_fixture_response_rejects_other_execution_modes() {
    let fixture = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/bedrock/converse-response.json");

    Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "ask",
            "--provider",
            "bedrock",
            "--model",
            "amazon.nova-lite-v1:0",
            "--dry-run",
            "--fixture-response",
        ])
        .arg(fixture)
        .arg("What is 2+2?")
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "choose only one of --dry-run, --live, or --fixture-response",
        ));
}

#[test]
fn ask_requires_explicit_dry_run_or_live_mode() {
    Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "ask",
            "--provider",
            "bedrock",
            "--model",
            "amazon.nova-lite-v1:0",
            "What is 2+2?",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "by-rs ask requires --dry-run or --live",
        ));
}

#[test]
fn ask_rejects_dry_run_and_live_together() {
    Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "ask",
            "--provider",
            "bedrock",
            "--model",
            "amazon.nova-lite-v1:0",
            "--dry-run",
            "--live",
            "What is 2+2?",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "choose only one of --dry-run or --live",
        ));
}

#[test]
fn ask_dry_run_uses_config_defaults_when_provider_and_model_are_omitted() {
    let home = tempfile::tempdir().unwrap();
    let brainyard = home.path().join(".brainyard");
    std::fs::create_dir_all(&brainyard).unwrap();
    std::fs::write(
        brainyard.join("config.edn"),
        r#"{:llm {:default-provider :bedrock
                 :default-model "amazon.nova-lite-v1:0"}}"#,
    )
    .unwrap();

    Command::cargo_bin("by-rs")
        .unwrap()
        .current_dir(home.path())
        .env("HOME", home.path())
        .env_remove("BRAINYARD_PROJECT_DIR")
        .args(["ask", "--dry-run", "What is 2+2?"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"provider\": \"bedrock\""))
        .stdout(predicate::str::contains(
            "\"modelId\": \"amazon.nova-lite-v1:0\"",
        ));
}

#[test]
fn ask_dry_run_parses_legacy_provider_model_positional() {
    Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "ask",
            "--dry-run",
            "bedrock:amazon.nova-lite-v1",
            "What is 2+2?",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"provider\": \"bedrock\""))
        .stdout(predicate::str::contains(
            "\"modelId\": \"amazon.nova-lite-v1\"",
        ))
        .stdout(predicate::str::contains("\"text\": \"What is 2+2?\""));
}

#[test]
fn ask_dry_run_keeps_bedrock_model_id_with_second_colon_as_question_text() {
    Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "ask",
            "--provider",
            "bedrock",
            "--model",
            "amazon.nova-lite-v1:0",
            "--dry-run",
            "bedrock:amazon.nova-lite-v1:0",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "\"modelId\": \"amazon.nova-lite-v1:0\"",
        ))
        .stdout(predicate::str::contains(
            "\"text\": \"bedrock:amazon.nova-lite-v1:0\"",
        ));
}

#[test]
fn ask_dry_run_resolves_bedrock_region_and_profile_from_environment() {
    Command::cargo_bin("by-rs")
        .unwrap()
        .env("AWS_REGION", "eu-west-1")
        .env("AWS_DEFAULT_REGION", "us-east-1")
        .env("AWS_PROFILE", "dev")
        .env("AWS_DEFAULT_PROFILE", "fallback")
        .args([
            "ask",
            "--provider",
            "bedrock",
            "--model",
            "amazon.nova-lite-v1:0",
            "--dry-run",
            "What is 2+2?",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"region\": \"eu-west-1\""))
        .stdout(predicate::str::contains("\"aws_profile\": \"dev\""));
}

#[test]
fn ask_dry_run_explicit_region_and_profile_win_over_environment() {
    Command::cargo_bin("by-rs")
        .unwrap()
        .env("AWS_REGION", "eu-west-1")
        .env("AWS_PROFILE", "dev")
        .args([
            "ask",
            "--provider",
            "bedrock",
            "--model",
            "amazon.nova-lite-v1:0",
            "--region",
            "ap-northeast-2",
            "--aws-profile",
            "sandbox",
            "--dry-run",
            "What is 2+2?",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"region\": \"ap-northeast-2\""))
        .stdout(predicate::str::contains("\"aws_profile\": \"sandbox\""));
}

#[test]
fn ask_dry_run_exposes_bedrock_inference_overrides() {
    Command::cargo_bin("by-rs")
        .unwrap()
        .env("BY_NO_DOTENV", "1")
        .env_remove("AWS_REGION")
        .env_remove("AWS_DEFAULT_REGION")
        .env_remove("AWS_PROFILE")
        .env_remove("AWS_DEFAULT_PROFILE")
        .args([
            "ask",
            "--provider",
            "bedrock",
            "--model",
            "amazon.nova-lite-v1:0",
            "--temperature",
            "0.2",
            "--max-tokens",
            "64",
            "--no-prompt-cache",
            "--dry-run",
            "What is 2+2?",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"region\": \"us-east-1\""))
        .stdout(predicate::str::contains("\"temperature\": 0.2"))
        .stdout(predicate::str::contains("\"maxTokens\": 64"))
        .stdout(predicate::str::contains("cachePoint").not());
}

#[test]
fn tui_snapshot_renders_static_chrome() {
    Command::cargo_bin("by-rs")
        .unwrap()
        .args([
            "tui",
            "snapshot",
            "--agent",
            "coact-agent",
            "--model",
            "bedrock:amazon.nova-lite-v1:0",
            "--rows",
            "12",
            "--cols",
            "56",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Brainyard by-rs"))
        .stdout(predicate::str::contains("agent coact-agent"))
        .stdout(predicate::str::contains(" 0:main0*"))
        .stdout(predicate::str::contains(
            "idle │ 0 calls │ 0 tokens │ $0.0000",
        ));
}

fn create_memory_schema(conn: &Connection) {
    conn.execute_batch(
        r#"
        CREATE TABLE memory_metadata (
          key TEXT PRIMARY KEY,
          value TEXT NOT NULL,
          updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
        );

        CREATE TABLE episodes (
          id INTEGER PRIMARY KEY AUTOINCREMENT,
          session_id TEXT NOT NULL,
          user_id TEXT NOT NULL,
          timestamp DATETIME DEFAULT CURRENT_TIMESTAMP,
          episode_type TEXT NOT NULL,
          role TEXT,
          content TEXT NOT NULL,
          metadata TEXT,
          tags TEXT,
          sources TEXT,
          entry_id TEXT,
          keep_flag INTEGER NOT NULL DEFAULT 0,
          archived_flag INTEGER NOT NULL DEFAULT 0,
          tombstoned_flag INTEGER NOT NULL DEFAULT 0
        );

        CREATE VIRTUAL TABLE episodes_fts USING fts5(
          content,
          episode_type,
          role,
          content='episodes',
          content_rowid='id',
          tokenize='porter unicode61'
        );

        CREATE TRIGGER episodes_ai AFTER INSERT ON episodes BEGIN
          INSERT INTO episodes_fts(rowid, content, episode_type, role)
          VALUES (new.id, new.content, new.episode_type, new.role);
        END;

        CREATE TABLE semantic_facts (
          id INTEGER PRIMARY KEY AUTOINCREMENT,
          user_id TEXT NOT NULL,
          fact_type TEXT NOT NULL,
          content TEXT NOT NULL,
          source TEXT,
          confidence REAL DEFAULT 1.0,
          created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
          updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
          access_count INTEGER DEFAULT 0,
          last_accessed DATETIME,
          metadata TEXT,
          tags TEXT,
          sources TEXT,
          entry_id TEXT,
          keep_flag INTEGER NOT NULL DEFAULT 0,
          archived_flag INTEGER NOT NULL DEFAULT 0,
          tombstoned_flag INTEGER NOT NULL DEFAULT 0
        );

        CREATE VIRTUAL TABLE semantic_fts USING fts5(
          content,
          fact_type,
          content='semantic_facts',
          content_rowid='id',
          tokenize='porter unicode61'
        );

        CREATE TRIGGER semantic_facts_ai AFTER INSERT ON semantic_facts BEGIN
          INSERT INTO semantic_fts(rowid, content, fact_type)
          VALUES (new.id, new.content, new.fact_type);
        END;

        CREATE TABLE memory_audit (
          id INTEGER PRIMARY KEY AUTOINCREMENT,
          user_id TEXT NOT NULL,
          session_id TEXT NOT NULL,
          agent_id TEXT,
          turn_id INTEGER NOT NULL,
          total_turns INTEGER,
          entry_id TEXT NOT NULL,
          layer TEXT NOT NULL,
          byte_cost INTEGER,
          created_at DATETIME DEFAULT CURRENT_TIMESTAMP
        );
        "#,
    )
    .unwrap();
}
