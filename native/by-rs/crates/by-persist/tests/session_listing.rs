use by_persist::{delete_session_dir, list_sessions};

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
        r#"{:id "alpha"
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
