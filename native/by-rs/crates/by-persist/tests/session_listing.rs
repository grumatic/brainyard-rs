use by_contracts::{parse_map, EdnMap, EdnValue};
use by_persist::{
    append_session_message, delete_session_dir, list_sessions, list_sessions_with_warnings,
    read_session_messages, read_session_snapshot, restore_session, save_session_meta,
    write_session_snapshot, SessionMessage, SessionMetaUpdate, SessionSnapshotKind,
};
use std::collections::BTreeMap;

#[test]
fn lists_session_dirs_with_optional_meta_edn() {
    let root = tempfile::tempdir().expect("temp root");
    let alpha = root.path().join("alpha");
    let zeta = root.path().join("zeta");
    let hidden = root.path().join(".hidden");
    std::fs::create_dir_all(&alpha).unwrap();
    std::fs::create_dir_all(&zeta).unwrap();
    std::fs::create_dir_all(&hidden).unwrap();
    std::fs::write(
        alpha.join("meta.edn"),
        r#"{:id "stale-meta-id"
            :label "Alpha session"
            :defagent-id :coact-agent
            :started-at 1780290000000
            :last-attached-at 1780290100000
            :last-active #inst "2026-06-01T00:00:00.000-00:00"}"#,
    )
    .unwrap();
    std::fs::write(alpha.join("messages.log"), "hello").unwrap();

    let sessions = list_sessions(root.path()).expect("session list should load");

    let ids: Vec<_> = sessions.iter().map(|session| session.id.as_str()).collect();
    assert_eq!(ids, vec!["alpha", "zeta"]);
    assert_eq!(sessions[0].label.as_deref(), Some("Alpha session"));
    assert_eq!(
        sessions[0].last_active.as_deref(),
        Some("2026-06-01T00:00:00.000-00:00")
    );
    assert_eq!(sessions[0].agent.as_deref(), Some("coact-agent"));
    assert_eq!(sessions[0].started_at_millis, Some(1780290000000));
    assert_eq!(sessions[0].last_attached_at_millis, Some(1780290100000));
    assert!(sessions[0].bytes >= 5);
    assert_eq!(sessions[1].label, None);
    assert_eq!(sessions[1].agent, None);
    assert_eq!(sessions[1].bytes, 0);
}

#[test]
fn missing_root_is_an_empty_session_list() {
    let root = tempfile::tempdir().expect("temp root");
    let missing = root.path().join("does-not-exist");

    let sessions = list_sessions(&missing).expect("missing root should not be fatal");

    assert!(sessions.is_empty());
}

#[test]
fn corrupt_meta_edn_keeps_session_visible_with_warning() {
    let root = tempfile::tempdir().expect("temp root");
    let broken = root.path().join("broken");
    std::fs::create_dir_all(&broken).unwrap();
    std::fs::write(broken.join("meta.edn"), "{:id ").unwrap();
    std::fs::write(broken.join("messages.log"), "hello").unwrap();

    let report = list_sessions_with_warnings(root.path()).expect("session list should load");

    assert_eq!(report.sessions.len(), 1);
    assert_eq!(report.sessions[0].id, "broken");
    assert_eq!(report.sessions[0].label, None);
    assert!(report.sessions[0].bytes >= 5);
    assert_eq!(report.warnings.len(), 1);
    assert_eq!(report.warnings[0].session_id, "broken");
    assert!(report.warnings[0].path.ends_with("meta.edn"));
    assert!(!report.warnings[0].message.is_empty());

    let sessions = list_sessions(root.path()).expect("legacy list should remain tolerant");
    assert_eq!(sessions[0].id, "broken");
}

