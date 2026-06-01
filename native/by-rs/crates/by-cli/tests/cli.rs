use assert_cmd::Command;
use predicates::prelude::*;
use rusqlite::Connection;

#[test]
fn help_exposes_read_only_spike_commands() {
    Command::cargo_bin("by-rs")
        .unwrap()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("agents"))
        .stdout(predicate::str::contains("models"))
        .stdout(predicate::str::contains("sessions"));
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
        .stdout(predicate::str::contains("coder"))
        .stdout(predicate::str::contains("Coder"));
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
        .stdout(predicate::str::contains("bedrock:amazon.nova-lite-v1:0"));
}

#[test]
fn sessions_list_reads_fixture_root_without_writing() {
    let root = tempfile::tempdir().unwrap();
    let session = root.path().join("alpha");
    std::fs::create_dir_all(&session).unwrap();
    std::fs::write(
        session.join("meta.edn"),
        r#"{:id "alpha" :label "Alpha session"}"#,
    )
    .unwrap();

    Command::cargo_bin("by-rs")
        .unwrap()
        .args(["sessions", "list", "--root"])
        .arg(root.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("alpha"))
        .stdout(predicate::str::contains("Alpha session"));
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
        .stdout(predicate::str::contains("bedrock:amazon.nova-lite-v1:0"))
        .stdout(predicate::str::contains("openai:gpt-5").not());
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
        .stdout(predicate::str::contains(
            "bedrock:openai.gpt-oss-120b-1:0 (us-east-1)",
        ));
}

#[test]
fn help_exposes_config_inspection_command() {
    Command::cargo_bin("by-rs")
        .unwrap()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("config"));
}

#[test]
fn config_show_reads_llm_defaults_without_writing() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.edn");
    std::fs::write(
        &path,
        r#"{:llm {:default-provider :bedrock
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
        .stdout(predicate::str::contains("llm.default-provider\tbedrock"))
        .stdout(predicate::str::contains(
            "llm.default-model\tamazon.nova-lite-v1:0",
        ))
        .stdout(predicate::str::contains(
            "llm.available-providers\tbedrock,claude-code",
        ));
}

#[test]
fn help_exposes_memory_search_command() {
    Command::cargo_bin("by-rs")
        .unwrap()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("memory"));
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
        .env("HOME", home.path())
        .args(["ask", "--dry-run", "What is 2+2?"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"provider\": \"bedrock\""))
        .stdout(predicate::str::contains(
            "\"modelId\": \"amazon.nova-lite-v1:0\"",
        ));
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
        "#,
    )
    .unwrap();
}
