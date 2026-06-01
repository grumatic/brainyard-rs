use by_memory::{inspect_memory, search_memory, MemoryLayer, MemorySearchRequest};
use rusqlite::Connection;

#[test]
fn searches_clojure_compatible_l2_and_l3_fts_tables() {
    let dir = tempfile::tempdir().expect("temp dir");
    let db_path = dir.path().join("memory.db");
    let conn = Connection::open(&db_path).expect("sqlite db");
    create_memory_schema(&conn);
    seed_memory_rows(&conn);
    drop(conn);

    let hits = search_memory(
        &db_path,
        MemorySearchRequest {
            query: "blue green".to_string(),
            limit: 10,
        },
    )
    .expect("search should read memory db");

    assert_eq!(hits.len(), 2);
    assert!(hits.iter().any(|hit| {
        hit.layer == MemoryLayer::L2
            && hit.kind == "conversation"
            && hit.content.contains("blue deploy")
    }));
    assert!(hits.iter().any(|hit| {
        hit.layer == MemoryLayer::L3
            && hit.kind == "preference"
            && hit.content.contains("green releases")
    }));
    assert!(hits.iter().all(|hit| !hit.content.contains("red rollback")));
}

#[test]
fn blank_queries_return_no_hits_without_touching_fts() {
    let dir = tempfile::tempdir().expect("temp dir");
    let db_path = dir.path().join("memory.db");
    let conn = Connection::open(&db_path).expect("sqlite db");
    create_memory_schema(&conn);
    seed_memory_rows(&conn);
    drop(conn);

    let hits = search_memory(&db_path, MemorySearchRequest::new("   "))
        .expect("blank search should be a harmless no-op");

    assert!(hits.is_empty());
}

#[test]
fn inspect_reports_schema_version_and_static_table_counts() {
    let dir = tempfile::tempdir().expect("temp dir");
    let db_path = dir.path().join("memory.db");
    let conn = Connection::open(&db_path).expect("sqlite db");
    create_memory_schema(&conn);
    seed_memory_rows(&conn);
    conn.execute(
        "INSERT OR REPLACE INTO memory_metadata (key, value)
         VALUES ('schema_version', '2.0.0')",
        [],
    )
    .expect("schema version");
    conn.execute(
        "INSERT INTO memory_audit
         (user_id, session_id, agent_id, turn_id, total_turns, entry_id, layer, byte_cost)
         VALUES ('u1', 's1', 'coact-agent', 1, 1, 'entry-1', 'l2', 42)",
        [],
    )
    .expect("audit row");
    drop(conn);

    let report = inspect_memory(&db_path).expect("inspect should read memory db");

    assert_eq!(report.schema_version.as_deref(), Some("2.0.0"));
    assert!(report.sqlite_user_version >= 0);
    assert!(!report.journal_mode.is_empty());
    assert_table(&report.tables, "memory_metadata", true, Some(1));
    assert_table(&report.tables, "episodes", true, Some(2));
    assert_table(&report.tables, "episodes_fts", true, Some(2));
    assert_table(&report.tables, "semantic_facts", true, Some(2));
    assert_table(&report.tables, "semantic_fts", true, Some(2));
    assert_table(&report.tables, "memory_audit", true, Some(1));
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
    .expect("schema");
}

fn assert_table(
    tables: &[by_memory::MemoryTableStats],
    name: &str,
    present: bool,
    rows: Option<i64>,
) {
    let table = tables
        .iter()
        .find(|table| table.name == name)
        .unwrap_or_else(|| panic!("missing table stats for {name}"));
    assert_eq!(table.present, present, "present mismatch for {name}");
    assert_eq!(table.rows, rows, "row count mismatch for {name}");
}

fn seed_memory_rows(conn: &Connection) {
    conn.execute(
        "INSERT INTO episodes (session_id, user_id, episode_type, role, content)
         VALUES ('s1', 'u1', 'conversation', 'assistant', 'remember the blue deploy plan')",
        [],
    )
    .expect("matching episode");
    conn.execute(
        "INSERT INTO episodes (session_id, user_id, episode_type, role, content)
         VALUES ('s1', 'u1', 'conversation', 'assistant', 'red rollback notes')",
        [],
    )
    .expect("non-matching episode");
    conn.execute(
        "INSERT INTO semantic_facts (user_id, fact_type, content, confidence)
         VALUES ('u1', 'preference', 'user prefers green releases', 0.9)",
        [],
    )
    .expect("matching fact");
    conn.execute(
        "INSERT INTO semantic_facts (user_id, fact_type, content, confidence, tombstoned_flag)
         VALUES ('u1', 'preference', 'green tombstoned result', 1.0, 1)",
        [],
    )
    .expect("tombstoned fact");
}
