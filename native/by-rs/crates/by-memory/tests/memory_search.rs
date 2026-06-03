use by_memory::{
    consolidate_l2_memory, entry_id_for, explain_memory_session, explain_memory_turn,
    extract_keywords, extract_keywords_with_limits, forget_memory_entry, inspect_memory,
    memory_stats, normalize_for_hash, promote_memory_entry, purge_plan_memory, read_memory,
    recall_memory, search_memory, set_memory_archive_flag, set_memory_keep_flag, sweep_l2_memory,
    write_memory_entry, MemoryConsolidateRequest, MemoryEntryMutationRequest, MemoryLayer,
    MemoryPromoteRequest, MemoryPurgePlanRequest, MemoryReadRequest, MemoryRecallRequest,
    MemorySearchRequest, MemoryStatsRequest, MemorySweepL2Request, MemoryWriteRequest,
};
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
fn recall_projects_combined_rrf_shape_from_l2_and_l3_without_l1_live_state() {
    let dir = tempfile::tempdir().expect("temp dir");
    let db_path = dir.path().join("memory.db");
    let conn = Connection::open(&db_path).expect("sqlite db");
    create_memory_schema(&conn);
    seed_recall_rows(&conn);
    drop(conn);

    let report = recall_memory(
        &db_path,
        MemoryRecallRequest {
            user_id: "u1".to_string(),
            session_id: Some("s1".to_string()),
            query: "blue green".to_string(),
            layer: None,
            limit: 10,
            match_mode: "or".to_string(),
            kind: None,
            min_confidence: None,
        },
    )
    .expect("recall should read memory db");

    assert_eq!(report["layer"], "combined");
    assert_eq!(report["match"], "or");
    assert_eq!(report["l1-persisted?"], false);
    assert_eq!(report["l1-live-skipped?"], true);
    assert_eq!(report["count"], 2);
    let entries = report["entries"].as_array().expect("entries");
    assert_eq!(entries[0]["_layer"], "l3");
    assert_eq!(entries[0]["kind"], "preference");
    assert!(
        entries[0]["_rrf_score"].as_f64().unwrap() > entries[1]["_rrf_score"].as_f64().unwrap()
    );
    assert!(entries.iter().any(|entry| {
        entry["_layer"] == "l2"
            && entry["id"] == "ep-blue"
            && entry["session-id"] == "s1"
            && entry["tags"].as_array().is_some_and(|tags| tags.len() == 1)
    }));
}

#[test]
fn recall_projects_layer_filters_match_modes_and_confidence() {
    let dir = tempfile::tempdir().expect("temp dir");
    let db_path = dir.path().join("memory.db");
    let conn = Connection::open(&db_path).expect("sqlite db");
    create_memory_schema(&conn);
    seed_recall_rows(&conn);
    drop(conn);

    let l2 = recall_memory(
        &db_path,
        MemoryRecallRequest {
            user_id: "u1".to_string(),
            session_id: Some("s1".to_string()),
            query: "blue deploy".to_string(),
            layer: Some(":l2".to_string()),
            limit: 10,
            match_mode: "and".to_string(),
            kind: Some(":conversation".to_string()),
            min_confidence: None,
        },
    )
    .expect("l2 recall should read episodes");
    assert_eq!(l2["layer"], "l2");
    assert_eq!(l2["match"], "and");
    assert_eq!(l2["count"], 1);
    assert_eq!(l2["entries"][0]["id"], "ep-blue");
    assert_eq!(l2["entries"][0]["role"], "assistant");
    assert_eq!(l2["entries"][0]["session-id"], "s1");

    let l3 = recall_memory(
        &db_path,
        MemoryRecallRequest {
            user_id: "u1".to_string(),
            session_id: Some("s1".to_string()),
            query: String::new(),
            layer: Some("l3".to_string()),
            limit: 10,
            match_mode: "phrase".to_string(),
            kind: Some("preference".to_string()),
            min_confidence: Some(0.8),
        },
    )
    .expect("l3 recall should read semantic facts");
    assert_eq!(l3["layer"], "l3");
    assert_eq!(l3["match"], "phrase");
    assert_eq!(l3["count"], 1);
    assert_eq!(l3["entries"][0]["id"], "fact-green");
    assert_eq!(l3["entries"][0]["confidence"], 0.9);

    let l1 = recall_memory(
        &db_path,
        MemoryRecallRequest {
            user_id: "u1".to_string(),
            session_id: Some("s1".to_string()),
            query: "anything".to_string(),
            layer: Some("l1".to_string()),
            limit: 10,
            match_mode: "or".to_string(),
            kind: None,
            min_confidence: None,
        },
    )
    .expect("l1 projection should be empty without live state");
    assert_eq!(l1["layer"], "l1");
    assert_eq!(l1["count"], 0);
    assert_eq!(l1["l1-persisted?"], false);
}

