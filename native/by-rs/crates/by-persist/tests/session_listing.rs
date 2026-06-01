use by_persist::list_sessions;

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
        r#"{:id "alpha" :label "Alpha session" :last-active #inst "2026-06-01T00:00:00.000-00:00"}"#,
    )
    .unwrap();

    let sessions = list_sessions(root.path()).expect("session list should load");

    let ids: Vec<_> = sessions.iter().map(|session| session.id.as_str()).collect();
    assert_eq!(ids, vec!["alpha", "zeta"]);
    assert_eq!(sessions[0].label.as_deref(), Some("Alpha session"));
    assert_eq!(
        sessions[0].last_active.as_deref(),
        Some("2026-06-01T00:00:00.000-00:00")
    );
    assert_eq!(sessions[1].label, None);
}

#[test]
fn missing_root_is_an_empty_session_list() {
    let root = tempfile::tempdir().expect("temp root");
    let missing = root.path().join("does-not-exist");

    let sessions = list_sessions(&missing).expect("missing root should not be fatal");

    assert!(sessions.is_empty());
}
