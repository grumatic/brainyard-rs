use by_contracts::parse_map;

#[test]
fn parses_known_session_meta_shape() {
    let meta = parse_map(
        r#"{:id "session-1"
            :label "Planning spike"
            :last-active #inst "2026-06-01T00:00:00.000-00:00"
            :tokens 42
            :working-dir nil}"#,
    )
    .expect("session meta should parse");

    assert_eq!(meta.string("id"), Some("session-1"));
    assert_eq!(meta.string("label"), Some("Planning spike"));
    assert_eq!(
        meta.instant("last-active"),
        Some("2026-06-01T00:00:00.000-00:00")
    );
    assert_eq!(meta.i64("tokens"), Some(42));
    assert!(meta.is_nil("working-dir"));
}

#[test]
fn rejects_non_map_roots() {
    let err = parse_map(r#"["not" "a" "map"]"#).expect_err("root vectors are unsupported here");
    assert!(err.to_string().contains("map"));
}