#[test]
fn read_projects_memory_agent_layer_queries_without_live_l1_state() {
    let dir = tempfile::tempdir().expect("temp dir");
    let db_path = dir.path().join("memory.db");
    let conn = Connection::open(&db_path).expect("sqlite db");
    create_memory_schema(&conn);
    seed_recall_rows(&conn);
    drop(conn);

    let by_id = read_memory(
        &db_path,
        MemoryReadRequest {
            user_id: "u1".to_string(),
            layer: "l2".to_string(),
            query: serde_json::json!({"id": "ep-blue"}),
            limit: 20,
            include_archived: false,
        },
    )
    .expect("read by l2 entry id");
    assert_eq!(by_id["layer"], "l2");
    assert_eq!(by_id["count"], 1);
    assert_eq!(by_id["entries"][0]["id"], "ep-blue");
    assert_eq!(by_id["entries"][0]["kind"], "conversation");
    assert_eq!(by_id["entries"][0]["archived"], false);

    let archived_hidden = read_memory(
        &db_path,
        MemoryReadRequest {
            user_id: "u1".to_string(),
            layer: "l2".to_string(),
            query: serde_json::json!({"id": "ep-archived"}),
            limit: 20,
            include_archived: false,
        },
    )
    .expect("archived rows should be hidden by default");
    assert_eq!(archived_hidden["count"], 0);

    let archived_visible = read_memory(
        &db_path,
        MemoryReadRequest {
            user_id: "u1".to_string(),
            layer: "l2".to_string(),
            query: serde_json::json!({"id": "ep-archived"}),
            limit: 20,
            include_archived: true,
        },
    )
    .expect("include archived should expose archived rows");
    assert_eq!(archived_visible["count"], 1);
    assert_eq!(archived_visible["entries"][0]["archived"], true);

    let l2_text = read_memory(
        &db_path,
        MemoryReadRequest {
            user_id: "u1".to_string(),
            layer: ":l2".to_string(),
            query: serde_json::json!({
                "text": "blue deploy",
                "session-id": "s1",
                "kind": ":conversation",
                "match": "and"
            }),
            limit: 20,
            include_archived: false,
        },
    )
    .expect("read l2 text query");
    assert_eq!(l2_text["count"], 1);
    assert_eq!(l2_text["entries"][0]["id"], "ep-blue");
    assert_eq!(l2_text["entries"][0]["session-id"], "s1");

    let l3_text = read_memory(
        &db_path,
        MemoryReadRequest {
            user_id: "u1".to_string(),
            layer: "l3".to_string(),
            query: serde_json::json!({
                "text": "green releases",
                "fact-type": "preference",
                "min-confidence": 0.8,
                "match": "phrase"
            }),
            limit: 20,
            include_archived: false,
        },
    )
    .expect("read l3 text query");
    assert_eq!(l3_text["layer"], "l3");
    assert_eq!(l3_text["count"], 1);
    assert_eq!(l3_text["entries"][0]["id"], "fact-green");
    assert_eq!(l3_text["entries"][0]["confidence"], 0.9);

    let l1 = read_memory(
        &db_path,
        MemoryReadRequest {
            user_id: "u1".to_string(),
            layer: "l1".to_string(),
            query: serde_json::json!({"session-id": "s1"}),
            limit: 20,
            include_archived: false,
        },
    )
    .expect("l1 read projection should be empty without live state");
    assert_eq!(l1["layer"], "l1");
    assert_eq!(l1["count"], 0);
    assert_eq!(l1["l1-persisted?"], false);
    assert_eq!(l1["l1-live-skipped?"], true);
}