#[test]
fn save_session_meta_creates_session_dir_and_writes_clojure_keys() {
    let root = tempfile::tempdir().expect("temp root");

    let meta_path = save_session_meta(
        root.path(),
        "agt-test",
        &SessionMetaUpdate {
            user_id: Some("alice".to_string()),
            agent_id: Some("coact-agent".to_string()),
            defagent_id: Some("coact-agent".to_string()),
            started_at_millis: Some(1780290000000),
            last_attached_at_millis: Some(1780290100000),
            working_dir: Some("/work/brainyard".to_string()),
            ..SessionMetaUpdate::default()
        },
    )
    .expect("meta write should succeed");

    assert!(meta_path.ends_with("meta.edn"));
    let raw = std::fs::read_to_string(&meta_path).unwrap();
    assert!(raw.contains(r#":user-id "alice""#));
    assert!(raw.contains(":agent-id :coact-agent"));
    assert!(raw.contains(":defagent-id :coact-agent"));
    assert!(raw.contains(":started-at 1780290000000"));
    assert!(raw.contains(":last-attached-at 1780290100000"));
    assert!(raw.contains(r#":working-dir "/work/brainyard""#));

    let sessions = list_sessions(root.path()).expect("session list should load");
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].id, "agt-test");
    assert_eq!(sessions[0].agent.as_deref(), Some("coact-agent"));
    assert_eq!(sessions[0].started_at_millis, Some(1780290000000));
    assert_eq!(sessions[0].last_attached_at_millis, Some(1780290100000));
}

#[test]
fn save_session_meta_merges_resume_attach_without_losing_started_at() {
    let root = tempfile::tempdir().expect("temp root");
    let session_dir = root.path().join("alpha");
    std::fs::create_dir_all(&session_dir).unwrap();
    std::fs::write(
        session_dir.join("meta.edn"),
        r#"{:label "Alpha"
            :defagent-id :coact-agent
            :started-at 1000
            :last-attached-at 2000
            :nested {:kept true}}"#,
    )
    .unwrap();

    save_session_meta(
        root.path(),
        "alpha",
        &SessionMetaUpdate {
            last_attached_at_millis: Some(3000),
            ..SessionMetaUpdate::default()
        },
    )
    .expect("resume attach update should succeed");

    let raw = std::fs::read_to_string(session_dir.join("meta.edn")).unwrap();
    let meta = parse_map(&raw).expect("updated meta should remain parseable");
    assert_eq!(meta.string("label"), Some("Alpha"));
    assert_eq!(meta.i64("started-at"), Some(1000));
    assert_eq!(meta.i64("last-attached-at"), Some(3000));
    assert!(raw.contains(":nested {:kept true}"));
}

#[test]
fn save_session_meta_rejects_path_traversal() {
    let root = tempfile::tempdir().expect("temp root");

    let err = save_session_meta(root.path(), "../outside", &SessionMetaUpdate::default())
        .expect_err("invalid id should fail");

    assert!(err.to_string().contains("invalid session id"));
}

#[test]
fn append_session_message_writes_restore_compatible_edn_lines() {
    let root = tempfile::tempdir().expect("temp root");

    let log_path = append_session_message(root.path(), "agt-test", "user", "hello \"brainyard\"")
        .expect("user append should succeed");
    append_session_message(root.path(), "agt-test", "assistant", "hi\nthere")
        .expect("assistant append should succeed");

    assert!(log_path.ends_with("messages.log"));
    let raw = std::fs::read_to_string(&log_path).unwrap();
    let lines = raw.lines().collect::<Vec<_>>();
    assert_eq!(lines.len(), 2);
    for line in &lines {
        let event = parse_map(line).expect("message event should be parseable EDN");
        assert!(event.i64("t").is_some());
        assert_eq!(
            event.get("kind"),
            Some(&by_contracts::EdnValue::Keyword("message".to_string()))
        );
    }
    assert!(raw.contains(r#":role "user""#));
    assert!(raw.contains(r#":content "hello \"brainyard\"""#));
    assert!(raw.contains(r#":role "assistant""#));
    assert!(raw.contains(r#":content "hi\nthere""#));
}

#[test]
fn append_session_message_rejects_path_traversal() {
    let root = tempfile::tempdir().expect("temp root");

    let err = append_session_message(root.path(), "../outside", "user", "hello")
        .expect_err("invalid id should fail");

    assert!(err.to_string().contains("invalid session id"));
}

#[test]
fn read_session_messages_restores_only_message_payloads() {
    let root = tempfile::tempdir().expect("temp root");
    let session_dir = root.path().join("agt-test");
    std::fs::create_dir_all(&session_dir).unwrap();
    std::fs::write(
        session_dir.join("messages.log"),
        r#"{:t 1 :kind :agent.ask/pre :payload {:input "ignored"}}
{:t 2 :kind :message :payload {:role "user" :content "hello \"brainyard\""}}
not valid edn
{:t 3 :kind :message :payload {:role "assistant" :content "hi\nthere"}}
{:t 4 :kind :message :payload {:role :tool :content "ignored non-string role"}}
"#,
    )
    .unwrap();

    let messages = read_session_messages(root.path(), "agt-test")
        .expect("message log should restore tolerantly");

    assert_eq!(
        messages,
        vec![
            SessionMessage {
                role: "user".to_string(),
                content: "hello \"brainyard\"".to_string(),
            },
            SessionMessage {
                role: "assistant".to_string(),
                content: "hi\nthere".to_string(),
            },
        ]
    );
}

#[test]
fn read_session_messages_missing_log_is_empty() {
    let root = tempfile::tempdir().expect("temp root");

    let messages =
        read_session_messages(root.path(), "agt-missing").expect("missing log should be empty");

    assert!(messages.is_empty());
}

#[test]
fn read_session_messages_rejects_path_traversal() {
    let root = tempfile::tempdir().expect("temp root");

    let err = read_session_messages(root.path(), "../outside").expect_err("invalid id should fail");

    assert!(err.to_string().contains("invalid session id"));
}

#[test]
fn reads_and_writes_edn_snapshots_with_non_map_roots() {
    let root = tempfile::tempdir().expect("temp root");
    let pending_dialogs = EdnValue::Vector(vec![
        EdnValue::Keyword("tool-approval".to_string()),
        EdnValue::Map(parse_map(r#"{:id "dlg-1"}"#).unwrap()),
    ]);

    let path = write_session_snapshot(
        root.path(),
        "agt-test",
        SessionSnapshotKind::PendingDialogs,
        &pending_dialogs,
    )
    .expect("snapshot write should succeed");

    assert!(path.ends_with("pending-dialogs.edn"));
    assert_eq!(
        read_session_snapshot(root.path(), "agt-test", SessionSnapshotKind::PendingDialogs)
            .expect("snapshot read should succeed"),
        Some(pending_dialogs)
    );

    let mut totals = BTreeMap::new();
    totals.insert("total-cost".to_string(), EdnValue::Float(0.0));
    let usage = EdnValue::Map(EdnMap::new(totals));
    write_session_snapshot(
        root.path(),
        "agt-test",
        SessionSnapshotKind::UsageTracker,
        &usage,
    )
    .expect("usage snapshot write should succeed");

    let raw = std::fs::read_to_string(root.path().join("agt-test/usage-tracker.edn")).unwrap();
    assert!(raw.contains(":total-cost 0.0"));
    assert_eq!(
        read_session_snapshot(root.path(), "agt-test", SessionSnapshotKind::UsageTracker)
            .expect("usage snapshot should parse"),
        Some(usage)
    );
}

#[test]
fn read_session_snapshot_missing_file_is_none() {
    let root = tempfile::tempdir().expect("temp root");

    let value = read_session_snapshot(root.path(), "agt-missing", SessionSnapshotKind::Session)
        .expect("missing snapshot should not fail");

    assert_eq!(value, None);
}

#[test]
fn session_snapshot_io_rejects_path_traversal() {
    let root = tempfile::tempdir().expect("temp root");

    let read_err = read_session_snapshot(root.path(), "../outside", SessionSnapshotKind::Session)
        .expect_err("invalid id should fail");
    assert!(read_err.to_string().contains("invalid session id"));

    let write_err = write_session_snapshot(
        root.path(),
        "../outside",
        SessionSnapshotKind::Session,
        &EdnValue::Map(EdnMap::default()),
    )
    .expect_err("invalid id should fail");
    assert!(write_err.to_string().contains("invalid session id"));
}

#[test]
fn restore_session_combines_session_snapshot_meta_messages_and_usage() {
    let root = tempfile::tempdir().expect("temp root");
    let session_dir = root.path().join("real-id");
    std::fs::create_dir_all(&session_dir).unwrap();
    std::fs::write(
        session_dir.join("meta.edn"),
        r#"{:id "stale-meta-id"
            :user-id "alice"
            :defagent-id :meta-agent}"#,
    )
    .unwrap();
    std::fs::write(
        session_dir.join("session.edn"),
        r#"{:id "stale-session-id"
            :user-id "bob"
            :agent-id :session-agent
            :total-turns 2
            :agent-activity-seq 9}"#,
    )
    .unwrap();
    std::fs::write(
        session_dir.join("messages.log"),
        r#"{:t 1 :kind :agent.ask/pre :payload {:input "ignored"}}
{:t 2 :kind :message :payload {:role "user" :content "First turn"}}
{:t 3 :kind :message :payload {:role "assistant" :content "Answer"}}
"#,
    )
    .unwrap();
    std::fs::write(
        session_dir.join("usage-tracker.edn"),
        r#"{:totals {:total-cost 0.0 :call-count 1} :history []}"#,
    )
    .unwrap();

    let restored = restore_session(root.path(), "real-id").expect("session should restore");

    assert_eq!(restored.id, "real-id");
    assert_eq!(restored.user_id.as_deref(), Some("bob"));
    assert_eq!(restored.agent.as_deref(), Some("session-agent"));
    assert_eq!(restored.total_turns, Some(2));
    assert_eq!(restored.agent_activity_seq, Some(9));
    assert_eq!(
        restored.messages,
        vec![
            SessionMessage {
                role: "user".to_string(),
                content: "First turn".to_string(),
            },
            SessionMessage {
                role: "assistant".to_string(),
                content: "Answer".to_string(),
            },
        ]
    );
    assert_eq!(
        restored.usage_tracker,
        Some(EdnValue::Map(
            parse_map(r#"{:totals {:total-cost 0.0 :call-count 1} :history []}"#).unwrap()
        ))
    );
}

#[test]
fn restore_session_rejects_non_map_session_snapshot() {
    let root = tempfile::tempdir().expect("temp root");
    let session_dir = root.path().join("broken");
    std::fs::create_dir_all(&session_dir).unwrap();
    std::fs::write(session_dir.join("session.edn"), r#"[:not-a-map]"#).unwrap();

    let err = restore_session(root.path(), "broken").expect_err("session.edn must be a map");

    assert!(err.to_string().contains("must be an EDN map"));
}

#[test]
fn delete_session_dir_removes_existing_tree_and_reports_missing() {
    let root = tempfile::tempdir().expect("temp root");
    let doomed = root.path().join("doomed");
    std::fs::create_dir_all(doomed.join("nested")).unwrap();
    std::fs::write(doomed.join("nested/messages.log"), "goodbye").unwrap();

    assert!(delete_session_dir(root.path(), "doomed").expect("delete should succeed"));
    assert!(!doomed.exists());
    assert!(!delete_session_dir(root.path(), "doomed").expect("missing should not fail"));
}

#[test]
fn delete_session_dir_rejects_path_traversal() {
    let root = tempfile::tempdir().expect("temp root");

    let err = delete_session_dir(root.path(), "../outside").expect_err("invalid id should fail");

    assert!(err.to_string().contains("invalid session id"));
}