#[test]
fn memory_policy_toggles_update_persisted_l2_l3_without_live_agent() {
    let dir = tempfile::tempdir().expect("temp dir");
    let db_path = dir.path().join("memory.db");
    let conn = Connection::open(&db_path).expect("sqlite db");
    create_memory_schema(&conn);
    seed_recall_rows(&conn);
    drop(conn);

    let keep = set_memory_keep_flag(
        &db_path,
        MemoryEntryMutationRequest {
            user_id: "u1".to_string(),
            layer: "l2".to_string(),
            entry_id: "ep-blue".to_string(),
            value: true,
        },
    )
    .expect("keep toggle should update episode");
    assert_eq!(keep["ok"], true);
    assert_eq!(keep["layer"], "l2");
    assert_eq!(keep["value"], true);

    let archive = set_memory_archive_flag(
        &db_path,
        MemoryEntryMutationRequest {
            user_id: "u1".to_string(),
            layer: "l3".to_string(),
            entry_id: "fact-green".to_string(),
            value: true,
        },
    )
    .expect("archive toggle should update semantic fact");
    assert_eq!(archive["ok"], true);
    assert_eq!(archive["layer"], "l3");
    assert_eq!(archive["value"], true);

    let hidden = read_memory(
        &db_path,
        MemoryReadRequest {
            user_id: "u1".to_string(),
            layer: "l3".to_string(),
            query: serde_json::json!({"id": "fact-green"}),
            limit: 20,
            include_archived: false,
        },
    )
    .expect("archived facts should be hidden by default");
    assert_eq!(hidden["count"], 0);

    let visible = read_memory(
        &db_path,
        MemoryReadRequest {
            user_id: "u1".to_string(),
            layer: "l3".to_string(),
            query: serde_json::json!({"id": "fact-green"}),
            limit: 20,
            include_archived: true,
        },
    )
    .expect("archived facts should be visible when requested");
    assert_eq!(visible["count"], 1);
    assert_eq!(visible["entries"][0]["archived"], true);

    let forget = forget_memory_entry(
        &db_path,
        MemoryEntryMutationRequest::new("u1", ":l2", "ep-blue"),
    )
    .expect("forget should tombstone episode");
    assert_eq!(forget["ok"], true);
    assert_eq!(forget["layer"], "l2");

    let tombstoned_hidden = read_memory(
        &db_path,
        MemoryReadRequest {
            user_id: "u1".to_string(),
            layer: "l2".to_string(),
            query: serde_json::json!({"id": "ep-blue"}),
            limit: 20,
            include_archived: true,
        },
    )
    .expect("tombstoned entries should remain hidden");
    assert_eq!(tombstoned_hidden["count"], 0);

    let l1_keep = set_memory_keep_flag(
        &db_path,
        MemoryEntryMutationRequest::new("u1", "l1", "ep-blue"),
    )
    .expect("l1 toggle should report a local projection error");
    assert_eq!(l1_keep["ok"], false);
    assert!(l1_keep["error"]
        .as_str()
        .unwrap()
        .contains("layer must be l2 or l3"));
}

#[test]
fn memory_write_persists_l2_l3_entries_without_live_agent() {
    let dir = tempfile::tempdir().expect("temp dir");
    let db_path = dir.path().join("memory.db");
    let conn = Connection::open(&db_path).expect("sqlite db");
    create_memory_schema(&conn);
    drop(conn);

    let l2_content = "remember blue deploy window";
    let l2 = write_memory_entry(
        &db_path,
        MemoryWriteRequest::new(
            "u1",
            "l2",
            serde_json::json!({
                "session-id": "s-write",
                "kind": "observation",
                "role": "assistant",
                "content": l2_content,
                "tags": ["blue"],
                "data": {"plan": 1}
            }),
        ),
    )
    .expect("l2 memory write should persist");
    assert_eq!(l2["layer"], "l2");
    assert_eq!(l2["entry"]["session-id"], "s-write");
    assert_eq!(l2["entry"]["kind"], "observation");
    assert_eq!(l2["entry"]["role"], "assistant");
    assert_eq!(l2["entry"]["content"], l2_content);
    assert_eq!(l2["entry"]["tags"][0], "blue");
    assert_eq!(l2["entry"]["data"]["plan"], 1);
    assert!(l2["entry-id"].as_str().unwrap().starts_with("l2/"));

    let l2_read = read_memory(
        &db_path,
        MemoryReadRequest {
            user_id: "u1".to_string(),
            layer: "l2".to_string(),
            query: serde_json::json!({"id": l2["entry-id"]}),
            limit: 20,
            include_archived: false,
        },
    )
    .expect("l2 write should be readable");
    assert_eq!(l2_read["count"], 1);
    assert_eq!(l2_read["entries"][0]["content"], l2_content);

    let l3_content = "User prefers green release windows";
    let expected_l3_id = entry_id_for("l3", l3_content).expect("l3 entry id");
    let l3 = write_memory_entry(
        &db_path,
        MemoryWriteRequest::new(
            "u1",
            "l3",
            serde_json::json!({
                "kind": "preference",
                "content": l3_content,
                "source": "manual",
                "confidence": 0.8,
                "tags": ["green"],
                "metadata": {"origin": "test"}
            }),
        ),
    )
    .expect("l3 memory write should persist");
    assert_eq!(l3["entry-id"], expected_l3_id);
    assert_eq!(l3["entry"]["source"], "manual");
    assert_eq!(l3["entry"]["confidence"], 0.8);
    assert_eq!(l3["entry"]["metadata"]["origin"], "test");

    let duplicate = write_memory_entry(
        &db_path,
        MemoryWriteRequest::new(
            "u1",
            "l3",
            serde_json::json!({
                "kind": "preference",
                "content": l3_content,
            }),
        ),
    )
    .expect("duplicate l3 memory write should return existing entry");
    assert_eq!(duplicate["duplicate?"], true);
    assert_eq!(duplicate["entry-id"], expected_l3_id);

    let l1 = write_memory_entry(
        &db_path,
        MemoryWriteRequest::new(
            "u1",
            "l1",
            serde_json::json!({"content": "needs live memory manager"}),
        ),
    )
    .expect("l1 write should report local projection gap");
    assert_eq!(l1["l1-live-skipped?"], true);
    assert!(l1["error"].as_str().unwrap().contains("requires live"));
}

#[test]
fn memory_promote_persists_cross_layer_copy_with_provenance_without_live_agent() {
    let dir = tempfile::tempdir().expect("temp dir");
    let db_path = dir.path().join("memory.db");
    let conn = Connection::open(&db_path).expect("sqlite db");
    create_memory_schema(&conn);
    drop(conn);

    let report = promote_memory_entry(
        &db_path,
        MemoryPromoteRequest {
            user_id: "u1".to_string(),
            from_layer: "l2".to_string(),
            to_layer: "l3".to_string(),
            entry: serde_json::json!({
                "id": "ep-promote",
                "kind": "observation",
                "content": "blue deploy should become semantic memory",
                "user-id": "u1",
                "session-id": "s1",
                "role": "assistant",
                "tags": ["deploy"],
                "sources": [{"type": "capture", "id": "turn-1"}],
                "confidence": 0.8
            }),
            new_entry_id: Some("fact-promoted".to_string()),
        },
    )
    .expect("promote should write l3");

    assert_eq!(report["entry-id"], "fact-promoted");
    assert_eq!(report["from"], "l2");
    assert_eq!(report["to"], "l3");
    assert_eq!(report["source-entry-id"], "ep-promote");
    assert_eq!(report["entry"]["layer"], "l3");
    assert_eq!(report["entry"]["id"], "fact-promoted");
    assert_eq!(report["entry"]["sources"].as_array().unwrap().len(), 2);
    assert_eq!(report["entry"]["sources"][1]["type"], "promotion");
    assert_eq!(report["entry"]["sources"][1]["id"], "ep-promote");
    assert_eq!(report["entry"]["sources"][1]["from-layer"], "l2");

    let read = read_memory(
        &db_path,
        MemoryReadRequest {
            user_id: "u1".to_string(),
            layer: "l3".to_string(),
            query: serde_json::json!({"id": "fact-promoted"}),
            limit: 5,
            include_archived: false,
        },
    )
    .expect("read promoted fact");
    assert_eq!(read["count"], 1);
    assert_eq!(read["entries"][0]["sources"][1]["from-layer"], "l2");

    let l1 = promote_memory_entry(
        &db_path,
        MemoryPromoteRequest {
            user_id: "u1".to_string(),
            from_layer: "l2".to_string(),
            to_layer: "l1".to_string(),
            entry: serde_json::json!({"id": "ep-promote", "content": "x"}),
            new_entry_id: None,
        },
    )
    .expect("l1 report");
    assert_eq!(l1["l1-live-skipped?"], true);
    assert!(l1["error"].as_str().unwrap().contains("l1"));
}

#[test]
fn memory_sweep_l2_tombstones_expired_non_kept_episodes_without_live_agent() {
    let dir = tempfile::tempdir().expect("temp dir");
    let db_path = dir.path().join("memory.db");
    let conn = Connection::open(&db_path).expect("sqlite db");
    create_memory_schema(&conn);
    conn.execute(
        "INSERT INTO episodes
         (session_id, user_id, timestamp, episode_type, role, content, entry_id)
         VALUES ('s-old', 'u1', datetime('now', '-31 days'), 'conversation', 'assistant',
                 'old blue episode', 'ep-old')",
        [],
    )
    .expect("old episode");
    conn.execute(
        "INSERT INTO episodes
         (session_id, user_id, timestamp, episode_type, role, content, entry_id, keep_flag)
         VALUES ('s-keep', 'u1', datetime('now', '-31 days'), 'conversation', 'assistant',
                 'kept old episode', 'ep-keep', 1)",
        [],
    )
    .expect("kept old episode");
    conn.execute(
        "INSERT INTO episodes
         (session_id, user_id, timestamp, episode_type, role, content, entry_id)
         VALUES ('s-new', 'u1', datetime('now', '-1 days'), 'conversation', 'assistant',
                 'new episode', 'ep-new')",
        [],
    )
    .expect("new episode");
    conn.execute(
        "INSERT INTO episodes
         (session_id, user_id, timestamp, episode_type, role, content, entry_id)
         VALUES ('s-other', 'u2', datetime('now', '-31 days'), 'conversation', 'assistant',
                 'other user old episode', 'ep-other')",
        [],
    )
    .expect("other user old episode");
    drop(conn);

    let report = sweep_l2_memory(
        &db_path,
        MemorySweepL2Request {
            user_id: "u1".to_string(),
            retention_days: 30,
        },
    )
    .expect("sweep should tombstone eligible l2 rows");
    assert_eq!(report["tombstoned"], 1);
    assert_eq!(report["retention-days"], 30);

    let conn = Connection::open(&db_path).expect("sqlite db");
    let tombstoned = |entry_id: &str, user_id: &str| -> i64 {
        conn.query_row(
            "SELECT tombstoned_flag FROM episodes WHERE entry_id = ?1 AND user_id = ?2",
            (entry_id, user_id),
            |row| row.get(0),
        )
        .expect("episode tombstone flag")
    };
    assert_eq!(tombstoned("ep-old", "u1"), 1);
    assert_eq!(tombstoned("ep-keep", "u1"), 0);
    assert_eq!(tombstoned("ep-new", "u1"), 0);
    assert_eq!(tombstoned("ep-other", "u2"), 0);

    let read = read_memory(
        &db_path,
        MemoryReadRequest {
            user_id: "u1".to_string(),
            layer: "l2".to_string(),
            query: serde_json::json!({"id": "ep-old"}),
            limit: 5,
            include_archived: true,
        },
    )
    .expect("tombstoned entries should be hidden from normal reads");
    assert_eq!(read["count"], 0);
}

#[test]
fn memory_consolidate_writes_l3_summary_and_keeps_sources_without_live_agent() {
    let dir = tempfile::tempdir().expect("temp dir");
    let db_path = dir.path().join("memory.db");
    let conn = Connection::open(&db_path).expect("sqlite db");
    create_memory_schema(&conn);
    for (entry_id, timestamp, content) in [
        ("ep-c1", "2026-01-01 00:00:00", "blue deploy started"),
        ("ep-c2", "2026-01-01 00:01:00", "blue deploy checked health"),
        ("ep-c3", "2026-01-01 00:02:00", "blue deploy finished"),
    ] {
        conn.execute(
            "INSERT INTO episodes
             (session_id, user_id, timestamp, episode_type, role, content, tags, entry_id)
             VALUES (?1, 'u1', ?2, 'conversation', 'assistant', ?3,
                     '[\"deploy\",\"event:turn\",\"kind:message\"]', ?4)",
            ("s-consolidate", timestamp, content, entry_id),
        )
        .expect("consolidation episode");
    }
    conn.execute(
        "INSERT INTO episodes
         (session_id, user_id, timestamp, episode_type, role, content, tags, entry_id)
         VALUES ('s-consolidate', 'u2', '2026-01-01 00:01:30', 'conversation', 'assistant',
                 'other user deploy', '[\"deploy\"]', 'ep-other-user')",
        [],
    )
    .expect("other user row");
    drop(conn);

    let report = consolidate_l2_memory(
        &db_path,
        MemoryConsolidateRequest {
            user_id: "u1".to_string(),
            session_id: Some("s-consolidate".to_string()),
            window_ms: 600_000,
            min_batch: 3,
            reducer: "heuristic".to_string(),
        },
    )
    .expect("consolidation should produce a local l3 summary");

    assert_eq!(report["report"]["from-layer"], "l2");
    assert_eq!(report["report"]["to-layer"], "l3");
    assert_eq!(report["report"]["produced"], 1);
    assert_eq!(report["report"]["consumed"], 3);
    assert_eq!(report["report"]["auto-kept"], 3);
    assert_eq!(
        report["report"]["batches"][0]["tags"],
        serde_json::json!(["deploy"])
    );

    let read = read_memory(
        &db_path,
        MemoryReadRequest {
            user_id: "u1".to_string(),
            layer: "l3".to_string(),
            query: serde_json::json!({"kind": "summary"}),
            limit: 5,
            include_archived: false,
        },
    )
    .expect("read consolidated summary");
    assert_eq!(read["count"], 1);
    assert!(read["entries"][0]["content"]
        .as_str()
        .expect("summary content")
        .contains("[summary of 3 events] tags=[\"deploy\"]"));
    assert_eq!(read["entries"][0]["sources"].as_array().unwrap().len(), 3);
    assert_eq!(read["entries"][0]["sources"][0]["from-layer"], "l2");

    let conn = Connection::open(&db_path).expect("sqlite db");
    let kept: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM episodes WHERE user_id = 'u1' AND keep_flag = 1",
            [],
            |row| row.get(0),
        )
        .expect("kept count");
    assert_eq!(kept, 3);
}

#[test]
fn purge_plan_projects_candidates_without_live_registry_state() {
    let dir = tempfile::tempdir().expect("temp dir");
    let db_path = dir.path().join("memory.db");
    let sessions_root = dir.path().join("sessions");
    std::fs::create_dir_all(sessions_root.join("s-live")).expect("live session dir");
    std::fs::create_dir_all(sessions_root.join(".hidden")).expect("hidden dir");
    let conn = Connection::open(&db_path).expect("sqlite db");
    create_memory_schema(&conn);
    conn.execute(
        "INSERT INTO episodes
         (session_id, user_id, episode_type, role, content, entry_id)
         VALUES ('s-orphan', 'u1', 'conversation', 'assistant',
                 'orphan blue episode content that should be truncated only if very long',
                 'ep-orphan')",
        [],
    )
    .expect("orphan episode");
    conn.execute(
        "INSERT INTO episodes
         (session_id, user_id, episode_type, role, content, entry_id)
         VALUES ('s-live', 'u1', 'conversation', 'assistant',
                 'live session episode content',
                 'ep-live')",
        [],
    )
    .expect("live episode");
    conn.execute(
        "INSERT INTO episodes
         (session_id, user_id, episode_type, role, content, keep_flag, entry_id)
         VALUES ('s-keep', 'u1', 'conversation', 'assistant',
                 'kept episode content',
                 1,
                 'ep-keep')",
        [],
    )
    .expect("kept episode");
    conn.execute(
        "INSERT INTO semantic_facts
         (user_id, fact_type, content, confidence, last_accessed, entry_id)
         VALUES ('u1', 'preference', 'stale low confidence fact', 0.4,
                 '2000-01-01 00:00:00', 'fact-stale')",
        [],
    )
    .expect("stale fact");
    conn.execute(
        "INSERT INTO semantic_facts
         (user_id, fact_type, content, confidence, last_accessed, entry_id)
         VALUES ('u1', 'preference', 'old but high confidence fact', 0.9,
                 '2000-01-01 00:00:00', 'fact-strong')",
        [],
    )
    .expect("strong fact");
    drop(conn);

    let report = purge_plan_memory(
        &db_path,
        MemoryPurgePlanRequest {
            user_id: "u1".to_string(),
            sessions_root: Some(sessions_root),
            cap: 500,
            stale_days: 60,
        },
    )
    .expect("purge plan should read candidates without writes");

    assert_eq!(
        report["l2-orphan-sessions"],
        serde_json::json!(["s-orphan"])
    );
    assert_eq!(report["counts"]["l2-orphan-sessions"], 1);
    assert_eq!(report["counts"]["l2-orphan-episodes"], 1);
    assert_eq!(report["l2-orphan-episodes"][0]["entry-id"], "ep-orphan");
    assert_eq!(report["l2-orphan-episodes"][0]["session-id"], "s-orphan");
    assert_eq!(report["counts"]["l3-stale"], 1);
    assert_eq!(report["l3-stale-facts"][0]["entry-id"], "fact-stale");
    assert_eq!(report["l3-stale-facts"][0]["reason"], "stale");
    assert_eq!(report["l3-orphan-facts"], serde_json::json!([]));
    assert_eq!(report["registry-live-skipped?"], true);
}

#[test]
fn entry_ids_match_clojure_memory_agent_content_hashing() {
    assert_eq!(
        entry_id_for(":l3", "User prefers polylith").unwrap(),
        "l3/768f6b2db81a6f38"
    );
    assert_eq!(
        entry_id_for("l3", "User prefers Polylith").unwrap(),
        entry_id_for("L3", "user  prefers   polylith").unwrap()
    );
    assert_eq!(
        entry_id_for("l3", "User prefers polylith.").unwrap(),
        entry_id_for(":l3", "User; prefers, polylith!").unwrap()
    );
    assert_ne!(
        entry_id_for("l3", "polylith").unwrap(),
        entry_id_for("l3", "monorepo").unwrap()
    );
    assert_eq!(normalize_for_hash(" Hello, WORLD!!!\n"), "hello world ");
    assert!(entry_id_for("scratch", "anything").is_err());
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

#[test]
fn stats_reports_clojure_memory_agent_count_projection() {
    let dir = tempfile::tempdir().expect("temp dir");
    let db_path = dir.path().join("memory.db");
    let conn = Connection::open(&db_path).expect("sqlite db");
    create_memory_schema(&conn);
    seed_memory_rows(&conn);
    conn.execute(
        "INSERT INTO episodes
         (session_id, user_id, episode_type, role, content, keep_flag, archived_flag)
         VALUES ('s2', 'u1', 'conversation', 'assistant', 'archived blue note', 1, 1)",
        [],
    )
    .expect("archived episode");
    conn.execute(
        "INSERT INTO episodes
         (session_id, user_id, episode_type, role, content, tombstoned_flag)
         VALUES ('s3', 'u1', 'conversation', 'assistant', 'deleted blue note', 1)",
        [],
    )
    .expect("tombstoned episode");
    conn.execute(
        "INSERT INTO semantic_facts (user_id, fact_type, content, confidence, archived_flag)
         VALUES ('u1', 'decision', 'medium confidence fact', 0.6, 1)",
        [],
    )
    .expect("medium fact");
    conn.execute(
        "INSERT INTO semantic_facts (user_id, fact_type, content, confidence)
         VALUES ('u1', 'risk', 'low confidence fact', 0.2)",
        [],
    )
    .expect("low fact");
    conn.execute(
        "INSERT INTO memory_audit
         (user_id, session_id, agent_id, turn_id, total_turns, entry_id, layer, byte_cost)
         VALUES ('u1', 's1', 'coact-agent', 1, 1, 'entry-1', 'l2', 42)",
        [],
    )
    .expect("audit row");
    drop(conn);

    let report = memory_stats(
        &db_path,
        MemoryStatsRequest {
            user_id: "u1".to_string(),
            session_id: Some("s1".to_string()),
        },
    )
    .expect("stats should read db");

    assert_eq!(report["stats"]["l1"]["count"], 0);
    assert_eq!(report["stats"]["l1"]["session-id"], "s1");
    assert_eq!(report["stats"]["l2"]["total"], 3);
    assert_eq!(report["stats"]["l2"]["current-session"], 2);
    assert_eq!(report["stats"]["l2"]["sessions-known"], 3);
    assert_eq!(report["stats"]["l2"]["keep-flagged"], 1);
    assert_eq!(report["stats"]["l2"]["archived"], 1);
    assert_eq!(report["stats"]["l2"]["tombstoned"], 1);
    assert_eq!(report["stats"]["l3"]["total"], 3);
    assert_eq!(report["stats"]["l3"]["by-kind"]["preference"], 1);
    assert_eq!(report["stats"]["l3"]["by-kind"]["decision"], 1);
    assert_eq!(report["stats"]["l3"]["by-kind"]["risk"], 1);
    assert_eq!(report["stats"]["l3"]["confidence-buckets"]["high"], 1);
    assert_eq!(report["stats"]["l3"]["confidence-buckets"]["medium"], 1);
    assert_eq!(report["stats"]["l3"]["confidence-buckets"]["low"], 1);
    assert_eq!(report["stats"]["l3"]["archived"], 1);
    assert_eq!(report["stats"]["l3"]["tombstoned"], 1);
    assert_eq!(report["stats"]["audit"]["rows"], 1);
    assert_eq!(report["stats"]["capture"]["running?"], false);
    assert_eq!(report["stats"]["health"]["status"], "ok");
}

#[test]
fn extracts_keywords_with_clojure_fts_stopwords_and_limits() {
    let keywords = extract_keywords("AWS EC2 costs are high, need to optimize EC2 spending");
    assert!(keywords.iter().any(|keyword| keyword == "ec2"));
    assert!(!keywords.iter().any(|keyword| keyword == "are"));
    assert!(!keywords.iter().any(|keyword| keyword == "need"));

    assert_eq!(
        extract_keywords_with_limits("a bb ccc dddd", 4, 10),
        vec!["dddd"]
    );
    assert_eq!(
        extract_keywords_with_limits("alpha beta gamma delta epsilon zeta", 3, 3).len(),
        3
    );
    assert!(extract_keywords("").is_empty());
}

#[test]
fn explain_turn_hydrates_audited_l2_and_l3_entries() {
    let dir = tempfile::tempdir().expect("temp dir");
    let db_path = dir.path().join("memory.db");
    let conn = Connection::open(&db_path).expect("sqlite db");
    create_memory_schema(&conn);
    seed_explain_rows(&conn);
    drop(conn);

    let report = explain_memory_turn(&db_path, "s1", Some("coact-agent"), 3)
        .expect("explain turn should read audit rows");

    assert_eq!(report["session-id"], "s1");
    assert_eq!(report["agent-id"], "coact-agent");
    assert_eq!(report["turn-id"], 3);
    assert_eq!(report["user-id"], "u1");
    assert_eq!(report["prompt-bytes"], 51);
    let entries = report["entries"].as_array().expect("entries array");
    assert_eq!(entries.len(), 2);
    assert!(entries.iter().any(|item| {
        item["audit"]["entry_id"] == "ep-explain"
            && item["entry"]["layer"] == "l2"
            && item["entry"]["content"] == "explain blue episode"
            && item["entry"]["archived"] == true
            && item["entry"]["tags"]
                .as_array()
                .is_some_and(|tags| tags.len() == 2)
            && item["entry"]["metadata"]["origin"] == "fixture"
    }));
    assert!(entries.iter().any(|item| {
        item["audit"]["entry_id"] == "fact-explain"
            && item["entry"]["layer"] == "l3"
            && item["entry"]["content"] == "explain green fact"
            && item["entry"]["tombstoned"] == true
            && item["entry"]["source"] == "manual"
            && item["entry"]["access-count"] == 7
    }));
}

#[test]
fn explain_session_groups_turns_by_agent_and_turn_ordered_by_total_turns() {
    let dir = tempfile::tempdir().expect("temp dir");
    let db_path = dir.path().join("memory.db");
    let conn = Connection::open(&db_path).expect("sqlite db");
    create_memory_schema(&conn);
    seed_explain_rows(&conn);
    conn.execute(
        "INSERT INTO memory_audit
         (user_id, session_id, agent_id, turn_id, total_turns, entry_id, layer, byte_cost)
         VALUES ('u1', 's1', 'sub-agent', 1, 1, 'ep-explain', 'l2', 11)",
        [],
    )
    .expect("earlier audit row");
    drop(conn);

    let report = explain_memory_session(&db_path, "s1").expect("explain session");

    assert_eq!(report["session-id"], "s1");
    let turns = report["turns"].as_array().expect("turns array");
    assert_eq!(turns.len(), 2);
    assert_eq!(turns[0]["agent-id"], "sub-agent");
    assert_eq!(turns[0]["turn-id"], 1);
    assert_eq!(turns[0]["total-turns"], 1);
    assert_eq!(turns[0]["prompt-bytes"], 11);
    assert_eq!(turns[1]["agent-id"], "coact-agent");
    assert_eq!(turns[1]["turn-id"], 3);
    assert_eq!(turns[1]["total-turns"], 2);
    assert_eq!(turns[1]["prompt-bytes"], 51);
}

#[test]
fn explain_missing_audit_table_is_empty_and_read_only() {
    let dir = tempfile::tempdir().expect("temp dir");
    let db_path = dir.path().join("memory.db");
    let conn = Connection::open(&db_path).expect("sqlite db");
    conn.execute_batch("CREATE TABLE memory_metadata (key TEXT PRIMARY KEY, value TEXT NOT NULL);")
        .expect("minimal schema");
    drop(conn);

    let report = explain_memory_turn(&db_path, "s1", None, 1)
        .expect("missing audit table should be a harmless empty explanation");

    assert_eq!(report["session-id"], "s1");
    assert_eq!(report["turn-id"], 1);
    assert!(report["entries"].as_array().expect("entries").is_empty());
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

fn seed_recall_rows(conn: &Connection) {
    conn.execute(
        "INSERT INTO episodes
         (session_id, user_id, episode_type, role, content, tags, entry_id)
         VALUES
         ('s1', 'u1', 'conversation', 'assistant', 'remember the blue deploy plan',
          '[\"blue\"]', 'ep-blue')",
        [],
    )
    .expect("matching episode");
    conn.execute(
        "INSERT INTO episodes
         (session_id, user_id, episode_type, role, content, entry_id)
         VALUES
         ('s2', 'u1', 'conversation', 'assistant', 'blue deploy other session', 'ep-other')",
        [],
    )
    .expect("other session episode");
    conn.execute(
        "INSERT INTO episodes
         (session_id, user_id, episode_type, role, content, entry_id)
         VALUES
         ('s1', 'u2', 'conversation', 'assistant', 'blue deploy wrong user', 'ep-wrong-user')",
        [],
    )
    .expect("wrong user episode");
    conn.execute(
        "INSERT INTO episodes
         (session_id, user_id, episode_type, role, content, archived_flag, entry_id)
         VALUES
         ('s1', 'u1', 'conversation', 'assistant', 'archived blue deploy', 1, 'ep-archived')",
        [],
    )
    .expect("archived episode");
    conn.execute(
        "INSERT INTO semantic_facts
         (user_id, fact_type, content, confidence, tags, entry_id)
         VALUES
         ('u1', 'preference', 'user prefers green releases', 0.9,
          '[\"green\"]', 'fact-green')",
        [],
    )
    .expect("matching fact");
    conn.execute(
        "INSERT INTO semantic_facts
         (user_id, fact_type, content, confidence, entry_id)
         VALUES ('u1', 'preference', 'low confidence yellow fact', 0.4, 'fact-low')",
        [],
    )
    .expect("low confidence fact");
    conn.execute(
        "INSERT INTO semantic_facts
         (user_id, fact_type, content, confidence, tombstoned_flag, entry_id)
         VALUES ('u1', 'preference', 'green tombstoned fact', 1.0, 1, 'fact-tombstoned')",
        [],
    )
    .expect("tombstoned fact");
}

fn seed_explain_rows(conn: &Connection) {
    conn.execute(
        "INSERT INTO episodes
         (session_id, user_id, episode_type, role, content, metadata, tags, sources, entry_id,
          keep_flag, archived_flag)
         VALUES
         ('s1', 'u1', 'conversation', 'assistant', 'explain blue episode',
          '{\"ttl\":\"session\",\"data\":{\"topic\":\"blue\"},\"metadata\":{\"origin\":\"fixture\"}}',
          '[\"blue\",\"audit\"]', '[{\"kind\":\"manual\"}]', 'ep-explain', 1, 1)",
        [],
    )
    .expect("explain episode");
    conn.execute(
        "INSERT INTO semantic_facts
         (user_id, fact_type, content, source, confidence, access_count, metadata, tags, sources,
          entry_id, tombstoned_flag)
         VALUES
         ('u1', 'preference', 'explain green fact', 'manual', 0.8, 7,
          '{\"data\":{\"topic\":\"green\"}}',
          '[\"green\"]', '[\"manual\"]', 'fact-explain', 1)",
        [],
    )
    .expect("explain fact");
    conn.execute(
        "INSERT INTO memory_audit
         (user_id, session_id, agent_id, turn_id, total_turns, entry_id, layer, byte_cost)
         VALUES ('u1', 's1', 'coact-agent', 3, 2, 'ep-explain', 'l2', 20)",
        [],
    )
    .expect("episode audit row");
    conn.execute(
        "INSERT INTO memory_audit
         (user_id, session_id, agent_id, turn_id, total_turns, entry_id, layer, byte_cost)
         VALUES ('u1', 's1', 'coact-agent', 3, 2, 'fact-explain', 'l3', 31)",
        [],
    )
    .expect("fact audit row");
}
